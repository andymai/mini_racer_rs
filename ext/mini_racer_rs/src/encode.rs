use std::collections::HashMap;

use magnus::encoding::{EncodingCapable, RbEncoding};
use magnus::r_hash::ForEach;
use magnus::typed_data::Obj;
use magnus::value::ReprValue;
use magnus::{
    Error, Fixnum, Float, Module, Object, RArray, RClass, RHash, RModule, RObject, RString, Ruby,
    Time, Value,
};
use mini_racer_wire::Writer;

use crate::gc_root::{OnceRoot, Pins, StaticRoot};
use crate::rb_raw::{self, RubyType};

const BIGINT_MAX_WORDS: usize = (16 << 20) / 8;
// mini_racer formats these messages into a 64-byte buffer.
const MESSAGE_MAX_BYTES: usize = 63;

/// Resolved by `Context#initialize` while unresolved, as `mini_racer` resolves
/// it at each `Context.new`, since `date` may be required after this gem.
pub static DATE_TIME: OnceRoot<Value> = OnceRoot::new();

static BINARY: StaticRoot<RClass> = StaticRoot::new(|ruby| {
    ruby.class_object()
        .const_get::<_, RModule>("MiniRacer")
        .and_then(|racer| racer.const_get("Binary"))
        .expect("the vocabulary defines MiniRacer::Binary")
});

enum EncodeError {
    // mini_racer's text for a value it cannot send, cut as it cuts it.
    Unsupported(Vec<u8>),
    Raised(Error),
}

impl EncodeError {
    fn unsupported(message: &str) -> Self {
        let bytes = message.as_bytes();
        Self::Unsupported(bytes.get(..MESSAGE_MAX_BYTES).unwrap_or(bytes).to_vec())
    }
}

impl From<Error> for EncodeError {
    fn from(error: Error) -> Self {
        Self::Raised(error)
    }
}

enum Item {
    // A pinned value, by its index in the holder.
    Value(usize),
    // An element read when its turn comes, as serialize1 reads them, so a
    // to_time that changes the array is seen.
    Element { array: usize, index: isize },
    EndArray(u32),
    EndObject(u32),
}

// Iterative so a deeply nested argument cannot overflow the Ruby thread's
// stack; how deep V8 then reads is V8's own limit.
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
/// assigns on reading. Each caller raises `mini_racer`'s text for a value it
/// cannot send in its own way, through `unsupported`.
pub fn encode(
    ruby: &Ruby,
    value: Value,
    unsupported: impl FnOnce(Vec<u8>) -> Error,
) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    let mut encoder = Encoder {
        ruby,
        pins: Pins::new(ruby),
        writer: Writer::new(&mut out),
        work: Vec::new(),
        seen: HashMap::new(),
    };
    let root = encoder.pins.push(value);
    encoder.work.push(Item::Value(root));
    encoder.run().map_err(|error| match error {
        EncodeError::Unsupported(message) => unsupported(message),
        EncodeError::Raised(error) => error,
    })?;
    Ok(out)
}

fn length(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

impl Encoder<'_, '_> {
    fn pinned(&self, index: usize) -> Value {
        self.pins
            .get(self.ruby, index)
            .unwrap_or_else(|| self.ruby.qnil().as_value())
    }

    fn run(&mut self) -> Result<(), EncodeError> {
        while let Some(item) = self.work.pop() {
            match item {
                Item::EndArray(length) => self.writer.end_array(length),
                Item::EndObject(count) => self.writer.end_object(count),
                Item::Value(index) => self.value(self.pinned(index))?,
                Item::Element { array, index } => {
                    let element = RArray::from_value(self.pinned(array))
                        .map(|array| array.entry::<Value>(index))
                        .transpose()?
                        .unwrap_or_else(|| self.ruby.qnil().as_value());
                    self.value(element)?;
                }
            }
        }
        Ok(())
    }

    fn value(&mut self, value: Value) -> Result<(), EncodeError> {
        let ruby = self.ruby;
        match rb_raw::ruby_type(ruby, value) {
            RubyType::Array => {
                self.array(value);
                Ok(())
            }
            RubyType::Hash => self.hash(value),
            RubyType::Data => self.data(value),
            RubyType::Nil => {
                self.writer.null();
                Ok(())
            }
            RubyType::True => {
                self.writer.bool(true);
                Ok(())
            }
            RubyType::False => {
                self.writer.bool(false);
                Ok(())
            }
            RubyType::Bignum => self.bigint(value),
            RubyType::Fixnum => {
                self.writer
                    .integer(Fixnum::from_value(value).map_or(0, Fixnum::to_i64));
                Ok(())
            }
            RubyType::Float => {
                self.writer
                    .number(Float::from_value(value).map_or(f64::NAN, Float::to_f64));
                Ok(())
            }
            RubyType::Symbol => {
                self.string(rb_raw::sym2str(ruby, value));
                Ok(())
            }
            RubyType::String => {
                self.string(RString::from_value(value).unwrap_or_else(|| ruby.str_new("")));
                Ok(())
            }
            RubyType::Object => self.object(value),
            RubyType::Other(kind) => Err(EncodeError::unsupported(&format!(
                "unsupported type {kind:x}"
            ))),
        }
    }

    fn array(&mut self, value: Value) {
        if let Some(&id) = self.seen.get(&rb_raw::identity(value)) {
            self.writer.object_ref(id);
            return;
        }
        let len = RArray::from_value(value).map_or(0, RArray::len);
        let count = length(len);
        let id = self.writer.begin_array(count);
        let array = self.pins.push(value);
        self.seen.insert(rb_raw::identity(value), id);
        self.work.push(Item::EndArray(count));
        for index in (0..isize::try_from(len).unwrap_or(isize::MAX)).rev() {
            self.work.push(Item::Element { array, index });
        }
    }

    fn hash(&mut self, value: Value) -> Result<(), EncodeError> {
        if let Some(&id) = self.seen.get(&rb_raw::identity(value)) {
            self.writer.object_ref(id);
            return Ok(());
        }
        self.pins.push(value);
        let hash = RHash::from_value(value).unwrap_or_else(|| self.ruby.hash_new());
        let mut pairs = Vec::new();
        let mut keys_fit = true;
        hash.foreach(|key: Value, item: Value| {
            keys_fit &= matches!(
                rb_raw::ruby_type(self.ruby, key),
                RubyType::Fixnum | RubyType::String | RubyType::Symbol
            );
            pairs.push((self.pins.push(key), self.pins.push(item)));
            Ok(ForEach::Continue)
        })?;
        let id = self.writer.begin_object();
        self.seen.insert(rb_raw::identity(value), id);
        if !keys_fit {
            return Err(EncodeError::unsupported("TODO serialize as Map"));
        }
        self.work.push(Item::EndObject(length(pairs.len())));
        for (key, item) in pairs.into_iter().rev() {
            self.work.push(Item::Value(item));
            self.work.push(Item::Value(key));
        }
        Ok(())
    }

    // Dates and typed arrays are objects V8 numbers, so a Ruby object sent
    // twice goes the second time as a reference, as Arrays and Hashes do.
    fn repeated(&mut self, value: Value) -> bool {
        if let Some(&id) = self.seen.get(&rb_raw::identity(value)) {
            self.writer.object_ref(id);
            return true;
        }
        false
    }

    fn remember(&mut self, value: Value, id: u32) {
        self.pins.push(value);
        self.seen.insert(rb_raw::identity(value), id);
    }

    fn data(&mut self, value: Value) -> Result<(), EncodeError> {
        let ruby = self.ruby;
        if self.repeated(value) {
            return Ok(());
        }
        let mut time = value;
        if DATE_TIME.get(ruby).is_some_and(|date_time| {
            rb_raw::identity(value.class().as_value()) == rb_raw::identity(date_time)
        }) {
            time = value.funcall("to_time", ())?;
            self.pins.push(time);
        }
        if rb_raw::identity(time.class().as_value())
            == rb_raw::identity(ruby.class_time().as_value())
        {
            let id = self.date(time)?;
            self.remember(value, id);
        } else {
            self.writer.utf8(b"Undefined Conversion");
        }
        Ok(())
    }

    fn object(&mut self, value: Value) -> Result<(), EncodeError> {
        let ruby = self.ruby;
        if self.repeated(value) {
            return Ok(());
        }
        if value.respond_to("to_time", false)? {
            let time: Value = value.funcall("to_time", ())?;
            self.pins.push(time);
            if time.is_kind_of(ruby.class_time()) {
                let id = self.date(time)?;
                self.remember(value, id);
                return Ok(());
            }
            return Err(EncodeError::unsupported(&format!(
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
            let id = rb_raw::with_string_bytes(ruby, data, |bytes| self.writer.uint8_array(bytes));
            self.remember(value, id);
            return Ok(());
        }
        Err(EncodeError::unsupported(&format!(
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
    fn date(&mut self, value: Value) -> Result<u32, EncodeError> {
        let time = Time::from_value(value)
            .ok_or_else(|| EncodeError::unsupported("unsupported type Time"))?
            .timespec()?;
        let micros = time.tv_nsec / 1000;
        Ok(self
            .writer
            .date(time.tv_sec as f64 * 1e3 + micros as f64 / 1e3))
    }

    fn bigint(&mut self, value: Value) -> Result<(), EncodeError> {
        let Some((negative, digits)) = rb_raw::integer_words(self.ruby, value, BIGINT_MAX_WORDS)?
        else {
            return Err(EncodeError::unsupported("bigint too big"));
        };
        self.writer
            .bigint(negative, &digits)
            .map_err(|error| EncodeError::unsupported(&error.to_string()))
    }

    fn string(&mut self, text: RString) {
        let encoding = RbEncoding::from(text.enc_get());
        rb_raw::with_string_bytes(self.ruby, text, |bytes| match encoding.name() {
            "ISO-8859-1" => self.writer.latin1(bytes),
            "UTF-16LE" => self.writer.utf16le(bytes),
            _ => self.writer.utf8(bytes),
        });
    }
}
