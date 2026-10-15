use magnus::value::ReprValue;
use magnus::{Error, IntoValue, Ruby, Value};
use mini_racer_wire::{DecodeError, Scalar, Visitor, decode_value, text};

use crate::{errors, rb_raw};

#[derive(Debug, Clone, Copy, PartialEq)]
enum Number {
    Integer(i64),
    BigInteger,
    Float,
}

// Upstream returns any finite integral double as an Integer (-0.0 included).
#[allow(
    clippy::float_cmp,
    reason = "fract() is exact; an integral double has a zero fraction"
)]
#[allow(
    clippy::cast_possible_truncation,
    reason = "range-checked against i64 first"
)]
fn classify(n: f64) -> Number {
    const I64_END: f64 = 9_223_372_036_854_775_808.0;
    if !n.is_finite() || n.fract() != 0.0 {
        Number::Float
    } else if (-I64_END..I64_END).contains(&n) {
        Number::Integer(n as i64)
    } else {
        Number::BigInteger
    }
}

struct RubyVisitor<'r> {
    ruby: &'r Ruby,
}

impl Visitor for RubyVisitor<'_> {
    type Value = Value;
    type Error = Error;

    fn scalar(&mut self, scalar: Scalar<'_>) -> Result<Value, Error> {
        let ruby = self.ruby;
        Ok(match scalar {
            Scalar::Undefined | Scalar::Null => ruby.qnil().as_value(),
            Scalar::Bool(b) => b.into_value_with(ruby),
            Scalar::Int32(n) => ruby.integer_from_i64(n.into()).as_value(),
            Scalar::Uint32(n) => ruby.integer_from_u64(n.into()).as_value(),
            Scalar::Number(n) => match classify(n) {
                Number::Integer(i) => ruby.integer_from_i64(i).as_value(),
                Number::BigInteger => rb_raw::dbl2big(ruby, n),
                Number::Float => ruby.float_from_f64(n).as_value(),
            },
            Scalar::Utf8(bytes) => ruby.enc_str_new(bytes, ruby.utf8_encoding()).as_value(),
            Scalar::Latin1(bytes) if bytes.is_ascii() => {
                ruby.enc_str_new(bytes, ruby.utf8_encoding()).as_value()
            }
            Scalar::Latin1(bytes) => ruby.str_new(&text::latin1_to_utf8(bytes)).as_value(),
            Scalar::Utf16Le(bytes) => {
                if let Some(utf8) = text::utf16le_to_utf8(bytes) {
                    ruby.str_new(&utf8).as_value()
                } else {
                    let utf16 = ruby
                        .find_encoding("UTF-16LE")
                        .ok_or_else(|| errors::internal(ruby, "no UTF-16LE encoding".to_owned()))?;
                    ruby.enc_str_new(bytes, utf16).as_value()
                }
            }
        })
    }
}

pub fn value(ruby: &Ruby, bytes: &[u8]) -> Result<Value, Error> {
    decode_value(bytes, &mut RubyVisitor { ruby }).map_err(|e| match e {
        DecodeError::Visitor(error) => error,
        other => errors::runtime(ruby, other.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_numbers_like_upstream() {
        assert_eq!(classify(2.0), Number::Integer(2));
        assert_eq!(classify(-0.0), Number::Integer(0));
        assert_eq!(classify(1.5), Number::Float);
        assert_eq!(classify(f64::NAN), Number::Float);
        assert_eq!(classify(f64::INFINITY), Number::Float);
        assert_eq!(classify(1e300), Number::BigInteger);
        assert_eq!(
            classify(-9_223_372_036_854_775_808.0),
            Number::Integer(i64::MIN)
        );
        assert_eq!(classify(9_223_372_036_854_775_808.0), Number::BigInteger);
    }
}
