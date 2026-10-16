#![cfg(loom)]

use std::time::Duration;

use loom::sync::Arc;
use loom::sync::atomic::{AtomicBool, Ordering};
use loom::thread;
use mini_racer_sync::{
    Channel, ChannelError, DispatchKind, Halt, Handoff, Interrupt, Nesting, Outcome, Protocol,
    Side, TermCell, TerminationCause, Terminator, ToRuby, ToV8,
};

#[test]
fn a_waiter_always_receives_the_completed_value() {
    loom::model(|| {
        let handoff = Arc::new(Handoff::default());
        let runner = Arc::clone(&handoff);
        let completer = thread::spawn(move || runner.complete(7_u8));

        assert_eq!(handoff.wait(), Outcome::Done(7));
        completer.join().expect("completer thread");
    });
}

#[test]
fn an_interrupt_never_loses_the_result() {
    loom::model(|| {
        let handoff = Arc::new(Handoff::default());
        let runner = Arc::clone(&handoff);
        let interrupter = Arc::clone(&handoff);
        let completer = thread::spawn(move || runner.complete(7_u8));
        let interrupt = thread::spawn(move || interrupter.interrupt());

        match handoff.wait() {
            Outcome::Done(value) => assert_eq!(value, 7),
            Outcome::Interrupted => assert_eq!(handoff.wait(), Outcome::Done(7)),
        }
        completer.join().expect("completer thread");
        interrupt.join().expect("interrupter thread");
    });
}

#[test]
fn a_timed_waiter_receives_the_completed_value() {
    loom::model(|| {
        let handoff = Arc::new(Handoff::default());
        let runner = Arc::clone(&handoff);
        let completer = thread::spawn(move || runner.complete(7_u8));

        assert_eq!(handoff.wait_for(Duration::from_secs(60)), Some(7));
        completer.join().expect("completer thread");
    });
}

struct Numbers;

impl Protocol for Numbers {
    type Request = u8;
    type Response = u8;
    type Callback = u8;
    type Reply = u8;
}

const ABORT: u8 = u8::MAX;

fn next_for_ruby<P: Protocol>(
    channel: &Channel<P>,
    timeout: Option<Duration>,
) -> Result<ToRuby<P>, ChannelError> {
    loop {
        if let Some(message) = channel.recv_ruby()? {
            return Ok(message);
        }
        channel.park(Side::Ruby, timeout);
    }
}

fn next_for_v8<P: Protocol>(channel: &Channel<P>) -> Result<ToV8<P>, ChannelError> {
    loop {
        if let Some(message) = channel.recv_v8()? {
            return Ok(message);
        }
        channel.park(Side::V8, None);
    }
}

fn request(message: Result<ToV8<Numbers>, ChannelError>) -> Option<u8> {
    match message {
        Ok(ToV8::Request(value)) => Some(value),
        _ => None,
    }
}

fn reply(message: Result<ToV8<Numbers>, ChannelError>) -> Option<u8> {
    match message {
        Ok(ToV8::Reply(value)) => Some(value),
        _ => None,
    }
}

fn response(message: Result<ToRuby<Numbers>, ChannelError>) -> Option<u8> {
    match message {
        Ok(ToRuby::Response(value)) => Some(value),
        _ => None,
    }
}

fn callback(message: Result<ToRuby<Numbers>, ChannelError>) -> Option<u8> {
    match message {
        Ok(ToRuby::Callback(value)) => Some(value),
        _ => None,
    }
}

#[derive(Default)]
struct Isolate {
    terminating: AtomicBool,
}

impl Isolate {
    fn terminating(&self) -> bool {
        self.terminating.load(Ordering::SeqCst)
    }
}

struct Handle {
    isolate: Arc<Isolate>,
    runner: Option<Arc<Channel<Numbers>>>,
}

impl Terminator for Handle {
    fn terminate(&self) {
        self.isolate.terminating.store(true, Ordering::SeqCst);
        if let Some(runner) = &self.runner {
            runner.wake(Side::V8);
        }
    }

    fn cancel(&self) {
        self.isolate.terminating.store(false, Ordering::SeqCst);
    }
}

fn term_cell(runner: Option<Arc<Channel<Numbers>>>) -> (Arc<TermCell<Handle>>, Arc<Isolate>) {
    let isolate = Arc::new(Isolate::default());
    let cell = Arc::new(TermCell::default());
    cell.install(Handle {
        isolate: Arc::clone(&isolate),
        runner,
    });
    (cell, isolate)
}

fn run(cell: &TermCell<Handle>, kind: DispatchKind) -> Option<Halt> {
    let id = cell.begin(kind).expect("nothing armed");
    cell.end(id)
}

// The larger models bound preemptions so they finish in seconds; loom's
// documentation finds two or three enough to expose most bugs.
fn bounded(preemptions: usize, model: impl Fn() + Sync + Send + 'static) {
    let mut builder = loom::model::Builder::new();
    builder.preemption_bound = Some(preemptions);
    builder.check(model);
}

// Model 1: neither side sleeps through the handover that wakes it, on either
// park path.
#[test]
fn a_round_trip_never_loses_a_wakeup() {
    loom::model(|| {
        let channel = Arc::new(Channel::<Numbers>::default());
        let runner = Arc::clone(&channel);
        let v8 = thread::spawn(move || {
            let value = request(next_for_v8(&runner)).expect("a request");
            runner.respond(value + 1).expect("open");
        });

        channel.request(1).expect("open");
        let answer = next_for_ruby(&channel, Some(Duration::from_secs(60)));
        assert_eq!(response(answer), Some(2));
        v8.join().expect("runner thread");
    });
}

// Model 2: a callback nests a request, and everything unwinds in order.
#[test]
fn nested_requests_and_callbacks_unwind_in_order() {
    bounded(4, || {
        let channel = Arc::new(Channel::<Numbers>::default());
        channel.request(1).expect("open");
        let outer = request(next_for_v8(&channel)).expect("the outer request");
        let runner = Arc::clone(&channel);
        let v8 = thread::spawn(move || {
            runner.callback(10).expect("open");
            let inner = request(next_for_v8(&runner)).expect("the nested request");
            runner.respond(inner + 1).expect("open");
            let answer = reply(next_for_v8(&runner)).expect("the reply");
            runner.respond(answer + outer).expect("open");
        });

        let argument = callback(next_for_ruby(&channel, None)).expect("the callback");
        channel.request(argument + 5).expect("open");
        let nested = response(next_for_ruby(&channel, None)).expect("the nested response");
        channel.reply(nested).expect("open");
        assert_eq!(response(next_for_ruby(&channel, None)), Some(17));
        v8.join().expect("runner thread");
        assert_eq!(
            channel.nesting(),
            Nesting {
                requests: 0,
                callbacks: 0
            }
        );
    });
}

// Model 3: Ruby, woken by its interrupt handler while V8 posts a callback,
// cancels and drains: it aborts the callback, gets the cancelled response,
// and leaves no termination behind.
#[test]
fn a_cancel_racing_a_callback_drains_cleanly() {
    bounded(3, || {
        let channel = Arc::new(Channel::<Numbers>::default());
        let (cell, isolate) = term_cell(None);
        let id = cell.begin(DispatchKind::Script).expect("nothing armed");
        channel.request(1).expect("open");
        request(next_for_v8(&channel)).expect("a request");

        let runner = Arc::clone(&channel);
        let runner_cell = Arc::clone(&cell);
        let v8 = thread::spawn(move || {
            runner.callback(7).expect("open");
            let answer = reply(next_for_v8(&runner)).expect("the reply");
            let halt = runner_cell.end(id);
            runner.respond(answer).expect("open");
            halt
        });
        let waker = Arc::clone(&channel);
        let interrupt = thread::spawn(move || waker.wake(Side::Ruby));

        channel.park(Side::Ruby, None);
        assert!(cell.request(Interrupt::Cancel(id)));
        let mut aborted = 0;
        let result = loop {
            match next_for_ruby(&channel, None).expect("open") {
                ToRuby::Callback(_) => {
                    aborted += 1;
                    channel.reply(ABORT).expect("open");
                }
                ToRuby::Response(value) => break value,
            }
        };
        let halt = v8.join().expect("runner thread");
        interrupt.join().expect("interrupt thread");

        assert_eq!((aborted, result), (1, ABORT));
        assert_eq!(halt, Some(Halt::Terminated(TerminationCause::Cancelled)));
        assert_eq!(cell.armed(), None);
        assert_eq!(cell.check(DispatchKind::Script), None);
        assert!(!isolate.terminating());
    });
}

// Model 4: a wake from outside always ends a park, whenever it lands.
#[test]
fn an_external_wake_ends_a_park() {
    loom::model(|| {
        let channel = Arc::new(Channel::<Numbers>::default());
        channel.request(1).expect("open");
        let waker = Arc::clone(&channel);
        let interrupter = thread::spawn(move || waker.wake(Side::Ruby));

        channel.park(Side::Ruby, None);
        interrupter.join().expect("interrupter thread");
        assert!(!channel.ready(Side::Ruby));
    });
}

struct Owned;

impl Protocol for Owned {
    type Request = Arc<()>;
    type Response = Arc<()>;
    type Callback = Arc<()>;
    type Reply = Arc<()>;
}

// Model 5: the runner closing mid-request wakes the waiter, a response sent
// before the close is still delivered, and loom's leak check proves every
// message is dropped exactly once.
#[test]
fn close_racing_an_in_flight_request_delivers_or_fails_cleanly() {
    loom::model(|| {
        let channel = Arc::new(Channel::<Owned>::default());
        let runner = Arc::clone(&channel);
        let v8 = thread::spawn(move || {
            let responded =
                matches!(runner.recv_v8(), Ok(Some(_))) && runner.respond(Arc::new(())).is_ok();
            runner.close();
            responded
        });

        let outcome = channel
            .request(Arc::new(()))
            .and_then(|()| next_for_ruby(&channel, None))
            .map(|message| matches!(message, ToRuby::Response(_)));
        let responded = v8.join().expect("runner thread");
        assert_eq!(
            outcome,
            if responded {
                Ok(true)
            } else {
                Err(ChannelError::Closed)
            }
        );
    });
}

// Two threads sending on one side is misuse; exactly one send lands and the
// other is refused rather than overwriting it.
#[test]
fn a_second_sender_on_one_side_is_refused() {
    loom::model(|| {
        let channel = Arc::new(Channel::<Numbers>::default());
        let other = Arc::clone(&channel);
        let sender = thread::spawn(move || other.request(2));

        let mine = channel.request(1);
        let theirs = sender.join().expect("sender thread");
        let delivered = request(next_for_v8(&channel));

        match (mine, theirs) {
            (Ok(()), Err(ChannelError::OutOfTurn)) => assert_eq!(delivered, Some(1)),
            (Err(ChannelError::OutOfTurn), Ok(())) => assert_eq!(delivered, Some(2)),
            _ => unreachable!("exactly one send lands"),
        }
        assert_eq!(channel.nesting().requests, 1);
    });
}

// A stop landing while the runner checks and goes to sleep in an await loop
// still wakes it.
#[test]
fn a_stop_wakes_a_runner_sleeping_in_an_await_loop() {
    loom::model(|| {
        let channel = Arc::new(Channel::<Numbers>::default());
        channel.request(1).expect("open");
        request(next_for_v8(&channel)).expect("a request");
        let (cell, isolate) = term_cell(Some(Arc::clone(&channel)));
        let id = cell.begin(DispatchKind::Script).expect("nothing armed");
        let stopper_cell = Arc::clone(&cell);
        let stopper = thread::spawn(move || stopper_cell.request(Interrupt::Stop));

        while cell.check(DispatchKind::Script).is_none() {
            channel.sleep(Side::V8, None);
        }
        stopper.join().expect("stopper thread");

        assert_eq!(cell.end(id), Some(Halt::Terminated(TerminationCause::Stop)));
        assert!(!isolate.terminating());
    });
}

// Model 8: a timeout firing as its dispatch ends never terminates the next one.
#[test]
fn a_late_timeout_never_terminates_the_next_dispatch() {
    loom::model(|| {
        let (cell, isolate) = term_cell(None);
        let first = cell.begin(DispatchKind::Script).expect("nothing armed");
        let watchdog_cell = Arc::clone(&cell);
        let watchdog = thread::spawn(move || watchdog_cell.request(Interrupt::Timeout(first)));

        let halt = cell.end(first);
        let second = cell.begin(DispatchKind::Script).expect("nothing armed");
        assert!(!isolate.terminating());
        let fired = watchdog.join().expect("watchdog thread");

        assert!(!isolate.terminating());
        assert_eq!(cell.end(second), None);
        assert_eq!(
            halt == Some(Halt::Terminated(TerminationCause::Timeout)),
            fired
        );
    });
}

// Model 9: a stop is reported by exactly one script, whenever it lands, and
// maintenance never consumes it.
#[test]
fn a_stop_is_reported_exactly_once() {
    loom::model(|| {
        let (cell, isolate) = term_cell(None);
        let stopper_cell = Arc::clone(&cell);
        let stopper = thread::spawn(move || stopper_cell.request(Interrupt::Stop));

        let maintenance = run(&cell, DispatchKind::Maintenance);
        let first = run(&cell, DispatchKind::Script);
        stopper.join().expect("stopper thread");
        let second = run(&cell, DispatchKind::Script);

        let stopped = Some(Halt::Terminated(TerminationCause::Stop));
        assert_eq!(maintenance, None);
        assert_eq!(
            [first, second]
                .into_iter()
                .filter(|halt| *halt == stopped)
                .count(),
            1
        );
        assert!(!isolate.terminating());
    });
}
