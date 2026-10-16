#![no_main]

use libfuzzer_sys::fuzz_target;
use mini_racer_wire::text::{latin1_to_utf8, utf16le_to_utf8};

// Written without char::decode_utf16 so it checks the transcoder rather than
// restating it.
fn well_formed(units: &[u16]) -> bool {
    let mut rest = units;
    while let [unit, tail @ ..] = rest {
        rest = match (unit, tail) {
            (0xD800..=0xDBFF, [0xDC00..=0xDFFF, after @ ..]) => after,
            (0xD800..=0xDFFF, _) => return false,
            _ => tail,
        };
    }
    true
}

fuzz_target!(|data: &[u8]| {
    let latin1: Vec<u8> = latin1_to_utf8(data)
        .chars()
        .map(|c| u8::try_from(u32::from(c)).expect("Latin-1 maps bytes to U+0000..=U+00FF"))
        .collect();
    assert_eq!(latin1, data);

    let (pairs, rest) = data.as_chunks::<2>();
    let units: Vec<u16> = pairs.iter().copied().map(u16::from_le_bytes).collect();
    let decoded = utf16le_to_utf8(data);
    assert_eq!(decoded.is_some(), rest.is_empty() && well_formed(&units));
    if let Some(text) = decoded {
        let encoded: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(encoded, data);
    }
});
