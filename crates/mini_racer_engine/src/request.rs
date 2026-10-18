use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, Ordering};

use mini_racer_wire::{WRITE_VERSION, tag};
use v8::ValueDeserializerHelper;

// Deeper values are left to V8, which reads as deep as its stack allows.
const MAX_DEPTH: u32 = 64;
// Far below V8's own limit on an array's length, which it enforces itself.
const MAX_ELEMENTS: usize = 1 << 24;
// V8 throws for a BigInt past its limit, which this stays well short of.
const MAX_BIGINT_WORDS: usize = 1 << 20;
// V8's deserializer builds objects faster than the runner, which defines each
// property through the API, unless they hold two properties or fewer each. A
// call with a few properties in all still gains, as it saves the
// deserializer's setup.
const FEW_PROPERTIES: usize = 4;
const PROPERTIES_PER_OBJECT: usize = 2;
// Each property grows the object's store a few slots at a time, so an object
// of many costs the square of their number; V8 reads the call instead.
const MAX_KEYS: usize = 64;

/// A call as `mini_racer`'s encoder sends it: the function's dotted path and
/// the arguments.
pub struct Call<'s, 'b> {
    pub path: Cow<'b, str>,
    pub args: Vec<v8::Local<'s, v8::Value>>,
}

pub enum Read<'s, 'b> {
    Built(Call<'s, 'b>),
    /// Left to V8's deserializer, which reads it as it always did.
    Unbuilt,
    /// V8's deserializer failed on a part of it, as it would have on the
    /// whole, and its exception is pending.
    Failed,
}

/// Whether the runner builds a call whose arguments hold `objects` objects
/// with `properties` properties in all, rather than V8's deserializer.
#[must_use]
pub const fn worth_building(objects: usize, properties: usize) -> bool {
    properties <= FEW_PROPERTIES || properties <= objects.saturating_mul(PROPERTIES_PER_OBJECT)
}

/// Builds a call's arguments as V8's deserializer would, but with each array
/// packed, as an array built in JavaScript is: V8 serializes a holey array,
/// which its deserializer makes, element by element as a sparse array, so a
/// function returning an argument costs far more than it need.
///
/// A two-byte string or a binary is read by V8's own deserializer from its
/// bytes alone. The whole request is `Unbuilt` for anything else this would
/// not build as V8 does or that V8 would refuse (a key that is not a string or
/// an int32, a key given twice, an array inside itself, an object of more than
/// `MAX_KEYS` keys, nesting past `MAX_DEPTH`), and for any termination V8 may
/// have pending. V8 acts on one before each value it reads and as each array
/// or object begins; this looks at the same points.
pub fn read<'s, 'b>(
    scope: &v8::PinScope<'s, '_>,
    bytes: &'b [u8],
    termination_requested: &AtomicBool,
) -> Read<'s, 'b> {
    if scope.is_execution_terminating() {
        return Read::Unbuilt;
    }
    let mut builder = Builder {
        scope,
        bytes: Cursor { rest: bytes },
        termination_requested,
        failed: false,
        objects: Vec::new(),
        elements: Vec::new(),
        keys: Vec::new(),
        shapes: Vec::new(),
    };
    match builder.call() {
        Some(call) => Read::Built(call),
        None if builder.failed => Read::Failed,
        None => Read::Unbuilt,
    }
}

struct Reader;

impl v8::ValueDeserializerImpl for Reader {}

#[derive(Clone, Copy)]
struct Cursor<'b> {
    rest: &'b [u8],
}

impl<'b> Cursor<'b> {
    fn byte(&mut self) -> Option<u8> {
        let (&first, rest) = self.rest.split_first()?;
        self.rest = rest;
        Some(first)
    }

    const fn peek(&self) -> Option<u8> {
        match self.rest.first() {
            Some(&byte) => Some(byte),
            None => None,
        }
    }

    fn varint(&mut self) -> Option<u64> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = self.byte()?;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(value);
            }
        }
        None
    }

    fn varint_u32(&mut self) -> Option<u32> {
        u32::try_from(self.varint()?).ok()
    }

    fn take(&mut self, len: usize) -> Option<&'b [u8]> {
        let (head, rest) = self.rest.split_at_checked(len)?;
        self.rest = rest;
        Some(head)
    }

    fn text(&mut self) -> Option<&'b [u8]> {
        let len = usize::try_from(self.varint_u32()?).ok()?;
        self.take(len)
    }

    fn double(&mut self) -> Option<f64> {
        let bytes: [u8; 8] = self.take(8)?.try_into().ok()?;
        Some(f64::from_le_bytes(bytes))
    }

    // V8 refuses an array longer than the bytes left, since every element
    // takes at least one.
    fn array_length(&mut self) -> Option<u32> {
        let length = self.varint_u32()?;
        usize::try_from(length)
            .ok()
            .filter(|&count| count <= self.rest.len() && count <= MAX_ELEMENTS)?;
        Some(length)
    }

    fn array_end(&mut self, length: u32) -> Option<()> {
        (self.byte()? == tag::END_DENSE_ARRAY
            && self.varint()? == 0
            && self.varint()? == u64::from(length))
        .then_some(())
    }

    fn bigint_bytes(&mut self) -> Option<(bool, &'b [u8])> {
        let bitfield = self.varint_u32()?;
        let negative = bitfield & 1 == 1;
        let len = usize::try_from(bitfield >> 1).ok()?;
        // V8 has no negative zero, and the encoder writes whole words.
        if !len.is_multiple_of(8) || len / 8 > MAX_BIGINT_WORDS || (negative && len == 0) {
            return None;
        }
        Some((negative, self.take(len)?))
    }

    // A buffer and the `Uint8Array` over it, which the encoder always writes
    // together, past the buffer's tag.
    fn binary(&mut self) -> Option<()> {
        let len = usize::try_from(self.varint()?).ok()?;
        self.take(len)?;
        if self.byte()? != tag::ARRAY_BUFFER_VIEW || self.byte()? != tag::UINT8_ARRAY {
            return None;
        }
        let (offset, length, flags) = (self.varint()?, self.varint()?, self.varint()?);
        (offset == 0 && length == u64::try_from(len).ok()? && flags == 0).then_some(())
    }
}

// A key as V8 tells keys apart: an int32 by the digits of the property it names.
#[derive(Clone, Copy)]
enum Key<'b> {
    Text(&'b [u8]),
    Digits { buffer: [u8; 11], len: usize },
}

impl Key<'_> {
    fn int(n: i32) -> Self {
        let mut buffer = [0u8; 11];
        let mut magnitude = n.unsigned_abs();
        let mut len = 0;
        loop {
            let digit = u8::try_from(magnitude % 10).unwrap_or(0);
            if let Some(slot) = buffer.get_mut(len) {
                *slot = b'0' + digit;
            }
            len += 1;
            magnitude /= 10;
            if magnitude == 0 {
                break;
            }
        }
        if n < 0 {
            if let Some(slot) = buffer.get_mut(len) {
                *slot = b'-';
            }
            len += 1;
        }
        if let Some(digits) = buffer.get_mut(..len) {
            digits.reverse();
        }
        Self::Digits { buffer, len }
    }

    fn bytes(&self) -> &[u8] {
        match self {
            Self::Text(bytes) => bytes,
            Self::Digits { buffer, len } => buffer.get(..*len).unwrap_or_default(),
        }
    }
}

impl PartialEq for Key<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.bytes() == other.bytes()
    }
}

struct Builder<'s, 'b, 'p, 'i> {
    scope: &'p v8::PinScope<'s, 'i>,
    bytes: Cursor<'b>,
    termination_requested: &'p AtomicBool,
    failed: bool,
    // The value of each id V8 gives an object as it reads; an array has none
    // until it is complete, where V8's would already have one, and nor does
    // anything inside a value V8 read.
    objects: Vec<Option<v8::Local<'s, v8::Value>>>,
    // The elements of the arrays being read, innermost last.
    elements: Vec<v8::Local<'s, v8::Value>>,
    // The keys of the objects being read, innermost last.
    keys: Vec<Key<'b>>,
    // The names last made for each key position at each depth, so objects of
    // one shape share them instead of looking each up again.
    shapes: Vec<Vec<(Key<'b>, v8::Local<'s, v8::String>)>>,
}

const fn zigzag(raw: u32) -> i32 {
    let magnitude = (raw >> 1).cast_signed();
    if raw & 1 == 0 { magnitude } else { !magnitude }
}

impl<'s, 'b> Builder<'s, 'b, '_, '_> {
    fn unterminated(&self) -> Option<()> {
        (!self.termination_requested.load(Ordering::Acquire)).then_some(())
    }

    fn call(&mut self) -> Option<Call<'s, 'b>> {
        if self.bytes.byte()? != tag::HEADER || self.bytes.varint()? != u64::from(WRITE_VERSION) {
            return None;
        }
        self.unterminated()?;
        if self.bytes.byte()? != tag::BEGIN_DENSE_ARRAY {
            return None;
        }
        self.unterminated()?;
        let length = self.bytes.array_length()?;
        let count = usize::try_from(length).ok().filter(|&count| count > 0)?;
        self.objects.push(None);
        let path = self.path()?;
        let mut args = Vec::with_capacity(count.saturating_sub(1));
        for _ in 1..count {
            args.push(self.value(MAX_DEPTH)?);
        }
        self.bytes.array_end(length)?;
        self.bytes.rest.is_empty().then_some(Call { path, args })
    }

    // V8's path is the name converted to UTF-8, as a one-byte string's each
    // byte becomes the character it is.
    fn path(&mut self) -> Option<Cow<'b, str>> {
        self.unterminated()?;
        match self.bytes.byte()? {
            tag::UTF8_STRING => std::str::from_utf8(self.bytes.text()?)
                .ok()
                .map(Cow::Borrowed),
            tag::ONE_BYTE_STRING => {
                let text = self.bytes.text()?;
                Some(match std::str::from_utf8(text) {
                    Ok(ascii) if text.is_ascii() => Cow::Borrowed(ascii),
                    _ => Cow::Owned(text.iter().copied().map(char::from).collect()),
                })
            }
            _ => None,
        }
    }

    fn value(&mut self, depth: u32) -> Option<v8::Local<'s, v8::Value>> {
        self.unterminated()?;
        let scope = self.scope;
        let at = self.bytes;
        Some(match self.bytes.byte()? {
            tag::INT32 => v8::Integer::new(scope, zigzag(self.bytes.varint_u32()?)).into(),
            tag::DOUBLE => v8::Number::new(scope, self.bytes.double()?).into(),
            tag::NULL => v8::null(scope).into(),
            tag::UNDEFINED => v8::undefined(scope).into(),
            tag::TRUE => v8::Boolean::new(scope, true).into(),
            tag::FALSE => v8::Boolean::new(scope, false).into(),
            tag::UTF8_STRING => {
                v8::String::new_from_utf8(scope, self.bytes.text()?, v8::NewStringType::Normal)?
                    .into()
            }
            tag::ONE_BYTE_STRING => {
                v8::String::new_from_one_byte(scope, self.bytes.text()?, v8::NewStringType::Normal)?
                    .into()
            }
            tag::BIGINT => {
                let (negative, digits) = self.bytes.bigint_bytes()?;
                let words: Vec<u64> = digits
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .map(|word| u64::from_le_bytes(*word))
                    .collect();
                v8::BigInt::new_from_words(scope, negative, &words)?.into()
            }
            tag::DATE => {
                let date: v8::Local<'s, v8::Value> =
                    v8::Date::new(scope, self.bytes.double()?)?.into();
                self.objects.push(Some(date));
                date
            }
            tag::OBJECT_REFERENCE => {
                let id = usize::try_from(self.bytes.varint_u32()?).ok()?;
                (*self.objects.get(id)?)?
            }
            tag::BEGIN_DENSE_ARRAY => self.array(depth.checked_sub(1)?)?,
            tag::BEGIN_OBJECT => self.object(depth.checked_sub(1)?)?,
            tag::TWO_BYTE_STRING => {
                self.bytes.text()?.len().is_multiple_of(2).then_some(())?;
                self.delegate(at, 0, None)?
            }
            tag::ARRAY_BUFFER => {
                self.bytes.binary()?;
                self.delegate(at, 2, Some(1))?
            }
            _ => return None,
        })
    }

    // Reads the value from `at` up to the cursor with V8's own deserializer,
    // from its bytes alone. It takes `ids` object ids, of which `own` is the
    // value's.
    fn delegate(
        &mut self,
        at: Cursor<'b>,
        ids: usize,
        own: Option<usize>,
    ) -> Option<v8::Local<'s, v8::Value>> {
        let len = at.rest.len().checked_sub(self.bytes.rest.len())?;
        let mut bytes = vec![tag::HEADER, u8::try_from(WRITE_VERSION).ok()?];
        bytes.extend_from_slice(at.rest.get(..len)?);
        let context = self.scope.get_current_context();
        let deserializer = v8::ValueDeserializer::new(self.scope, Box::new(Reader), &bytes);
        let value = (deserializer.read_header(context) == Some(true))
            .then(|| deserializer.read_value(context))
            .flatten();
        let Some(value) = value else {
            self.failed = true;
            return None;
        };
        let first = self.objects.len();
        self.objects
            .resize(first.checked_add(ids)?, None::<v8::Local<'s, v8::Value>>);
        if let Some(slot) = own.and_then(|own| self.objects.get_mut(first.checked_add(own)?)) {
            *slot = Some(value);
        }
        Some(value)
    }

    fn array(&mut self, depth: u32) -> Option<v8::Local<'s, v8::Value>> {
        self.unterminated()?;
        let length = self.bytes.array_length()?;
        let id = self.objects.len();
        self.objects.push(None);
        let start = self.elements.len();
        for _ in 0..length {
            let element = self.value(depth)?;
            self.elements.push(element);
        }
        self.bytes.array_end(length)?;
        let array = v8::Array::new_with_elements(self.scope, self.elements.get(start..)?).into();
        self.elements.truncate(start);
        *self.objects.get_mut(id)? = Some(array);
        Some(array)
    }

    // Each property is defined as V8's deserializer defines it, never set, so
    // no setter or prototype sees it.
    fn object(&mut self, depth: u32) -> Option<v8::Local<'s, v8::Value>> {
        self.unterminated()?;
        let object = v8::Object::new(self.scope);
        self.objects.push(Some(object.into()));
        let start = self.keys.len();
        let mut count = 0usize;
        while self.bytes.peek()? != tag::END_OBJECT {
            self.unterminated()?;
            let key = self.key(start, count, depth)?;
            let value = self.value(depth)?;
            object
                .create_data_property(self.scope, key, value)
                .filter(|&created| created)?;
            count = count.checked_add(1).filter(|&count| count <= MAX_KEYS)?;
        }
        self.bytes.byte()?;
        self.keys.truncate(start);
        (self.bytes.varint()? == u64::try_from(count).ok()?).then_some(object.into())
    }

    fn key(
        &mut self,
        start: usize,
        position: usize,
        depth: u32,
    ) -> Option<v8::Local<'s, v8::Name>> {
        let key = match self.bytes.byte()? {
            tag::UTF8_STRING => {
                let text = self.bytes.text()?;
                std::str::from_utf8(text).ok()?;
                Key::Text(text)
            }
            tag::ONE_BYTE_STRING => {
                let text = self.bytes.text()?;
                if !text.is_ascii() {
                    return None;
                }
                Key::Text(text)
            }
            tag::INT32 => Key::int(zigzag(self.bytes.varint_u32()?)),
            _ => return None,
        };
        // V8's deserializer fails on a key the object already has.
        if self.keys.get(start..)?.contains(&key) {
            return None;
        }
        self.keys.push(key);
        let level = usize::try_from(MAX_DEPTH.checked_sub(depth)?).ok()?;
        if self.shapes.len() <= level {
            self.shapes.resize_with(level.checked_add(1)?, Vec::new);
        }
        let shape = self.shapes.get_mut(level)?;
        if let Some((known, name)) = shape.get(position)
            && *known == key
        {
            return Some((*name).into());
        }
        let name = match key {
            Key::Text(text) => {
                v8::String::new_from_utf8(self.scope, text, v8::NewStringType::Internalized)?
            }
            Key::Digits { .. } => v8::String::new_from_one_byte(
                self.scope,
                key.bytes(),
                v8::NewStringType::Internalized,
            )?,
        };
        let shape = self.shapes.get_mut(level)?;
        if let Some(slot) = shape.get_mut(position) {
            *slot = (key, name);
        } else if shape.len() == position {
            shape.push((key, name));
        }
        Some(name.into())
    }
}
