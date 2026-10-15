use crate::READ_VERSIONS;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Scalar<'a> {
    Undefined,
    Null,
    Bool(bool),
    Int32(i32),
    Uint32(u32),
    Number(f64),
    Utf8(&'a [u8]),
    Latin1(&'a [u8]),
    Utf16Le(&'a [u8]),
}

pub trait Visitor {
    type Value: Copy;
    type Error;

    fn scalar(&mut self, scalar: Scalar<'_>) -> Result<Self::Value, Self::Error>;
}

#[derive(Debug, thiserror::Error)]
pub enum DecodeError<E> {
    #[error("bad header")]
    Header,
    #[error("unsupported wire format version {0}")]
    Version(u32),
    #[error("truncated input")]
    Truncated,
    #[error("varint overflow")]
    Varint,
    #[error("bad tag: {tag:#04x}")]
    Unsupported { tag: u8 },
    #[error("{0}")]
    Visitor(E),
}

mod tag {
    pub const HEADER: u8 = 0xFF;
    pub const PADDING: u8 = 0x00;
    pub const VERIFY_OBJECT_COUNT: u8 = b'?';
    pub const UNDEFINED: u8 = b'_';
    pub const NULL: u8 = b'0';
    pub const TRUE: u8 = b'T';
    pub const FALSE: u8 = b'F';
    pub const INT32: u8 = b'I';
    pub const UINT32: u8 = b'U';
    pub const DOUBLE: u8 = b'N';
    pub const UTF8_STRING: u8 = b'S';
    pub const ONE_BYTE_STRING: u8 = b'"';
    pub const TWO_BYTE_STRING: u8 = b'c';
}

struct Reader<'a> {
    rest: &'a [u8],
}

impl<'a> Reader<'a> {
    fn byte<E>(&mut self) -> Result<u8, DecodeError<E>> {
        let (&first, rest) = self.rest.split_first().ok_or(DecodeError::Truncated)?;
        self.rest = rest;
        Ok(first)
    }

    fn varint<E>(&mut self) -> Result<u64, DecodeError<E>> {
        let mut result = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = self.byte()?;
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
}

const fn zigzag(raw: u32) -> i32 {
    let magnitude = raw >> 1;
    if raw & 1 == 0 {
        magnitude.cast_signed()
    } else {
        !magnitude.cast_signed()
    }
}

pub fn decode_value<V: Visitor>(
    bytes: &[u8],
    visitor: &mut V,
) -> Result<V::Value, DecodeError<V::Error>> {
    let mut reader = Reader { rest: bytes };
    if reader.byte()? != tag::HEADER {
        return Err(DecodeError::Header);
    }
    let version = reader.varint_u32()?;
    if !READ_VERSIONS.contains(&version) {
        return Err(DecodeError::Version(version));
    }
    loop {
        let scalar = match reader.byte()? {
            tag::PADDING => continue,
            tag::VERIFY_OBJECT_COUNT => {
                reader.varint()?;
                continue;
            }
            tag::UNDEFINED => Scalar::Undefined,
            tag::NULL => Scalar::Null,
            tag::TRUE => Scalar::Bool(true),
            tag::FALSE => Scalar::Bool(false),
            tag::INT32 => Scalar::Int32(zigzag(reader.varint_u32()?)),
            tag::UINT32 => Scalar::Uint32(reader.varint_u32()?),
            tag::DOUBLE => Scalar::Number(reader.double()?),
            tag::UTF8_STRING => {
                let len = reader.varint()?;
                Scalar::Utf8(reader.take(len)?)
            }
            tag::ONE_BYTE_STRING => {
                let len = reader.varint()?;
                Scalar::Latin1(reader.take(len)?)
            }
            tag::TWO_BYTE_STRING => {
                let len = reader.varint()?;
                Scalar::Utf16Le(reader.take(len)?)
            }
            other => return Err(DecodeError::Unsupported { tag: other }),
        };
        return visitor.scalar(scalar).map_err(DecodeError::Visitor);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq)]
    enum Out {
        Nil,
        Bool(bool),
        Int(i64),
        Num(f64),
        Text(usize),
    }

    #[derive(Default)]
    struct Collect {
        texts: Vec<String>,
    }

    impl Visitor for Collect {
        type Value = Out;
        type Error = std::convert::Infallible;

        fn scalar(&mut self, scalar: Scalar<'_>) -> Result<Out, Self::Error> {
            let text = |s: String, texts: &mut Vec<String>| {
                texts.push(s);
                Out::Text(texts.len())
            };
            Ok(match scalar {
                Scalar::Undefined | Scalar::Null => Out::Nil,
                Scalar::Bool(b) => Out::Bool(b),
                Scalar::Int32(i) => Out::Int(i.into()),
                Scalar::Uint32(u) => Out::Int(u.into()),
                Scalar::Number(n) => Out::Num(n),
                Scalar::Utf8(b) => text(String::from_utf8_lossy(b).into_owned(), &mut self.texts),
                Scalar::Latin1(b) => text(crate::text::latin1_to_utf8(b), &mut self.texts),
                Scalar::Utf16Le(b) => text(
                    crate::text::utf16le_to_utf8(b).unwrap_or_default(),
                    &mut self.texts,
                ),
            })
        }
    }

    fn decode(
        bytes: &[u8],
    ) -> (
        Result<Out, DecodeError<std::convert::Infallible>>,
        Vec<String>,
    ) {
        let mut collect = Collect::default();
        let out = decode_value(bytes, &mut collect);
        (out, collect.texts)
    }

    #[test]
    fn decodes_primitives() {
        assert_eq!(decode(&[0xFF, 0x0F, b'_']).0.ok(), Some(Out::Nil));
        assert_eq!(decode(&[0xFF, 0x0F, b'0']).0.ok(), Some(Out::Nil));
        assert_eq!(decode(&[0xFF, 0x0F, b'T']).0.ok(), Some(Out::Bool(true)));
        assert_eq!(decode(&[0xFF, 0x0F, b'F']).0.ok(), Some(Out::Bool(false)));
        assert_eq!(decode(&[0xFF, 0x0F, b'I', 0x04]).0.ok(), Some(Out::Int(2)));
        assert_eq!(decode(&[0xFF, 0x0F, b'I', 0x03]).0.ok(), Some(Out::Int(-2)));
        assert_eq!(
            decode(&[0xFF, 0x0F, b'I', 0xFF, 0xFF, 0xFF, 0xFF, 0x0F])
                .0
                .ok(),
            Some(Out::Int(i64::from(i32::MIN)))
        );
        assert_eq!(
            decode(&[0xFF, 0x0F, b'U', 0x80, 0x01]).0.ok(),
            Some(Out::Int(128))
        );
        let mut number = vec![0xFF, 0x0F, b'N'];
        number.extend_from_slice(&1.5f64.to_le_bytes());
        assert_eq!(decode(&number).0.ok(), Some(Out::Num(1.5)));
    }

    #[test]
    fn decodes_strings_after_padding() {
        let (out, texts) = decode(&[0xFF, 0x10, 0x00, b'"', 0x03, b'c', 0xE9, b'!']);
        assert_eq!(out.ok(), Some(Out::Text(1)));
        assert_eq!(texts, vec!["cé!".to_owned()]);
        let (_, texts) = decode(&[0xFF, 0x0F, b'c', 0x04, 0x71, 0x67, 0xAC, 0x4E]);
        assert_eq!(texts, vec!["東京".to_owned()]);
        let (_, texts) = decode(&[0xFF, 0x0F, b'S', 0x02, 0xC3, 0xA9]);
        assert_eq!(texts, vec!["é".to_owned()]);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(matches!(decode(&[]).0, Err(DecodeError::Truncated)));
        assert!(matches!(decode(&[0x00, 0x0F]).0, Err(DecodeError::Header)));
        assert!(matches!(
            decode(&[0xFF, 0x0C, b'0']).0,
            Err(DecodeError::Version(12))
        ));
        assert!(matches!(
            decode(&[0xFF, 0x11, b'0']).0,
            Err(DecodeError::Version(17))
        ));
        assert!(matches!(
            decode(&[0xFF, 0x0F, b'"', 0x05, b'a']).0,
            Err(DecodeError::Truncated)
        ));
        assert!(matches!(
            decode(&[0xFF, 0x0F, b'N', 0x00]).0,
            Err(DecodeError::Truncated)
        ));
        assert!(matches!(
            decode(&[0xFF, 0x0F, b'o']).0,
            Err(DecodeError::Unsupported { tag: b'o' })
        ));
        assert!(matches!(
            decode(&[
                0xFF, 0x0F, b'I', 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF
            ])
            .0,
            Err(DecodeError::Varint)
        ));
    }
}
