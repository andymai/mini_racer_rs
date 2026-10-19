//! A bundle and its source map for the source map tests: every column of
//! the bundle's line `l` maps to line 10 (l + 1), at the column plus 100, of
//! source `l % 2`, so where any frame maps follows from where it is.

#![allow(
    clippy::expect_used,
    clippy::panic,
    dead_code,
    reason = "test helpers fail the test by panicking, and each file uses some"
)]

use mini_racer_engine::process::fork;
use mini_racer_engine::{
    ContextConfig, ContextHandle, EngineError, Host, JsErrorKind, WaitHandle, WaitOutcome,
};

pub struct Parking;

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

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn vlq(out: &mut String, value: i64) {
    let mut unsigned = (value.unsigned_abs() << 1) | u64::from(value < 0);
    loop {
        let digit = unsigned & 31;
        unsigned >>= 5;
        let digit = if unsigned == 0 { digit } else { digit | 32 };
        out.push(char::from(BASE64[usize::try_from(digit).expect("a digit")]));
        if unsigned == 0 {
            return;
        }
    }
}

pub const SOURCES: [&str; 2] = ["./src/Header.jsx", "./src/App.jsx"];
pub const ROOT: &str = "webpack://app/";

pub fn expected(line: usize, column: usize) -> String {
    format!(
        "{ROOT}{}:{}:{}",
        SOURCES[line % 2],
        10 * (line + 1) + 1,
        column + 101
    )
}

pub fn map(bundle: &str) -> String {
    map_with_root(bundle, ROOT)
}

pub fn map_with_root(bundle: &str, root: &str) -> String {
    let mut mappings = String::new();
    let (mut source, mut original, mut original_column) = (0_i64, 0_i64, 0_i64);
    for (line, text) in bundle.split('\n').enumerate() {
        if line > 0 {
            mappings.push(';');
        }
        let mut generated = 0_i64;
        for column in 0..text.encode_utf16().count() {
            let column = i64::try_from(column).expect("short");
            let line = i64::try_from(line).expect("short");
            if column > 0 {
                mappings.push(',');
            }
            let fields = [
                (column, generated),
                (line % 2, source),
                (10 * (line + 1), original),
                (column + 100, original_column),
            ];
            for (value, previous) in fields {
                vlq(&mut mappings, value - previous);
            }
            (generated, source, original, original_column) =
                (column, line % 2, 10 * (line + 1), column + 100);
        }
    }
    format!(
        r#"{{"version":3,"sourceRoot":"{root}","sources":["{}","{}"],"sourcesContent":["skipped"],"names":[],"mappings":"{mappings}"}}"#,
        SOURCES[0], SOURCES[1]
    )
}

// A bundle each test makes its own with `tag`, as the registry is the
// process's.
pub fn bundle(tag: &str) -> String {
    format!(
        "function Ue{tag}() {{ throw new Error('boom {tag}'); }}\n\
         function Header{tag}(props) {{ return Ue{tag}(); }}\n\
         function render{tag}() {{ return [1].map(function mapper() {{ return Header{tag}({{}}); }})[0]; }}\n\
         async function deep{tag}() {{ await null; return Header{tag}({{}}); }} async function outer{tag}() {{ return await deep{tag}(); }}\n\
         globalThis.render = render{tag}; globalThis.deep = outer{tag}; /*{}*/ /*0*/\n",
        "pad ".repeat(256)
    )
}

// What the baseline evaluates: the same text with the bundle's marker
// comment changed, which moves nothing.
pub fn unmatched(source: &str, bundle: &str) -> String {
    let start = source.find(bundle).expect("the bundle");
    let at = start + bundle.find("/*0*/").expect("the marker") + 2;
    let mut text = source.to_owned();
    text.replace_range(at..=at, "1");
    text
}

pub fn spawn() -> ContextHandle {
    ContextHandle::spawn(ContextConfig::default(), &mut Parking).expect("spawn")
}

pub fn eval(context: &ContextHandle, name: &str, source: &str) -> Result<Vec<u8>, EngineError> {
    context.eval(name.to_owned(), source.to_owned(), &mut Parking)
}

pub fn runtime_error(result: Result<Vec<u8>, EngineError>) -> String {
    match result {
        Err(EngineError::Js(js)) if js.kind == JsErrorKind::Runtime => js.message,
        other => panic!("expected a runtime error, got {other:?}"),
    }
}

// The baseline's message with each frame that falls in the bundle relocated
// by `expected`, given where the bundle starts, but for the last `outside`
// frames, those of the script that called into the bundle.
pub fn mapped(
    baseline: &str,
    name: &str,
    (line, column): (usize, usize),
    (lines, outside): (usize, usize),
) -> String {
    let count = baseline.split('\n').count();
    baseline
        .split('\n')
        .enumerate()
        .map(|(index, text)| {
            if index + outside >= count {
                return text.to_owned();
            }
            let Some(at) = text.rfind(&format!("{name}:")) else {
                return text.to_owned();
            };
            if text.trim_start().starts_with("at async") || !text.starts_with("    at ") {
                return text.to_owned();
            }
            let location = text[at + name.len() + 1..].trim_end_matches(')');
            let (l, c) = location.split_once(':').expect("line and column");
            let (l, c): (usize, usize) = (l.parse().expect("line"), c.parse().expect("column"));
            let (l, c) = (l - 1, c - 1);
            if l < line || (l == line && c < column) || l - line >= lines {
                return text.to_owned();
            }
            let c = if l == line { c - column } else { c };
            let close = if text.ends_with(')') { ")" } else { "" };
            format!("{}{}{close}", &text[..at], expected(l - line, c))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn register(bundle: &str) {
    fork::current()
        .source_maps()
        .register(bundle, map(bundle).as_bytes())
        .expect("a map for the bundle");
}
