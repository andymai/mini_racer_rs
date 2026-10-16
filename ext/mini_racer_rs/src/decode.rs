use magnus::value::ReprValue;
use magnus::{
    Class, Error, Fixnum, IntoValue, Module, Object, RArray, RClass, RHash, RModule, RObject,
    RString, Ruby, Value,
};
use mini_racer_wire::{
    Begun, Container, DecodeError, Limits, Scalar, Text, Visitor, decode_value, marker, text,
};

use crate::gc_root::StaticRoot;
use crate::{errors, rb_raw};

static JAVASCRIPT_FUNCTION: StaticRoot<RClass> = StaticRoot::new(|ruby| {
    ruby.class_object()
        .const_get::<_, RModule>("MiniRacer")
        .and_then(|racer| racer.const_get("JavaScriptFunction"))
        .expect("init defines MiniRacer::JavaScriptFunction")
});

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

// mini_racer's Ruby shapes: an array drops its named properties and a sparse
// array its indexes, a set becomes an Array, a map keeps its keys as they are,
// an object's keys become Strings, and an error becomes a ScriptError.
enum Frame {
    Array(RArray),
    Sparse { array: RArray, named: bool },
    Object(RHash),
    Map(RHash),
    Error(ErrorParts),
}

#[derive(Clone, Copy)]
struct ErrorParts {
    error: Value,
    message: Option<Value>,
    stack: Option<Value>,
    cause: Option<Value>,
}

// The visitor and its frames live on the native stack for the whole decode,
// where Ruby's GC finds them (I3); `refs` holds every value V8 gave an id.
struct RubyVisitor<'r> {
    ruby: &'r Ruby,
    refs: RArray,
}

impl RubyVisitor<'_> {
    fn bad_state<T>(&self) -> Result<T, Error> {
        Err(errors::runtime(self.ruby, "bad state"))
    }

    fn string(&self, text: Text<'_>) -> Result<Value, Error> {
        let ruby = self.ruby;
        Ok(match text {
            Text::Utf8(bytes) => ruby.enc_str_new(bytes, ruby.utf8_encoding()).as_value(),
            Text::Latin1(bytes) if bytes.is_ascii() => {
                ruby.enc_str_new(bytes, ruby.utf8_encoding()).as_value()
            }
            Text::Latin1(bytes) => ruby.str_new(&text::latin1_to_utf8(bytes)).as_value(),
            Text::Utf16Le(bytes) if marker::is_javascript_function(bytes) => {
                JAVASCRIPT_FUNCTION.get(ruby).new_instance(())?
            }
            Text::Utf16Le(bytes) => {
                if let Some(utf8) = text::utf16le_to_utf8(bytes) {
                    ruby.str_new(&utf8).as_value()
                } else {
                    let utf16 = ruby
                        .find_encoding("UTF-16LE")
                        .ok_or_else(|| errors::internal(ruby, "no UTF-16LE encoding"))?;
                    ruby.enc_str_new(bytes, utf16).as_value()
                }
            }
        })
    }

    // mini_racer splits a millisecond time into seconds and microseconds,
    // truncating each, and lets Time normalize a negative remainder.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a finite JavaScript time is within 8.64e15 ms"
    )]
    fn time(&self, millis: f64) -> Result<Value, Error> {
        if !millis.is_finite() {
            return Err(errors::raised(
                self.ruby,
                self.ruby.exception_range_error(),
                "invalid Date",
            ));
        }
        let seconds = (millis / 1e3) as i64;
        let micros = (1e3 * (millis % 1e3)) as i64;
        Ok(self.ruby.time_new(seconds, micros)?.as_value())
    }

    fn script_error(&self, parts: ErrorParts) -> Result<Value, Error> {
        let ruby = self.ruby;
        // mini_racer builds both from C strings, so they are binary.
        let mut message = parts
            .message
            .unwrap_or_else(|| ruby.str_from_slice(b"JS exception").as_value());
        if let Some(stack) = parts.stack {
            message = message.funcall("concat", (ruby.str_from_slice(b"\n"), stack))?;
        }
        let _: Value = parts.error.funcall("initialize", (message,))?;
        if let Some(cause) = parts.cause.filter(|cause| !cause.is_nil()) {
            RObject::from_value(parts.error)
                .ok_or_else(|| errors::internal(ruby, "error is not an object"))?
                .ivar_set("@cause", cause)?;
        }
        Ok(parts.error)
    }
}

fn is_index(key: Value) -> bool {
    Fixnum::from_value(key).is_some_and(|key| i32::try_from(key.to_i64()).is_ok())
}

impl Visitor for RubyVisitor<'_> {
    type Value = Value;
    type Frame = Frame;
    type Error = Error;

    fn scalar(&mut self, scalar: Scalar<'_>) -> Result<Value, Error> {
        let ruby = self.ruby;
        Ok(match scalar {
            Scalar::Undefined | Scalar::Null | Scalar::Hole => ruby.qnil().as_value(),
            Scalar::Bool(b) => b.into_value_with(ruby),
            Scalar::Int32(n) => ruby.integer_from_i64(n.into()).as_value(),
            Scalar::Uint32(n) => ruby.integer_from_u64(n.into()).as_value(),
            Scalar::Number(n) => match classify(n) {
                Number::Integer(i) => ruby.integer_from_i64(i).as_value(),
                Number::BigInteger => rb_raw::dbl2big(ruby, n),
                Number::Float => ruby.float_from_f64(n).as_value(),
            },
            Scalar::BigInt { negative, digits } => rb_raw::integer_unpack(ruby, digits, negative)?,
            Scalar::Text(text) | Scalar::RegExp { pattern: text, .. } => self.string(text)?,
            Scalar::Date(millis) => self.time(millis)?,
            Scalar::ArrayBuffer(bytes) => ruby.str_from_slice(bytes).as_value(),
        })
    }

    fn begin(&mut self, container: Container) -> Result<Begun<Frame, Value>, Error> {
        let ruby = self.ruby;
        let frame = match container {
            Container::Values | Container::Set => Frame::Array(ruby.ary_new()),
            Container::DenseArray { length } => {
                Frame::Array(ruby.ary_new_capa(usize::try_from(length).unwrap_or(0)))
            }
            Container::SparseArray { .. } => Frame::Sparse {
                array: ruby.ary_new(),
                named: false,
            },
            Container::Object => Frame::Object(ruby.hash_new()),
            Container::Map => Frame::Map(ruby.hash_new()),
            // Allocated now so a cause can refer back to the error itself.
            Container::Error => Frame::Error(ErrorParts {
                error: errors::script_error(ruby).obj_alloc()?.as_value(),
                message: None,
                stack: None,
                cause: None,
            }),
        };
        let value = match &frame {
            Frame::Array(array) | Frame::Sparse { array, .. } => array.as_value(),
            Frame::Object(hash) | Frame::Map(hash) => hash.as_value(),
            Frame::Error(parts) => parts.error,
        };
        Ok(Begun {
            frame,
            value: Some(value),
        })
    }

    fn push(&mut self, frame: &mut Frame, value: Value) -> Result<(), Error> {
        match frame {
            Frame::Array(array) | Frame::Sparse { array, .. } => array.push(value),
            Frame::Object(_) | Frame::Map(_) | Frame::Error(_) => self.bad_state(),
        }
    }

    fn insert(&mut self, frame: &mut Frame, key: Value, value: Value) -> Result<(), Error> {
        match frame {
            Frame::Array(_) => Ok(()),
            Frame::Sparse { array, named } => {
                if !*named && is_index(key) {
                    array.push(value)
                } else {
                    *named = true;
                    Ok(())
                }
            }
            Frame::Object(hash) => match RString::from_value(key) {
                Some(key) => hash.aset(key, value),
                None => hash.aset(key.funcall::<_, _, Value>("to_s", ())?, value),
            },
            Frame::Map(hash) => hash.aset(key, value),
            Frame::Error(parts) => {
                let field = RString::from_value(key)
                    .map(|key| rb_raw::string_bytes(self.ruby, key))
                    .unwrap_or_default();
                match field.as_slice() {
                    b"message" => parts.message = Some(value),
                    b"stack" => parts.stack = Some(value),
                    b"cause" => parts.cause = Some(value),
                    _ => return self.bad_state(),
                }
                Ok(())
            }
        }
    }

    fn end(&mut self, frame: Frame) -> Result<Value, Error> {
        match frame {
            Frame::Array(array) | Frame::Sparse { array, .. } => Ok(array.as_value()),
            Frame::Object(hash) | Frame::Map(hash) => Ok(hash.as_value()),
            Frame::Error(parts) => self.script_error(parts),
        }
    }

    fn register(&mut self, id: u32, value: Value) -> Result<(), Error> {
        self.refs
            .store(isize::try_from(id).unwrap_or(isize::MAX), value)
    }

    fn back_ref(&mut self, id: u32) -> Result<Value, Error> {
        self.refs.entry(isize::try_from(id).unwrap_or(isize::MAX))
    }
}

// mini_racer decodes a result inside its [result, error] reply, one level
// below its limit.
const RESULT_LIMITS: Limits = Limits {
    max_depth: 95,
    max_bigint_bytes: 16 << 20,
};

pub fn value(ruby: &Ruby, bytes: &[u8]) -> Result<Value, Error> {
    let mut visitor = RubyVisitor {
        ruby,
        refs: ruby.ary_new(),
    };
    decode_value(bytes, RESULT_LIMITS, &mut visitor).map_err(|e| match e {
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
