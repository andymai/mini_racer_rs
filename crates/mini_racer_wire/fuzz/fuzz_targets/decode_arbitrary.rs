#![no_main]

use std::convert::Infallible;

use libfuzzer_sys::fuzz_target;
use mini_racer_wire::{Scalar, Visitor, decode_value};

struct Discard;

impl Visitor for Discard {
    type Value = ();
    type Error = Infallible;

    fn scalar(&mut self, _scalar: Scalar<'_>) -> Result<(), Infallible> {
        Ok(())
    }
}

fuzz_target!(|data: &[u8]| {
    let _ = decode_value(data, &mut Discard);
});
