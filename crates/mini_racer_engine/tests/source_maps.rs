//! Runtime errors from a bundle with a registered source map, against the
//! same source in a context it does not match: the source with the bundle's
//! marker comment changed, so every position is the same.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers fail the test by panicking"
)]

mod source_map_fixture;

use std::fmt::Write as _;

use mini_racer_engine::process::fork;
use mini_racer_engine::{ContextHandle, SourceMapError};
use source_map_fixture::{
    Parking, bundle, eval, mapped, register, runtime_error, spawn, unmatched,
};

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
    assert!(runtime_error(eval(&matched, "<eval>", "render()")).contains("(webpack://app/"));
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

// `Error.stackTraceLimit` has no cap: an error thrown 5,000 frames deep
// prints them all, and V8's detailed trace covers the top ten.
#[test]
fn rewrites_the_top_frames_of_an_error_thrown_thousands_deep() {
    let bundle = format!(
        "function UeK() {{ throw new Error('deep K'); }}\n\
         function downK(n) {{ return n ? downK(n - 1) : UeK(); }}\n\
         globalThis.down = downK; /*{}*/\n",
        "pad ".repeat(256)
    );
    register(&bundle);
    let matched = spawn();
    eval(&matched, "<eval>", &bundle).expect("the bundle loads");
    let message = runtime_error(eval(
        &matched,
        "<eval>",
        "Error.stackTraceLimit = Infinity; down(5000)",
    ));
    let lines: Vec<&str> = message.lines().collect();
    assert!(lines.len() > 5_000, "{} lines", lines.len());
    assert!(
        lines[1].starts_with("    at UeK (webpack://app/./src/Header.jsx:11:"),
        "{}",
        lines[1]
    );
    for line in &lines[2..11] {
        assert!(
            line.starts_with("    at downK (webpack://app/./src/App.jsx:21:"),
            "{line}"
        );
    }
    assert!(
        lines[11].starts_with("    at downK (<eval>:2:"),
        "{}",
        lines[11]
    );
    assert!(
        lines[4_000].starts_with("    at downK (<eval>:2:"),
        "{}",
        lines[4_000]
    );
}

#[test]
fn refuses_a_map_that_does_not_parse_or_fit() {
    let maps = fork::current().source_maps();
    let bundle = "x".repeat(1024);
    for map in [
        "{",
        r#"{"version":2,"sources":[],"mappings":""}"#,
        r#"{"version":3,"sections":[]}"#,
        r#"{"version":3,"sources":["a.js"],"mappings":";AAAA"}"#,
    ] {
        assert!(
            matches!(
                maps.register(&bundle, map.as_bytes()),
                Err(SourceMapError::Map(_))
            ),
            "{map}"
        );
    }
}
