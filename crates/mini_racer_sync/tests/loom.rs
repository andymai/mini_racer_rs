#![cfg(loom)]

use std::time::Duration;

use loom::sync::Arc;
use loom::sync::atomic::{AtomicBool, Ordering};
use loom::thread;
use mini_racer_sync::primitives::{Condvar, Mutex};
use mini_racer_sync::{
    Channel, ChannelError, DispatchKind, ForkGate, Halt, Interrupt, Nesting, Notifier, Phase,
    Protocol, RunnerPhase, Side, TaskQueue, TermCell, TerminationCause, Terminator, ToRuby, ToV8,
    lock, wait,
};

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
    tasks: Option<Arc<TaskQueue<u8>>>,
}

impl Terminator for Handle {
    fn terminate(&self) {
        self.isolate.terminating.store(true, Ordering::SeqCst);
        if let Some(tasks) = &self.tasks {
            tasks.ring();
        }
    }

    fn cancel(&self) {
        self.isolate.terminating.store(false, Ordering::SeqCst);
    }
}

fn term_cell(tasks: Option<Arc<TaskQueue<u8>>>) -> (Arc<TermCell<Handle>>, Arc<Isolate>) {
    let isolate = Arc::new(Isolate::default());
    let cell = Arc::new(TermCell::default());
    cell.install(Handle {
        isolate: Arc::clone(&isolate),
        tasks,
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

// Stands in for the descriptor a fiber waits on: level-triggered, it stays
// rung until drained, and only the channel's notifier rings it.
#[derive(Default)]
struct Doorbell {
    rung: Mutex<bool>,
    ring: Condvar,
}

impl Doorbell {
    fn drain(&self) {
        *lock(&self.rung) = false;
    }

    fn wait(&self) {
        let mut rung = lock(&self.rung);
        while !*rung {
            rung = wait(&self.ring, rung);
        }
    }
}

struct Ring(Arc<Doorbell>);

impl Notifier for Ring {
    fn notify(&self) {
        *lock(&self.0.rung) = true;
        self.0.ring.notify_one();
    }
}

// Model 6: a waiter that drains its descriptor, then waits on it with
// `park_notified`, never sleeps through the response or a callback.
#[test]
fn a_waiter_woken_by_the_notifier_never_misses_its_turn() {
    bounded(3, || {
        let channel = Arc::new(Channel::<Numbers>::default());
        let bell = Arc::new(Doorbell::default());
        channel.set_notifier(Side::Ruby, Some(Box::new(Ring(Arc::clone(&bell)))));
        let runner = Arc::clone(&channel);
        let v8 = thread::spawn(move || {
            let value = request(next_for_v8(&runner)).expect("a request");
            runner.callback(value + 1).expect("open");
            let value = reply(next_for_v8(&runner)).expect("a reply");
            runner.respond(value + 1).expect("open");
        });

        channel.request(1).expect("open");
        let next = || loop {
            if let Some(message) = channel.recv_ruby().expect("open") {
                return message;
            }
            bell.drain();
            channel.park_notified(Side::Ruby, || bell.wait());
        };
        assert_eq!(callback(Ok(next())), Some(2));
        channel.reply(3).expect("open");
        assert_eq!(response(Ok(next())), Some(4));
        v8.join().expect("runner thread");
    });
}

// Model 7: a fork that finds no runner `Running` never overlaps a runner using
// its isolate, whether the runner wakes just before the fork begins or just
// after, until the fork ends.
#[test]
fn a_quiesced_fork_never_overlaps_a_runner_using_its_isolate() {
    bounded(3, || {
        let gate = Arc::new(ForkGate::default());
        let phase = Arc::new(RunnerPhase::default());
        let using = Arc::new(AtomicBool::new(false));
        phase.set(Phase::Deposited);
        let runner = {
            let (gate, phase, using) = (Arc::clone(&gate), Arc::clone(&phase), Arc::clone(&using));
            thread::spawn(move || {
                gate.resume(&phase);
                using.store(true, Ordering::SeqCst);
                using.store(false, Ordering::SeqCst);
                phase.set(Phase::Deposited);
            })
        };

        gate.begin();
        while phase.get() == Phase::Running {
            thread::yield_now();
        }
        assert!(!using.load(Ordering::SeqCst));
        thread::yield_now();
        assert!(!using.load(Ordering::SeqCst));
        gate.end();
        runner.join().expect("runner thread");
    });
}

// Model 10: a runner interrupted in JavaScript for a fork holds no isolate
// until that fork ends, parks again when a second fork begins before it
// wakes, and an interrupt that lands after its fork ended returns at once.
#[test]
fn an_interrupted_runner_parks_for_its_fork_and_no_longer() {
    bounded(3, || {
        let gate = Arc::new(ForkGate::default());
        let phase = Arc::new(RunnerPhase::default());
        let using = Arc::new(AtomicBool::new(true));
        let interrupt = Arc::new(AtomicBool::new(false));
        let runner = {
            let (gate, phase) = (Arc::clone(&gate), Arc::clone(&phase));
            let (using, interrupt) = (Arc::clone(&using), Arc::clone(&interrupt));
            thread::spawn(move || {
                // Where V8 checks for interrupts while the script runs.
                for _ in 0..3 {
                    if interrupt.swap(false, Ordering::SeqCst) {
                        using.store(false, Ordering::SeqCst);
                        gate.pause(&phase);
                        using.store(true, Ordering::SeqCst);
                    }
                    thread::yield_now();
                }
                using.store(false, Ordering::SeqCst);
                phase.set(Phase::Deposited);
            })
        };

        for _ in 0..2 {
            gate.begin();
            interrupt.store(true, Ordering::SeqCst);
            while phase.get() == Phase::Running {
                thread::yield_now();
            }
            assert!(!using.load(Ordering::SeqCst));
            thread::yield_now();
            assert!(!using.load(Ordering::SeqCst));
            gate.end();
        }
        interrupt.store(true, Ordering::SeqCst);
        runner.join().expect("runner thread");
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

// Model 11: a stop landing while an awaiting runner checks for one and goes
// to wait for a task still wakes it.
#[test]
fn a_stop_wakes_a_runner_awaiting_a_task() {
    loom::model(|| {
        let tasks = Arc::new(TaskQueue::default());
        let (cell, isolate) = term_cell(Some(Arc::clone(&tasks)));
        let id = cell.begin(DispatchKind::Script).expect("nothing armed");
        let stopper_cell = Arc::clone(&cell);
        let stopper = thread::spawn(move || stopper_cell.request(Interrupt::Stop));

        while cell.check(DispatchKind::Script).is_none() {
            tasks.wait();
        }
        stopper.join().expect("stopper thread");

        assert_eq!(cell.end(id), Some(Halt::Terminated(TerminationCause::Stop)));
        assert!(!isolate.terminating());
    });
}

// Model 12: a task posted from another thread while the runner finds the
// queue empty and goes to wait is never slept through.
#[test]
fn a_task_posted_while_the_runner_waits_is_never_missed() {
    loom::model(|| {
        let tasks = Arc::new(TaskQueue::default());
        let poster_tasks = Arc::clone(&tasks);
        let poster =
            thread::spawn(move || poster_tasks.post(7, true, Duration::ZERO).expect("open"));

        let task = loop {
            if let Some(task) = tasks.pop(false) {
                break task;
            }
            tasks.wait();
        };
        poster.join().expect("poster thread");

        assert_eq!(task, 7);
    });
}

// Model 13: a post racing the queue's close lands in exactly one place: in
// what close hands back, or refused back to the poster.
#[test]
fn a_post_racing_close_is_handed_back_exactly_once() {
    loom::model(|| {
        let tasks = Arc::new(TaskQueue::default());
        let poster_tasks = Arc::clone(&tasks);
        let poster = thread::spawn(move || poster_tasks.post(7, true, Duration::ZERO));

        let closed = tasks.close();
        let refused = poster.join().expect("poster thread");

        match refused {
            Ok(()) => assert_eq!(closed, [7]),
            Err(task) => {
                assert_eq!(task, 7);
                assert!(closed.is_empty());
            }
        }
        assert_eq!(tasks.pop(false), None);
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

// Model 14: a nested script's own timeout firing as the script ends never
// leaves the untimed dispatch around it terminated.
#[test]
fn a_nested_timeout_racing_its_end_leaves_the_outer_dispatch_running() {
    loom::model(|| {
        let (cell, isolate) = term_cell(None);
        let pump = cell.begin(DispatchKind::Pump).expect("nothing armed");
        let nested = cell.time_nested().expect("an untimed dispatch");
        let watchdog_cell = Arc::clone(&cell);
        let watchdog = thread::spawn(move || watchdog_cell.request(Interrupt::Timeout(nested)));

        cell.untime_nested(nested);
        watchdog.join().expect("watchdog thread");

        assert!(!isolate.terminating());
        assert_eq!(cell.check(DispatchKind::Script), None);
        assert_eq!(cell.end(pump), None);
    });
}

// Model 15: a timeout landing while a settled dispatch serializes its result
// neither replaces that result nor terminates the next dispatch.
#[test]
fn a_timeout_while_serializing_leaves_the_result_and_the_next_dispatch() {
    loom::model(|| {
        let (cell, isolate) = term_cell(None);
        let first = cell.begin(DispatchKind::Script).expect("nothing armed");
        assert_eq!(cell.settle(first), None);
        let watchdog_cell = Arc::clone(&cell);
        let watchdog = thread::spawn(move || watchdog_cell.request(Interrupt::Timeout(first)));

        assert_eq!(cell.finish(first), None);
        let second = cell.begin(DispatchKind::Script).expect("nothing armed");
        watchdog.join().expect("watchdog thread");

        assert!(!isolate.terminating());
        assert_eq!(cell.end(second), None);
    });
}
