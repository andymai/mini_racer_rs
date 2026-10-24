#![no_main]

use std::convert::Infallible;

use libfuzzer_sys::fuzz_target;
use mini_racer_wire::{Begun, Container, Limits, Scalar, Visitor, decode_value};

struct Discard;

impl Visitor for Discard {
    type Value = ();
    type Frame = ();
    type Error = Infallible;

    fn scalar(&mut self, _scalar: Scalar<'_>) -> Result<(), Infallible> {
        Ok(())
    }

    fn begin(&mut self, _container: Container) -> Result<Begun<(), ()>, Infallible> {
        Ok(Begun {
            frame: (),
            value: Some(()),
        })
    }

    fn push(&mut self, (): &mut (), (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    fn insert(&mut self, (): &mut (), (): (), (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    fn end(&mut self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    fn register(&mut self, _id: u32, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    fn back_ref(&mut self, _id: u32) -> Result<(), Infallible> {
        Ok(())
    }
}

// The tight limits reach the depth and BigInt checks with short inputs.
fuzz_target!(|data: &[u8]| {
    let tight = Limits {
        max_depth: 8,
        max_bigint_bytes: 64,
    };
    let _ = decode_value(data, Limits::default(), &mut Discard);
    let _ = decode_value(data, tight, &mut Discard);
});
