#![forbid(unsafe_code)]
#![deny(
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::indexing_slicing
)]

mod decode;
pub mod marker;
pub mod text;
mod write;

use std::ops::RangeInclusive;

pub use decode::{
    Begun, Container, DecodeError, Limits, Scalar, Text, Visitor, decode_value, decode_values,
};
pub use write::{WriteError, Writer};

pub const READ_VERSIONS: RangeInclusive<u32> = 13..=16;
pub const WRITE_VERSION: u32 = 15;

mod tag {
    pub const HEADER: u8 = 0xFF;
    pub const PADDING: u8 = 0x00;
    pub const VERIFY_OBJECT_COUNT: u8 = b'?';
    pub const THE_HOLE: u8 = b'-';
    pub const UNDEFINED: u8 = b'_';
    pub const NULL: u8 = b'0';
    pub const TRUE: u8 = b'T';
    pub const FALSE: u8 = b'F';
    pub const INT32: u8 = b'I';
    pub const UINT32: u8 = b'U';
    pub const DOUBLE: u8 = b'N';
    pub const BIGINT: u8 = b'Z';
    pub const UTF8_STRING: u8 = b'S';
    pub const ONE_BYTE_STRING: u8 = b'"';
    pub const TWO_BYTE_STRING: u8 = b'c';
    pub const OBJECT_REFERENCE: u8 = b'^';
    pub const BEGIN_OBJECT: u8 = b'o';
    pub const END_OBJECT: u8 = b'{';
    pub const BEGIN_SPARSE_ARRAY: u8 = b'a';
    pub const END_SPARSE_ARRAY: u8 = b'@';
    pub const BEGIN_DENSE_ARRAY: u8 = b'A';
    pub const END_DENSE_ARRAY: u8 = b'$';
    pub const DATE: u8 = b'D';
    pub const TRUE_OBJECT: u8 = b'y';
    pub const FALSE_OBJECT: u8 = b'x';
    pub const NUMBER_OBJECT: u8 = b'n';
    pub const BIGINT_OBJECT: u8 = b'z';
    pub const STRING_OBJECT: u8 = b's';
    pub const REGEXP: u8 = b'R';
    pub const BEGIN_MAP: u8 = b';';
    pub const END_MAP: u8 = b':';
    pub const BEGIN_SET: u8 = b'\'';
    pub const END_SET: u8 = b',';
    pub const ARRAY_BUFFER: u8 = b'B';
    pub const IMMUTABLE_ARRAY_BUFFER: u8 = b'C';
    pub const RESIZABLE_ARRAY_BUFFER: u8 = b'~';
    pub const ARRAY_BUFFER_VIEW: u8 = b'V';
    pub const ERROR: u8 = b'r';
    pub const ERROR_MESSAGE: u8 = b'm';
    pub const ERROR_STACK: u8 = b's';
    pub const ERROR_CAUSE: u8 = b'c';
    pub const ERROR_END: u8 = b'.';
    pub const UINT8_ARRAY: u8 = b'B';
}
