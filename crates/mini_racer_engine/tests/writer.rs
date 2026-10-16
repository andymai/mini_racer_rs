//! V8's own deserializer reads what `Writer` writes: each case writes one value
//! and a JavaScript predicate over it, bound to `v`, must hold.

#![allow(clippy::panic, reason = "test helpers fail the test by panicking")]

use mini_racer_engine::process;
use mini_racer_wire::Writer;
use v8::ValueDeserializerHelper;

struct Reader;

impl v8::ValueDeserializerImpl for Reader {}

type Case<'a> = (&'a dyn Fn(&mut Writer<'_>), &'a str);

fn reads(cases: &[Case<'_>]) {
    process::ensure_initialized();
    let mut isolate = v8::Isolate::new(v8::CreateParams::default());
    let context = {
        v8::scope!(let scope, &mut isolate);
        let context = v8::Context::new(scope, v8::ContextOptions::default());
        v8::Global::new(scope, context)
    };
    v8::scope_with_context!(let scope, &mut isolate, &context);
    let context = scope.get_current_context();
    let name = v8::String::new(scope, "v").expect("name");
    for (write, predicate) in cases {
        let mut bytes = Vec::new();
        write(&mut Writer::new(&mut bytes));
        let deserializer = v8::ValueDeserializer::new(scope, Box::new(Reader), &bytes);
        assert_eq!(deserializer.read_header(context), Some(true), "{predicate}");
        let value = deserializer
            .read_value(context)
            .unwrap_or_else(|| panic!("V8 rejects the bytes for {predicate}"));
        context.global(scope).set(scope, name.into(), value);
        let source = v8::String::new(scope, predicate).expect("source");
        let holds = v8::Script::compile(scope, source, None)
            .and_then(|script| script.run(scope))
            .is_some_and(|result| result.is_true());
        assert!(holds, "{predicate}");
    }
}

fn utf16le(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

#[test]
fn strings() {
    reads(&[
        (&|w| w.utf8("h\u{e9}llo".as_bytes()), r#"v === "héllo""#),
        (&|w| w.utf8(b""), r#"v === """#),
        (&|w| w.latin1(b"caf\xe9"), r#"v === "café""#),
        (
            &|w| w.utf16le(&utf16le("\u{6771}\u{4eac}")),
            r#"v === "東京""#,
        ),
        (&|w| w.utf16le(&utf16le("\u{1f600}")), r#"v === "😀""#),
        (
            &|w| w.utf16le(&[0x00, 0xd8, b'x', 0x00]),
            "v.length === 2 && v.charCodeAt(0) === 0xd800 && v[1] === 'x'",
        ),
    ]);
}

#[test]
fn integers_cross_into_doubles_and_bigints_where_mini_racer_does() {
    reads(&[
        (&|w| w.integer(i64::from(i32::MAX)), "v === 2147483647"),
        (&|w| w.integer(i64::from(i32::MIN)), "v === -2147483648"),
        (&|w| w.integer(1 << 31), "v === 2147483648"),
        (&|w| w.integer(i64::MAX / 1024), "v === 9007199254740991"),
        (
            &|w| w.integer(i64::MAX / 1024 + 1),
            "v === 9007199254740992n",
        ),
        (
            &|w| w.integer(i64::MIN / 1024 + 1),
            "v === -9007199254740991",
        ),
        (&|w| w.integer(i64::MIN / 1024), "v === -9007199254740992n"),
        (&|w| w.integer(i64::MAX), "v === 2n ** 63n - 1n"),
        (&|w| w.integer(i64::MIN), "v === -(2n ** 63n)"),
    ]);
}

#[test]
fn bigints_numbers_and_constants() {
    reads(&[
        (&|w| w.bigint(false, &[0; 8]).expect("word"), "v === 0n"),
        (
            &|w| w.bigint(true, &[5, 0, 0, 0, 0, 0, 0, 0]).expect("word"),
            "v === -5n",
        ),
        (
            &|w| {
                w.bigint(false, &[0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0])
                    .expect("words");
            },
            "v === 2n ** 64n",
        ),
        (&|w| w.number(1.5), "v === 1.5"),
        (&|w| w.number(-0.0), "Object.is(v, -0)"),
        (&|w| w.number(f64::NAN), "Number.isNaN(v)"),
        (&|w| w.number(f64::NEG_INFINITY), "v === -Infinity"),
        (&|w| w.null(), "v === null"),
        (&|w| w.undefined(), "v === undefined"),
        (&|w| w.bool(true), "v === true"),
        (&|w| w.bool(false), "v === false"),
    ]);
}

#[test]
fn dates_and_binary() {
    reads(&[
        (
            &|w| {
                w.date(1_622_505_600_000.0);
            },
            "v instanceof Date && v.getTime() === 1622505600000",
        ),
        (
            &|w| {
                w.date(f64::INFINITY);
            },
            "v instanceof Date && Number.isNaN(v.getTime())",
        ),
        (
            &|w| {
                w.uint8_array(b"ab");
            },
            "v instanceof Uint8Array && v.length === 2 && v[0] === 97 && v[1] === 98",
        ),
        (
            &|w| {
                w.uint8_array(b"");
            },
            "v instanceof Uint8Array && v.length === 0",
        ),
    ]);
}

#[test]
fn containers_and_the_ids_references_use() {
    reads(&[
        (
            &|w| {
                w.begin_object();
                w.utf8(b"a");
                w.integer(1);
                w.utf8(b"b");
                w.begin_array(1);
                w.bool(true);
                w.end_array(1);
                w.end_object(2);
            },
            "Object.keys(v).join() === 'a,b' && v.a === 1 && v.b.length === 1 && v.b[0] === true",
        ),
        (
            &|w| {
                let object = w.begin_object();
                w.utf8(b"self");
                w.object_ref(object);
                w.end_object(1);
            },
            "v.self === v",
        ),
        (
            &|w| {
                let array = w.begin_array(5);
                let date = w.date(1.0);
                let binary = w.uint8_array(b"ab");
                w.object_ref(date);
                w.object_ref(binary);
                w.object_ref(array);
                w.end_array(5);
            },
            "v[2] === v[0] && v[3] === v[1] && v[4] === v && v[1] instanceof Uint8Array",
        ),
    ]);
}
