use std::collections::HashMap;

use magnus::encoding::{EncodingCapable, RbEncoding};
use magnus::r_hash::ForEach;
use magnus::rb_sys::AsRawValue;
use magnus::value::ReprValue;
use magnus::{
    Error, Fixnum, Float, Module, Object, RArray, RClass, RHash, RModule, RObject, RString, Ruby,
    Symbol, Time, Value,
};
use mini_racer_wire::Writer;

use crate::gc_root::{OnceRoot, Pins, StaticRoot};
use crate::rb_raw;
use magnus::typed_data::Obj;

// Ruby's builtin type numbers, as mini_racer's serialize1 switches on them.
const T_OBJECT: u32 = 0x01;
const T_FLOAT: u32 = 0x04;
const T_STRING: u32 = 0x05;
const T_ARRAY: u32 = 0x07;
const T_HASH: u32 = 0x08;
const T_BIGNUM: u32 = 0x0a;
const T_DATA: u32 = 0x0c;
const T_NIL: u32 = 0x11;
const T_TRUE: u32 = 0x12;
const T_FALSE: u32 = 0x13;
const T_SYMBOL: u32 = 0x14;
const T_FIXNUM: u32 = 0x15;

const BIGINT_MAX_WORDS: usize = (16 << 20) / 8;

/// Resolved by `Context#initialize` while unresolved, as `mini_racer` resolves it
/// at each `Context.new`, since `date` may be required after this gem.
pub static DATE_TIME: OnceRoot<RClass> = OnceRoot::new();

static BINARY: StaticRoot<RClass> = StaticRoot::new(|ruby| {
    ruby.class_object()
        .const_get::<_, RModule>("MiniRacer")
        .and_then(|racer| racer.const_get("Binary"))
        .expect("the vocabulary defines MiniRacer::Binary")
});

pub enum EncodeError {
    /// `mini_racer`'s text for a value it cannot send.
    Unsupported(String),
    Raised(Error),
}

impl From<Error> for EncodeError {
    fn from(error: Error) -> Self {
        Self::Raised(error)
    }
}

enum Item {
    Value(usize),
    EndArray(u32),
    EndObject(u32),
}

// Iterative so a deeply nested argument cannot overflow the Ruby thread's stack;
// V8's own reader then decides how deep is too deep, as with mini_racer.
struct Encoder<'r, 'o> {
    ruby: &'r Ruby,
    // The object, not a reference into its data: each use of the object keeps
    // it where the GC finds it, while a bare data pointer lets an optimized
    // build drop it and the GC free it mid-encode.
    pins: Obj<Pins>,
    writer: Writer<'o>,
    work: Vec<Item>,
    seen: HashMap<u64, u32>,
}

/// Writes `value` as `mini_racer`'s `serialize` does, with the object ids V8
/// assigns on reading.
pub fn encode(ruby: &Ruby, value: Value) -> Result<Vec<u8>, EncodeError> {
    let mut out = Vec::new();
    let mut encoder = Encoder {
        ruby,
        pins: Pins::new(ruby),
        writer: Writer::new(&mut out),
        work: Vec::new(),
        seen: HashMap::new(),
    };
    encoder.queue(value);
    encoder.run()?;
    Ok(out)
}

fn length(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

impl Encoder<'_, '_> {
    fn queue(&mut self, value: Value) {
        let index = self.pins.push(value);
        self.work.push(Item::Value(index));
    }

    fn run(&mut self) -> Result<(), EncodeError> {
        while let Some(item) = self.work.pop() {
            match item {
                Item::EndArray(length) => self.writer.end_array(length),
                Item::EndObject(count) => self.writer.end_object(count),
                Item::Value(index) => {
                    let value = self
                        .pins
                        .get(self.ruby, index)
                        .ok_or_else(|| EncodeError::Unsupported("lost value".to_owned()))?;
                    self.value(value)?;
                }
            }
        }
        Ok(())
    }

    fn value(&mut self, value: Value) -> Result<(), EncodeError> {
        let ruby = self.ruby;
        match rb_raw::value_type(ruby, value) {
            T_ARRAY => {
                self.array(value);
                Ok(())
            }
            T_HASH => self.hash(value),
            T_DATA => self.data(value),
            T_NIL => {
                self.writer.null();
                Ok(())
            }
            T_TRUE => {
                self.writer.bool(true);
                Ok(())
            }
            T_FALSE => {
                self.writer.bool(false);
                Ok(())
            }
            T_BIGNUM => self.bigint(value),
            T_FIXNUM => {
                let n = Fixnum::from_value(value).map_or(0, Fixnum::to_i64);
                self.writer.integer(n);
                Ok(())
            }
            T_FLOAT => {
                let n = Float::from_value(value).map_or(f64::NAN, Float::to_f64);
                self.writer.number(n);
                Ok(())
            }
            T_SYMBOL => {
                let text = Symbol::from_value(value)
                    .map(ReprValue::to_r_string)
                    .transpose()?;
                self.string(text.unwrap_or_else(|| ruby.str_new("")));
                Ok(())
            }
            T_STRING => {
                self.string(RString::from_value(value).unwrap_or_else(|| ruby.str_new("")));
                Ok(())
            }
            T_OBJECT => self.object(value),
            other => Err(EncodeError::Unsupported(format!(
                "unsupported type {other:x}"
            ))),
        }
    }

    fn array(&mut self, value: Value) {
        if let Some(&id) = self.seen.get(&value.as_raw()) {
            self.writer.object_ref(id);
            return;
        }
        let array = RArray::from_value(value).unwrap_or_else(|| self.ruby.ary_new());
        let items: Vec<Value> = array.into_iter().collect();
        let length = length(items.len());
        let id = self.writer.begin_array(length);
        self.seen.insert(value.as_raw(), id);
        self.work.push(Item::EndArray(length));
        for item in items.into_iter().rev() {
            self.queue(item);
        }
    }

    fn hash(&mut self, value: Value) -> Result<(), EncodeError> {
        if let Some(&id) = self.seen.get(&value.as_raw()) {
            self.writer.object_ref(id);
            return Ok(());
        }
        let hash = RHash::from_value(value).unwrap_or_else(|| self.ruby.hash_new());
        let mut pairs = Vec::new();
        hash.foreach(|key: Value, item: Value| {
            pairs.push((self.pins.push(key), self.pins.push(item)));
            Ok(ForEach::Continue)
        })?;
        let id = self.writer.begin_object();
        self.seen.insert(value.as_raw(), id);
        for &(key, _) in &pairs {
            let key = self
                .pins
                .get(self.ruby, key)
                .unwrap_or_else(|| self.ruby.qnil().as_value());
            if !matches!(
                rb_raw::value_type(self.ruby, key),
                T_FIXNUM | T_STRING | T_SYMBOL
            ) {
                return Err(EncodeError::Unsupported("TODO serialize as Map".to_owned()));
            }
        }
        self.work.push(Item::EndObject(length(pairs.len())));
        for (key, item) in pairs.into_iter().rev() {
            self.work.push(Item::Value(item));
            self.work.push(Item::Value(key));
        }
        Ok(())
    }

    fn data(&mut self, value: Value) -> Result<(), EncodeError> {
        let ruby = self.ruby;
        let mut value = value;
        if DATE_TIME
            .get(ruby)
            .is_some_and(|date_time| value.class().as_raw() == date_time.as_raw())
        {
            value = value.funcall("to_time", ())?;
            self.pins.push(value);
        }
        if value.class().as_raw() == ruby.class_time().as_raw() {
            self.date(value)
        } else {
            self.writer.utf8(b"Undefined Conversion");
            Ok(())
        }
    }

    fn object(&mut self, value: Value) -> Result<(), EncodeError> {
        let ruby = self.ruby;
        if value.respond_to("to_time", false)? {
            let time: Value = value.funcall("to_time", ())?;
            self.pins.push(time);
            if time.is_kind_of(ruby.class_time()) {
                return self.date(time);
            }
            return Err(EncodeError::Unsupported(format!(
                "unsupported type {}",
                rb_raw::class_name(ruby, time)
            )));
        }
        if value.is_kind_of(BINARY.get(ruby)) {
            let data: Value = RObject::from_value(value)
                .map(|object| object.ivar_get("@data"))
                .transpose()?
                .unwrap_or_else(|| ruby.qnil().as_value());
            let data = rb_raw::expect_string(ruby, data)?;
            rb_raw::with_string_bytes(ruby, data, |bytes| {
                self.writer.uint8_array(bytes);
            });
            return Ok(());
        }
        Err(EncodeError::Unsupported(format!(
            "unsupported type {}",
            rb_raw::class_name(ruby, value)
        )))
    }

    // mini_racer reads the time as a timeval, whole microseconds.
    #[expect(
        clippy::cast_precision_loss,
        reason = "mini_racer does the same arithmetic in doubles"
    )]
    #[expect(
        clippy::suboptimal_flops,
        reason = "a fused multiply-add would round differently"
    )]
    fn date(&mut self, value: Value) -> Result<(), EncodeError> {
        let time = Time::from_value(value)
            .ok_or_else(|| EncodeError::Unsupported("unsupported type Time".to_owned()))?
            .timespec()?;
        let micros = time.tv_nsec / 1000;
        self.writer
            .date(time.tv_sec as f64 * 1e3 + micros as f64 / 1e3);
        Ok(())
    }

    fn bigint(&mut self, value: Value) -> Result<(), EncodeError> {
        let Some((negative, digits)) = rb_raw::integer_words(self.ruby, value, BIGINT_MAX_WORDS)?
        else {
            return Err(EncodeError::Unsupported("bigint too big".to_owned()));
        };
        self.writer
            .bigint(negative, &digits)
            .map_err(|error| EncodeError::Unsupported(error.to_string()))
    }

    fn string(&mut self, text: RString) {
        let ruby = self.ruby;
        let encoding = RbEncoding::from(text.enc_get());
        rb_raw::with_string_bytes(ruby, text, |bytes| match encoding.name() {
            "ISO-8859-1" => self.writer.latin1(bytes),
            "UTF-16LE" => self.writer.utf16le(bytes),
            _ => self.writer.utf8(bytes),
        });
    }
}
