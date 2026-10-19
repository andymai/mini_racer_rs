//! V8 reads the text a `Transcoder` decodes as the string it makes of that
//! text's UTF-8: the same units, stored one byte or two a unit as it stores
//! them.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers fail the test by panicking"
)]

use mini_racer_engine::{Decoded, Transcoder, process};
use mini_racer_wire::Writer;
use v8::ValueDeserializerHelper;

struct Reader;

impl v8::ValueDeserializerImpl for Reader {}

fn read<'s>(
    scope: &v8::PinScope<'s, '_>,
    write: impl FnOnce(&mut Writer<'_>),
) -> v8::Local<'s, v8::String> {
    let mut bytes = Vec::new();
    write(&mut Writer::new(&mut bytes));
    let context = scope.get_current_context();
    let deserializer = v8::ValueDeserializer::new(scope, Box::new(Reader), &bytes);
    assert_eq!(deserializer.read_header(context), Some(true));
    let value = deserializer
        .read_value(context)
        .expect("V8 reads the bytes");
    v8::Local::<v8::String>::try_from(value).expect("a string")
}

fn texts() -> Vec<String> {
    let pad = "the quick brown fox ";
    let mut texts: Vec<String> = [
        "caf\u{e9} na\u{ef}ve Z\u{fc}rich cr\u{e8}me \u{ff}",
        "\u{6771}\u{4eac} \u{5927}\u{962a} \u{540d}\u{53e4}\u{5c4b}",
        "\u{1f600} party \u{1f389} fun \u{1f680} go \u{10ffff}",
        "\u{feff}\u{fffd}\u{ffff}\u{e000}\u{d7ff}\u{800}\u{7ff}\u{100}",
        "0123456789abcd\u{e9}",
        "0123456789abcd\u{100}",
        "\u{e9}\u{100}",
    ]
    .iter()
    .flat_map(|text| {
        [
            (*text).to_owned(),
            format!("{pad}{text}"),
            format!("{text}{pad}"),
        ]
    })
    .collect();
    for (start, end) in [
        (0x80, 0x100),
        (0x100, 0x800),
        (0x800, 0xD800),
        (0xE000, 0x11_0000),
    ] {
        for first in (start..end).step_by(4099) {
            texts.push(
                (first..end.min(first + 40))
                    .filter_map(char::from_u32)
                    .collect(),
            );
        }
    }
    let latin1: String = (0x80..0x100).filter_map(char::from_u32).collect();
    texts.push(latin1.repeat(9));
    texts.push(format!("{}\u{1f600}", latin1.repeat(9)));
    texts
}

#[test]
fn decoded_text_reads_as_v8_reads_its_utf8() {
    process::ensure_initialized();
    let mut isolate = v8::Isolate::new(v8::CreateParams::default());
    let context = {
        v8::scope!(let scope, &mut isolate);
        let context = v8::Context::new(scope, v8::ContextOptions::default());
        v8::Global::new(scope, context)
    };
    v8::scope_with_context!(let scope, &mut isolate, &context);
    let mut transcoder = Transcoder::default();
    let mut decoded = 0;
    for text in texts() {
        let from_utf8 = read(scope, |w| w.utf8(text.as_bytes()));
        let from_units = match transcoder.decode(text.as_bytes()) {
            Some(Decoded::OneByte(bytes)) => read(scope, |w| w.latin1(bytes)),
            Some(Decoded::TwoByte(units)) => read(scope, |w| w.utf16_aligned(units)),
            None => continue,
        };
        decoded += 1;
        assert!(from_units.strict_equals(from_utf8.into()), "{text:?}");
        assert_eq!(from_units.is_onebyte(), from_utf8.is_onebyte(), "{text:?}");
    }
    assert!(decoded > 100, "only {decoded} decoded");
}
