#![forbid(unsafe_code)]
#![deny(
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::indexing_slicing
)]

mod map;
mod rewrite;
mod vlq;

pub use map::{MapError, Original, Placement, SourceMap, check_length};
pub use rewrite::{Frame, Match, rewrite};
