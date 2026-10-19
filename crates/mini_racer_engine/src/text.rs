use crate::sys;

// V8 decodes UTF-8 this short about as fast as a conversion starts, as
// rusty_v8's own `String::new_from_utf8` finds.
const SHORT: usize = 16;

/// The characters of valid UTF-8 as V8 stores them once it has decoded it:
/// one byte each when every one is at most U+00FF, UTF-16 units otherwise.
pub enum Decoded<'a> {
    OneByte(&'a [u8]),
    TwoByte(&'a [u16]),
}

/// Decodes UTF-8 ahead of V8, which copies one-byte and two-byte text where
/// it would decode UTF-8 a byte at a time, into buffers kept for the next
/// text.
#[derive(Default)]
pub struct Transcoder {
    one_byte: Vec<u8>,
    two_byte: Vec<u16>,
}

impl Transcoder {
    /// `text` as V8 would decode it; `None` when it is short enough for V8 to
    /// decode as fast, or is not valid UTF-8, whose replacement characters
    /// are V8's to choose.
    #[expect(
        clippy::inline_always,
        reason = "the encoder calls it for each non-ASCII value; outlined, it slowed short ones 12%"
    )]
    #[inline(always)]
    pub fn decode(&mut self, text: &[u8]) -> Option<Decoded<'_>> {
        if text.len() < SHORT {
            return None;
        }
        self.decode_long(text)
    }

    fn decode_long(&mut self, text: &[u8]) -> Option<Decoded<'_>> {
        grow(&mut self.one_byte, text.len());
        if let Some(count) = sys::utf8_to_latin1(text, &mut self.one_byte) {
            return self.one_byte.get(..count).map(Decoded::OneByte);
        }
        grow(&mut self.two_byte, text.len());
        let count = sys::utf8_to_utf16(text, &mut self.two_byte)?;
        self.two_byte.get(..count).map(Decoded::TwoByte)
    }
}

// Each has room for as many units as the text has bytes, which simdutf needs,
// and is made afresh as it grows: the allocator hands out a long one's zeroed
// pages untouched, where zeroing it in place would commit room for three
// times the units a CJK text decodes to.
fn grow<T: Copy + Default>(buffer: &mut Vec<T>, len: usize) {
    if buffer.len() < len {
        *buffer = vec![T::default(); len];
    }
}

#[cfg(test)]
mod tests {
    use super::{Decoded, SHORT, Transcoder};

    // What V8 makes of valid UTF-8, worked out from its characters.
    fn expected(text: &str) -> (Option<Vec<u8>>, Vec<u16>) {
        let latin1 = text.chars().map(|c| u8::try_from(c).ok()).collect();
        (latin1, text.encode_utf16().collect())
    }

    fn check(transcoder: &mut Transcoder, text: &str) {
        let (latin1, utf16) = expected(text);
        match (transcoder.decode(text.as_bytes()), latin1) {
            (Some(Decoded::OneByte(bytes)), Some(latin1)) => assert_eq!(bytes, latin1, "{text:?}"),
            (Some(Decoded::TwoByte(units)), None) => assert_eq!(units, utf16, "{text:?}"),
            (None, _) => assert!(text.len() < SHORT, "{text:?} left to V8"),
            _ => panic!("{text:?} decoded in the wrong form"),
        }
    }

    #[test]
    fn valid_text_decodes_to_the_units_v8_stores() {
        let mut transcoder = Transcoder::default();
        let pad = "the quick brown fox ";
        for text in [
            "caf\u{e9}",
            "caf\u{e9} na\u{ef}ve Z\u{fc}rich cr\u{e8}me \u{ff}",
            "\u{6771}\u{4eac} \u{5927}\u{962a} \u{540d}\u{53e4}\u{5c4b}",
            "\u{1f600} party \u{1f389} fun \u{1f680} go \u{10ffff}",
            "0123456789abcde\u{e9}",
            "0123456789abc\u{e9}",
            "0123456789abcd\u{e9}",
        ] {
            check(&mut transcoder, text);
        }
        let ranges = [
            0x80..0x100,
            0x100..0x800,
            0x800..0xD800,
            0xE000..0x1_0000,
            0x1_0000..0x11_0000,
        ];
        for range in ranges {
            for start in range.clone().step_by(61) {
                let text: String = (start..range.end.min(start + 23))
                    .filter_map(char::from_u32)
                    .collect();
                check(&mut transcoder, &format!("{pad}{text}"));
                check(&mut transcoder, &format!("{text}{pad}"));
            }
        }
        let latin1: String = (0x80..0x100).filter_map(char::from_u32).collect();
        check(&mut transcoder, &latin1.repeat(9));
        check(&mut transcoder, &format!("{}\u{100}", latin1.repeat(9)));
    }

    #[test]
    fn invalid_or_short_text_is_left_to_v8() {
        let mut transcoder = Transcoder::default();
        let pad = b"the quick brown fox ";
        for invalid in [
            &b"\xff"[..],
            b"\x80",
            b"\xc0\x80",
            b"\xc3",
            b"\xed\xa0\x80",
            b"\xed\xbf\xbf",
            b"\xf4\x90\x80\x80",
            b"\xf5\x80\x80\x80",
            b"\xe0\x80\xaf",
            b"\xf0\x9f\x98",
            b"\xe6\x9d",
        ] {
            for text in [[pad, invalid].concat(), [invalid, pad].concat()] {
                assert!(transcoder.decode(&text).is_none(), "{text:x?}");
            }
        }
        assert!(transcoder.decode("caf\u{e9}".as_bytes()).is_none());
        assert!(transcoder.decode(b"").is_none());
    }
}
