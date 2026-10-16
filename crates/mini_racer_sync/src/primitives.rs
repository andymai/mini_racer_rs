// Protocol types name only these, so the loom models in tests/loom.rs check
// the same code the engine runs.
#[cfg(loom)]
pub use loom::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(loom)]
pub use loom::sync::{Condvar, Mutex, MutexGuard};
#[cfg(not(loom))]
pub use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(not(loom))]
pub use std::sync::{Condvar, Mutex, MutexGuard};
