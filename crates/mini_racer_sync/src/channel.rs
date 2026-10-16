use std::time::Duration;

use crate::lock::lock;
use crate::parker::Parker;
use crate::primitives::{AtomicU64, Mutex, Ordering};

// One word holds whose turn it is, whether the channel is closed, which sides
// are parked, and the request and callback depths, so the update that hands
// over the turn also tells the sender whether the receiver is asleep. Parking
// sets its flag with an update to the same word, so a handover and a park are
// totally ordered and no wakeup is lost.
const V8_TURN: u64 = 1;
const CLOSED: u64 = 1 << 1;
const RUBY_PARKED: u64 = 1 << 2;
const V8_PARKED: u64 = 1 << 3;
const RUBY_NOTIFIED: u64 = 1 << 4;
const V8_NOTIFIED: u64 = 1 << 5;
const REQUESTS_SHIFT: u32 = 16;
const CALLBACKS_SHIFT: u32 = 32;
const COUNT: u64 = 0xFFFF;

/// The payloads a channel moves. Ruby sends requests and replies; V8 sends
/// responses and callbacks.
pub trait Protocol {
    type Request;
    type Response;
    type Callback;
    type Reply;
}

pub enum ToV8<P: Protocol> {
    Request(P::Request),
    Reply(P::Reply),
}

pub enum ToRuby<P: Protocol> {
    Response(P::Response),
    Callback(P::Callback),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Ruby,
    V8,
}

impl Side {
    const fn turn(self) -> u64 {
        match self {
            Self::Ruby => 0,
            Self::V8 => V8_TURN,
        }
    }

    const fn parked(self) -> u64 {
        match self {
            Self::Ruby => RUBY_PARKED,
            Self::V8 => V8_PARKED,
        }
    }

    const fn notified(self) -> u64 {
        match self {
            Self::Ruby => RUBY_NOTIFIED,
            Self::V8 => V8_NOTIFIED,
        }
    }

    const fn other(self) -> Self {
        match self {
            Self::Ruby => Self::V8,
            Self::V8 => Self::Ruby,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ChannelError {
    #[error("channel closed")]
    Closed,
    #[error("not this side's turn")]
    OutOfTurn,
    #[error("nothing to receive")]
    Empty,
    #[error("the previous message was never received")]
    Unreceived,
    #[error("nothing open to finish")]
    Unbalanced,
    #[error("nesting too deep")]
    TooDeep,
}

/// Open requests and callbacks. Ruby's turn always has as many of each; V8's
/// turn has one more request than callbacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Nesting {
    pub requests: u64,
    pub callbacks: u64,
}

#[derive(Clone, Copy)]
enum Step {
    Request,
    Reply,
    Response,
    Callback,
}

impl Step {
    fn apply(self, open: Nesting) -> Result<Nesting, ChannelError> {
        let Nesting {
            requests,
            callbacks,
        } = open;
        let deeper = |n: u64| {
            n.checked_add(1)
                .filter(|n| *n <= COUNT)
                .ok_or(ChannelError::TooDeep)
        };
        let shallower = |n: u64| n.checked_sub(1).ok_or(ChannelError::Unbalanced);
        Ok(match self {
            Self::Request => Nesting {
                requests: deeper(requests)?,
                callbacks,
            },
            Self::Reply => Nesting {
                requests,
                callbacks: shallower(callbacks)?,
            },
            Self::Response => Nesting {
                requests: shallower(requests)?,
                callbacks,
            },
            Self::Callback => Nesting {
                requests,
                callbacks: deeper(callbacks)?,
            },
        })
    }
}

const fn nesting(state: u64) -> Nesting {
    Nesting {
        requests: (state >> REQUESTS_SHIFT) & COUNT,
        callbacks: (state >> CALLBACKS_SHIFT) & COUNT,
    }
}

const fn with_nesting(state: u64, open: Nesting) -> u64 {
    let counts = (COUNT << REQUESTS_SHIFT) | (COUNT << CALLBACKS_SHIFT);
    (state & !counts) | (open.requests << REQUESTS_SHIFT) | (open.callbacks << CALLBACKS_SHIFT)
}

const fn closed(state: u64) -> bool {
    state & CLOSED != 0
}

const fn holds_turn(state: u64, side: Side) -> bool {
    state & V8_TURN == side.turn()
}

const fn ready(state: u64, side: Side) -> bool {
    holds_turn(state, side) || closed(state)
}

/// The turn-taking channel between a Ruby thread and a context's runner.
///
/// Each side is driven by one thread at a time. A sender holds its outgoing
/// slot's lock for the whole handover, so a second sender on the same side
/// gets `OutOfTurn` instead of overwriting, and a receiver only locks a slot
/// once the turn says it is full, so the locks are effectively uncontended.
pub struct Channel<P: Protocol> {
    state: AtomicU64,
    to_v8: Mutex<Option<ToV8<P>>>,
    to_ruby: Mutex<Option<ToRuby<P>>>,
    ruby: Parker,
    v8: Parker,
    ruby_notifier: Mutex<Option<Box<dyn Notifier>>>,
    v8_notifier: Mutex<Option<Box<dyn Notifier>>>,
}

/// Rung for a side that waits with `park_notified`, such as a fiber waiting
/// for a file descriptor to become readable.
pub trait Notifier: Send + Sync {
    fn notify(&self);
}

impl<P: Protocol> Default for Channel<P> {
    fn default() -> Self {
        Self {
            state: AtomicU64::new(0),
            to_v8: Mutex::new(None),
            to_ruby: Mutex::new(None),
            ruby: Parker::default(),
            v8: Parker::default(),
            ruby_notifier: Mutex::new(None),
            v8_notifier: Mutex::new(None),
        }
    }
}

impl<P: Protocol> Channel<P> {
    /// Ruby starts an operation: outermost when nothing is open, nested when a
    /// callback is.
    pub fn request(&self, request: P::Request) -> Result<(), ChannelError> {
        self.publish(
            Side::Ruby,
            &self.to_v8,
            ToV8::Request(request),
            Step::Request,
        )
    }

    /// Ruby finishes the innermost callback.
    pub fn reply(&self, reply: P::Reply) -> Result<(), ChannelError> {
        self.publish(Side::Ruby, &self.to_v8, ToV8::Reply(reply), Step::Reply)
    }

    /// V8 finishes the innermost request.
    pub fn respond(&self, response: P::Response) -> Result<(), ChannelError> {
        self.publish(
            Side::V8,
            &self.to_ruby,
            ToRuby::Response(response),
            Step::Response,
        )
    }

    /// V8 asks Ruby to run a callback inside the innermost request.
    pub fn callback(&self, callback: P::Callback) -> Result<(), ChannelError> {
        self.publish(
            Side::V8,
            &self.to_ruby,
            ToRuby::Callback(callback),
            Step::Callback,
        )
    }

    /// The message for Ruby, or `None` while it is V8's turn.
    pub fn recv_ruby(&self) -> Result<Option<ToRuby<P>>, ChannelError> {
        self.recv(Side::Ruby, &self.to_ruby)
    }

    /// The message for V8, or `None` while it is Ruby's turn.
    pub fn recv_v8(&self) -> Result<Option<ToV8<P>>, ChannelError> {
        self.recv(Side::V8, &self.to_v8)
    }

    /// Whether `side` has the turn or the channel is closed.
    #[must_use]
    pub fn ready(&self, side: Side) -> bool {
        ready(self.state.load(Ordering::Acquire), side)
    }

    /// Open, Ruby's turn, and no request or callback open. A thread that has
    /// claimed the context but not sent yet still counts as idle.
    #[must_use]
    pub fn idle(&self) -> bool {
        let state = self.state.load(Ordering::Acquire);
        holds_turn(state, Side::Ruby)
            && !closed(state)
            && nesting(state)
                == Nesting {
                    requests: 0,
                    callbacks: 0,
                }
    }

    #[must_use]
    pub fn is_closed(&self) -> bool {
        closed(self.state.load(Ordering::Acquire))
    }

    #[must_use]
    pub fn nesting(&self) -> Nesting {
        nesting(self.state.load(Ordering::Acquire))
    }

    /// Waits for the turn: returns once `side` has it, the channel closes,
    /// `wake(side)` is called, or the timeout passes. May return early;
    /// callers recheck `ready`.
    pub fn park(&self, side: Side, timeout: Option<Duration>) {
        if self.mark(side, side.parked()) {
            self.parker(side).park(timeout);
            self.state.fetch_and(!side.parked(), Ordering::AcqRel);
        }
    }

    /// `park` for a waiter that sleeps elsewhere: unless `side` has the turn,
    /// runs `wait`, which should return once the side's notifier rings, the
    /// channel closes or `wake(side)` is called, and may return earlier.
    pub fn park_notified(&self, side: Side, wait: impl FnOnce()) {
        if self.mark(side, side.notified()) {
            wait();
            self.state.fetch_and(!side.notified(), Ordering::AcqRel);
        }
    }

    /// What `park_notified` on `side` waits for; `None` removes it.
    pub fn set_notifier(&self, side: Side, notifier: Option<Box<dyn Notifier>>) {
        *lock(self.notifier(side)) = notifier;
    }

    // Sets `flag` unless `side` has the turn; returns whether it did.
    fn mark(&self, side: Side, flag: u64) -> bool {
        let mut state = self.state.load(Ordering::Acquire);
        loop {
            if ready(state, side) {
                return false;
            }
            match self.state.compare_exchange_weak(
                state,
                state | flag,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(actual) => state = actual,
            }
        }
    }

    fn ring(&self, side: Side) {
        if let Some(notifier) = lock(self.notifier(side)).as_ref() {
            notifier.notify();
        }
    }

    const fn notifier(&self, side: Side) -> &Mutex<Option<Box<dyn Notifier>>> {
        match side {
            Side::Ruby => &self.ruby_notifier,
            Side::V8 => &self.v8_notifier,
        }
    }

    /// Waits for `wake(side)`, `close`, or the timeout whoever holds the turn,
    /// so a runner holding it can wait for posted tasks. A wake that came
    /// first ends the wait at once.
    pub fn sleep(&self, side: Side, timeout: Option<Duration>) {
        if !self.is_closed() {
            self.parker(side).park(timeout);
        }
    }

    /// Wakes `side` from `park` or `sleep` without handing it the turn.
    pub fn wake(&self, side: Side) {
        self.parker(side).unpark();
        self.ring(side);
    }

    /// Closes the channel and wakes both sides. A message already sent is
    /// still received; every later send fails with `Closed`.
    pub fn close(&self) {
        self.state.fetch_or(CLOSED, Ordering::AcqRel);
        self.ruby.unpark();
        self.v8.unpark();
        self.ring(Side::Ruby);
        self.ring(Side::V8);
    }

    fn publish<M>(
        &self,
        from: Side,
        outgoing: &Mutex<Option<M>>,
        message: M,
        step: Step,
    ) -> Result<(), ChannelError> {
        let mut slot = lock(outgoing);
        let mut state = self.state.load(Ordering::Acquire);
        if closed(state) {
            return Err(ChannelError::Closed);
        }
        if !holds_turn(state, from) {
            return Err(ChannelError::OutOfTurn);
        }
        if slot.is_some() {
            return Err(ChannelError::Unreceived);
        }
        // Only the turn holder changes the counts, and the slot lock keeps
        // this the only sender, so a concurrent update can only set a parked
        // flag or close.
        let open = step.apply(nesting(state))?;
        *slot = Some(message);
        let to = from.other();
        loop {
            if closed(state) {
                slot.take();
                return Err(ChannelError::Closed);
            }
            let next = with_nesting(
                (state & !V8_TURN & !to.parked() & !to.notified()) | to.turn(),
                open,
            );
            match self
                .state
                .compare_exchange_weak(state, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => break,
                Err(actual) => state = actual,
            }
        }
        drop(slot);
        if state & to.parked() != 0 {
            self.parker(to).unpark();
        }
        if state & to.notified() != 0 {
            self.ring(to);
        }
        Ok(())
    }

    fn recv<M>(&self, side: Side, incoming: &Mutex<Option<M>>) -> Result<Option<M>, ChannelError> {
        let state = self.state.load(Ordering::Acquire);
        if !holds_turn(state, side) {
            return if closed(state) {
                Err(ChannelError::Closed)
            } else {
                Ok(None)
            };
        }
        let message = lock(incoming).take();
        match message {
            Some(message) => Ok(Some(message)),
            None if closed(state) => Err(ChannelError::Closed),
            None => Err(ChannelError::Empty),
        }
    }

    const fn parker(&self, side: Side) -> &Parker {
        match side {
            Side::Ruby => &self.ruby,
            Side::V8 => &self.v8,
        }
    }
}

#[cfg(all(test, not(loom)))]
mod tests {
    use std::sync::Arc;
    use std::thread;

    use super::*;

    struct Text;

    impl Protocol for Text {
        type Request = &'static str;
        type Response = &'static str;
        type Callback = &'static str;
        type Reply = &'static str;
    }

    fn request(channel: &Channel<Text>) -> Option<&'static str> {
        match channel.recv_v8() {
            Ok(Some(ToV8::Request(body))) => Some(body),
            _ => None,
        }
    }

    fn reply(channel: &Channel<Text>) -> Option<&'static str> {
        match channel.recv_v8() {
            Ok(Some(ToV8::Reply(body))) => Some(body),
            _ => None,
        }
    }

    fn response(channel: &Channel<Text>) -> Option<&'static str> {
        match channel.recv_ruby() {
            Ok(Some(ToRuby::Response(body))) => Some(body),
            _ => None,
        }
    }

    fn callback(channel: &Channel<Text>) -> Option<&'static str> {
        match channel.recv_ruby() {
            Ok(Some(ToRuby::Callback(body))) => Some(body),
            _ => None,
        }
    }

    const fn open(requests: u64, callbacks: u64) -> Nesting {
        Nesting {
            requests,
            callbacks,
        }
    }

    #[test]
    fn a_request_and_its_response_change_turns() {
        let channel = Channel::<Text>::default();
        assert!(channel.ready(Side::Ruby));
        assert!(matches!(channel.recv_v8(), Ok(None)));

        channel.request("eval").unwrap();
        assert!(channel.ready(Side::V8));
        assert!(!channel.ready(Side::Ruby));
        assert!(matches!(channel.recv_ruby(), Ok(None)));
        assert_eq!(request(&channel), Some("eval"));

        channel.respond("2").unwrap();
        assert_eq!(response(&channel), Some("2"));
        assert_eq!(channel.nesting(), open(0, 0));
    }

    #[test]
    fn nested_requests_and_callbacks_unwind_in_order() {
        let channel = Channel::<Text>::default();
        channel.request("outer").unwrap();
        assert_eq!(request(&channel), Some("outer"));
        channel.callback("ruby fn").unwrap();
        assert_eq!(channel.nesting(), open(1, 1));
        assert_eq!(callback(&channel), Some("ruby fn"));

        channel.request("inner").unwrap();
        assert_eq!(channel.nesting(), open(2, 1));
        assert_eq!(request(&channel), Some("inner"));
        channel.respond("inner done").unwrap();
        assert_eq!(response(&channel), Some("inner done"));

        channel.reply("ruby fn done").unwrap();
        assert_eq!(reply(&channel), Some("ruby fn done"));
        channel.respond("outer done").unwrap();
        assert_eq!(response(&channel), Some("outer done"));
        assert_eq!(channel.nesting(), open(0, 0));
    }

    #[test]
    fn misuse_is_an_error() {
        let channel = Channel::<Text>::default();
        assert_eq!(channel.recv_ruby().err(), Some(ChannelError::Empty));
        assert_eq!(channel.reply("stray"), Err(ChannelError::Unbalanced));
        assert_eq!(channel.respond("early"), Err(ChannelError::OutOfTurn));

        channel.request("eval").unwrap();
        assert_eq!(channel.request("again"), Err(ChannelError::OutOfTurn));
        assert_eq!(request(&channel), Some("eval"));
        assert_eq!(channel.recv_v8().err(), Some(ChannelError::Empty));
    }

    #[test]
    fn a_message_left_unreceived_is_never_overwritten() {
        let channel = Channel::<Text>::default();
        channel.request("outer").unwrap();
        request(&channel);
        channel.callback("ruby fn").unwrap();
        channel.reply("skipped the callback").unwrap();
        reply(&channel);

        assert_eq!(channel.respond("outer done"), Err(ChannelError::Unreceived));
        assert_eq!(channel.nesting(), open(1, 0));
    }

    #[test]
    fn a_message_sent_before_close_is_still_received() {
        let channel = Channel::<Text>::default();
        channel.request("eval").unwrap();
        request(&channel);
        channel.respond("2").unwrap();
        channel.close();

        assert!(channel.is_closed());
        assert_eq!(response(&channel), Some("2"));
        assert_eq!(channel.recv_ruby().err(), Some(ChannelError::Closed));
        assert_eq!(channel.request("late"), Err(ChannelError::Closed));
    }

    #[test]
    fn close_fails_the_side_waiting_for_the_turn() {
        let channel = Channel::<Text>::default();
        channel.request("eval").unwrap();
        channel.close();

        assert!(channel.ready(Side::Ruby));
        assert!(channel.ready(Side::V8));
        assert_eq!(channel.recv_ruby().err(), Some(ChannelError::Closed));
        assert_eq!(channel.respond("late"), Err(ChannelError::Closed));
    }

    #[test]
    fn park_returns_at_once_when_the_turn_is_already_here() {
        let channel = Channel::<Text>::default();
        channel.park(Side::Ruby, None);
        channel.park(Side::Ruby, Some(Duration::from_secs(60)));
    }

    #[test]
    fn a_wake_ends_a_park_without_the_turn() {
        let channel = Channel::<Text>::default();
        channel.wake(Side::V8);
        channel.park(Side::V8, None);

        assert!(!channel.ready(Side::V8));
    }

    #[test]
    fn park_gives_up_at_the_timeout() {
        let channel = Channel::<Text>::default();
        channel.park(Side::V8, Some(Duration::from_millis(5)));

        assert!(!channel.ready(Side::V8));
    }

    #[test]
    fn sleep_waits_for_a_wake_even_while_holding_the_turn() {
        let channel = Arc::new(Channel::<Text>::default());
        channel.request("eval_await").unwrap();
        channel.sleep(Side::V8, Some(Duration::from_millis(5)));

        let waker = Arc::clone(&channel);
        let task = thread::spawn(move || waker.wake(Side::V8));
        channel.sleep(Side::V8, None);
        task.join().unwrap();
        assert_eq!(request(&channel), Some("eval_await"));
    }

    #[test]
    fn round_trips_between_threads() {
        let channel = Arc::new(Channel::<Text>::default());
        let runner = Arc::clone(&channel);
        let v8 = thread::spawn(move || {
            for _ in 0..1000 {
                let message = loop {
                    if let Some(message) = runner.recv_v8().unwrap() {
                        break message;
                    }
                    runner.park(Side::V8, None);
                };
                assert!(matches!(message, ToV8::Request("ping")));
                runner.respond("pong").unwrap();
            }
        });

        for _ in 0..1000 {
            channel.request("ping").unwrap();
            while !channel.ready(Side::Ruby) {
                channel.park(Side::Ruby, None);
            }
            assert_eq!(response(&channel), Some("pong"));
        }
        v8.join().unwrap();
    }
}
