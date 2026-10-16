use std::borrow::Cow;

// Returned HTML is mostly ASCII, so ASCII is copied up to a block at a time
// and everything else a character at a time.
const BLOCK: usize = 32;

fn ascii(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).expect("an ASCII block is UTF-8")
}

/// Borrows when every byte is ASCII.
#[must_use]
pub fn latin1_to_utf8(bytes: &[u8]) -> Cow<'_, str> {
    if bytes.len() < BLOCK {
        return if bytes.is_ascii() {
            Cow::Borrowed(ascii(bytes))
        } else {
            Cow::Owned(bytes.iter().copied().map(char::from).collect())
        };
    }
    let wide: usize = bytes
        .chunks(BLOCK)
        .map(|block| {
            usize::from(
                block
                    .iter()
                    .map(|&byte| u8::from(!byte.is_ascii()))
                    .sum::<u8>(),
            )
        })
        .sum();
    if wide == 0 {
        return Cow::Borrowed(ascii(bytes));
    }
    let mut out = String::with_capacity(bytes.len().saturating_add(wide));
    let mut span = 0;
    let mut at: usize = 0;
    for block in bytes.chunks(BLOCK) {
        let next = at.saturating_add(block.len());
        if !block.is_ascii() {
            out.push_str(ascii(bytes.get(span..at).unwrap_or_default()));
            for &byte in block {
                out.push(char::from(byte));
            }
            span = next;
        }
        at = next;
    }
    out.push_str(ascii(bytes.get(span..).unwrap_or_default()));
    Cow::Owned(out)
}

const fn unit(pair: [u8; 2]) -> u16 {
    u16::from_le_bytes(pair)
}

#[must_use]
pub fn utf16le_to_utf8(bytes: &[u8]) -> Option<String> {
    let (pairs, rest) = bytes.as_chunks::<2>();
    if !rest.is_empty() {
        return None;
    }
    if pairs.len() < BLOCK {
        return char::decode_utf16(pairs.iter().map(|&pair| unit(pair)))
            .collect::<Result<String, _>>()
            .ok();
    }
    let mut out = String::with_capacity(pairs.len());
    let mut cursor = pairs;
    while let Some((&pair, after)) = cursor.split_first() {
        let first = unit(pair);
        if first >= 0x80 {
            let decoded = if (0xD800..0xE000).contains(&first) {
                let (&low, after) = after.split_first()?;
                cursor = after;
                let high = u32::from(first).wrapping_sub(0xD800);
                let low = u32::from(unit(low)).wrapping_sub(0xDC00);
                if high >= 0x400 || low >= 0x400 {
                    return None;
                }
                char::from_u32(
                    high.wrapping_shl(10)
                        .wrapping_add(low)
                        .wrapping_add(0x1_0000),
                )?
            } else {
                cursor = after;
                char::from_u32(u32::from(first))?
            };
            out.push(decoded);
        } else if after.first().is_none_or(|&next| unit(next) >= 0x80) {
            out.push(char::from(pair[0]));
            cursor = after;
        } else {
            let block = cursor.get(..BLOCK).unwrap_or(cursor);
            let run = if block.iter().fold(0, |any, &pair| any | unit(pair)) < 0x80 {
                block.len()
            } else {
                block
                    .iter()
                    .position(|&pair| unit(pair) >= 0x80)
                    .unwrap_or(block.len())
            };
            let mut narrow = [0; BLOCK];
            for (slot, &[low, _]) in narrow.iter_mut().zip(block) {
                *slot = low;
            }
            out.push_str(ascii(narrow.get(..run).unwrap_or_default()));
            cursor = cursor.get(run..).unwrap_or_default();
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn latin1_maps_bytes_to_code_points() {
        assert_eq!(latin1_to_utf8(b"abc"), "abc");
        assert!(matches!(latin1_to_utf8(b"abc"), Cow::Borrowed(_)));
        assert_eq!(latin1_to_utf8(&[0x63, 0x61, 0x66, 0xE9]), "café");
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

    fn utf16(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    #[test]
    fn utf16_decodes_a_pair_across_the_block_edge() {
        let text = format!("{}😀{}", "a".repeat(BLOCK - 1), "b".repeat(BLOCK));
        assert_eq!(utf16le_to_utf8(&utf16(&text)), Some(text));
        let mut lone = utf16(&"a".repeat(BLOCK - 1));
        lone.extend_from_slice(&[0x3D, 0xD8]);
        assert_eq!(utf16le_to_utf8(&lone), None);
    }

    // Long ASCII runs with other characters between them, so whole ASCII
    // blocks, mixed blocks and pairs on block edges all come up.
    fn runs<T: std::fmt::Debug>(
        other: impl Strategy<Value = Vec<T>>,
        ascii: impl Fn(u8) -> T + Clone,
    ) -> impl Strategy<Value = Vec<T>> {
        proptest::collection::vec((0usize..80, other), 0..24).prop_map(move |segments| {
            segments
                .into_iter()
                .flat_map(|(run, extra)| {
                    b"abcdefghijklmnopqrstuvwxyz"
                        .iter()
                        .cycle()
                        .take(run)
                        .map(|&byte| ascii.clone()(byte))
                        .chain(extra)
                })
                .collect()
        })
    }

    fn units_by_char(units: &[u16]) -> Option<String> {
        char::decode_utf16(units.iter().copied())
            .collect::<Result<String, _>>()
            .ok()
    }

    proptest! {
        #[test]
        fn latin1_matches_mapping_each_byte(bytes in runs(proptest::collection::vec(0x80u8..=0xFF, 0..3), |b| b)) {
            let by_byte: String = bytes.iter().copied().map(char::from).collect();
            prop_assert_eq!(latin1_to_utf8(&bytes), by_byte);
        }

        #[test]
        fn utf16_matches_decoding_each_unit(units in runs(
            prop_oneof![
                8 => proptest::collection::vec(0x80u16..0xD800, 1..3),
                2 => proptest::collection::vec(0xE000u16..=0xFFFF, 1..3),
                4 => (0xD800u16..0xDC00, 0xDC00u16..0xE000).prop_map(|(high, low)| vec![high, low]),
                1 => (0xD800u16..0xE000).prop_map(|lone| vec![lone]),
            ],
            u16::from,
        )) {
            let bytes: Vec<u8> = units.iter().flat_map(|unit| unit.to_le_bytes()).collect();
            prop_assert_eq!(utf16le_to_utf8(&bytes), units_by_char(&units));
        }
    }
}
