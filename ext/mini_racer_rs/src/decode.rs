use std::cmp::Ordering;
use std::ops::Range;

use magnus::typed_data::Obj;
use magnus::value::ReprValue;
use magnus::{
    Class, Error, Fixnum, IntoValue, Module, Object, RArray, RClass, RHash, RModule, RObject,
    RString, Ruby, Value,
};
use mini_racer_wire::{
    Begun, Container, DecodeError, Limits, Scalar, Text, Visitor, decode_value, marker, text,
};

use crate::gc_root::{Spill, StaticRoot};
use crate::{errors, rb_raw};

#[expect(
    clippy::expect_used,
    reason = "init defines the class, and a root initializer cannot return an error"
)]
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

const BATCH: usize = 64;

// Decoded elements wait here so a batch of them costs one rb_protect rather
// than one per value. Every container's begin and end empties it, so only the
// innermost open array ever has elements waiting, and whatever a child does
// (a Map hashing an ancestor as its key) sees each array with every element
// decoded so far, as mini_racer builds them.
struct Pending {
    target: Option<RArray>,
    values: [Value; BATCH],
    filled: usize,
}

const PAIRS: usize = 128;

// Every open hash's pairs wait here, innermost last, until the hash ends and
// takes them in one rb_hash_bulk_insert while it is still empty, so Ruby sizes
// its table once, as json 3.1's parser builds objects. A hash's run starts
// with the hash and the start of the run around it. Past PAIRS values the
// runs move to a Spill, whose mark pins them.
struct Pairs {
    values: [Value; PAIRS],
    spill: Option<Obj<Spill>>,
    len: usize,
    innermost: Option<usize>,
    // Every pair below this is in its hash already.
    inserted: usize,
}

impl Pairs {
    fn new(ruby: &Ruby) -> Self {
        Self {
            values: [ruby.qnil().as_value(); PAIRS],
            spill: None,
            len: 0,
            innermost: None,
            inserted: 0,
        }
    }

    fn push(&mut self, ruby: &Ruby, value: Value) {
        if self.spill.is_none() {
            if let Some(slot) = self.values.get_mut(self.len) {
                *slot = value;
                self.len = self.len.saturating_add(1);
                return;
            }
            self.spill = Some(Spill::new(ruby, &self.values));
        }
        if let Some(spill) = &self.spill {
            spill.push(value);
        }
        self.len = self.len.saturating_add(1);
    }

    fn get(&self, ruby: &Ruby, at: usize) -> Option<Value> {
        self.spill
            .as_ref()
            .map_or_else(|| self.values.get(at).copied(), |spill| spill.get(ruby, at))
    }

    fn open(&mut self, ruby: &Ruby, hash: RHash) {
        let outer = self
            .innermost
            .and_then(|at| i64::try_from(at).ok())
            .unwrap_or(-1);
        self.innermost = Some(self.len);
        self.push(ruby, hash.as_value());
        self.push(ruby, ruby.integer_from_i64(outer).as_value());
    }

    fn outer(&self, ruby: &Ruby, at: usize) -> Option<usize> {
        self.get(ruby, at.saturating_add(1))
            .and_then(Fixnum::from_value)
            .and_then(|outer| usize::try_from(outer.to_i64()).ok())
    }

    // An empty table takes the pairs at once; one that holds pairs already
    // takes them one by one, since rb_hash_bulk_insert would rehash it whole.
    fn put(&self, ruby: &Ruby, hash: RHash, range: Range<usize>) -> Result<(), Error> {
        if range.is_empty() {
            return Ok(());
        }
        let put = |pairs: &[Value]| {
            if hash.is_empty() {
                hash.bulk_insert(pairs)
            } else {
                rb_raw::hash_aset_pairs(ruby, hash, pairs.as_chunks::<2>().0)
            }
        };
        match &self.spill {
            Some(spill) => spill.with_values(ruby, range, put),
            None => put(self.values.get(range).unwrap_or_default()),
        }
    }

    // Puts the innermost hash's waiting pairs in and ends its run.
    fn close(&mut self, ruby: &Ruby, hash: RHash) -> Result<(), Error> {
        let Some(at) = self.innermost else {
            return Err(errors::internal(ruby, "decode pairs without a hash"));
        };
        self.put(
            ruby,
            hash,
            at.saturating_add(2).max(self.inserted)..self.len,
        )?;
        self.innermost = self.outer(ruby, at);
        self.len = at;
        self.inserted = self.inserted.min(at);
        if let Some(spill) = &self.spill {
            spill.truncate(at);
        }
        Ok(())
    }

    // Puts every open hash's waiting pairs in, innermost first.
    fn put_all(&mut self, ruby: &Ruby) -> Result<(), Error> {
        let mut end = self.len;
        let mut run = self.innermost;
        while let Some(at) = run
            && end > self.inserted
        {
            let hash = self
                .get(ruby, at)
                .and_then(RHash::from_value)
                .ok_or_else(|| errors::internal(ruby, "decode pairs without a hash"))?;
            self.put(ruby, hash, at.saturating_add(2).max(self.inserted)..end)?;
            end = at;
            run = self.outer(ruby, at);
        }
        self.inserted = self.len;
        Ok(())
    }
}

// json 3.1's parser keeps the names of objects inside arrays, which repeat,
// for the rest of a parse: at most this many, each at most CACHED_KEY_BYTES
// long and starting with a letter.
const CACHED_KEYS: usize = 63;
const CACHED_KEY_BYTES: usize = 55;

// Interned keys sorted by length, then bytes, so a repeated key skips Ruby's
// fstring table.
struct KeyCache {
    keys: [Value; CACHED_KEYS],
    len: usize,
}

impl KeyCache {
    fn fetch(&mut self, ruby: &Ruby, bytes: &[u8]) -> Result<Value, Error> {
        let held = self.keys.get(..self.len).unwrap_or_default();
        let found = held.binary_search_by(|&key| {
            RString::from_value(key).map_or(Ordering::Less, |key| {
                rb_raw::with_string_bytes(ruby, key, |key| {
                    key.len().cmp(&bytes.len()).then_with(|| key.cmp(bytes))
                })
            })
        });
        let at = match found {
            Ok(at) => {
                return held
                    .get(at)
                    .copied()
                    .ok_or_else(|| errors::internal(ruby, "key cache"));
            }
            Err(at) => at,
        };
        let key = rb_raw::interned_utf8(ruby, bytes)?.as_value();
        if self.len < CACHED_KEYS {
            self.keys.copy_within(at..self.len, at.saturating_add(1));
            if let Some(slot) = self.keys.get_mut(at) {
                *slot = key;
            }
            self.len = self.len.saturating_add(1);
        }
        Ok(key)
    }
}

// The visitor and its frames live on the native stack for the whole decode,
// where Ruby's GC finds them (I3); `refs` holds every value V8 gave an id,
// made at the first, so a result without one allocates none.
struct RubyVisitor<'r> {
    ruby: &'r Ruby,
    refs: Option<RArray>,
    pending: Pending,
    pairs: Option<Pairs>,
    keys: Option<KeyCache>,
    // Arrays and sets open around the current value.
    arrays: u32,
}

impl RubyVisitor<'_> {
    fn add(&mut self, target: RArray, values: &[Value]) -> Result<(), Error> {
        self.add_with(target, values.len(), |slots| slots.copy_from_slice(values))
    }

    // `fill` writes the `count` values straight into the batch, where the GC
    // sees each one as soon as it is made.
    fn add_with(
        &mut self,
        target: RArray,
        count: usize,
        fill: impl FnOnce(&mut [Value]),
    ) -> Result<(), Error> {
        if self.pending.filled.saturating_add(count) > BATCH {
            self.flush()?;
        }
        if self.pending.filled == 0 {
            self.pending.target = Some(target);
        }
        let start = self.pending.filled;
        let end = start.saturating_add(count);
        let slots = self
            .pending
            .values
            .get_mut(start..end)
            .ok_or_else(|| errors::internal(self.ruby, "decode batch overflow"))?;
        fill(slots);
        self.pending.filled = end;
        Ok(())
    }

    // Converts a run straight into the batch, so a run as long as the batch
    // is one rb_ary_cat and a short one shares its append with its neighbors.
    fn extend<T: Copy>(
        &mut self,
        array: RArray,
        values: &[T],
        convert: impl Fn(&Ruby, T) -> Value,
    ) -> Result<(), Error> {
        let ruby = self.ruby;
        for chunk in values.chunks(BATCH) {
            self.add_with(array, chunk.len(), |slots| {
                for (slot, &n) in slots.iter_mut().zip(chunk) {
                    *slot = convert(ruby, n);
                }
            })?;
        }
        Ok(())
    }

    fn insert_each<T: Copy>(
        &mut self,
        frame: &mut Frame,
        entries: &[(i32, T)],
        convert: impl Fn(&Ruby, T) -> Value,
    ) -> Result<(), Error> {
        for &(key, n) in entries {
            let key = int32(self.ruby, key);
            let value = convert(self.ruby, n);
            self.insert(frame, key, value)?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), Error> {
        let filled = std::mem::take(&mut self.pending.filled);
        let values = self.pending.values.get(..filled).unwrap_or_default();
        match self.pending.target {
            _ if filled == 0 => Ok(()),
            Some(array) => array.cat(values),
            None => Err(errors::internal(self.ruby, "decode batch without a target")),
        }
    }

    fn pairs(&mut self) -> Result<&mut Pairs, Error> {
        let ruby = self.ruby;
        self.pairs
            .as_mut()
            .ok_or_else(|| errors::internal(ruby, "decode pairs without a hash"))
    }

    fn open_hash(&mut self) -> RHash {
        let ruby = self.ruby;
        let hash = ruby.hash_new();
        self.pairs
            .get_or_insert_with(|| Pairs::new(ruby))
            .open(ruby, hash);
        hash
    }

    // A key whose hashing Ruby code could run for, or that holds other
    // values, such as an Array that refers back to an open hash, first sees
    // every open hash with all its pairs so far, as mini_racer stores each
    // pair as it decodes it.
    fn put_pair(&mut self, hash: RHash, key: Value, value: Value) -> Result<(), Error> {
        let ruby = self.ruby;
        let pairs = self.pairs()?;
        if rb_raw::plain_key(ruby, key) {
            pairs.push(ruby, key);
            pairs.push(ruby, value);
            return Ok(());
        }
        pairs.put_all(ruby)?;
        rb_raw::hash_aset_pairs(ruby, hash, &[[key, value]])
    }

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

    // The key `Hash#[]=` would store, made interned in the first place. One
    // with a lone surrogate stays UTF-16LE, which the hash interns itself,
    // and the function marker stays a JavaScriptFunction.
    fn interned(&mut self, text: Text<'_>) -> Result<Value, Error> {
        let ruby = self.ruby;
        match text {
            Text::Utf8(bytes) => self.cached(bytes),
            Text::Latin1(bytes) if bytes.is_ascii() => self.cached(bytes),
            Text::Latin1(bytes) => {
                Ok(rb_raw::interned_utf8(ruby, text::latin1_to_utf8(bytes).as_bytes())?.as_value())
            }
            Text::Utf16Le(bytes) if marker::is_javascript_function(bytes) => self.string(text),
            Text::Utf16Le(bytes) => match text::utf16le_to_utf8(bytes) {
                Some(utf8) => Ok(rb_raw::interned_utf8(ruby, utf8.as_bytes())?.as_value()),
                None => self.string(text),
            },
        }
    }

    fn cached(&mut self, bytes: &[u8]) -> Result<Value, Error> {
        let ruby = self.ruby;
        if self.arrays == 0
            || bytes.len() > CACHED_KEY_BYTES
            || !bytes.first().is_some_and(u8::is_ascii_alphabetic)
        {
            return Ok(rb_raw::interned_utf8(ruby, bytes)?.as_value());
        }
        self.keys
            .get_or_insert_with(|| KeyCache {
                keys: [ruby.qnil().as_value(); CACHED_KEYS],
                len: 0,
            })
            .fetch(ruby, bytes)
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

fn int32(ruby: &Ruby, n: i32) -> Value {
    ruby.integer_from_i64(n.into()).as_value()
}

fn number(ruby: &Ruby, n: f64) -> Value {
    match classify(n) {
        Number::Integer(i) => ruby.integer_from_i64(i).as_value(),
        Number::BigInteger => rb_raw::dbl2big(ruby, n),
        Number::Float => ruby.float_from_f64(n).as_value(),
    }
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
            Scalar::Int32(n) => int32(ruby, n),
            Scalar::Uint32(n) => ruby.integer_from_u64(n.into()).as_value(),
            Scalar::Number(n) => number(ruby, n),
            Scalar::BigInt { negative, digits } => rb_raw::integer_unpack(ruby, digits, negative)?,
            Scalar::Text(text) | Scalar::RegExp { pattern: text, .. } => self.string(text)?,
            Scalar::Date(millis) => self.time(millis)?,
            Scalar::ArrayBuffer(bytes) => ruby.str_from_slice(bytes).as_value(),
        })
    }

    fn begin(&mut self, container: Container) -> Result<Begun<Frame, Value>, Error> {
        self.flush()?;
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
            Container::Object => Frame::Object(self.open_hash()),
            Container::Map => Frame::Map(self.open_hash()),
            // Allocated now so a cause can refer back to the error itself.
            Container::Error => Frame::Error(ErrorParts {
                error: errors::script_error(ruby).obj_alloc()?.as_value(),
                message: None,
                stack: None,
                cause: None,
            }),
        };
        let value = match &frame {
            Frame::Array(array) | Frame::Sparse { array, .. } => {
                self.arrays = self.arrays.saturating_add(1);
                array.as_value()
            }
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
            Frame::Array(array) | Frame::Sparse { array, .. } => self.add(*array, &[value]),
            Frame::Object(_) | Frame::Map(_) | Frame::Error(_) => self.bad_state(),
        }
    }

    fn insert(&mut self, frame: &mut Frame, key: Value, value: Value) -> Result<(), Error> {
        match frame {
            Frame::Array(_) => Ok(()),
            Frame::Sparse { array, named } => {
                if !*named && is_index(key) {
                    self.add(*array, &[value])
                } else {
                    *named = true;
                    Ok(())
                }
            }
            Frame::Object(hash) => {
                let key = match RString::from_value(key) {
                    Some(key) => key.as_value(),
                    None => key.funcall::<_, _, Value>("to_s", ())?,
                };
                self.put_pair(*hash, key, value)
            }
            Frame::Map(hash) => self.put_pair(*hash, key, value),
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
        self.flush()?;
        match frame {
            Frame::Array(array) | Frame::Sparse { array, .. } => {
                self.arrays = self.arrays.saturating_sub(1);
                Ok(array.as_value())
            }
            Frame::Object(hash) | Frame::Map(hash) => {
                let ruby = self.ruby;
                self.pairs()?.close(ruby, hash)?;
                Ok(hash.as_value())
            }
            Frame::Error(parts) => self.script_error(parts),
        }
    }

    fn key(&mut self, frame: &Frame, text: Text<'_>) -> Result<Value, Error> {
        match frame {
            Frame::Object(_) | Frame::Map(_) => self.interned(text),
            Frame::Array(_) | Frame::Sparse { .. } | Frame::Error(_) => self.string(text),
        }
    }

    fn register(&mut self, id: u32, value: Value) -> Result<(), Error> {
        let ruby = self.ruby;
        self.refs
            .get_or_insert_with(|| ruby.ary_new())
            .store(isize::try_from(id).unwrap_or(isize::MAX), value)
    }

    // The decoder refers back only to an id it saw registered.
    fn back_ref(&mut self, id: u32) -> Result<Value, Error> {
        self.refs.map_or_else(
            || self.bad_state(),
            |refs| refs.entry(isize::try_from(id).unwrap_or(isize::MAX)),
        )
    }

    fn push_int32s(&mut self, frame: &mut Frame, values: &[i32]) -> Result<(), Error> {
        match frame {
            Frame::Array(array) | Frame::Sparse { array, .. } => self.extend(*array, values, int32),
            Frame::Object(_) | Frame::Map(_) | Frame::Error(_) => self.bad_state(),
        }
    }

    fn push_numbers(&mut self, frame: &mut Frame, values: &[f64]) -> Result<(), Error> {
        match frame {
            Frame::Array(array) | Frame::Sparse { array, .. } => {
                self.extend(*array, values, number)
            }
            Frame::Object(_) | Frame::Map(_) | Frame::Error(_) => self.bad_state(),
        }
    }

    // An int32 key is a Fixnum on every supported (64-bit) platform, so it
    // passes `is_index` and `insert` would append each value.
    fn insert_int32s(&mut self, frame: &mut Frame, entries: &[(i32, i32)]) -> Result<(), Error> {
        match frame {
            Frame::Sparse {
                array,
                named: false,
            } => self.extend(*array, entries, |ruby, (_, n)| int32(ruby, n)),
            _ => self.insert_each(frame, entries, int32),
        }
    }

    fn insert_numbers(&mut self, frame: &mut Frame, entries: &[(i32, f64)]) -> Result<(), Error> {
        match frame {
            Frame::Sparse {
                array,
                named: false,
            } => self.extend(*array, entries, |ruby, (_, n)| number(ruby, n)),
            _ => self.insert_each(frame, entries, number),
        }
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
        refs: None,
        pending: Pending {
            target: None,
            values: [ruby.qnil().as_value(); BATCH],
            filled: 0,
        },
        pairs: None,
        keys: None,
        arrays: 0,
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
