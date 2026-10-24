use std::sync::Arc;

use crate::process::sources::Units;
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
// simdutf needs room for a unit per byte it decodes, more than a source's
// units, sized exactly, have once it holds a character of two bytes or more,
// so each run is decoded into a buffer small enough to stay in cache and
// copied in.
const RUN: usize = 64 << 10;
const CONTINUATION_BITS: u8 = 0xC0;
const CONTINUATION: u8 = 0x80;

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

    fn decode_long(&mut self, text: &[u8]) -> Option<Decoded<'_>> {
        if one_byte(text) {
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

// In valid UTF-8 a byte from 0xC4 up starts a character past U+00FF, and text
// without one that does not convert to Latin-1 is not valid, so one scan picks
// the one conversion to run.
fn one_byte(text: &[u8]) -> bool {
    text.iter().fold(0, |most, &byte| most.max(byte)) < WIDE_LEAD
}

/// Valid UTF-8 of any length decoded as `Transcoder::decode` decodes it, into
/// one shared allocation of exactly its units; `None` when it is not valid.
pub fn units(text: &[u8]) -> Option<Units> {
    if one_byte(text) {
        let len = v8::simdutf::latin1_length_from_utf8(text);
        return filled(text, len, sys::utf8_to_latin1).map(Units::OneByte);
    }
    let len = v8::simdutf::utf16_length_from_utf8(text);
    filled(text, len, sys::utf8_to_utf16).map(Units::TwoByte)
}

// `len` is simdutf's count of the units, exact only for valid UTF-8, which
// never decodes to more units than it has bytes. The units are written once,
// in place: decoded whole into a buffer and then copied into an `Arc`, the
// 88 MB of units of a 44 MB bundle took 104 ms against 37.
fn filled<T: Copy + Default>(
    text: &[u8],
    len: usize,
    decode: fn(&[u8], &mut [T]) -> Option<usize>,
) -> Option<Arc<[T]>> {
    if len > text.len() {
        return None;
    }
    let mut units = sys::Filling::new(len);
    let mut run = vec![T::default(); RUN.min(text.len())];
    let mut rest = text;
    while !rest.is_empty() {
        let (head, tail) = rest.split_at(run_end(rest));
        let count = decode(head, &mut run)?;
        units.append(run.get(..count)?)?;
        rest = tail;
    }
    units.filled()
}

// Runs end where a character starts, so valid UTF-8 splits into runs each
// valid alone, and invalid UTF-8 leaves at least one run invalid. The search
// back stops after three continuation bytes: valid UTF-8 has no fourth, and
// the run after, opening with one, is refused.
fn run_end(text: &[u8]) -> usize {
    if text.len() <= RUN {
        return text.len();
    }
    let mut end = RUN;
    while end > RUN - 3
        && text
            .get(end)
            .is_some_and(|&byte| byte & CONTINUATION_BITS == CONTINUATION)
    {
        end -= 1;
    }
    end
}

#[cfg(test)]
mod tests {
    use super::{Decoded, LONG, RUN, SHORT, Transcoder, units};
    use crate::sys;

    const INVALID: [&[u8]; 14] = [
        b"\xff",
        b"\x80",
        b"\xc0\x80",
        b"\xc1\xbf",
        b"\xc3",
        b"\xc2a",
        b"\xed\xa0\x80",
        b"\xed\xbf\xbf",
        b"\xf4\x90\x80\x80",
        b"\xf5\x80\x80\x80",
        b"\xe0\x80\xaf",
        b"\xf0\x9f\x98",
        b"\xe6\x9d",
        b"\xf0\x9f\x98\x80\x80",
    ];

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
        for invalid in INVALID {
            for text in [[pad, invalid].concat(), [invalid, pad].concat()] {
                assert!(transcoder.decode(&text).is_none(), "{text:x?}");
            }
        }
        assert!(transcoder.decode("caf\u{e9}".as_bytes()).is_none());
        assert!(transcoder.decode(b"").is_none());
    }

    const WIDE: [&str; 11] = [
        "\u{e9}",
        "\u{ff}",
        "\u{100}",
        "\u{7ff}",
        "\u{800}",
        "\u{feff}",
        "\u{fffd}",
        "\u{ffff}",
        "\u{10000}",
        "\u{1f600}",
        "\u{10ffff}",
    ];

    // Each width of character starting on every byte up to the end of a run
    // and just past it, runs of one width shifted so they end inside one, and
    // widths mixed, besides ASCII of a run, a byte more and two runs.
    fn across_runs() -> Vec<String> {
        let mut texts: Vec<String> = [RUN, RUN + 1, 2 * RUN].map(|len| "a".repeat(len)).into();
        for wide in WIDE {
            for before in 0..=4 {
                texts.push(format!("{}{wide}bcd", "a".repeat(RUN - before)));
            }
            for shift in 0..4 {
                let count = 2 * RUN / wide.len() + 1;
                texts.push(format!("{}{}", "a".repeat(shift), wide.repeat(count)));
            }
        }
        let mut mixed = String::new();
        for wide in WIDE.iter().cycle() {
            if mixed.len() > 3 * RUN {
                break;
            }
            mixed.push('a');
            mixed.push_str(wide);
        }
        texts.push(mixed);
        texts
    }

    // Invalid UTF-8 just before the end of a run, across it and after it,
    // ending the text or followed by Latin-1 or wider text, so either
    // conversion meets it; text simdutf counts more units than bytes in; and
    // a fourth continuation byte ending a run.
    fn invalid_across_runs() -> Vec<Vec<u8>> {
        let mut texts = Vec::new();
        for invalid in INVALID {
            for at in [0, RUN - 3, RUN - 2, RUN - 1, RUN, RUN + 1] {
                for after in ["", "caf\u{e9} ", "caf\u{e9} \u{1f600} "] {
                    let mut text = vec![b'a'; at];
                    text.extend_from_slice(invalid);
                    text.extend_from_slice(after.repeat(RUN / 8).as_bytes());
                    texts.push(text);
                }
            }
        }
        texts.push(vec![0xF0; 2 * RUN]);
        let mut fourth = vec![b'a'; RUN - 4];
        fourth.extend_from_slice(b"\xf0\x9f\x98\x80\x80");
        fourth.extend_from_slice("\u{1f600}".repeat(RUN).as_bytes());
        texts.push(fourth);
        texts
    }

    // A shared source reads as V8 reads the UTF-8 itself, which a context
    // keeping its own copy hands `String::NewFromUtf8`: the same characters,
    // stored one byte a character or two as V8 stores them.
    #[test]
    fn units_read_as_v8_reads_their_utf8() {
        crate::process::ensure_initialized();
        let mut isolate = v8::Isolate::new(v8::CreateParams::default());
        let context = {
            v8::scope!(let scope, &mut isolate);
            let context = v8::Context::new(scope, v8::ContextOptions::default());
            v8::Global::new(scope, context)
        };
        v8::scope_with_context!(let scope, &mut isolate, &context);
        for (at, text) in across_runs().iter().enumerate() {
            let units = units(text.as_bytes()).unwrap_or_else(|| panic!("text {at} refused"));
            let shared = sys::external_string(scope, &units).expect("a string");
            let own = v8::String::new_from_utf8(scope, text.as_bytes(), v8::NewStringType::Normal)
                .expect("a string");
            assert!(shared.strict_equals(own.into()), "text {at}");
            assert_eq!(shared.is_onebyte(), own.is_onebyte(), "text {at}");
        }
        for (at, text) in invalid_across_runs().iter().enumerate() {
            assert!(units(text).is_none(), "invalid text {at}");
        }
    }
}
