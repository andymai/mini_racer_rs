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
    /// `length` is at most the bytes left in the input.
    DenseArray {
        length: u32,
    },
    /// `length` can be far larger than the entries that follow; do not
    /// reserve by it.
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
///
/// Runs of numbers in an array can come all at once instead, through the
/// methods whose defaults make those calls.
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

    /// Remembers `value` as V8's object `id` for later back references. Ids
    /// arrive in increasing order, except that a container whose `begin`
    /// returned no value is registered when it ends.
    fn register(&mut self, id: u32, value: Self::Value) -> Result<(), Self::Error>;

    fn back_ref(&mut self, id: u32) -> Result<Self::Value, Self::Error>;

    /// Consecutive int32 elements of a dense array, each of which would
    /// otherwise reach `scalar` and then `push`.
    fn push_int32s(&mut self, frame: &mut Self::Frame, values: &[i32]) -> Result<(), Self::Error> {
        for &n in values {
            let value = self.scalar(Scalar::Int32(n))?;
            self.push(frame, value)?;
        }
        Ok(())
    }

    /// Consecutive double elements of a dense array, each of which would
    /// otherwise reach `scalar` and then `push`.
    fn push_numbers(&mut self, frame: &mut Self::Frame, values: &[f64]) -> Result<(), Self::Error> {
        for &n in values {
            let value = self.scalar(Scalar::Number(n))?;
            self.push(frame, value)?;
        }
        Ok(())
    }

    /// Consecutive entries of a sparse array with an int32 key and an int32
    /// value, each of which would otherwise reach `scalar` for its key, then
    /// for its value, and then `insert`.
    fn insert_int32s(
        &mut self,
        frame: &mut Self::Frame,
        entries: &[(i32, i32)],
    ) -> Result<(), Self::Error> {
        for &(key, n) in entries {
            let key = self.scalar(Scalar::Int32(key))?;
            let value = self.scalar(Scalar::Int32(n))?;
            self.insert(frame, key, value)?;
        }
        Ok(())
    }

    /// Consecutive entries of a sparse array with an int32 key and a double
    /// value, each of which would otherwise reach `scalar` for its key, then
    /// for its value, and then `insert`.
    fn insert_numbers(
        &mut self,
        frame: &mut Self::Frame,
        entries: &[(i32, f64)],
    ) -> Result<(), Self::Error> {
        for &(key, n) in entries {
            let key = self.scalar(Scalar::Int32(key))?;
            let value = self.scalar(Scalar::Number(n))?;
            self.insert(frame, key, value)?;
        }
        Ok(())
    }
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
    #[error("bad object reference {0}")]
    Reference(u32),
    #[error("bad arraybuffer")]
    ArrayBuffer,
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

// A view's offset and length must be multiples of its element size.
const fn element_size(subtag: u8) -> Option<u64> {
    match subtag {
        b'?' | b'B' | b'C' | b'b' => Some(1),
        b'W' | b'w' | b'h' => Some(2),
        b'D' | b'd' | b'f' => Some(4),
        b'F' | b'Q' | b'q' => Some(8),
        _ => None,
    }
}

const REGEXP_FLAG_BITS: u32 = 9;
const REGEXP_UNICODE: u32 = 1 << 4;
// V8 rejects it unless its experimental regexp engine is on, which it is not.
const REGEXP_LINEAR: u32 = 1 << 6;
const REGEXP_UNICODE_SETS: u32 = 1 << 8;

const VIEW_LENGTH_TRACKING: u64 = 1;
const VIEW_BACKED_BY_RESIZABLE: u64 = 1 << 1;

#[derive(Clone, Copy)]
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

    // Every NaN reads as the one quiet NaN, as in V8 and mini_racer.
    fn double<E>(&mut self) -> Result<f64, DecodeError<E>> {
        let bytes: [u8; 8] = self
            .take(8)?
            .try_into()
            .map_err(|_| DecodeError::Truncated)?;
        let value = f64::from_le_bytes(bytes);
        Ok(if value.is_nan() { f64::NAN } else { value })
    }

    // V8 pads before a two-byte string to align its characters.
    const fn skip_padding(&mut self) {
        while let Some((&tag::PADDING, rest)) = self.rest.split_first() {
            self.rest = rest;
        }
    }

    // The readers a run takes elements with. Each consumes nothing unless
    // the whole element reads cleanly, so whatever stops a run is read again,
    // and reported, by the general path. They are marked inline because the
    // generic decoder is compiled in the visitor's crate, which cannot inline
    // them otherwise.
    #[inline]
    fn int32_element(&mut self) -> Option<i32> {
        let mut ahead = *self;
        if ahead.byte::<()>().ok()? != tag::INT32 {
            return None;
        }
        let n = zigzag(ahead.varint_u32::<()>().ok()?);
        *self = ahead;
        Some(n)
    }

    #[inline]
    fn number_element(&mut self) -> Option<f64> {
        let mut ahead = *self;
        if ahead.byte::<()>().ok()? != tag::DOUBLE {
            return None;
        }
        let n = ahead.double::<()>().ok()?;
        *self = ahead;
        Some(n)
    }

    #[inline]
    fn int32_entry(&mut self) -> Option<(i32, i32)> {
        let mut ahead = *self;
        let entry = (ahead.int32_element()?, ahead.int32_element()?);
        *self = ahead;
        Some(entry)
    }

    #[inline]
    fn number_entry(&mut self) -> Option<(i32, f64)> {
        let mut ahead = *self;
        let entry = (ahead.int32_element()?, ahead.number_element()?);
        *self = ahead;
        Some(entry)
    }
}

const RUN: usize = 64;

const fn zigzag(raw: u32) -> i32 {
    let magnitude = raw >> 1;
    if raw & 1 == 0 {
        magnitude.cast_signed()
    } else {
        !magnitude.cast_signed()
    }
}

#[derive(Clone, Copy)]
struct Buffer {
    id: u32,
    len: u64,
    resizable: bool,
}

struct Decoder<'a, 'v, V> {
    reader: Reader<'a>,
    visitor: &'v mut V,
    version: u32,
    limits: Limits,
    next_id: u32,
    // One bit per id, set once registered, so a reference to an id not yet
    // registered fails here as in V8, whatever the visitor stores.
    registered: Vec<u64>,
    // In increasing id order, so a view after a back reference can be
    // checked against its buffer.
    buffers: Vec<Buffer>,
}

type Decoded<V> = Result<<V as Visitor>::Value, DecodeError<<V as Visitor>::Error>>;

impl<'a, 'v, V: Visitor> Decoder<'a, 'v, V> {
    fn visit<T>(result: Result<T, V::Error>) -> Result<T, DecodeError<V::Error>> {
        result.map_err(DecodeError::Visitor)
    }

    fn new(
        bytes: &'a [u8],
        limits: Limits,
        visitor: &'v mut V,
    ) -> Result<Self, DecodeError<V::Error>> {
        let mut reader = Reader { rest: bytes };
        if reader.byte()? != tag::HEADER {
            return Err(DecodeError::Header);
        }
        let version = reader.varint_u32()?;
        if !READ_VERSIONS.contains(&version) {
            return Err(DecodeError::Version(version));
        }
        Ok(Self {
            reader,
            visitor,
            version,
            limits,
            next_id: 0,
            registered: Vec::new(),
            buffers: Vec::new(),
        })
    }

    fn id(&mut self) -> Result<u32, DecodeError<V::Error>> {
        let id = self.next_id;
        self.next_id = id.checked_add(1).ok_or(DecodeError::TooManyObjects)?;
        Ok(id)
    }

    const fn bit(id: u32) -> (usize, u64) {
        ((id / 64) as usize, 1 << (id % 64))
    }

    fn is_registered(&self, id: u32) -> bool {
        let (word, mask) = Self::bit(id);
        self.registered
            .get(word)
            .is_some_and(|bits| bits & mask != 0)
    }

    fn register(&mut self, id: u32, value: V::Value) -> Result<(), DecodeError<V::Error>> {
        let (word, mask) = Self::bit(id);
        if self.registered.len() <= word {
            self.registered.resize(word.saturating_add(1), 0);
        }
        if let Some(bits) = self.registered.get_mut(word) {
            *bits |= mask;
        }
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
                let bytes = self.reader.take(len)?;
                if !len.is_multiple_of(2) {
                    return Err(DecodeError::Utf16Length);
                }
                Text::Utf16Le(bytes)
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
        let digits = self.reader.take(len)?;
        if len > self.limits.max_bigint_bytes {
            return Err(DecodeError::BigIntTooBig);
        }
        Ok(Scalar::BigInt { negative, digits })
    }

    fn tag(&mut self) -> Result<u8, DecodeError<V::Error>> {
        loop {
            match self.reader.byte()? {
                tag::PADDING => {}
                tag::VERIFY_OBJECT_COUNT => {
                    self.reader.varint_u32()?;
                }
                tag => return Ok(tag),
            }
        }
    }

    // Where V8 reads a string by itself: any value, which must be a string.
    fn string(&mut self) -> Result<Option<Text<'a>>, DecodeError<V::Error>> {
        let tag = self.tag()?;
        self.text(tag)
    }

    fn value(&mut self, depth: u32) -> Decoded<V> {
        let tag = self.tag()?;
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
                let text = self.string()?.ok_or(DecodeError::StringObject)?;
                return self.scalar_with_id(Scalar::Text(text));
            }
            tag::DATE => {
                let time = self.reader.double()?;
                return self.scalar_with_id(Scalar::Date(time));
            }
            tag::REGEXP => {
                // mini_racer counts the pattern as a level of nesting.
                Self::child(depth)?;
                let pattern = self.string()?.ok_or(DecodeError::RegExp)?;
                let flags = self.reader.varint_u32()?;
                let unicode_both = REGEXP_UNICODE | REGEXP_UNICODE_SETS;
                if flags >> REGEXP_FLAG_BITS != 0
                    || flags & REGEXP_LINEAR != 0
                    || flags & unicode_both == unicode_both
                {
                    return Err(DecodeError::RegExp);
                }
                return self.scalar_with_id(Scalar::RegExp { pattern, flags });
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
        let resizable = tag == tag::RESIZABLE_ARRAY_BUFFER;
        if resizable && self.reader.varint()? < len {
            return Err(DecodeError::ArrayBuffer);
        }
        let id = self.id()?;
        let bytes = self.reader.take(len)?;
        let value = Self::visit(self.visitor.scalar(Scalar::ArrayBuffer(bytes)))?;
        self.register(id, value)?;
        let buffer = Buffer { id, len, resizable };
        self.buffers.push(buffer);
        self.view(value, buffer)
    }

    fn back_ref(&mut self) -> Decoded<V> {
        let id = self.reader.varint_u32()?;
        if !self.is_registered(id) {
            return Err(DecodeError::Reference(id));
        }
        let value = Self::visit(self.visitor.back_ref(id))?;
        let buffer = self
            .buffers
            .binary_search_by_key(&id, |buffer| buffer.id)
            .ok()
            .and_then(|index| self.buffers.get(index).copied());
        buffer.map_or(Ok(value), |buffer| self.view(value, buffer))
    }

    // A view over a buffer decodes to the buffer itself but still takes an
    // object id of its own.
    fn view(&mut self, buffer: V::Value, of: Buffer) -> Decoded<V> {
        if self.reader.peek::<V::Error>().ok() != Some(tag::ARRAY_BUFFER_VIEW) {
            return Ok(buffer);
        }
        self.reader.byte()?;
        let size = u8::try_from(self.reader.varint()?)
            .ok()
            .and_then(element_size)
            .ok_or(DecodeError::View)?;
        let offset = self.reader.varint()?;
        let len = self.reader.varint()?;
        if offset > of.len
            || len > of.len.saturating_sub(offset)
            || !offset.is_multiple_of(size)
            || !len.is_multiple_of(size)
        {
            return Err(DecodeError::View);
        }
        if self.version >= 14 {
            let flags = self.reader.varint()?;
            let tracking = flags & VIEW_LENGTH_TRACKING != 0;
            let backed = flags & VIEW_BACKED_BY_RESIZABLE != 0;
            if (tracking || backed) != of.resizable || (of.resizable && !backed) {
                return Err(DecodeError::View);
            }
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
        // Every element takes at least a byte.
        let Some(mut left) = usize::try_from(length)
            .ok()
            .filter(|&length| length <= self.reader.rest.len())
        else {
            return Err(DecodeError::Truncated);
        };
        let (mut frame, id, registered) = self.begin(Container::DenseArray { length })?;
        while left > 0 {
            let taken = match self.element_run(&mut frame, depth, left)? {
                0 => {
                    self.element(&mut frame, depth)?;
                    1
                }
                run => run,
            };
            left = left.saturating_sub(taken);
        }
        let properties = self.entries(&mut frame, depth, tag::END_DENSE_ARRAY)?;
        self.expect_count(properties.into(), "array property")?;
        self.expect_count(length.into(), "array element")?;
        self.end(frame, id, registered)
    }

    fn element(&mut self, frame: &mut V::Frame, depth: u32) -> Result<(), DecodeError<V::Error>> {
        let element = if self.reader.peek()? == tag::THE_HOLE {
            self.reader.byte()?;
            Self::visit(self.visitor.scalar(Scalar::Hole))?
        } else {
            self.value(Self::child(depth)?)?
        };
        Self::visit(self.visitor.push(frame, element))
    }

    fn element_run(
        &mut self,
        frame: &mut V::Frame,
        depth: u32,
        limit: usize,
    ) -> Result<usize, DecodeError<V::Error>> {
        match self.reader.rest.first() {
            Some(&tag::INT32) => {
                self.run(frame, depth, limit, Reader::int32_element, V::push_int32s)
            }
            Some(&tag::DOUBLE) => {
                self.run(frame, depth, limit, Reader::number_element, V::push_numbers)
            }
            _ => Ok(0),
        }
    }

    // Hands the visitor up to `limit` elements that `read` takes, at once,
    // and returns how many it read; 0 leaves the next element to the general
    // path, which reads it again and reports what stopped `read`. A run
    // checks the depth as its first element would. A lone element skips the
    // buffer, which costs more to fill than a run of one saves. Never inlined,
    // so the buffer is not on the stack in every frame of a deeply nested value.
    #[inline(never)]
    fn run<T: Copy>(
        &mut self,
        frame: &mut V::Frame,
        depth: u32,
        limit: usize,
        read: impl Fn(&mut Reader<'a>) -> Option<T>,
        hand: impl FnOnce(&mut V, &mut V::Frame, &[T]) -> Result<(), V::Error>,
    ) -> Result<usize, DecodeError<V::Error>> {
        let Some(first) = read(&mut self.reader) else {
            return Ok(0);
        };
        Self::child(depth)?;
        let mut ahead = self.reader;
        if limit < 2 || read(&mut ahead).is_none() {
            Self::visit(hand(self.visitor, frame, &[first]))?;
            return Ok(1);
        }
        let mut buffer = [first; RUN];
        let mut filled = 1usize;
        for slot in buffer.iter_mut().take(limit).skip(1) {
            let Some(value) = read(&mut self.reader) else {
                break;
            };
            *slot = value;
            filled = filled.saturating_add(1);
        }
        let run = buffer.get(..filled).unwrap_or_default();
        Self::visit(hand(self.visitor, frame, run))?;
        Ok(run.len())
    }

    fn sparse_array(&mut self, depth: u32) -> Decoded<V> {
        let length = self.reader.varint_u32()?;
        let (mut frame, id, registered) = self.begin(Container::SparseArray { length })?;
        let mut properties = 0u32;
        while self.reader.peek()? != tag::END_SPARSE_ARRAY {
            // A run stops short of the entry that overflows the count, so the
            // general path inserts it and then fails.
            let room = usize::try_from(u32::MAX.saturating_sub(properties)).unwrap_or(usize::MAX);
            let taken = match self.int32_keyed(&mut frame, depth, room)? {
                0 => {
                    self.insert(&mut frame, depth)?;
                    1
                }
                run => u32::try_from(run).map_err(|_| DecodeError::TooManyObjects)?,
            };
            properties = properties
                .checked_add(taken)
                .ok_or(DecodeError::TooManyObjects)?;
        }
        self.reader.byte()?;
        self.expect_count(properties.into(), "element")?;
        self.expect_count(length.into(), "element")?;
        self.end(frame, id, registered)
    }

    // Reads entries whose key is an int32, the shape V8 writes for a holey
    // array's elements, and returns how many: a run while their values are
    // int32s or doubles, and otherwise the one entry, as `insert` reads it but
    // without reading its key twice. 0 leaves the next entry to `insert`.
    fn int32_keyed(
        &mut self,
        frame: &mut V::Frame,
        depth: u32,
        limit: usize,
    ) -> Result<usize, DecodeError<V::Error>> {
        if limit == 0 {
            return Ok(0);
        }
        let mut ahead = self.reader;
        let Some(key) = ahead.int32_element() else {
            return Ok(0);
        };
        match ahead.rest.first() {
            Some(&tag::INT32) => {
                self.run(frame, depth, limit, Reader::int32_entry, V::insert_int32s)
            }
            Some(&tag::DOUBLE) => {
                self.run(frame, depth, limit, Reader::number_entry, V::insert_numbers)
            }
            _ => {
                self.reader = ahead;
                Self::child(depth)?;
                let key = Self::visit(self.visitor.scalar(Scalar::Int32(key)))?;
                let value = self.value(Self::child(depth)?)?;
                Self::visit(self.visitor.insert(frame, key, value))?;
                Ok(1)
            }
        }
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

    // V8 reads an optional prototype, message and stack in that order, then
    // registers the error, so a cause after them can refer back to it.
    fn error(&mut self, depth: u32) -> Decoded<V> {
        let id = self.id()?;
        let begun = Self::visit(self.visitor.begin(Container::Error))?;
        let mut frame = begun.frame;
        let mut field = self.reader.byte()?;
        if matches!(field, b'E' | b'F' | b'R' | b'S' | b'T' | b'U') {
            field = self.reader.byte()?;
        }
        for (tag, name) in [(tag::ERROR_MESSAGE, "message"), (tag::ERROR_STACK, "stack")] {
            if field == tag {
                Self::child(depth)?;
                let text = self.string()?.ok_or(DecodeError::ErrorField(name))?;
                let key = self.key(name)?;
                let value = Self::visit(self.visitor.scalar(Scalar::Text(text)))?;
                Self::visit(self.visitor.insert(&mut frame, key, value))?;
                field = self.reader.byte()?;
            }
        }
        if let Some(value) = begun.value {
            self.register(id, value)?;
        }
        if field == tag::ERROR_CAUSE {
            let key = self.key("cause")?;
            let value = self.value(Self::child(depth)?)?;
            Self::visit(self.visitor.insert(&mut frame, key, value))?;
            field = self.reader.byte()?;
        }
        if field != tag::ERROR_END {
            return Err(DecodeError::ErrorObject);
        }
        self.end(frame, id, begun.value.is_some())
    }

    fn key(&mut self, name: &'static str) -> Decoded<V> {
        Self::visit(
            self.visitor
                .scalar(Scalar::Text(Text::Utf8(name.as_bytes()))),
        )
    }
}

/// Decodes the one value of a V8 `ValueSerializer` buffer.
pub fn decode_value<V: Visitor>(bytes: &[u8], limits: Limits, visitor: &mut V) -> Decoded<V> {
    Decoder::new(bytes, limits, visitor)?.value(limits.max_depth)
}

/// Decodes every value of a buffer written as several values after one
/// header, which share object ids. They arrive as a `Container::Values`.
pub fn decode_values<V: Visitor>(bytes: &[u8], limits: Limits, visitor: &mut V) -> Decoded<V> {
    let mut decoder = Decoder::new(bytes, limits, visitor)?;
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
