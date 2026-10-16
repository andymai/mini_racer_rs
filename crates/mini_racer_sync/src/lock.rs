use crate::primitives::{Mutex, MutexGuard};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("a thread panicked while holding this lock")]
pub struct Poisoned;

pub fn lock_or_poison<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, Poisoned> {
    mutex.lock().map_err(|_| Poisoned)
}

#[cfg(all(test, not(loom)))]
mod tests {
    use super::*;

    #[test]
    fn an_uncontended_lock_succeeds() {
        let mutex = Mutex::new(1);

        assert_eq!(lock_or_poison(&mutex).map(|guard| *guard), Ok(1));
    }

    #[test]
    fn a_panic_while_held_poisons_the_lock() {
        let mutex = Mutex::new(0);
        let unwound = std::panic::catch_unwind(|| {
            let _guard = lock_or_poison(&mutex);
            std::panic::resume_unwind(Box::new("unwinding while the lock is held"));
        });

        assert!(unwound.is_err());
        assert!(matches!(lock_or_poison(&mutex), Err(Poisoned)));
    }
}
