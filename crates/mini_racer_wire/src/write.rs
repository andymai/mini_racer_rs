use crate::{WRITE_VERSION, tag};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WriteError {
    #[error("bad bigint")]
    BigInt,
}

// The bit pattern mini_racer writes for every NaN.
const CANONICAL_NAN: f64 = f64::from_bits(0x7FF8_0000_0000_0000);

/// Writes the V8 wire format the way `mini_racer`'s encoder does, into a buffer
/// the caller owns and can reuse.
///
/// Each method that writes an object returns the id V8 gives that object on
/// reading, for `object_ref`.
pub struct Writer<'a> {
    out: &'a mut Vec<u8>,
    next_id: u32,
}

impl<'a> Writer<'a> {
    /// Starts a frame: clears `out` and writes the header.
    pub fn new(out: &'a mut Vec<u8>) -> Self {
        out.clear();
        out.push(tag::HEADER);
        let mut writer = Self { out, next_id: 0 };
        writer.varint(WRITE_VERSION.into());
        writer
    }

    const fn id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id = id.saturating_add(1);
        id
    }

    fn varint(&mut self, mut value: u64) {
        while value > 0x7F {
            self.out
                .push(0x80 | u8::try_from(value & 0x7F).unwrap_or(0));
            value >>= 7;
        }
        self.out.push(u8::try_from(value).unwrap_or(u8::MAX));
    }

    fn zigzag(&mut self, value: i32) {
        self.varint(u64::from(((value << 1) ^ (value >> 31)).cast_unsigned()));
    }

    fn len(&mut self, len: usize) {
        self.varint(u64::try_from(len).unwrap_or(u64::MAX));
    }

    pub fn null(&mut self) {
        self.out.push(tag::NULL);
    }

    pub fn undefined(&mut self) {
        self.out.push(tag::UNDEFINED);
    }

    pub fn bool(&mut self, value: bool) {
        self.out.push(if value { tag::TRUE } else { tag::FALSE });
    }

    pub fn number(&mut self, value: f64) {
        self.out.push(tag::DOUBLE);
        let value = if value.is_nan() { CANONICAL_NAN } else { value };
        self.out.extend_from_slice(&value.to_le_bytes());
    }

    /// An int32 when it fits; otherwise, as `mini_racer` does, a double up to
    /// `2**63 / 1024` in magnitude, and a `BigInt` beyond.
    pub fn integer(&mut self, value: i64) {
        if let Ok(small) = i32::try_from(value) {
            self.out.push(tag::INT32);
            self.zigzag(small);
        } else if value > i64::MIN / 1024 && value <= i64::MAX / 1024 {
            #[expect(
                clippy::cast_precision_loss,
                reason = "mini_racer rounds these to a double too"
            )]
            self.number(value as f64);
        } else {
            let magnitude = value.unsigned_abs().to_le_bytes();
            // Eight bytes always form one whole word.
            let _ = self.bigint(value < 0, &magnitude);
        }
    }

    /// `digits` is the magnitude as little-endian 64-bit words.
    pub fn bigint(&mut self, negative: bool, digits: &[u8]) -> Result<(), WriteError> {
        if !digits.len().is_multiple_of(8) {
            return Err(WriteError::BigInt);
        }
        let used = digits
            .iter()
            .rposition(|&byte| byte != 0)
            .map_or(0, |last| last.saturating_add(1));
        self.out.push(tag::BIGINT);
        if used == 0 {
            self.out.push(0);
            return Ok(());
        }
        let words = used.div_ceil(8).saturating_mul(8);
        let bitfield =
            u64::try_from(words).unwrap_or(u64::MAX).saturating_mul(2) | u64::from(negative);
        self.varint(bitfield);
        self.out
            .extend_from_slice(digits.get(..words).unwrap_or(digits));
        Ok(())
    }

    pub fn utf8(&mut self, bytes: &[u8]) {
        self.out.push(tag::UTF8_STRING);
        self.len(bytes.len());
        self.out.extend_from_slice(bytes);
    }

    pub fn latin1(&mut self, bytes: &[u8]) {
        self.out.push(tag::ONE_BYTE_STRING);
        self.len(bytes.len());
        self.out.extend_from_slice(bytes);
    }

    /// `bytes` is UTF-16LE; its length is in bytes.
    pub fn utf16le(&mut self, bytes: &[u8]) {
        self.out.push(tag::TWO_BYTE_STRING);
        self.len(bytes.len());
        self.out.extend_from_slice(bytes);
    }

    /// Milliseconds since the Unix epoch; non-finite times become NaN.
    pub fn date(&mut self, millis: f64) -> u32 {
        self.out.push(tag::DATE);
        let millis = if millis.is_finite() {
            millis
        } else {
            CANONICAL_NAN
        };
        self.out.extend_from_slice(&millis.to_le_bytes());
        self.id()
    }

    /// A `Uint8Array` over a fresh buffer holding `bytes`. The id is the
    /// view's; the buffer takes the one before it.
    pub fn uint8_array(&mut self, bytes: &[u8]) -> u32 {
        self.out.push(tag::ARRAY_BUFFER);
        self.len(bytes.len());
        self.out.extend_from_slice(bytes);
        self.out.push(tag::ARRAY_BUFFER_VIEW);
        self.out.push(tag::UINT8_ARRAY);
        self.varint(0);
        self.len(bytes.len());
        self.varint(0);
        self.id();
        self.id()
    }

    pub fn begin_object(&mut self) -> u32 {
        self.out.push(tag::BEGIN_OBJECT);
        self.id()
    }

    pub fn end_object(&mut self, properties: u32) {
        self.out.push(tag::END_OBJECT);
        self.varint(properties.into());
    }

    pub fn begin_array(&mut self, length: u32) -> u32 {
        self.out.push(tag::BEGIN_DENSE_ARRAY);
        self.varint(length.into());
        self.id()
    }

    pub fn end_array(&mut self, length: u32) {
        self.out.push(tag::END_DENSE_ARRAY);
        self.varint(0);
        self.varint(length.into());
    }

    pub fn object_ref(&mut self, id: u32) {
        self.out.push(tag::OBJECT_REFERENCE);
        self.varint(id.into());
    }
}
