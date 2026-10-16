// Protocol code names only these, so loom models check the same code that ships.
#[cfg(loom)]
pub use loom::sync::{Mutex, MutexGuard};
#[cfg(not(loom))]
pub use std::sync::{Mutex, MutexGuard};
