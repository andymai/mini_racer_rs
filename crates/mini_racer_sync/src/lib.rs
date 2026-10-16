#![forbid(unsafe_code)]

mod lock;
pub mod primitives;

pub use lock::{Poisoned, lock_or_poison};
