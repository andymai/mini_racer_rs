//! Runtime errors from a bundle with a registered source map, against the
//! same source in a context it does not match: the source with the bundle's
//! last byte changed, so every position is the same.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers fail the test by panicking"
)]

use std::fmt::Write as _;

use mini_racer_engine::process::fork;
use mini_racer_engine::{
    ContextConfig, ContextHandle, EngineError, Host, JsErrorKind, SourceMapError, WaitHandle,
    WaitOutcome,
};

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

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn vlq(out: &mut String, value: i64) {
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

const SOURCES: [&str; 2] = ["./src/Header.jsx", "./src/App.jsx"];
const ROOT: &str = "webpack://app/";

// Every column of bundle line `l` maps to line 10 (l + 1), at the column plus
// 100, of source `l % 2`, so where any frame maps follows from where it is.
fn expected(line: usize, column: usize) -> String {
    format!(
        "{ROOT}{}:{}:{}",
        SOURCES[line % 2],
        10 * (line + 1) + 1,
        column + 101
    )
}

fn map(bundle: &str) -> String {
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
        r#"{{"version":3,"sourceRoot":"{ROOT}","sources":["{}","{}"],"sourcesContent":["skipped"],"names":[],"mappings":"{mappings}"}}"#,
        SOURCES[0], SOURCES[1]
    )
}

// A bundle each test makes its own with `tag`, as the registry is the
// process's.
fn bundle(tag: &str) -> String {
    format!(
        "function Ue{tag}() {{ throw new Error('boom {tag}'); }}\n\
         function Header{tag}(props) {{ return Ue{tag}(); }}\n\
         function render{tag}() {{ return [1].map(function mapper() {{ return Header{tag}({{}}); }})[0]; }}\n\
         async function deep{tag}() {{ await null; return Header{tag}({{}}); }} async function outer{tag}() {{ return await deep{tag}(); }}\n\
         globalThis.render = render{tag}; globalThis.deep = outer{tag}; /*0*/\n"
    )
}

// What the baseline evaluates: the same text with the bundle's marker
// comment changed, which moves nothing.
fn unmatched(source: &str, bundle: &str) -> String {
    let start = source.find(bundle).expect("the bundle");
    let at = start + bundle.find("/*0*/").expect("the marker") + 2;
    let mut text = source.to_owned();
    text.replace_range(at..=at, "1");
    text
}

fn spawn() -> ContextHandle {
    ContextHandle::spawn(ContextConfig::default(), &mut Parking).expect("spawn")
}

fn eval(context: &ContextHandle, name: &str, source: &str) -> Result<Vec<u8>, EngineError> {
    context.eval(name.to_owned(), source.to_owned(), &mut Parking)
}

fn runtime_error(result: Result<Vec<u8>, EngineError>) -> String {
    match result {
        Err(EngineError::Js(js)) if js.kind == JsErrorKind::Runtime => js.message,
        other => panic!("expected a runtime error, got {other:?}"),
    }
}

// The baseline's message with each frame that falls in the bundle relocated
// by `expected`, given where the bundle starts, but for the last `outside`
// frames, those of the script that called into the bundle.
fn mapped(
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

fn register(bundle: &str) {
    fork::current()
        .source_maps()
        .register(bundle, map(bundle).as_bytes())
        .expect("a map for the bundle");
}

#[test]
fn rewrites_the_frames_of_a_runtime_error_in_the_bundle() {
    let bundle = bundle("A");
    register(&bundle);
    let prefix = "var global = global || this;\nvar self = self || this;\n";
    for (prefix, start) in [(prefix, (2, 0)), ("var early = 1; ", (0, 15)), ("", (0, 0))] {
        let source = format!("{prefix}{bundle}function outside() {{ return render(); }}\n");
        let (matched, baseline) = (spawn(), spawn());
        eval(&matched, "<eval>", &source).expect("the bundle loads");
        eval(&baseline, "<eval>", &unmatched(&source, &bundle)).expect("the baseline loads");
        for call in ["render()", "outside()", "HeaderA(1)"] {
            let got = runtime_error(eval(&matched, "<eval>", call));
            let want = runtime_error(eval(&baseline, "<eval>", call));
            assert_ne!(got, want, "{prefix:?} {call}: nothing was rewritten");
            assert_eq!(
                got,
                mapped(&want, "<eval>", start, (5, 1)),
                "{prefix:?} {call}"
            );
            assert!(
                got.contains("at UeA (webpack://app/./src/Header.jsx:11:"),
                "{got}"
            );
            assert_eq!(
                got.contains("at Array.map (<anonymous>)"),
                call != "HeaderA(1)",
                "{got}"
            );
        }
        // Errors outside the bundle, and parse errors, are left as they were.
        for source in ["throw new Error('plain')", "1 +"] {
            assert_eq!(
                format!("{:?}", eval(&matched, "<eval>", source)),
                format!("{:?}", eval(&baseline, "<eval>", source))
            );
        }
    }
}

// The flag a matched context's isolate turns on changes only what the
// engine reads: every value JavaScript sees is the baseline's.
#[test]
fn leaves_what_javascript_reads_alone() {
    let bundle = bundle("B");
    register(&bundle);
    let (matched, baseline) = (spawn(), spawn());
    eval(&matched, "<eval>", &bundle).expect("the bundle loads");
    eval(&baseline, "<eval>", &unmatched(&bundle, &bundle)).expect("the baseline loads");
    for probe in [
        "(() => { try { render() } catch (e) { return e.stack } })()",
        "(() => { Error.stackTraceLimit = 3; try { render() } catch (e) { return e.stack } finally { Error.stackTraceLimit = 10 } })()",
        "(() => { const o = {}; Error.captureStackTrace(o); return o.stack })()",
        "(() => { Error.prepareStackTrace = (e, s) => s.map(c => [c.getFunctionName(), c.getLineNumber(), c.getColumnNumber(), String(c)]); try { render() } catch (e) { return JSON.stringify(e.stack) } finally { delete Error.prepareStackTrace } })()",
        "(() => { let n = 0; for (let i = 0; i < 1000; i++) { try { throw {} } catch (e) { n++ } } return n })()",
        "(() => { delete Error.stackTraceLimit; const s = String(new Error('x').stack); Error.stackTraceLimit = 10; return s })()",
    ] {
        assert_eq!(
            eval(&matched, "<eval>", probe).expect("the probe runs"),
            eval(&baseline, "<eval>", probe).expect("the probe runs"),
            "{probe}"
        );
    }
}

// From its second sighting a large bundle reads its source from the copy
// contexts share, and from its third it compiles from the code cache; each
// compile is a new script, matched as it is compiled.
#[test]
fn rewrites_a_large_bundle_from_the_code_cache() {
    let mut bundle = bundle("C");
    for i in 0..25_000 {
        writeln!(bundle, "function filler{i}(a) {{ return a + {i}; }}").expect("write");
    }
    register(&bundle);
    let baseline_source = unmatched(&bundle, &bundle);
    for sighting in 1..=3 {
        let (matched, baseline) = (spawn(), spawn());
        eval(&matched, "bundle.js", &bundle).expect("the bundle loads");
        eval(&baseline, "bundle.js", &baseline_source).expect("the baseline loads");
        let got = runtime_error(eval(&matched, "<eval>", "render()"));
        let want = runtime_error(eval(&baseline, "<eval>", "render()"));
        assert_eq!(
            got,
            mapped(&want, "bundle.js", (0, 0), (25_005, 0)),
            "sighting {sighting}"
        );
        assert!(got.contains("webpack://app/./src/App.jsx"), "{got}");
    }
    assert!(fork::current().code_cache().hits() > 0);
}

// A call ExecJS shapes runs as a template, its own script, which keeps its
// frame; the bundle's frames under it are rewritten.
#[test]
fn rewrites_the_bundles_frames_under_a_fast_call() {
    let bundle = bundle("D");
    register(&bundle);
    let (matched, baseline) = (spawn(), spawn());
    eval(&matched, "<eval>", &bundle).expect("the bundle loads");
    eval(&baseline, "<eval>", &unmatched(&bundle, &bundle)).expect("the baseline loads");
    let call = "(render.apply(this, []))";
    let got = runtime_error(eval(&matched, "<eval>", call));
    let want = runtime_error(eval(&baseline, "<eval>", call));
    assert_eq!(got, mapped(&want, "<eval>", (0, 0), (5, 1)));
    assert_ne!(got, want);
}

#[test]
fn rewrites_an_awaited_rejection_but_not_its_async_frames() {
    let bundle = bundle("E");
    register(&bundle);
    let (matched, baseline) = (spawn(), spawn());
    eval(&matched, "<eval>", &bundle).expect("the bundle loads");
    eval(&baseline, "<eval>", &unmatched(&bundle, &bundle)).expect("the baseline loads");
    let awaited = |context: &ContextHandle| {
        runtime_error(context.eval_await("<eval>", "deep()", &mut Parking))
    };
    let (got, want) = (awaited(&matched), awaited(&baseline));
    assert_eq!(got, mapped(&want, "<eval>", (0, 0), (5, 0)));
    assert!(got.contains("    at async outerE (<eval>:4:"), "{got}");
    assert!(
        got.contains("at HeaderE (webpack://app/./src/App.jsx:21:"),
        "{got}"
    );
}

// V8 prints a script by the source URL it names, so the rewrite looks for
// that name.
#[test]
fn rewrites_a_bundle_named_by_its_source_url() {
    let bundle = format!("{}//# sourceURL=app.js\n", bundle("F"));
    register(&bundle);
    let (matched, baseline) = (spawn(), spawn());
    eval(&matched, "<eval>", &bundle).expect("the bundle loads");
    eval(&baseline, "<eval>", &unmatched(&bundle, &bundle)).expect("the baseline loads");
    let got = runtime_error(eval(&matched, "<eval>", "render()"));
    let want = runtime_error(eval(&baseline, "<eval>", "render()"));
    assert!(want.contains("(app.js:1:"), "{want}");
    assert_eq!(got, mapped(&want, "app.js", (0, 0), (6, 0)));
}

#[test]
fn refuses_a_map_that_does_not_parse_or_fit() {
    let maps = fork::current().source_maps();
    for (bundle, map) in [
        ("x", "{"),
        ("x", r#"{"version":2,"sources":[],"mappings":""}"#),
        ("x", r#"{"version":3,"sections":[]}"#),
        (
            "x",
            r#"{"version":3,"sources":["a.js"],"mappings":";AAAA"}"#,
        ),
        ("", r#"{"version":3,"sources":[],"mappings":""}"#),
    ] {
        assert!(
            matches!(
                maps.register(bundle, map.as_bytes()),
                Err(SourceMapError::Map(_))
            ),
            "{map}"
        );
    }
}
