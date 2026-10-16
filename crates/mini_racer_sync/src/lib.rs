#![forbid(unsafe_code)]

mod handoff;
mod lock;
pub mod primitives;

pub use handoff::{Handoff, Outcome};
pub use lock::lock;
