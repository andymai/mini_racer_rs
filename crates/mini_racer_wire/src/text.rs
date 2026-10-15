#[must_use]
pub fn latin1_to_utf8(bytes: &[u8]) -> String {
    bytes.iter().copied().map(char::from).collect()
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
}
