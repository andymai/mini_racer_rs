#![no_main]

use std::borrow::Cow;

use libfuzzer_sys::fuzz_target;
use mini_racer_sourcemap::{Frame, Match, Placement, SourceMap, rewrite};

const MAPPINGS: &[u8; 66] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/,;";

fn frames(bytes: &[u8]) -> Vec<Frame> {
    bytes
        .chunks_exact(5)
        .map(|chunk| Frame {
            script_id: i32::from(chunk[0] % 3),
            line: u32::from(u16::from_le_bytes([chunk[1], chunk[2]])),
            column: u32::from(u16::from_le_bytes([chunk[3], chunk[4]])),
        })
        .collect()
}

// The first byte picks a map read as raw JSON or one built around mappings
// drawn from their own alphabet, which reaches the VLQ and lookup paths far
// sooner; the rest splits at 0xFF, which UTF-8 never holds, into the bundle,
// the map or its mappings, the stack text and the frames.
fuzz_target!(|data: &[u8]| {
    let Some((&selector, rest)) = data.split_first() else {
        return;
    };
    let mut fields = rest.splitn(4, |&byte| byte == 0xFF);
    let bundle = String::from_utf8_lossy(fields.next().unwrap_or_default());
    let raw = fields.next().unwrap_or_default();
    let text = String::from_utf8_lossy(fields.next().unwrap_or_default());
    let frames = frames(fields.next().unwrap_or_default());

    assert!(matches!(rewrite(&text, &frames, &[]), Cow::Borrowed(same) if same == text));

    let built = selector & 1 == 0;
    let json = if built {
        let mappings: String = raw
            .iter()
            .map(|&byte| char::from(MAPPINGS[usize::from(byte) % MAPPINGS.len()]))
            .collect();
        format!(
            r#"{{"version":3,"sources":["a.js",null,"c.js"],"sourceRoot":"r","names":["n"],"mappings":"{mappings}"}}"#
        )
        .into_bytes()
    } else {
        raw.to_vec()
    };
    let Ok(map) = SourceMap::new(&bundle, &json) else {
        return;
    };
    let placement = map
        .find(text.as_bytes())
        .or_else(|| map.find(bundle.as_bytes()))
        .unwrap_or(Placement {
            line: u32::from(selector >> 4),
            column: u32::from((selector >> 1) & 7),
        });
    for frame in &frames {
        let _ = map.original(placement, frame.line, frame.column);
    }
    let matches = [Match {
        script_id: 1,
        name: "<eval>",
        map: &map,
        placement,
    }];
    let out = rewrite(&text, &frames, &matches);
    // Only locations change, and a built map's sources hold no line break.
    if built {
        assert_eq!(out.matches('\n').count(), text.matches('\n').count());
    }
});
