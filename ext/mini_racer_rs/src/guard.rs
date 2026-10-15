use std::panic::{AssertUnwindSafe, catch_unwind};

pub struct Panicked;

pub fn guard<R>(f: impl FnOnce() -> R) -> Result<R, Panicked> {
    catch_unwind(AssertUnwindSafe(f)).map_err(|_| Panicked)
}
