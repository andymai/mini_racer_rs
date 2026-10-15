#![forbid(unsafe_code)]
#![deny(
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::indexing_slicing
)]

mod decode;
pub mod text;

use std::ops::RangeInclusive;

pub use decode::{DecodeError, Scalar, Visitor, decode_value};

pub const READ_VERSIONS: RangeInclusive<u32> = 13..=16;
pub const WRITE_VERSION: u32 = 15;
