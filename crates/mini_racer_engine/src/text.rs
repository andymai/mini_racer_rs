use crate::sys;

// V8 decodes shorter UTF-8 faster than a conversion starts: converting all of
// it made arrays of short BMP and emoji strings about a tenth slower to send.
const SHORT: usize = 16;
// UTF-8 longer than this goes to V8 as mini_racer sends it. Decoded here, it
// takes a buffer of two bytes per byte and a request up to twice its length, both
// under the GVL: a 100 MB String with one character past Latin-1 would peak
// near 500 MB where its UTF-8 takes 200 MB. Below this, that extra memory
// stays within 24 MiB, and the values measured, up to 1 MB, all gain.
const LONG: usize = 8 << 20;
const WIDE_LEAD: u8 = 0xC4;

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
    /// decode as fast, past `LONG` bytes, or not valid UTF-8, whose
    /// replacement characters are V8's to choose.
    #[expect(
        clippy::inline_always,
        reason = "the encoder calls it for each non-ASCII value; outlined, it slowed short ones 12%"
    )]
    #[inline(always)]
    pub fn decode(&mut self, text: &[u8]) -> Option<Decoded<'_>> {
        if text.len() < SHORT || text.len() > LONG {
            return None;
        }
        self.decode_long(text)
    }

    // One conversion each: in valid UTF-8 a byte from 0xC4 up starts a
    // character past U+00FF, and text without one that does not convert to
    // Latin-1 is not valid.
    fn decode_long(&mut self, text: &[u8]) -> Option<Decoded<'_>> {
        if text.iter().fold(0, |most, &byte| most.max(byte)) < WIDE_LEAD {
            grow(&mut self.one_byte, text.len());
            let count = sys::utf8_to_latin1(text, &mut self.one_byte)?;
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
    use super::{Decoded, LONG, SHORT, Transcoder};

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

    // Values of a megabyte, the longest measured, are decoded; past `LONG`
    // text goes as UTF-8.
    #[test]
    fn text_up_to_long_is_decoded_and_longer_is_left_to_v8() {
        let mut transcoder = Transcoder::default();
        let megabyte = "caf\u{e9} ".repeat((1 << 20) / 6);
        assert!(matches!(
            transcoder.decode(megabyte.as_bytes()),
            Some(Decoded::OneByte(_))
        ));
        let megabyte = "\u{1f600} go ".repeat((1 << 20) / 8);
        assert!(matches!(
            transcoder.decode(megabyte.as_bytes()),
            Some(Decoded::TwoByte(_))
        ));
        let wide = "\u{6771}";
        let longest = format!("{wide}{}", "a".repeat(LONG - wide.len()));
        assert_eq!(longest.len(), LONG);
        match transcoder.decode(longest.as_bytes()) {
            Some(Decoded::TwoByte(units)) => assert_eq!(units.len(), LONG - 2),
            _ => panic!("text of `LONG` bytes is decoded"),
        }
        assert!(
            transcoder
                .decode(format!("{longest}a").as_bytes())
                .is_none()
        );
        let latin1 = "\u{e9}".repeat(LONG / 2 + 1);
        assert!(transcoder.decode(latin1.as_bytes()).is_none());
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
