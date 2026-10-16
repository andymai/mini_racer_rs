// Returned HTML is mostly ASCII, so whole blocks of it are checked and copied
// at once; only blocks holding other characters go one at a time.
const BLOCK: usize = 32;

#[must_use]
pub fn latin1_to_utf8(bytes: &[u8]) -> String {
    let mut out = Vec::with_capacity(bytes.len());
    let mut span = 0;
    let mut at: usize = 0;
    for block in bytes.chunks(BLOCK) {
        let next = at.saturating_add(block.len());
        if !block.is_ascii() {
            out.extend_from_slice(bytes.get(span..at).unwrap_or_default());
            out.reserve(block.len().saturating_mul(2));
            for &byte in block {
                let mut buffer = [0; 2];
                out.extend_from_slice(char::from(byte).encode_utf8(&mut buffer).as_bytes());
            }
            span = next;
        }
        at = next;
    }
    out.extend_from_slice(bytes.get(span..).unwrap_or_default());
    String::from_utf8(out).unwrap_or_default()
}

/// `None` for an odd length or a lone surrogate.
#[must_use]
pub fn utf16le_to_utf8(bytes: &[u8]) -> Option<String> {
    let (pairs, rest) = bytes.as_chunks::<2>();
    if !rest.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(pairs.len());
    let mut cursor = pairs;
    while !cursor.is_empty() {
        if let Some((block, after)) = cursor.split_at_checked(BLOCK)
            && block
                .iter()
                .fold(0, |any, pair| any | u16::from_le_bytes(*pair))
                < 0x80
        {
            out.extend(block.iter().map(|pair| pair[0]));
            cursor = after;
            continue;
        }
        let (&first, after) = cursor.split_first()?;
        cursor = after;
        let unit = u16::from_le_bytes(first);
        let code = match unit {
            0xD800..=0xDBFF => {
                let (&second, after) = cursor.split_first()?;
                let low = u16::from_le_bytes(second);
                if !(0xDC00..=0xDFFF).contains(&low) {
                    return None;
                }
                cursor = after;
                0x1_0000_u32
                    .checked_add((u32::from(unit) & 0x3FF) << 10)?
                    .checked_add(u32::from(low) & 0x3FF)?
            }
            0xDC00..=0xDFFF => return None,
            _ => u32::from(unit),
        };
        let mut buffer = [0; 4];
        out.extend_from_slice(char::from_u32(code)?.encode_utf8(&mut buffer).as_bytes());
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latin1_maps_bytes_to_code_points() {
        assert_eq!(latin1_to_utf8(b"abc"), "abc");
        assert_eq!(latin1_to_utf8(&[0x63, 0x61, 0x66, 0xE9]), "café");
    }

    fn utf16_by_char(bytes: &[u8]) -> Option<String> {
        let (pairs, rest) = bytes.as_chunks::<2>();
        if !rest.is_empty() {
            return None;
        }
        char::decode_utf16(pairs.iter().map(|pair| u16::from_le_bytes(*pair)))
            .collect::<Result<String, _>>()
            .ok()
    }

    proptest::proptest! {
        #[test]
        fn latin1_matches_mapping_each_byte(bytes: Vec<u8>) {
            let by_byte: String = bytes.iter().copied().map(char::from).collect();
            proptest::prop_assert_eq!(latin1_to_utf8(&bytes), by_byte);
        }

        #[test]
        fn utf16_matches_decoding_each_unit(bytes: Vec<u8>, mostly_ascii: bool) {
            let bytes = if mostly_ascii {
                bytes.iter().enumerate().map(|(i, &b)| if i % 2 == 1 && b % 5 != 0 { 0 } else { b }).collect()
            } else {
                bytes
            };
            proptest::prop_assert_eq!(utf16le_to_utf8(&bytes), utf16_by_char(&bytes));
        }
    }

    #[test]
    fn utf16_rejects_odd_lengths_and_lone_surrogates() {
        assert_eq!(
            utf16le_to_utf8(&[0x3D, 0xD8, 0x00, 0xDE]).as_deref(),
            Some("😀")
        );
        assert_eq!(utf16le_to_utf8(&[0x41]), None);
        assert_eq!(utf16le_to_utf8(&[0x3D, 0xD8]), None);
    }
}
