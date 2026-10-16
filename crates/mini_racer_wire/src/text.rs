// Returned HTML is mostly ASCII, which is copied in spans; only blocks holding
// another byte are converted a character at a time.
const LATIN1_BLOCK: usize = 32;
// Each block's count of non-ASCII bytes is summed in a u8.
const _: () = assert!(LATIN1_BLOCK <= u8::MAX as usize);

fn ascii(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).expect("ASCII is UTF-8")
}

#[must_use]
pub fn latin1_to_utf8(bytes: &[u8]) -> String {
    if bytes.len() < LATIN1_BLOCK {
        return bytes.iter().copied().map(char::from).collect();
    }
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

#[must_use]
pub fn utf16le_to_utf8(bytes: &[u8]) -> Option<String> {
    let (pairs, rest) = bytes.as_chunks::<2>();
    if !rest.is_empty() {
        return None;
    }
    let units = pairs.iter().copied().map(u16::from_le_bytes);
    char::decode_utf16(units)
        .collect::<Result<String, _>>()
        .ok()
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

    proptest! {
        // ASCII runs of every length with other bytes between them, so whole
        // ASCII blocks, mixed blocks and spans across block edges all come up.
        #[test]
        fn latin1_matches_mapping_each_byte(segments in proptest::collection::vec(
            (
                proptest::collection::vec(0u8..0x80, 0..80),
                proptest::collection::vec(0x80u8..=0xFF, 0..3),
            ),
            0..24,
        )) {
            let bytes: Vec<u8> = segments.into_iter().flat_map(|(run, wide)| run.into_iter().chain(wide)).collect();
            let by_byte: String = bytes.iter().copied().map(char::from).collect();
            prop_assert_eq!(latin1_to_utf8(&bytes), by_byte);
        }
    }
}
