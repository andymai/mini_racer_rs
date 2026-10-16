#![cfg(loom)]

use loom::sync::Arc;
use loom::thread;
use mini_racer_sync::lock_or_poison;
use mini_racer_sync::primitives::Mutex;

#[test]
fn lock_or_poison_serializes_writers() {
    loom::model(|| {
        let counter = Arc::new(Mutex::new(0_u32));
        let other = Arc::clone(&counter);
        let writer = thread::spawn(move || {
            *lock_or_poison(&other).expect("no writer panics") += 1;
        });
        *lock_or_poison(&counter).expect("no writer panics") += 1;
        writer.join().expect("writer thread");

        assert_eq!(*lock_or_poison(&counter).expect("no writer panics"), 2);
    });
}
