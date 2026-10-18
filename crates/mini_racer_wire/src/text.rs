// Returned HTML is mostly ASCII, which is copied in blocks; only blocks
// holding another byte are converted a byte at a time.
const LATIN1_BLOCK: usize = 32;
// Lengths are counted a block at a time in a u8, which vectorizes; a UTF-16
// unit adds at most 3.
const COUNT_BLOCK: usize = 64;
const _: () = assert!(COUNT_BLOCK * 3 <= u8::MAX as usize);

/// The length in UTF-8 of Latin-1 `bytes`.
#[must_use]
pub fn latin1_utf8_len(bytes: &[u8]) -> usize {
    let wide = bytes.chunks(COUNT_BLOCK).fold(0_usize, |wide, block| {
        let count = block.iter().fold(0_u8, |count, &byte| {
            count.wrapping_add(u8::from(!byte.is_ascii()))
        });
        wide.saturating_add(usize::from(count))
    });
    bytes.len().saturating_add(wide)
}

// Writes `c` at `at` and returns where it ends, or `None` when it does not fit.
fn put(out: &mut [u8], at: usize, c: char) -> Option<usize> {
    let end = at.checked_add(c.len_utf8())?;
    c.encode_utf8(out.get_mut(at..end)?);
    Some(end)
}

/// Writes the UTF-8 of Latin-1 `bytes` at the start of `out`, up to the last
/// character that fits, and returns how many bytes it wrote: all
/// `latin1_utf8_len(bytes)` of them when `out` is that long.
pub fn latin1_to_utf8_into(bytes: &[u8], out: &mut [u8]) -> usize {
    let mut at: usize = 0;
    for block in bytes.chunks(LATIN1_BLOCK) {
        let end = at.saturating_add(block.len());
        if block.is_ascii()
            && let Some(slots) = out.get_mut(at..end)
        {
            slots.copy_from_slice(block);
            at = end;
            continue;
        }
        for &byte in block {
            match put(out, at, char::from(byte)) {
                Some(next) => at = next,
                None => return at,
            }
        }
    }
    at
}

#[must_use]
#[expect(clippy::expect_used, reason = "only whole characters are written")]
pub fn latin1_to_utf8(bytes: &[u8]) -> String {
    let mut out = vec![0; latin1_utf8_len(bytes)];
    let written = latin1_to_utf8_into(bytes, &mut out);
    out.truncate(written);
    String::from_utf8(out).expect("Latin-1 transcodes to UTF-8")
}

fn utf16le_chars(bytes: &[u8]) -> Option<impl Iterator<Item = Result<char, ()>>> {
    let (pairs, rest) = bytes.as_chunks::<2>();
    let units = pairs.iter().copied().map(u16::from_le_bytes);
    rest.is_empty()
        .then(|| char::decode_utf16(units).map(|c| c.map_err(|_| ())))
}

/// The length in UTF-8 of UTF-16LE `bytes`, or `None` when they hold an odd
/// byte or a lone surrogate.
#[must_use]
pub fn utf16le_utf8_len(bytes: &[u8]) -> Option<usize> {
    let (pairs, rest) = bytes.as_chunks::<2>();
    if !rest.is_empty() {
        return None;
    }
    // A unit takes one byte, one more from U+0080 and one more from U+0800,
    // which counts the two units of a surrogate pair as six bytes for its four.
    let mut len: usize = 0;
    let mut surrogates = false;
    for block in pairs.chunks(COUNT_BLOCK) {
        let mut count: u8 = 0;
        for &pair in block {
            let unit = u16::from_le_bytes(pair);
            count = count
                .wrapping_add(1)
                .wrapping_add(u8::from(unit >= 0x80))
                .wrapping_add(u8::from(unit >= 0x800));
            surrogates |= (0xD800..0xE000).contains(&unit);
        }
        len = len.saturating_add(usize::from(count));
    }
    if !surrogates {
        return Some(len);
    }
    let mut surrogate_pairs: usize = 0;
    let mut units = pairs.iter().copied().map(u16::from_le_bytes);
    while let Some(unit) = units.next() {
        if (0xD800..0xDC00).contains(&unit) {
            units.next().filter(|low| (0xDC00..0xE000).contains(low))?;
            surrogate_pairs = surrogate_pairs.saturating_add(1);
        } else if (0xDC00..0xE000).contains(&unit) {
            return None;
        }
    }
    Some(len.saturating_sub(surrogate_pairs.saturating_mul(2)))
}

/// Writes the UTF-8 of UTF-16LE `bytes` at the start of `out` and returns how
/// many bytes it wrote.
///
/// `None` when they hold an odd byte or a lone surrogate, or when `out` lacks
/// room, which three bytes per unit never does.
pub fn utf16le_to_utf8_into(bytes: &[u8], out: &mut [u8]) -> Option<usize> {
    let mut at = 0;
    for c in utf16le_chars(bytes)? {
        at = put(out, at, c.ok()?)?;
    }
    Some(at)
}

#[must_use]
#[expect(clippy::expect_used, reason = "only whole characters are written")]
pub fn utf16le_to_utf8(bytes: &[u8]) -> Option<String> {
    let mut out = vec![0; utf16le_utf8_len(bytes)?];
    let written = utf16le_to_utf8_into(bytes, &mut out)?;
    out.truncate(written);
    Some(String::from_utf8(out).expect("UTF-16 transcodes to UTF-8"))
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
        assert_eq!(
            utf16le_utf8_len(&[0x3D, 0xD8, 0x00, 0xDE, 0x41, 0x00]),
            Some(5)
        );
        assert_eq!(utf16le_to_utf8(&[0x41]), None);
        assert_eq!(utf16le_utf8_len(&[0x41]), None);
        assert_eq!(utf16le_to_utf8(&[0x3D, 0xD8]), None);
        assert_eq!(utf16le_utf8_len(&[0x41, 0x00, 0x00, 0xDC]), None);
    }

    #[test]
    fn writers_stop_where_text_does_not_decode_or_fit() {
        let mut out = [0; 8];
        assert_eq!(
            utf16le_to_utf8_into(&[0x41, 0x00, 0x00, 0xDC], &mut out),
            None
        );
        assert_eq!(utf16le_to_utf8_into(&[0x41, 0x00, 0x42], &mut out), None);
        assert_eq!(
            utf16le_to_utf8_into(&[0x3D, 0xD8, 0x00, 0xDE], out.get_mut(..3).unwrap()),
            None
        );
        assert_eq!(
            utf16le_to_utf8_into(&[0x3D, 0xD8, 0x00, 0xDE], &mut out),
            Some(4)
        );
        let written = latin1_to_utf8_into(b"ab\xE9c", out.get_mut(..3).unwrap());
        assert_eq!(out.get(..written), Some(b"ab".as_slice()));
    }

    fn utf16le(units: &[u16]) -> Vec<u8> {
        units.iter().flat_map(|unit| unit.to_le_bytes()).collect()
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
            prop_assert_eq!(latin1_utf8_len(&bytes), by_byte.len());
            let mut out = vec![0; bytes.len().saturating_mul(2)];
            let written = latin1_to_utf8_into(&bytes, &mut out);
            prop_assert_eq!(out.get(..written), Some(by_byte.as_bytes()));
            prop_assert_eq!(latin1_to_utf8(&bytes), by_byte);
        }

        // Units from each range UTF-8 sizes differently, surrogates included,
        // so pairs, lone halves and pairs in the wrong order all come up.
        #[test]
        fn utf16_matches_std(units in proptest::collection::vec(
            prop_oneof![0u16..0x80, 0x80u16..0x800, 0x800u16..0xD800, 0xD800u16..0xE000, 0xE000u16..=0xFFFF],
            0..64,
        )) {
            let expected = String::from_utf16(&units).ok();
            prop_assert_eq!(utf16le_utf8_len(&utf16le(&units)), expected.as_ref().map(String::len));
            let mut out = vec![0; units.len().saturating_mul(3)];
            let written = utf16le_to_utf8_into(&utf16le(&units), &mut out);
            prop_assert_eq!(written.and_then(|n| out.get(..n)), expected.as_ref().map(String::as_bytes));
            prop_assert_eq!(utf16le_to_utf8(&utf16le(&units)), expected);
        }
    }
}
