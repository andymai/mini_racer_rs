//! `mini_racer` stands a two-byte string in for a function result, and its
//! Ruby side turns exactly that string into a `MiniRacer::JavaScriptFunction`.

/// U+0BFF, then `JavaScriptFunction`.
pub const JAVASCRIPT_FUNCTION: [u16; 19] = [
    0x0BFF, 0x4A, 0x61, 0x76, 0x61, 0x53, 0x63, 0x72, 0x69, 0x70, 0x74, 0x46, 0x75, 0x6E, 0x63,
    0x74, 0x69, 0x6F, 0x6E,
];

/// Whether UTF-16LE `bytes` are the marker and nothing else.
#[must_use]
pub fn is_javascript_function(bytes: &[u8]) -> bool {
    let (pairs, rest) = bytes.as_chunks::<2>();
    rest.is_empty()
        && pairs.len() == JAVASCRIPT_FUNCTION.len()
        && pairs
            .iter()
            .zip(JAVASCRIPT_FUNCTION)
            .all(|(pair, unit)| *pair == unit.to_le_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16le(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    #[test]
    fn only_the_whole_marker_matches() {
        assert!(is_javascript_function(&utf16le(
            "\u{bff}JavaScriptFunction"
        )));
        assert!(!is_javascript_function(&utf16le(
            "\u{bff}JavaScriptFunctio"
        )));
        assert!(!is_javascript_function(&utf16le(
            "\u{bff}JavaScriptFunctions"
        )));
        assert!(!is_javascript_function(&utf16le("JavaScriptFunction")));
    }
}
