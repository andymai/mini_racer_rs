use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use magnus::encoding::{EncodingCapable, RbEncoding};
use magnus::r_hash::ForEach;
use magnus::typed_data::Obj;
use magnus::value::ReprValue;
use magnus::{
    Error, Fixnum, Float, Module, Object, RArray, RClass, RHash, RModule, RObject, RString, Ruby,
    Time, Value,
};
use mini_racer_engine::CallRequest;
use mini_racer_wire::Writer;

use crate::gc_root::{OnceRoot, Pins, StaticRoot};
use crate::rb_raw::{self, RubyType};

const BIGINT_MAX_WORDS: usize = (16 << 20) / 8;
// mini_racer formats these messages into a 64-byte buffer.
const MESSAGE_MAX_BYTES: usize = 63;

/// Resolved by `Context#initialize` while unresolved, as `mini_racer` resolves
/// it at each `Context.new`, since `date` may be required after this gem.
pub static DATE_TIME: OnceRoot<Value> = OnceRoot::new();

#[expect(
    clippy::expect_used,
    reason = "the vocabulary defines the class, and a root initializer cannot return an error"
)]
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

// A pinned object's address is distinct for the whole encode; multiplying it
// spreads its aligned low bits over the bits the map buckets and tags by.
#[derive(Default)]
struct Address(u64);

impl Hasher for Address {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = self.0.rotate_left(8) ^ u64::from(byte);
        }
    }

    fn write_u64(&mut self, address: u64) {
        self.0 = (address >> 3).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}

// Iterative so a deeply nested argument cannot overflow the Ruby thread's
// stack; how deep V8 then reads is V8's own limit.
struct Encoder<'r, 'o> {
    ruby: &'r Ruby,
    // Made for the first value that holds others or can run Ruby code, before
    // anything is pinned or keyed by address. The object, not a reference into
    // its data: each use of the object keeps it where the GC finds it, while a
    // bare data pointer lets an optimized build drop it and the GC free it
    // mid-encode.
    pins: Option<Obj<Pins>>,
    writer: Writer<'o>,
    work: Vec<Item>,
    seen: HashMap<u64, u32, BuildHasherDefault<Address>>,
    // The hashes written and their pairs in all, which tell the runner
    // whether to build a call's arguments itself.
    objects: usize,
    properties: usize,
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
    Encoder::new(ruby, &mut out)
        .top(value)
        .map_err(|error| error.raise(unsupported))?;
    Ok(out)
}

/// Writes `[name, *args]`, the array `mini_racer` sends for a call, without
/// making it: no code can reach that array, so reading `args` as each turn
/// comes is reading it.
pub fn call(
    ruby: &Ruby,
    name: RString,
    args: &[Value],
    unsupported: impl FnOnce(Vec<u8>) -> Error,
) -> Result<CallRequest, Error> {
    let mut bytes = Vec::new();
    let mut encoder = Encoder::new(ruby, &mut bytes);
    let count = length(args.len().saturating_add(1));
    encoder.writer.begin_array(count);
    encoder.string(name);
    args.iter()
        .try_for_each(|&arg| encoder.top(arg))
        .map_err(|error| error.raise(unsupported))?;
    encoder.writer.end_array(count);
    let (objects, properties) = (encoder.objects, encoder.properties);
    Ok(CallRequest {
        bytes,
        objects,
        properties,
    })
}

impl EncodeError {
    fn raise(self, unsupported: impl FnOnce(Vec<u8>) -> Error) -> Error {
        match self {
            Self::Unsupported(message) => unsupported(message),
            Self::Raised(error) => error,
        }
    }
}

fn length(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

// The values whose encoding runs no Ruby code and holds no other value.
const fn immediate(kind: RubyType) -> bool {
    matches!(
        kind,
        RubyType::Fixnum
            | RubyType::Float
            | RubyType::Nil
            | RubyType::True
            | RubyType::False
            | RubyType::String
            | RubyType::Symbol
    )
}

impl<'r, 'o> Encoder<'r, 'o> {
    fn new(ruby: &'r Ruby, out: &'o mut Vec<u8>) -> Self {
        Self {
            ruby,
            pins: None,
            writer: Writer::new(out),
            work: Vec::new(),
            seen: HashMap::default(),
            objects: 0,
            properties: 0,
        }
    }
}

impl Encoder<'_, '_> {
    fn pin(&mut self, value: Value) -> usize {
        let ruby = self.ruby;
        self.pins.get_or_insert_with(|| Pins::new(ruby)).push(value)
    }

    fn pinned(&self, index: usize) -> Value {
        self.pins
            .as_ref()
            .and_then(|pins| pins.get(self.ruby, index))
            .unwrap_or_else(|| self.ruby.qnil().as_value())
    }

    // Writes an immediate at once; anything else waits, pinned, for `run`.
    fn top(&mut self, value: Value) -> Result<(), EncodeError> {
        if self.immediate(value, rb_raw::ruby_type(self.ruby, value)) {
            return Ok(());
        }
        let root = self.pin(value);
        self.work.push(Item::Value(root));
        self.run()
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

    fn immediate(&mut self, value: Value, kind: RubyType) -> bool {
        let ruby = self.ruby;
        match kind {
            RubyType::Fixnum => self
                .writer
                .integer(Fixnum::from_value(value).map_or(0, Fixnum::to_i64)),
            RubyType::Float => self
                .writer
                .number(Float::from_value(value).map_or(f64::NAN, Float::to_f64)),
            RubyType::Nil => self.writer.null(),
            RubyType::True => self.writer.bool(true),
            RubyType::False => self.writer.bool(false),
            RubyType::String => {
                self.string(RString::from_value(value).unwrap_or_else(|| ruby.str_new("")));
            }
            RubyType::Symbol => self.string(rb_raw::sym2str(ruby, value)),
            _ => return false,
        }
        true
    }

    fn value(&mut self, value: Value) -> Result<(), EncodeError> {
        let kind = rb_raw::ruby_type(self.ruby, value);
        if self.immediate(value, kind) {
            return Ok(());
        }
        match kind {
            RubyType::Array => {
                self.array(value);
                Ok(())
            }
            RubyType::Hash => self.hash(value),
            RubyType::Data => self.data(value),
            RubyType::Bignum => self.bigint(value),
            RubyType::Object => self.object(value),
            RubyType::Other(kind) => Err(EncodeError::unsupported(&format!(
                "unsupported type {kind:x}"
            ))),
            RubyType::Fixnum
            | RubyType::Float
            | RubyType::Nil
            | RubyType::True
            | RubyType::False
            | RubyType::String
            | RubyType::Symbol => Ok(()),
        }
    }

    // The leading immediates are written at once: reading one runs no code
    // that could change the array before its turn. The rest wait their turns.
    fn array(&mut self, value: Value) {
        if let Some(&id) = self.seen.get(&rb_raw::identity(value)) {
            self.writer.object_ref(id);
            return;
        }
        let array = RArray::from_value(value);
        let len = array.map_or(0, RArray::len);
        let count = length(len);
        let id = self.writer.begin_array(count);
        let pinned = self.pin(value);
        self.seen.insert(rb_raw::identity(value), id);
        let end = isize::try_from(len).unwrap_or(isize::MAX);
        let mut next = 0;
        while let Some(array) = array
            && next < end
        {
            let element = array
                .entry::<Value>(next)
                .unwrap_or_else(|_| self.ruby.qnil().as_value());
            if !self.immediate(element, rb_raw::ruby_type(self.ruby, element)) {
                break;
            }
            next += 1;
        }
        if next >= end {
            self.writer.end_array(count);
            return;
        }
        self.work.push(Item::EndArray(count));
        for index in (next..end).rev() {
            self.work.push(Item::Element {
                array: pinned,
                index,
            });
        }
    }

    // mini_racer collects every pair before it writes one. Until a value that
    // is not an immediate, each pair is written as it is collected, which no
    // Ruby code can tell apart; from there on, the pairs wait their turns.
    fn hash(&mut self, value: Value) -> Result<(), EncodeError> {
        if let Some(&id) = self.seen.get(&rb_raw::identity(value)) {
            self.writer.object_ref(id);
            return Ok(());
        }
        self.pin(value);
        let hash = RHash::from_value(value).unwrap_or_else(|| self.ruby.hash_new());
        self.objects = self.objects.saturating_add(1);
        self.properties = self.properties.saturating_add(hash.len());
        let id = self.writer.begin_object();
        self.seen.insert(rb_raw::identity(value), id);
        let mut pairs = Vec::new();
        let mut keys_fit = true;
        let mut written = 0u32;
        hash.foreach(|key: Value, item: Value| {
            let key_kind = rb_raw::ruby_type(self.ruby, key);
            keys_fit &= matches!(
                key_kind,
                RubyType::Fixnum | RubyType::String | RubyType::Symbol
            );
            let item_kind = rb_raw::ruby_type(self.ruby, item);
            if keys_fit && pairs.is_empty() && immediate(item_kind) {
                self.immediate(key, key_kind);
                self.immediate(item, item_kind);
                written = written.saturating_add(1);
            } else {
                pairs.push((self.pin(key), self.pin(item)));
            }
            Ok(ForEach::Continue)
        })?;
        if !keys_fit {
            return Err(EncodeError::unsupported("TODO serialize as Map"));
        }
        let count = written.saturating_add(length(pairs.len()));
        if pairs.is_empty() {
            self.writer.end_object(count);
            return Ok(());
        }
        self.work.push(Item::EndObject(count));
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
        self.pin(value);
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
            self.pin(time);
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
            self.pin(time);
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

    // mini_racer tells encodings apart by name. The ones it sends as UTF-8
    // that strings most often have are told by index, without the lookup.
    fn string(&mut self, text: RString) {
        let ruby = self.ruby;
        let index = text.enc_get();
        let utf8 = index == ruby.utf8_encindex()
            || index == ruby.usascii_encindex()
            || index == ruby.ascii8bit_encindex();
        rb_raw::with_string_bytes(ruby, text, |bytes| {
            if utf8 {
                return self.writer.utf8(bytes);
            }
            match RbEncoding::from(index).name() {
                "ISO-8859-1" => self.writer.latin1(bytes),
                "UTF-16LE" => self.writer.utf16le(bytes),
                _ => self.writer.utf8(bytes),
            }
        });
    }
}
