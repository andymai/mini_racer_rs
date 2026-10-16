use crate::READ_VERSIONS;
use crate::tag;

/// Text as V8 wrote it; the visitor transcodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Text<'a> {
    Utf8(&'a [u8]),
    Latin1(&'a [u8]),
    Utf16Le(&'a [u8]),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Scalar<'a> {
    Undefined,
    Null,
    /// An absent element of a dense array.
    Hole,
    Bool(bool),
    Int32(i32),
    Uint32(u32),
    Number(f64),
    /// Magnitude as little-endian 64-bit words.
    BigInt {
        negative: bool,
        digits: &'a [u8],
    },
    Text(Text<'a>),
    /// Milliseconds since the Unix epoch.
    Date(f64),
    RegExp {
        pattern: Text<'a>,
        flags: u32,
    },
    /// The whole buffer; a typed array view over it decodes to the same value.
    ArrayBuffer(&'a [u8]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    /// Every value of a multi-value frame, in order.
    Values,
    /// `length` comes from the input; do not reserve by it.
    DenseArray {
        length: u32,
    },
    SparseArray {
        length: u32,
    },
    Object,
    Map,
    Set,
    Error,
}

/// Receives decoded values. Containers get elements through `push` and keyed
/// entries (object properties, map entries, an array's named properties,
/// an error's fields) through `insert`.
pub trait Visitor {
    type Value: Copy;
    type Frame;
    type Error;

    fn scalar(&mut self, scalar: Scalar<'_>) -> Result<Self::Value, Self::Error>;

    /// Starts a container. A `value` returned here is the container itself;
    /// it is registered at once so children can refer back to it.
    fn begin(
        &mut self,
        container: Container,
    ) -> Result<Begun<Self::Frame, Self::Value>, Self::Error>;

    fn push(&mut self, frame: &mut Self::Frame, value: Self::Value) -> Result<(), Self::Error>;

    fn insert(
        &mut self,
        frame: &mut Self::Frame,
        key: Self::Value,
        value: Self::Value,
    ) -> Result<(), Self::Error>;

    fn end(&mut self, frame: Self::Frame) -> Result<Self::Value, Self::Error>;

    /// Remembers `value` as V8's object `id` for later back references; ids
    /// count up from 0 in the order V8 assigned them.
    fn register(&mut self, id: u32, value: Self::Value) -> Result<(), Self::Error>;

    fn back_ref(&mut self, id: u32) -> Result<Self::Value, Self::Error>;
}

pub struct Begun<F, V> {
    pub frame: F,
    pub value: Option<V>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Nesting below the outermost value; `mini_racer` stops at 96.
    pub max_depth: u32,
    pub max_bigint_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_depth: 96,
            max_bigint_bytes: 16 << 20,
        }
    }
}

// Messages follow mini_racer's where its decoder reports the same condition.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError<E> {
    #[error("bad header")]
    Header,
    #[error("unsupported wire format version {0}")]
    Version(u32),
    #[error("input too short")]
    Truncated,
    #[error("bad varint")]
    Varint,
    #[error("bad tag: {}", printable(*tag))]
    Unsupported { tag: u8 },
    #[error("too much recursion")]
    TooDeep,
    #[error("bad bigint")]
    BigInt,
    #[error("negative zero bigint")]
    NegativeZero,
    #[error("bigint too big")]
    BigIntTooBig,
    #[error("bad utf16 string size")]
    Utf16Length,
    #[error("{0} count mismatch")]
    Count(&'static str),
    #[error("bad typed array")]
    View,
    #[error("bad regexp")]
    RegExp,
    #[error("bad string object")]
    StringObject,
    #[error("error .{0} is not a string")]
    ErrorField(&'static str),
    #[error("bad error object")]
    ErrorObject,
    #[error("too many objects")]
    TooManyObjects,
    #[error("{0}")]
    Visitor(E),
}

fn printable(tag: u8) -> String {
    if tag > 32 && tag < 127 {
        char::from(tag).to_string()
    } else {
        format!("{tag:02x}")
    }
}

// Typed array subtags V8 writes after a view tag.
const VIEW_SUBTAGS: &[u8] = b"?BCDFQWbdfhqw";

struct Reader<'a> {
    rest: &'a [u8],
}

impl<'a> Reader<'a> {
    fn byte<E>(&mut self) -> Result<u8, DecodeError<E>> {
        let (&first, rest) = self.rest.split_first().ok_or(DecodeError::Truncated)?;
        self.rest = rest;
        Ok(first)
    }

    fn peek<E>(&self) -> Result<u8, DecodeError<E>> {
        self.rest.first().copied().ok_or(DecodeError::Truncated)
    }

    fn varint<E>(&mut self) -> Result<u64, DecodeError<E>> {
        let mut result = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = self.byte()?;
            // The tenth byte carries only bit 63; anything more overflows a u64.
            if shift == 63 && byte > 1 {
                return Err(DecodeError::Varint);
            }
            result |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(result);
            }
        }
        Err(DecodeError::Varint)
    }

    fn varint_u32<E>(&mut self) -> Result<u32, DecodeError<E>> {
        u32::try_from(self.varint()?).map_err(|_| DecodeError::Varint)
    }

    fn take<E>(&mut self, len: u64) -> Result<&'a [u8], DecodeError<E>> {
        let len = usize::try_from(len).map_err(|_| DecodeError::Truncated)?;
        let (head, rest) = self
            .rest
            .split_at_checked(len)
            .ok_or(DecodeError::Truncated)?;
        self.rest = rest;
        Ok(head)
    }

    fn double<E>(&mut self) -> Result<f64, DecodeError<E>> {
        let bytes: [u8; 8] = self
            .take(8)?
            .try_into()
            .map_err(|_| DecodeError::Truncated)?;
        Ok(f64::from_le_bytes(bytes))
    }

    // V8 pads before a two-byte string to align its characters.
    const fn skip_padding(&mut self) {
        while let Some((&tag::PADDING, rest)) = self.rest.split_first() {
            self.rest = rest;
        }
    }
}

const fn zigzag(raw: u32) -> i32 {
    let magnitude = raw >> 1;
    if raw & 1 == 0 {
        magnitude.cast_signed()
    } else {
        !magnitude.cast_signed()
    }
}

struct Decoder<'a, 'v, V> {
    reader: Reader<'a>,
    visitor: &'v mut V,
    version: u32,
    limits: Limits,
    next_id: u32,
    // The byte length of each object id that is an array buffer, so a view
    // after a back reference can be checked against it.
    buffers: Vec<Option<u64>>,
}

type Decoded<V> = Result<<V as Visitor>::Value, DecodeError<<V as Visitor>::Error>>;

impl<'a, V: Visitor> Decoder<'a, '_, V> {
    fn visit<T>(result: Result<T, V::Error>) -> Result<T, DecodeError<V::Error>> {
        result.map_err(DecodeError::Visitor)
    }

    fn id(&mut self) -> Result<u32, DecodeError<V::Error>> {
        let id = self.next_id;
        self.next_id = id.checked_add(1).ok_or(DecodeError::TooManyObjects)?;
        self.buffers.push(None);
        Ok(id)
    }

    fn register(&mut self, id: u32, value: V::Value) -> Result<(), DecodeError<V::Error>> {
        Self::visit(self.visitor.register(id, value))
    }

    fn scalar_with_id(&mut self, scalar: Scalar<'_>) -> Decoded<V> {
        let id = self.id()?;
        let value = Self::visit(self.visitor.scalar(scalar))?;
        self.register(id, value)?;
        Ok(value)
    }

    fn child(depth: u32) -> Result<u32, DecodeError<V::Error>> {
        depth.checked_sub(1).ok_or(DecodeError::TooDeep)
    }

    fn text(&mut self, tag: u8) -> Result<Option<Text<'a>>, DecodeError<V::Error>> {
        let text = match tag {
            tag::UTF8_STRING => {
                let len = self.reader.varint()?;
                Text::Utf8(self.reader.take(len)?)
            }
            tag::ONE_BYTE_STRING => {
                let len = self.reader.varint()?;
                Text::Latin1(self.reader.take(len)?)
            }
            tag::TWO_BYTE_STRING => {
                let len = self.reader.varint()?;
                if !len.is_multiple_of(2) {
                    return Err(DecodeError::Utf16Length);
                }
                Text::Utf16Le(self.reader.take(len)?)
            }
            _ => return Ok(None),
        };
        Ok(Some(text))
    }

    fn bigint(&mut self) -> Result<Scalar<'a>, DecodeError<V::Error>> {
        let bitfield = self.reader.varint()?;
        let negative = bitfield & 1 == 1;
        let len = bitfield >> 1;
        if !len.is_multiple_of(8) {
            return Err(DecodeError::BigInt);
        }
        if negative && len == 0 {
            return Err(DecodeError::NegativeZero);
        }
        if len > self.limits.max_bigint_bytes {
            return Err(DecodeError::BigIntTooBig);
        }
        Ok(Scalar::BigInt {
            negative,
            digits: self.reader.take(len)?,
        })
    }

    fn value(&mut self, depth: u32) -> Decoded<V> {
        let tag = loop {
            match self.reader.byte()? {
                tag::PADDING => {}
                tag::VERIFY_OBJECT_COUNT => {
                    self.reader.varint()?;
                }
                tag => break tag,
            }
        };
        if let Some(text) = self.text(tag)? {
            return Self::visit(self.visitor.scalar(Scalar::Text(text)));
        }
        let scalar = match tag {
            tag::UNDEFINED => Scalar::Undefined,
            tag::NULL => Scalar::Null,
            tag::TRUE => Scalar::Bool(true),
            tag::FALSE => Scalar::Bool(false),
            tag::INT32 => Scalar::Int32(zigzag(self.reader.varint_u32()?)),
            tag::UINT32 => Scalar::Uint32(self.reader.varint_u32()?),
            tag::DOUBLE => Scalar::Number(self.reader.double()?),
            tag::BIGINT => self.bigint()?,
            tag::TRUE_OBJECT => return self.scalar_with_id(Scalar::Bool(true)),
            tag::FALSE_OBJECT => return self.scalar_with_id(Scalar::Bool(false)),
            tag::NUMBER_OBJECT => {
                let number = self.reader.double()?;
                return self.scalar_with_id(Scalar::Number(number));
            }
            tag::BIGINT_OBJECT => {
                let bigint = self.bigint()?;
                return self.scalar_with_id(bigint);
            }
            tag::STRING_OBJECT => {
                self.reader.skip_padding();
                let inner = self.reader.byte()?;
                let text = self.text(inner)?.ok_or(DecodeError::StringObject)?;
                return self.scalar_with_id(Scalar::Text(text));
            }
            tag::DATE => {
                let time = self.reader.double()?;
                return self.scalar_with_id(Scalar::Date(time));
            }
            tag::REGEXP => {
                let id = self.id()?;
                self.reader.skip_padding();
                let inner = self.reader.byte()?;
                let pattern = self.text(inner)?.ok_or(DecodeError::RegExp)?;
                let flags = self.reader.varint_u32()?;
                let value = Self::visit(self.visitor.scalar(Scalar::RegExp { pattern, flags }))?;
                self.register(id, value)?;
                return Ok(value);
            }
            tag::ARRAY_BUFFER | tag::IMMUTABLE_ARRAY_BUFFER | tag::RESIZABLE_ARRAY_BUFFER => {
                return self.array_buffer(tag);
            }
            tag::OBJECT_REFERENCE => return self.back_ref(),
            tag::BEGIN_OBJECT => return self.object(depth),
            tag::BEGIN_DENSE_ARRAY => return self.dense_array(depth),
            tag::BEGIN_SPARSE_ARRAY => return self.sparse_array(depth),
            tag::BEGIN_MAP => return self.map(depth),
            tag::BEGIN_SET => return self.set(depth),
            tag::ERROR => return self.error(depth),
            tag => return Err(DecodeError::Unsupported { tag }),
        };
        Self::visit(self.visitor.scalar(scalar))
    }

    fn array_buffer(&mut self, tag: u8) -> Decoded<V> {
        let len = self.reader.varint()?;
        if tag == tag::RESIZABLE_ARRAY_BUFFER {
            self.reader.varint()?;
        }
        let id = self.id()?;
        let bytes = self.reader.take(len)?;
        let value = Self::visit(self.visitor.scalar(Scalar::ArrayBuffer(bytes)))?;
        self.register(id, value)?;
        if let Some(slot) = self.buffers.last_mut() {
            *slot = Some(len);
        }
        self.view(value, len)
    }

    fn back_ref(&mut self) -> Decoded<V> {
        let id = self.reader.varint_u32()?;
        let value = Self::visit(self.visitor.back_ref(id))?;
        match self.buffers.get(usize::try_from(id).unwrap_or(usize::MAX)) {
            Some(Some(len)) => self.view(value, *len),
            _ => Ok(value),
        }
    }

    // A view over the buffer just read decodes to the buffer itself, but still
    // takes an object id of its own.
    fn view(&mut self, buffer: V::Value, buffer_len: u64) -> Decoded<V> {
        if self.reader.peek::<V::Error>().ok() != Some(tag::ARRAY_BUFFER_VIEW) {
            return Ok(buffer);
        }
        self.reader.byte()?;
        let subtag = self.reader.varint()?;
        if !u8::try_from(subtag).is_ok_and(|subtag| VIEW_SUBTAGS.contains(&subtag)) {
            return Err(DecodeError::View);
        }
        let offset = self.reader.varint()?;
        let len = self.reader.varint()?;
        if offset > buffer_len || len > buffer_len.saturating_sub(offset) {
            return Err(DecodeError::View);
        }
        if self.version >= 14 {
            self.reader.varint()?;
        }
        let id = self.id()?;
        self.register(id, buffer)?;
        Ok(buffer)
    }

    fn begin(
        &mut self,
        container: Container,
    ) -> Result<(V::Frame, u32, bool), DecodeError<V::Error>> {
        let id = self.id()?;
        let begun = Self::visit(self.visitor.begin(container))?;
        if let Some(value) = begun.value {
            self.register(id, value)?;
        }
        Ok((begun.frame, id, begun.value.is_some()))
    }

    fn end(&mut self, frame: V::Frame, id: u32, registered: bool) -> Decoded<V> {
        let value = Self::visit(self.visitor.end(frame))?;
        if !registered {
            self.register(id, value)?;
        }
        Ok(value)
    }

    fn insert(&mut self, frame: &mut V::Frame, depth: u32) -> Result<(), DecodeError<V::Error>> {
        let key = self.value(Self::child(depth)?)?;
        let value = self.value(Self::child(depth)?)?;
        Self::visit(self.visitor.insert(frame, key, value))
    }

    // Reads keyed entries up to `end`, which may follow padding only if a key does.
    fn entries(
        &mut self,
        frame: &mut V::Frame,
        depth: u32,
        end: u8,
    ) -> Result<u32, DecodeError<V::Error>> {
        let mut count = 0u32;
        while self.reader.peek()? != end {
            self.insert(frame, depth)?;
            count = count.checked_add(1).ok_or(DecodeError::TooManyObjects)?;
        }
        self.reader.byte()?;
        Ok(count)
    }

    fn expect_count(
        &mut self,
        actual: u64,
        what: &'static str,
    ) -> Result<(), DecodeError<V::Error>> {
        if self.reader.varint()? == actual {
            Ok(())
        } else {
            Err(DecodeError::Count(what))
        }
    }

    fn object(&mut self, depth: u32) -> Decoded<V> {
        let (mut frame, id, registered) = self.begin(Container::Object)?;
        let count = self.entries(&mut frame, depth, tag::END_OBJECT)?;
        self.expect_count(count.into(), "object properties")?;
        self.end(frame, id, registered)
    }

    fn dense_array(&mut self, depth: u32) -> Decoded<V> {
        let length = self.reader.varint_u32()?;
        let (mut frame, id, registered) = self.begin(Container::DenseArray { length })?;
        for _ in 0..length {
            let element = if self.reader.peek()? == tag::THE_HOLE {
                self.reader.byte()?;
                Self::visit(self.visitor.scalar(Scalar::Hole))?
            } else {
                self.value(Self::child(depth)?)?
            };
            Self::visit(self.visitor.push(&mut frame, element))?;
        }
        let properties = self.entries(&mut frame, depth, tag::END_DENSE_ARRAY)?;
        self.expect_count(properties.into(), "array property")?;
        self.expect_count(length.into(), "array element")?;
        self.end(frame, id, registered)
    }

    fn sparse_array(&mut self, depth: u32) -> Decoded<V> {
        let length = self.reader.varint_u32()?;
        let (mut frame, id, registered) = self.begin(Container::SparseArray { length })?;
        let properties = self.entries(&mut frame, depth, tag::END_SPARSE_ARRAY)?;
        self.expect_count(properties.into(), "element")?;
        self.expect_count(length.into(), "element")?;
        self.end(frame, id, registered)
    }

    fn map(&mut self, depth: u32) -> Decoded<V> {
        let (mut frame, id, registered) = self.begin(Container::Map)?;
        let entries = self.entries(&mut frame, depth, tag::END_MAP)?;
        self.expect_count(u64::from(entries).saturating_mul(2), "map element")?;
        self.end(frame, id, registered)
    }

    fn set(&mut self, depth: u32) -> Decoded<V> {
        let (mut frame, id, registered) = self.begin(Container::Set)?;
        let mut count = 0u32;
        while self.reader.peek()? != tag::END_SET {
            let element = self.value(Self::child(depth)?)?;
            Self::visit(self.visitor.push(&mut frame, element))?;
            count = count.checked_add(1).ok_or(DecodeError::TooManyObjects)?;
        }
        self.reader.byte()?;
        self.expect_count(count.into(), "set element")?;
        self.end(frame, id, registered)
    }

    // V8 gives an error its id up front but registers it only once complete,
    // so nothing inside an error can refer back to it.
    fn error(&mut self, depth: u32) -> Decoded<V> {
        let id = self.id()?;
        let mut frame = Self::visit(self.visitor.begin(Container::Error))?.frame;
        loop {
            let field = match self.reader.byte()? {
                tag::ERROR_END => break,
                b'E' | b'F' | b'R' | b'S' | b'T' | b'U' => continue,
                tag::ERROR_MESSAGE => "message",
                tag::ERROR_STACK => "stack",
                tag::ERROR_CAUSE => "cause",
                _ => return Err(DecodeError::ErrorObject),
            };
            let key = Self::visit(
                self.visitor
                    .scalar(Scalar::Text(Text::Utf8(field.as_bytes()))),
            )?;
            if field != "cause"
                && !matches!(
                    self.reader.peek()?,
                    tag::UTF8_STRING | tag::ONE_BYTE_STRING | tag::TWO_BYTE_STRING | tag::PADDING
                )
            {
                return Err(DecodeError::ErrorField(field));
            }
            let value = self.value(Self::child(depth)?)?;
            Self::visit(self.visitor.insert(&mut frame, key, value))?;
        }
        let value = Self::visit(self.visitor.end(frame))?;
        self.register(id, value)?;
        Ok(value)
    }
}

fn header<V: Visitor>(bytes: &[u8]) -> Result<(Reader<'_>, u32), DecodeError<V::Error>> {
    let mut reader = Reader { rest: bytes };
    if reader.byte()? != tag::HEADER {
        return Err(DecodeError::Header);
    }
    let version = reader.varint_u32()?;
    if !READ_VERSIONS.contains(&version) {
        return Err(DecodeError::Version(version));
    }
    Ok((reader, version))
}

/// Decodes the one value of a V8 `ValueSerializer` buffer.
pub fn decode_value<V: Visitor>(bytes: &[u8], limits: Limits, visitor: &mut V) -> Decoded<V> {
    let (reader, version) = header::<V>(bytes)?;
    let mut decoder = Decoder {
        reader,
        visitor,
        version,
        limits,
        next_id: 0,
        buffers: Vec::new(),
    };
    decoder.value(limits.max_depth)
}

/// Decodes every value of a buffer written as several values after one
/// header, which share object ids. They arrive as a `Container::Values`.
pub fn decode_values<V: Visitor>(bytes: &[u8], limits: Limits, visitor: &mut V) -> Decoded<V> {
    let (reader, version) = header::<V>(bytes)?;
    let mut decoder = Decoder {
        reader,
        visitor,
        version,
        limits,
        next_id: 0,
        buffers: Vec::new(),
    };
    let mut frame = Decoder::<V>::visit(decoder.visitor.begin(Container::Values))?.frame;
    loop {
        decoder.reader.skip_padding();
        if decoder.reader.rest.is_empty() {
            break;
        }
        let value = decoder.value(limits.max_depth)?;
        Decoder::<V>::visit(decoder.visitor.push(&mut frame, value))?;
    }
    Decoder::<V>::visit(decoder.visitor.end(frame))
}
