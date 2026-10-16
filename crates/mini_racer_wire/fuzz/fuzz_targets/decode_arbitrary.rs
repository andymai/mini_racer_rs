#![no_main]

use std::convert::Infallible;

use libfuzzer_sys::fuzz_target;
use mini_racer_wire::text::{latin1_to_utf8, utf16le_to_utf8};
use mini_racer_wire::{Scalar, Visitor, decode_value};

struct Transcode;

impl Visitor for Transcode {
    type Value = ();
    type Error = Infallible;

    fn scalar(&mut self, scalar: Scalar<'_>) -> Result<(), Infallible> {
        match scalar {
            Scalar::Latin1(bytes) => assert_eq!(latin1_to_utf8(bytes).chars().count(), bytes.len()),
            Scalar::Utf16Le(bytes) => {
                let _ = utf16le_to_utf8(bytes);
            }
            _ => {}
        }
        Ok(())
    }
}

fuzz_target!(|data: &[u8]| {
    let _ = decode_value(data, &mut Transcode);
});
