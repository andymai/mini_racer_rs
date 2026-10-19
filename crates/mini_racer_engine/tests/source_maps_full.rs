//! A process maps at most `MAX_MAPS` bundles; in a file of its own, as the
//! registry is the process's.

#![allow(
    clippy::expect_used,
    reason = "test helpers fail the test by panicking"
)]

use mini_racer_engine::SourceMapError;
use mini_racer_engine::process::fork;
use mini_racer_engine::process::source_maps::MAX_MAPS;

const MAP: &str = r#"{"version":3,"sources":["a.js"],"mappings":"AAAA"}"#;

#[test]
fn refuses_a_bundle_past_the_last_and_replaces_a_known_one() {
    let maps = fork::current().source_maps();
    let bundle = |name: &str| format!("var {name} = 1;\n{}\n", "//".repeat(512));
    for n in 0..MAX_MAPS {
        maps.register(&bundle(&format!("bundle{n}")), MAP.as_bytes())
            .expect("room for it");
    }
    assert!(matches!(
        maps.register(&bundle("one_more"), MAP.as_bytes()),
        Err(SourceMapError::Full(MAX_MAPS))
    ));
    // The same bundle again takes its own place, whatever its map.
    let other = r#"{"version":3,"sources":["b.js"],"mappings":"AAAA"}"#;
    maps.register(&bundle("bundle0"), other.as_bytes())
        .expect("its own place");
    assert!(matches!(
        maps.register(&bundle("one_more"), MAP.as_bytes()),
        Err(SourceMapError::Full(MAX_MAPS))
    ));
}
