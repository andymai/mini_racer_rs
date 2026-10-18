//! Each result a runner replies with is, byte for byte, what V8's serializer
//! writes for the same value, whether or not the runner wrote it itself.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers fail the test by panicking"
)]

use std::fmt::Write;

use mini_racer_engine::{ContextConfig, ContextHandle, Host, WaitHandle, WaitOutcome, process};
use mini_racer_wire::marker;
use v8::ValueSerializerHelper;

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

// A value V8 cannot clone fails the write, which the test asserts on.
struct Refusing;

impl v8::ValueSerializerImpl for Refusing {
    fn throw_data_clone_error<'s>(
        &self,
        _scope: &mut v8::PinScope<'s, '_>,
        _message: v8::Local<'s, v8::String>,
    ) {
    }
}

// What V8's serializer writes for the value each source evaluates to, in an
// isolate of the test's own.
fn serialized(sources: &[String]) -> Vec<Vec<u8>> {
    process::ensure_initialized();
    let mut isolate = v8::Isolate::new(v8::CreateParams::default());
    let context = {
        v8::scope!(let scope, &mut isolate);
        let context = v8::Context::new(scope, v8::ContextOptions::default());
        v8::Global::new(scope, context)
    };
    v8::scope_with_context!(let scope, &mut isolate, &context);
    sources
        .iter()
        .map(|source| {
            let code = v8::String::new(scope, source).expect("source");
            let value = v8::Script::compile(scope, code, None)
                .and_then(|script| script.run(scope))
                .unwrap_or_else(|| panic!("{source} runs"));
            let serializer = v8::ValueSerializer::new(scope, Box::new(Refusing));
            serializer.write_header();
            let context = scope.get_current_context();
            assert_eq!(
                serializer.write_value(context, value),
                Some(true),
                "{source}"
            );
            serializer.release()
        })
        .collect()
}

fn replies(sources: &[String]) -> Vec<Vec<u8>> {
    let context = ContextHandle::spawn(ContextConfig::default(), &mut Parking).expect("spawn");
    sources
        .iter()
        .map(|source| {
            context
                .eval("<eval>".to_owned(), source.clone(), &mut Parking)
                .unwrap_or_else(|error| panic!("{source}: {error:?}"))
        })
        .collect()
}

fn same_bytes(cases: &[(&str, &str)]) {
    let sources: Vec<String> = cases
        .iter()
        .map(|(source, _)| (*source).to_owned())
        .collect();
    let expected: Vec<String> = cases.iter().map(|(_, value)| (*value).to_owned()).collect();
    for ((source, reply), v8) in sources
        .iter()
        .zip(replies(&sources))
        .zip(serialized(&expected))
    {
        assert_eq!(reply, v8, "{source}");
    }
}

fn same(sources: &[&str]) {
    let cases: Vec<(&str, &str)> = sources.iter().map(|source| (*source, *source)).collect();
    same_bytes(&cases);
}

#[test]
fn numbers() {
    same(&[
        "0",
        "1",
        "-1",
        "-0",
        "2147483647",
        "-2147483648",
        "2147483648",
        "1.5",
        "0.1 + 0.2",
        "2 ** 53",
        "1e300",
        "Number.MIN_VALUE",
        "Infinity",
        "-Infinity",
        "NaN",
        "new Float64Array([2])[0]",
        "(() => { const a = [1.5]; a[0] = 2; return a[0] })()",
    ]);
}

// A heap number holding an int32 is a double to V8's serializer, so only a
// small integer may be written as an int32.
#[test]
fn a_heap_number_holding_an_int32_stays_a_double() {
    let source = "new Float64Array([2])[0]".to_owned();
    let v8 = serialized(std::slice::from_ref(&source));
    let tag = v8.first().and_then(|bytes| bytes.get(2)).copied();
    assert_eq!(tag, Some(b'N'), "V8 writes {source} as a double");
    same(&["new Float64Array([2])[0]"]);
}

#[test]
fn constants() {
    same(&["true", "false", "null", "undefined", "void 0"]);
}

#[test]
fn strings_in_every_representation() {
    same(&[
        "''",
        "'hello'",
        "'caf\\u00e9'",
        "'\\u0100 hello'",
        "'\\ud83d\\ude00 hello'",
        "'\\ud800'",
        "'x'.repeat(30) + 'y'.repeat(30)",
        "('z'.repeat(40) + 'w').slice(1)",
        "'\\u0100\\u00e9'.slice(1)",
        "(() => { const s = 'ab' + 'cd'; ({ [s]: 1 }); return s })()",
        "'x'.repeat(1 << 20)",
    ]);
}

// V8 pads before a two-byte string whose characters would start at an odd
// offset, which turns on how many bytes its length takes.
#[test]
fn two_byte_strings_around_each_change_in_padding() {
    same(&[
        "'\\u0100'.repeat(63)",
        "'\\u0100'.repeat(64)",
        "'\\u0100'.repeat(8191)",
        "'\\u0100'.repeat(8192)",
        "'\\u0100'.repeat(1 << 20)",
    ]);
}

#[test]
fn values_a_result_becomes_first() {
    let mut function = String::from("'");
    for unit in marker::JAVASCRIPT_FUNCTION {
        write!(function, "\\u{unit:04x}").expect("formats");
    }
    function.push('\'');
    same_bytes(&[
        ("Symbol('described')", "'described'"),
        ("Symbol()", "undefined"),
        ("(function f() {})", &function),
        ("new Proxy(Object(1), {})", "Object(1)"),
    ]);
}

#[test]
fn values_left_to_v8() {
    same(&[
        "10n",
        "[1, 'a']",
        "({ a: 1 })",
        "new Date(0)",
        "new Uint8Array([1, 2])",
    ]);
}
