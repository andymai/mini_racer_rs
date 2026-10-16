#![cfg(loom)]

use std::time::Duration;

use loom::sync::Arc;
use loom::thread;
use mini_racer_sync::{Handoff, Outcome};

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

        if handoff.wait() == Outcome::Interrupted {
            assert_eq!(handoff.wait(), Outcome::Done(7));
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
