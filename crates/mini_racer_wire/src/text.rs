// Returned HTML is mostly ASCII, so input is checked for ASCII a block at a
// time and ASCII is copied in bulk; only blocks holding anything else are
// converted a character at a time.
const LATIN1_BLOCK: usize = 32;
// Each block's count of non-ASCII bytes is summed in a u8.
const _: () = assert!(LATIN1_BLOCK <= u8::MAX as usize);
const UTF16_WINDOW: usize = 8;
const NARROW_BYTES: usize = 64;

fn ascii(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).expect("ASCII is UTF-8")
}

#[must_use]
pub fn latin1_to_utf8(bytes: &[u8]) -> String {
    let wide: usize = bytes
        .chunks(LATIN1_BLOCK)
        .map(|block| {
            usize::from(
                block
                    .iter()
                    .map(|&byte| u8::from(!byte.is_ascii()))
                    .sum::<u8>(),
            )
        })
        .sum();
    let mut out = String::with_capacity(bytes.len().saturating_add(wide));
    let mut span = 0;
    let mut at: usize = 0;
    for block in bytes.chunks(LATIN1_BLOCK) {
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
    out
}

fn units(pairs: &[[u8; 2]]) -> impl Iterator<Item = u16> {
    pairs.iter().map(|&pair| u16::from_le_bytes(pair))
}

#[must_use]
pub fn utf16le_to_utf8(bytes: &[u8]) -> Option<String> {
    let (pairs, rest) = bytes.as_chunks::<2>();
    if !rest.is_empty() {
        return None;
    }
    if pairs.len() < UTF16_WINDOW {
        return char::decode_utf16(units(pairs))
            .collect::<Result<_, _>>()
            .ok();
    }
    let mut out = String::with_capacity(pairs.len());
    let mut narrow = [0; NARROW_BYTES];
    let mut filled: usize = 0;
    let mut cursor = pairs;
    while let Some(&first) = cursor.first() {
        let window = cursor.get(..UTF16_WINDOW).unwrap_or(cursor);
        let taken = if u16::from_le_bytes(first) < 0x80
            && units(window).fold(0, |any, unit| any | unit) < 0x80
        {
            if filled.saturating_add(window.len()) > NARROW_BYTES {
                out.push_str(ascii(narrow.get(..filled).unwrap_or_default()));
                filled = 0;
            }
            let slots = narrow.get_mut(filled..).unwrap_or_default();
            for (slot, &[low, _]) in slots.iter_mut().zip(window) {
                *slot = low;
            }
            filled = filled.saturating_add(window.len());
            window.len()
        } else {
            if filled > 0 {
                out.push_str(ascii(narrow.get(..filled).unwrap_or_default()));
                filled = 0;
            }
            // Each following window that starts outside ASCII joins the span,
            // so a surrogate pair is never split at the span's end.
            let mut end = window.len();
            while let Some(&next) = cursor.get(end)
                && u16::from_le_bytes(next) >= 0x80
            {
                end = end.saturating_add(UTF16_WINDOW).min(cursor.len());
            }
            for decoded in char::decode_utf16(units(cursor.get(..end).unwrap_or_default())) {
                out.push(decoded.ok()?);
            }
            end
        };
        cursor = cursor.get(taken..).unwrap_or_default();
    }
    out.push_str(ascii(narrow.get(..filled).unwrap_or_default()));
    Some(out)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn latin1_maps_bytes_to_code_points() {
        assert_eq!(latin1_to_utf8(b"abc"), "abc");
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
    fn utf16_decodes_a_pair_across_the_window_edge() {
        let text = format!(
            "{}é{}😀{}",
            "a".repeat(UTF16_WINDOW - 2),
            "b".repeat(UTF16_WINDOW),
            "c".repeat(UTF16_WINDOW)
        );
        assert_eq!(
            utf16(&text).get((2 * UTF16_WINDOW - 1) * 2..2 * UTF16_WINDOW * 2),
            Some([0x3D, 0xD8].as_slice())
        );
        assert_eq!(utf16le_to_utf8(&utf16(&text)), Some(text));
        let mut lone = utf16(&format!("é{}", "a".repeat(UTF16_WINDOW - 2)));
        lone.extend_from_slice(&[0x3D, 0xD8]);
        lone.extend(utf16(&"b".repeat(UTF16_WINDOW)));
        assert_eq!(utf16le_to_utf8(&lone), None);
    }

    // ASCII runs of every length, with other characters between them, so
    // whole ASCII blocks, mixed blocks and pairs on block edges all come up.
    fn runs<T: std::fmt::Debug>(
        other: impl Strategy<Value = Vec<T>>,
        ascii: impl Fn(u8) -> T + Clone,
    ) -> impl Strategy<Value = Vec<T>> {
        proptest::collection::vec((proptest::collection::vec(0u8..0x80, 0..80), other), 0..24)
            .prop_map(move |segments| {
                segments
                    .into_iter()
                    .flat_map(|(run, extra)| run.into_iter().map(ascii.clone()).chain(extra))
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
                16 => proptest::collection::vec(0x80u16..0xD800, 1..3),
                4 => proptest::collection::vec(0xE000u16..=0xFFFF, 1..3),
                8 => (0xD800u16..0xDC00, 0xDC00u16..0xE000).prop_map(|(high, low)| vec![high, low]),
                1 => (0xD800u16..0xE000).prop_map(|lone| vec![lone]),
            ],
            u16::from,
        )) {
            let bytes: Vec<u8> = units.iter().flat_map(|unit| unit.to_le_bytes()).collect();
            prop_assert_eq!(utf16le_to_utf8(&bytes), units_by_char(&units));
        }
    }
}
