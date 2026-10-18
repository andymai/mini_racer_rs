//! A call's arguments arrive as V8's deserializer would make them, except that
//! an array is packed, so one a function returns serializes as the same array
//! made in JavaScript does.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers fail the test by panicking"
)]

use mini_racer_engine::{
    ContextConfig, ContextHandle, EngineError, Host, JsErrorKind, TerminationCause, WaitHandle,
    WaitOutcome,
};
use mini_racer_wire::Writer;

struct Parking;

impl Host for Parking {
    fn wait(&mut self, wait: &WaitHandle<'_>) -> WaitOutcome {
        self.wait_blocking(wait);
        WaitOutcome::Ready
    }

    fn wait_blocking(&mut self, wait: &WaitHandle<'_>) {
        while !wait.ready() {
            wait.park();
        }
    }
}

fn spawn(config: ContextConfig) -> ContextHandle {
    let context = ContextHandle::spawn(config, &mut Parking).expect("spawn");
    eval(
        &context,
        "globalThis.id = (x) => x; globalThis.same = (a, b) => a === b",
    );
    context
}

fn eval(context: &ContextHandle, source: &str) -> Vec<u8> {
    context
        .eval("<eval>".to_owned(), source.to_owned(), &mut Parking)
        .unwrap_or_else(|error| panic!("{source}: {error:?}"))
}

// `[path, ...arguments]`, as the extension's encoder writes a call.
fn request(path: &str, count: u32, arguments: impl FnOnce(&mut Writer<'_>)) -> Vec<u8> {
    let mut out = Vec::new();
    let mut writer = Writer::new(&mut out);
    let length = count.checked_add(1).expect("short");
    writer.begin_array(length);
    writer.utf8(path.as_bytes());
    arguments(&mut writer);
    writer.end_array(length);
    out
}

fn call(context: &ContextHandle, request: Vec<u8>) -> Result<Vec<u8>, EngineError> {
    context.call(request, &mut Parking)
}

// Passed through `id`, the value the arguments write comes back as the value
// `source` evaluates to.
fn returns(context: &ContextHandle, arguments: impl FnOnce(&mut Writer<'_>), source: &str) {
    let returned = call(context, request("id", 1, arguments)).expect("call");
    assert_eq!(returned, eval(context, source), "{source}");
}

#[test]
fn a_returned_argument_array_serializes_as_the_same_array_from_javascript() {
    let context = spawn(ContextConfig::default());
    returns(
        &context,
        |w| {
            w.begin_array(6);
            w.integer(1);
            w.number(2.5);
            w.utf8(b"a");
            w.null();
            w.bool(true);
            w.begin_array(1);
            w.integer(3);
            w.end_array(1);
            w.end_array(6);
        },
        "[1, 2.5, 'a', null, true, [3]]",
    );
    returns(
        &context,
        |w| {
            w.begin_array(3);
            for n in ["x", "y", "z"] {
                w.utf8(n.as_bytes());
            }
            w.end_array(3);
        },
        "['x', 'y', 'z']",
    );
}

#[test]
fn shared_and_circular_values_keep_their_identity() {
    let context = spawn(ContextConfig::default());
    eval(
        &context,
        "globalThis.selfish = (o) => o.self === o; \
         globalThis.circular = (a) => a[0] === a; \
         globalThis.up = (a) => a[0].up === a",
    );
    let truth = eval(&context, "true");
    let shared_array = request("same", 2, |w| {
        w.begin_array(1);
        w.integer(1);
        w.end_array(1);
        w.object_ref(1);
    });
    let shared_date = request("same", 2, |w| {
        let date = w.date(0.0);
        w.object_ref(date);
    });
    let own_object = request("selfish", 1, |w| {
        let object = w.begin_object();
        w.utf8(b"self");
        w.object_ref(object);
        w.end_object(1);
    });
    let own_array = request("circular", 1, |w| {
        let array = w.begin_array(1);
        w.object_ref(array);
        w.end_array(1);
    });
    let enclosing_array = request("up", 1, |w| {
        let array = w.begin_array(1);
        w.begin_object();
        w.utf8(b"up");
        w.object_ref(array);
        w.end_object(1);
        w.end_array(1);
    });
    for sent in [
        shared_array,
        shared_date,
        own_object,
        own_array,
        enclosing_array,
    ] {
        assert_eq!(call(&context, sent).expect("call"), truth);
    }
}

#[derive(Clone, Copy)]
enum Key {
    Utf8(&'static str),
    Latin1(&'static [u8]),
    Int(i64),
}

// An object whose keys each hold 0.
fn object(w: &mut Writer<'_>, keys: &[Key]) {
    w.begin_object();
    for key in keys {
        match *key {
            Key::Utf8(text) => w.utf8(text.as_bytes()),
            Key::Latin1(bytes) => w.latin1(bytes),
            Key::Int(n) => w.integer(n),
        }
        w.integer(0);
    }
    w.end_object(u32::try_from(keys.len()).expect("short"));
}

#[test]
fn keys_name_properties_as_v8_names_them() {
    let context = spawn(ContextConfig::default());
    eval(&context, "globalThis.keys = (o) => Object.keys(o).join()");
    let sent = request("keys", 1, |w| {
        object(
            w,
            &[
                Key::Utf8("b"),
                Key::Int(2),
                Key::Latin1(b"a"),
                Key::Int(-1),
                Key::Utf8("\u{e9}"),
                Key::Utf8("__proto__"),
                Key::Int(1),
            ],
        );
    });
    assert_eq!(
        call(&context, sent).expect("call"),
        eval(
            &context,
            "Object.keys({ b: 0, 2: 0, a: 0, '-1': 0, '\\u00e9': 0, ['__proto__']: 0, 1: 0 }).join()"
        )
    );
    returns(
        &context,
        |w| {
            object(
                w,
                &[Key::Int(1), Key::Utf8("01"), Key::Utf8("-0"), Key::Int(0)],
            );
        },
        "({ 1: 0, '01': 0, '-0': 0, 0: 0 })",
    );
}

// Objects of one shape share the names of their keys, which must still
// follow each object's own keys, in order and at every depth.
#[test]
fn objects_of_one_shape_and_of_others_keep_their_own_keys() {
    let context = spawn(ContextConfig::default());
    let shapes: [&[Key]; 5] = [
        &[Key::Utf8("a"), Key::Utf8("b")],
        &[Key::Utf8("a"), Key::Utf8("b")],
        &[Key::Utf8("b"), Key::Utf8("a")],
        &[Key::Utf8("a"), Key::Int(7)],
        &[Key::Latin1(b"a")],
    ];
    returns(
        &context,
        |w| {
            w.begin_array(6);
            for keys in shapes {
                object(w, keys);
            }
            w.begin_object();
            w.utf8(b"a");
            object(w, &[Key::Utf8("b"), Key::Utf8("a")]);
            w.utf8(b"b");
            object(w, &[Key::Utf8("a"), Key::Utf8("c")]);
            w.end_object(2);
            w.end_array(6);
        },
        "[{ a: 0, b: 0 }, { a: 0, b: 0 }, { b: 0, a: 0 }, { a: 0, 7: 0 }, { a: 0 }, \
         { a: { b: 0, a: 0 }, b: { a: 0, c: 0 } }]",
    );
}

// V8's deserializer refuses an object given the same key twice, in whatever
// form, and mini_racer reports that as an internal error.
#[test]
fn a_key_given_twice_is_refused_as_v8_refuses_it() {
    let context = spawn(ContextConfig::default());
    let pairs = [
        [Key::Utf8("a"), Key::Latin1(b"a")],
        [Key::Int(1), Key::Utf8("1")],
        [Key::Latin1(b"-7"), Key::Int(-7)],
        [Key::Utf8("\u{e9}"), Key::Utf8("\u{e9}")],
    ];
    for keys in pairs {
        match call(&context, request("id", 1, |w| object(w, &keys))) {
            Err(EngineError::Js(js)) => {
                assert_eq!(js.kind, JsErrorKind::Internal);
                assert!(
                    js.message.contains("Unable to deserialize cloned data"),
                    "{}",
                    js.message
                );
            }
            other => panic!("expected V8's refusal, got {other:?}"),
        }
    }
    let many: Vec<Key> = (0..40).map(Key::Int).chain([Key::Utf8("39")]).collect();
    assert!(matches!(
        call(&context, request("id", 1, |w| object(w, &many))),
        Err(EngineError::Js(js)) if js.kind == JsErrorKind::Internal
    ));
}

// An object with many keys, a two-byte string and a binary are read by V8's
// own deserializer from their bytes, inside a request the runner builds.
#[test]
fn values_v8_reads_for_the_runner_keep_their_values_and_identity() {
    let context = spawn(ContextConfig::default());
    let keys: Vec<Key> = (0..30).map(Key::Int).chain([Key::Utf8("name")]).collect();
    let literal = (0..30)
        .map(|n| format!("{n}: 0"))
        .collect::<Vec<_>>()
        .join(", ");
    returns(
        &context,
        |w| object(w, &keys),
        &format!("({{ {literal}, name: 0 }})"),
    );
    let two_byte: Vec<u8> = "\u{6771}\u{4eac}"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    returns(
        &context,
        |w| {
            w.begin_array(3);
            w.utf8(b"a");
            w.utf16le(&two_byte);
            w.integer(1);
            w.end_array(3);
        },
        "['a', '\\u6771\\u4eac', 1]",
    );
    returns(
        &context,
        |w| {
            w.begin_array(1);
            w.uint8_array(b"ab");
            w.end_array(1);
        },
        "[new Uint8Array([97, 98])]",
    );
    eval(
        &context,
        "globalThis.inner = (o, i) => o.inner === i; \
         globalThis.shares = (o) => o.k0 === o.k1 && o.k0.length === 1",
    );
    let truth = eval(&context, "true");
    let big = |w: &mut Writer<'_>, inner: bool| {
        let object = w.begin_object();
        for n in 0..30 {
            w.utf8(format!("k{n}").as_bytes());
            w.integer(n);
        }
        if inner {
            w.utf8(b"inner");
            w.begin_array(1);
            w.integer(1);
            w.end_array(1);
        }
        w.end_object(if inner { 31 } else { 30 });
        object
    };
    let twice = request("same", 2, |w| {
        let object = big(w, false);
        w.object_ref(object);
    });
    let inside = request("inner", 2, |w| {
        let object = big(w, true);
        w.object_ref(object.checked_add(1).expect("inner id"));
    });
    let shared = request("shares", 1, |w| {
        w.begin_object();
        let array = {
            w.utf8(b"k0");
            let array = w.begin_array(1);
            w.integer(1);
            w.end_array(1);
            array
        };
        w.utf8(b"k1");
        w.object_ref(array);
        for n in 2..30 {
            w.utf8(format!("k{n}").as_bytes());
            w.integer(n);
        }
        w.end_object(30);
    });
    let binary_twice = request("same", 2, |w| {
        let view = w.uint8_array(b"ab");
        w.object_ref(view);
    });
    for sent in [twice, inside, shared, binary_twice] {
        assert_eq!(call(&context, sent).expect("call"), truth);
    }
}

#[test]
fn strings_arrive_in_the_form_v8_reads_them_in() {
    let context = spawn(ContextConfig::default());
    returns(
        &context,
        |w| w.utf8("h\u{e9}llo".as_bytes()),
        "'h\\u00e9llo'",
    );
    returns(&context, |w| w.latin1(b"caf\xe9"), "'caf\\u00e9'");
    returns(
        &context,
        |w| {
            let units: Vec<u8> = "\u{6771}\u{4eac}"
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect();
            w.utf16le(&units);
        },
        "'\\u6771\\u4eac'",
    );
    returns(&context, |w| w.utf8(b"a\xffb"), "'a\\ufffdb'");
    returns(&context, |w| w.utf8(b""), "''");
}

fn null(w: &mut Writer<'_>) {
    w.null();
}

fn undefined(w: &mut Writer<'_>) {
    w.undefined();
}

#[test]
fn numbers_and_constants_arrive_as_v8_reads_them() {
    let context = spawn(ContextConfig::default());
    returns(&context, |w| w.integer(i64::from(i32::MIN)), "-2147483648");
    returns(&context, |w| w.integer(1 << 31), "2147483648");
    returns(&context, |w| w.number(1.0), "1");
    returns(&context, |w| w.number(-0.0), "-0");
    returns(&context, |w| w.number(f64::NAN), "NaN");
    returns(&context, null, "null");
    returns(&context, undefined, "undefined");
    returns(&context, |w| w.bool(false), "false");
}

#[test]
fn bigints_arrive_whole() {
    let context = spawn(ContextConfig::default());
    let cases: [(bool, &[u64], &str); 7] = [
        (false, &[], "0n"),
        (true, &[1], "-1n"),
        (false, &[1 << 63], "2n ** 63n"),
        (false, &[u64::MAX], "2n ** 64n - 1n"),
        (false, &[0, 1], "2n ** 64n"),
        (true, &[0, 1], "-(2n ** 64n)"),
        (false, &[0, 0, 1], "2n ** 128n"),
    ];
    for (negative, words, source) in cases {
        let digits: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
        returns(
            &context,
            |w| w.bigint(negative, &digits).expect("words"),
            source,
        );
    }
}

#[test]
fn dates_arrive_as_v8_reads_them() {
    let context = spawn(ContextConfig::default());
    for (millis, source) in [
        (0.0, "new Date(0)"),
        (1.5, "new Date(1.5)"),
        (-8.64e15, "new Date(-8.64e15)"),
        (f64::INFINITY, "new Date(NaN)"),
    ] {
        returns(
            &context,
            |w| {
                w.date(millis);
            },
            source,
        );
    }
}

// Past the depth the runner builds to, V8's deserializer reads the request,
// as deep as its stack allows.
#[test]
fn nesting_at_any_depth_arrives_whole() {
    let context = spawn(ContextConfig::default());
    eval(
        &context,
        "globalThis.depth = (a) => { let n = 0; while (Array.isArray(a)) { a = a[0]; n++ } return [n, a] }",
    );
    for depth in [1, 63, 64, 65, 100, 1000] {
        let sent = request("depth", 1, |w| {
            for _ in 0..depth {
                w.begin_array(1);
            }
            w.integer(7);
            for _ in 0..depth {
                w.end_array(1);
            }
        });
        assert_eq!(
            call(&context, sent).expect("call"),
            eval(&context, &format!("[{depth}, 7]")),
            "{depth}"
        );
    }
}

// V8's deserializer acts on a termination before each value, so a collection
// past `max_memory` while the arguments are read ends the call before the
// function runs, even one V8 enters without checking, as `fill`: whether the
// runner builds the values or V8 reads them for it.
#[test]
fn a_heap_past_max_memory_while_arguments_are_read_stops_the_call_before_it_runs() {
    let count = 2_000_000;
    // V8 keeps one-character strings in a cache, so these allocate.
    let text = |n: u32| {
        let letter = b'a' + u8::try_from(n % 26).expect("letter");
        let digit = b'0' + u8::try_from(n / 26 % 10).expect("digit");
        [letter, digit]
    };
    let in_array = request("filled.fill", 1, |w| {
        w.begin_array(count);
        for n in 0..count {
            w.utf8(&text(n));
        }
        w.end_array(count);
    });
    let in_object = request("filled.fill", 1, |w| {
        w.begin_object();
        for n in 0..count {
            w.integer(i64::from(n));
            w.utf8(&text(n));
        }
        w.end_object(count);
    });
    for sent in [in_array, in_object] {
        let context = spawn(ContextConfig {
            max_memory: Some(16_000_000),
            ..ContextConfig::default()
        });
        eval(&context, "globalThis.filled = [0]");
        match call(&context, sent) {
            Err(EngineError::Js(js)) => assert_eq!(
                js.kind,
                JsErrorKind::Terminated(TerminationCause::MaxMemory)
            ),
            other => panic!("expected out of memory, got {other:?}"),
        }
        assert_eq!(eval(&context, "filled[0]"), eval(&context, "0"));
    }
}
