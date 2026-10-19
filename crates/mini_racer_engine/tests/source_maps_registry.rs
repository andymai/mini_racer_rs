//! Which registered map an eval matches, and which an error reads, in a file
//! of its own as the registry is the process's and holds 8 bundles.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers fail the test by panicking"
)]

mod source_map_fixture;

use mini_racer_engine::JsText;
use mini_racer_engine::process::fork;
use source_map_fixture::{Parking, bundle, eval, map_with_root, register, runtime_error, spawn};

#[test]
fn refuses_a_bundle_shorter_than_1_kib() {
    let maps = fork::current().source_maps();
    let short = "x".repeat(1023);
    let map = r#"{"version":3,"sources":["a.js"],"mappings":"AAAA"}"#;
    let error = maps.register(&short, map.as_bytes()).err();
    assert!(
        error
            .as_ref()
            .is_some_and(|error| error.to_string().contains("shorter than 1024 bytes")),
        "{error:?}"
    );
    assert!(maps.register(&"y".repeat(1024), map.as_bytes()).is_ok());
}

// A bundle registered first and held inside a longer one gives way to the
// longer, whose map covers the whole source.
#[test]
fn prefers_the_longest_bundle_a_source_holds() {
    let inner = bundle("G");
    let outer = format!("{inner}function extraG() {{ return renderG(); }}\n");
    let maps = fork::current().source_maps();
    maps.register(&inner, map_with_root(&inner, "webpack://inner/").as_bytes())
        .expect("the inner bundle");
    maps.register(&outer, map_with_root(&outer, "webpack://outer/").as_bytes())
        .expect("the outer bundle");
    let context = spawn();
    eval(&context, "<eval>", &outer).expect("the bundle loads");
    let message = runtime_error(eval(&context, "<eval>", "render()"));
    assert!(
        message.contains("(webpack://outer/./src/Header.jsx:11:"),
        "{message}"
    );
}

// V8's compilation cache hands a source evaluated again the script it made
// before, under the same id, which a map registered meanwhile must win.
#[test]
fn reads_the_latest_map_for_a_script_compiled_again() {
    let bundle = bundle("H");
    let maps = fork::current().source_maps();
    maps.register(
        &bundle,
        map_with_root(&bundle, "webpack://first/").as_bytes(),
    )
    .expect("the first map");
    let context = spawn();
    eval(&context, "<eval>", &bundle).expect("the bundle loads");
    let message = runtime_error(eval(&context, "<eval>", "render()"));
    assert!(message.contains("(webpack://first/"), "{message}");
    maps.register(
        &bundle,
        map_with_root(&bundle, "webpack://second/").as_bytes(),
    )
    .expect("the second map");
    eval(&context, "<eval>", &bundle).expect("the bundle loads again");
    let message = runtime_error(eval(&context, "<eval>", "render()"));
    assert!(message.contains("(webpack://second/"), "{message}");
}

// Each eval of another source holding the bundle is a script of its own; a
// context keeps the latest 16.
#[test]
fn keeps_the_latest_16_scripts_of_a_context() {
    let bundle = bundle("I");
    register(&bundle);
    let context = spawn();
    for n in 0..17 {
        eval(&context, "<eval>", &format!("/*{n}*/\n{bundle}")).expect("the bundle loads");
        if n == 0 {
            eval(&context, "<eval>", "globalThis.first = render").expect("kept");
        }
    }
    let newest = runtime_error(eval(&context, "<eval>", "render()"));
    assert!(newest.contains("(webpack://app/"), "{newest}");
    // The first script's own frames are past the latest 16; the functions
    // they call are the newest script's.
    let oldest = runtime_error(eval(&context, "<eval>", "first()"));
    assert!(oldest.contains("    at mapper (<eval>:4:"), "{oldest}");
    assert!(oldest.contains("    at renderI (<eval>:4:"), "{oldest}");
    assert!(oldest.contains("    at UeI (webpack://app/"), "{oldest}");
}

// The registry holds UTF-8 bundles, and an eval sent as Latin-1 or UTF-16
// is never searched for one.
#[test]
fn never_matches_a_latin1_or_utf16_source() {
    let bundle = bundle("J");
    register(&bundle);
    let latin1 = JsText::Latin1(bundle.as_bytes().to_vec());
    let utf16 = JsText::Utf16(bundle.encode_utf16().collect());
    for source in [latin1, utf16] {
        let context = spawn();
        context
            .eval(JsText::from("<eval>"), source, &mut Parking)
            .expect("the bundle loads");
        let message = runtime_error(eval(&context, "<eval>", "render()"));
        assert!(message.contains("    at UeJ (<eval>:1:"), "{message}");
    }
    let context = spawn();
    eval(&context, "<eval>", &bundle).expect("the bundle loads");
    let message = runtime_error(eval(&context, "<eval>", "render()"));
    assert!(message.contains("(webpack://app/"), "{message}");
}
