use std::borrow::Cow;
use std::fmt::Write as _;

use crate::map::{Placement, SourceMap};

/// A frame of V8's detailed stack trace for an error: its script, and its
/// one-based line and column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub script_id: i32,
    pub line: u32,
    pub column: u32,
}

/// A script that holds a registered bundle: the name V8 prints for it in a
/// frame, and where in it the bundle starts.
#[derive(Clone, Copy)]
pub struct Match<'a> {
    pub script_id: i32,
    pub name: &'a str,
    pub map: &'a SourceMap,
    pub placement: Placement,
}

const FRAME: &str = "    at ";
// Past V8's frame limit for a detailed trace, which the engine sets, and so
// past any trace it hands over; pairing costs this much for each line at most.
const FRAMES: usize = 256;

// One line of V8's default stack format: a frame located by line and column,
// which V8's detailed trace has a frame for; an async frame, where that trace
// stops; or one with no such location (a builtin's `<anonymous>`, `native`,
// `index N`), which it leaves out.
enum Kind {
    Located { line: u32, column: u32 },
    Async,
    Other,
}

fn kind(text: &str) -> Kind {
    let Some(rest) = text.strip_prefix(FRAME) else {
        return Kind::Other;
    };
    if rest.starts_with("async ") {
        return Kind::Async;
    }
    let location = rest.strip_suffix(')').unwrap_or(rest);
    let mut parts = location.rsplitn(3, ':');
    let last = parts.next().unwrap_or_default();
    let before = parts.next();
    // V8 places a WebAssembly frame on line 1 at its byte offset plus one.
    if before.is_some_and(|before| before.starts_with("wasm-function[")) {
        return last
            .strip_prefix("0x")
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .and_then(|offset| offset.checked_add(1))
            .map_or(Kind::Other, |column| Kind::Located { line: 1, column });
    }
    match (before.and_then(number), number(last), parts.next()) {
        (Some(line), Some(column), Some(_)) => Kind::Located { line, column },
        _ => Kind::Other,
    }
}

fn number(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// Rewrites, in V8's stack text for an error, the location of each frame
/// inside a matched bundle that the bundle's map maps, keeping the function
/// name; everything else stays byte for byte.
///
/// The frame lines that end the text are paired in order with V8's detailed
/// frames, which leave out builtins, stop at the first async frame and may
/// stop before the text does, and each pair must agree on line and column.
/// When pairing from the first of those lines with a position disagrees, the
/// lines before a later one are taken as the message's, which may hold lines
/// that look like frames, if pairing from exactly one later line agrees.
/// With none, or more than one, or a matched frame whose line does not name
/// its script as `Match::name`, the text comes back as it was.
#[must_use]
pub fn rewrite<'t>(text: &'t str, frames: &[Frame], matches: &[Match<'_>]) -> Cow<'t, str> {
    let frames = frames.get(..FRAMES).unwrap_or(frames);
    let lines: Vec<&str> = text.split('\n').collect();
    let run = lines
        .iter()
        .rposition(|line| !line.starts_with(FRAME))
        .map_or(0, |at| at.saturating_add(1));
    let trailing = lines.get(run..).unwrap_or_default();
    let kinds: Vec<Kind> = trailing.iter().map(|line| kind(line)).collect();
    // A pairing walks only these, so trying each start costs at most a pass
    // over the detailed frames.
    let marks: Vec<usize> = (0..kinds.len())
        .filter(|&at| !matches!(kinds.get(at), Some(Kind::Other)))
        .collect();
    let mut starts = (0..marks.len()).filter(|&mark| {
        marks
            .get(mark)
            .is_some_and(|&at| matches!(kinds.get(at), Some(Kind::Located { .. })))
    });
    let from = |mark: usize| pair(&kinds, marks.get(mark..).unwrap_or_default(), frames);
    let pairs = if let Some(pairs) = starts.next().and_then(from) {
        pairs
    } else {
        let mut fits = starts.filter_map(from);
        match (fits.next(), fits.next()) {
            (Some(only), None) => only,
            _ => return Cow::Borrowed(text),
        }
    };
    let mut paired = pairs.iter().peekable();
    let mut out = String::with_capacity(text.len());
    let mut changed = false;
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let frame = paired
            .next_if(|&&(at, _)| run.checked_add(at) == Some(index))
            .map(|&(_, frame)| frame);
        match frame.map_or(Relocated::Kept, |frame| relocate(line, frame, matches)) {
            Relocated::Mismatch => return Cow::Borrowed(text),
            Relocated::Line(line) => {
                out.push_str(&line);
                changed = true;
            }
            Relocated::Kept => out.push_str(line),
        }
    }
    if changed {
        Cow::Owned(out)
    } else {
        Cow::Borrowed(text)
    }
}

// The lines, by index into the run, and the detailed frames they pair with,
// walking `marks` (the run's located and async lines) from the first; `None`
// when a pair disagrees or none pairs.
fn pair(kinds: &[Kind], marks: &[usize], frames: &[Frame]) -> Option<Vec<(usize, Frame)>> {
    let mut pairs = Vec::new();
    for (&at, &frame) in marks.iter().zip(frames) {
        match kinds.get(at)? {
            Kind::Located { line, column } if (frame.line, frame.column) == (*line, *column) => {
                pairs.push((at, frame));
            }
            Kind::Located { .. } => return None,
            Kind::Async | Kind::Other => break,
        }
    }
    (!pairs.is_empty()).then_some(pairs)
}

enum Relocated {
    Line(String),
    Kept,
    Mismatch,
}

fn relocate(line: &str, frame: Frame, matches: &[Match<'_>]) -> Relocated {
    let Some(script) = matches
        .iter()
        .find(|script| script.script_id == frame.script_id)
    else {
        return Relocated::Kept;
    };
    let Some(original) = script
        .map
        .original(script.placement, frame.line, frame.column)
    else {
        return Relocated::Kept;
    };
    let location = format!("{}:{}:{}", script.name, frame.line, frame.column);
    let (body, close) = line
        .strip_suffix(')')
        .map_or((line, ""), |body| (body, ")"));
    let Some(head) = body.strip_suffix(location.as_str()) else {
        return Relocated::Mismatch;
    };
    let framed = if close.is_empty() {
        head == FRAME
    } else {
        head.ends_with(" (")
    };
    if !framed {
        return Relocated::Mismatch;
    }
    let mut out = String::with_capacity(line.len());
    out.push_str(head);
    let _ = write!(
        out,
        "{}{}:{}:{}{close}",
        original.root,
        original.source,
        original.line.saturating_add(1),
        original.column.saturating_add(1)
    );
    Relocated::Line(out)
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::{Frame, Match, rewrite};
    use crate::map::tests::{Seg, mappings};
    use crate::map::{Placement, SourceMap};

    const HEADER: &str = "webpack://app/./src/Header.jsx";
    const APP: &str = "webpack://app/./src/App.jsx";

    // Three lines of 64 units: the first maps from column 0 to Header.jsx
    // 41:13 and from column 30 to 51:3, the second from column 0 to App.jsx
    // 10:5, and the third to nothing.
    fn map() -> SourceMap {
        let bundle = format!("{}\n", vec!["x".repeat(64); 3].join("\n"));
        let text = mappings(&[
            &[Seg::M(0, 0, 40, 12), Seg::M(30, 0, 50, 2)],
            &[Seg::M(0, 1, 9, 4)],
            &[Seg::U(0)],
        ]);
        let json = format!(
            r#"{{"version":3,"sources":["./src/Header.jsx","./src/App.jsx"],"sourceRoot":"webpack://app/","mappings":"{text}"}}"#
        );
        SourceMap::new(&bundle, json.as_bytes()).unwrap()
    }

    const EVAL: i32 = 7;
    // The bundle starts on line 11 of the script, which V8 names <eval>.
    const OWN_LINE: Placement = Placement {
        line: 10,
        column: 0,
    };

    fn frame(script_id: i32, line: u32, column: u32) -> Frame {
        Frame {
            script_id,
            line,
            column,
        }
    }

    fn rewritten(text: &str, frames: &[Frame], placement: Placement) -> String {
        let map = map();
        let matches = [Match {
            script_id: EVAL,
            name: "<eval>",
            map: &map,
            placement,
        }];
        rewrite(text, frames, &matches).into_owned()
    }

    fn unchanged(text: &str, frames: &[Frame]) {
        let map = map();
        let matches = [Match {
            script_id: EVAL,
            name: "<eval>",
            map: &map,
            placement: OWN_LINE,
        }];
        assert!(
            matches!(rewrite(text, frames, &matches), Cow::Borrowed(same) if same == text),
            "{text}"
        );
    }

    #[test]
    fn rewrites_the_location_and_keeps_the_function_name() {
        let text = "Error: boom\n    at Ue (<eval>:11:5)\n    at new Header (<eval>:11:40)\n    at <eval>:12:3";
        let frames = [frame(EVAL, 11, 5), frame(EVAL, 11, 40), frame(EVAL, 12, 3)];
        assert_eq!(
            rewritten(text, &frames, OWN_LINE),
            format!(
                "Error: boom\n    at Ue ({HEADER}:41:13)\n    at new Header ({HEADER}:51:3)\n    at {APP}:10:5"
            )
        );
    }

    #[test]
    fn keeps_function_names_with_spaces_and_parentheses() {
        let text = "TypeError: x\n    at Object.<computed> [as on (x)] (<eval>:11:5)";
        assert_eq!(
            rewritten(text, &[frame(EVAL, 11, 5)], OWN_LINE),
            format!("TypeError: x\n    at Object.<computed> [as on (x)] ({HEADER}:41:13)")
        );
    }

    // V8's detailed trace has no frame for a builtin and one for an eval's
    // own script, at the position after its origin.
    #[test]
    fn pairs_past_builtin_eval_and_other_scripts_frames() {
        let text = [
            "Error: map",
            "    at mapper (<eval>:11:5)",
            "    at Array.map (<anonymous>)",
            "    at eval (eval at Ue (<eval>:11:31), <anonymous>:1:7)",
            "    at new Promise (<anonymous>)",
            "    at helper (other.js:3:9)",
            "    at <eval>:12:1",
        ]
        .join("\n");
        let frames = [
            frame(EVAL, 11, 5),
            frame(8, 1, 7),
            frame(9, 3, 9),
            frame(EVAL, 12, 1),
        ];
        let expected = [
            "Error: map".to_owned(),
            format!("    at mapper ({HEADER}:41:13)"),
            "    at Array.map (<anonymous>)".to_owned(),
            "    at eval (eval at Ue (<eval>:11:31), <anonymous>:1:7)".to_owned(),
            "    at new Promise (<anonymous>)".to_owned(),
            "    at helper (other.js:3:9)".to_owned(),
            format!("    at {APP}:10:5"),
        ]
        .join("\n");
        assert_eq!(rewritten(&text, &frames, OWN_LINE), expected);
    }

    // V8's detailed trace stops at the first async frame, so nothing names
    // the script of one or of any after it.
    #[test]
    fn leaves_async_frames_alone() {
        let text = "Error: late\n    at inner (<eval>:11:5)\n    at async Ue (<eval>:11:31)\n    at async Promise.all (index 0)\n    at async <eval>:12:1";
        let frames = [frame(EVAL, 11, 5)];
        assert_eq!(
            rewritten(text, &frames, OWN_LINE),
            format!(
                "Error: late\n    at inner ({HEADER}:41:13)\n    at async Ue (<eval>:11:31)\n    at async Promise.all (index 0)\n    at async <eval>:12:1"
            )
        );
        let text = "Error: late\n    at inner (<eval>:11:5)\n    at async Ue (<eval>:11:31)\n    at <eval>:12:1";
        let frames = [frame(EVAL, 11, 5), frame(EVAL, 12, 1)];
        assert_eq!(
            rewritten(text, &frames, OWN_LINE),
            format!(
                "Error: late\n    at inner ({HEADER}:41:13)\n    at async Ue (<eval>:11:31)\n    at <eval>:12:1"
            )
        );
    }

    // The text can have more frames than V8's detailed trace, which stops at
    // its own limit.
    #[test]
    fn leaves_frames_past_the_detailed_trace_alone() {
        let text =
            "Error: deep\n    at a (<eval>:11:5)\n    at b (<eval>:12:1)\n    at c (<eval>:11:31)";
        let frames = [frame(EVAL, 11, 5), frame(EVAL, 12, 1)];
        assert_eq!(
            rewritten(text, &frames, OWN_LINE),
            format!(
                "Error: deep\n    at a ({HEADER}:41:13)\n    at b ({APP}:10:5)\n    at c (<eval>:11:31)"
            )
        );
    }

    #[test]
    fn passes_over_message_lines_that_look_like_frames() {
        let frames = [frame(EVAL, 11, 5), frame(EVAL, 12, 1)];
        let real = format!("    at Ue ({HEADER}:41:13)\n    at {APP}:10:5");
        for fake in [
            "    at fake (<eval>:1:1)",
            "    at fake (<eval>:11:5)",
            "    at Array.map (<anonymous>)\n    at fake (<eval>:99:1)",
            "\n    at fake (<eval>:11:5)\n    at <eval>:11:5",
            "\n    at fake (<eval>:1:1)\n    at fake (native)\n    at Array.map (<anonymous>)",
        ] {
            let text =
                format!("Error: bad input{fake}\n    at Ue (<eval>:11:5)\n    at <eval>:12:1");
            let text = text.replacen("input    at", "input\n    at", 1);
            let message = &text[..text
                .find("\n    at Ue (<eval>:11:5)\n    at <eval>:12:1")
                .unwrap()];
            assert_eq!(
                rewritten(&text, &frames, OWN_LINE),
                format!("{message}\n{real}"),
                "{fake:?}"
            );
        }
    }

    // An error built from another's stack carries that stack's frames in its
    // message, in the same bundle.
    #[test]
    fn leaves_a_wrapped_stack_in_the_message_alone() {
        let text = "Error: wrapped: Error: inner\n    at inner (<eval>:11:31)\n    at Ue (<eval>:11:5)\n    at <eval>:12:1\n    at Ue (<eval>:11:9)\n    at <eval>:12:1";
        let frames = [frame(EVAL, 11, 9), frame(EVAL, 12, 1)];
        assert_eq!(
            rewritten(text, &frames, OWN_LINE),
            format!(
                "Error: wrapped: Error: inner\n    at inner (<eval>:11:31)\n    at Ue (<eval>:11:5)\n    at <eval>:12:1\n    at Ue ({HEADER}:41:13)\n    at {APP}:10:5"
            )
        );
    }

    #[test]
    fn leaves_text_that_pairs_more_than_one_way_alone() {
        let text =
            "Error: x\n    at a (<eval>:30:1)\n    at b (<eval>:11:5)\n    at c (<eval>:11:5)";
        unchanged(text, &[frame(EVAL, 11, 5)]);
    }

    // Every start pairs until its last frame and then disagrees, which costs
    // each line at most the frames' cap however many frames come in.
    #[test]
    #[expect(clippy::indexing_slicing, reason = "a test's fixed layout")]
    fn caps_the_frames_a_pairing_walks() {
        let text = format!("Error: x{}", "\n    at f (<eval>:11:5)".repeat(100_000));
        let mut frames = vec![frame(EVAL, 11, 5); 1_000_000];
        frames[super::FRAMES - 1] = frame(EVAL, 11, 6);
        let started = std::time::Instant::now();
        unchanged(&text, &frames);
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    // Each start is tried against the detailed frames alone, so a message of
    // many lines that look like frames costs a pass per line, not the run.
    #[test]
    fn passes_over_a_long_message_of_frame_lines() {
        let message = "    at x (native)\n    at y (<eval>:1:1)\n".repeat(50_000);
        let text = format!("Error: log\n{message}    at Ue (<eval>:11:5)\n    at <eval>:12:1");
        let frames = [frame(EVAL, 11, 5), frame(EVAL, 12, 1)];
        assert_eq!(
            rewritten(&text, &frames, OWN_LINE),
            format!("Error: log\n{message}    at Ue ({HEADER}:41:13)\n    at {APP}:10:5")
        );
    }

    #[test]
    fn leaves_text_from_a_user_prepare_stack_trace_alone() {
        let frames = [frame(EVAL, 11, 5), frame(EVAL, 12, 1)];
        unchanged("Error: boom\n  > Ue@<eval>:11:5\n  > <eval>:12:1", &frames);
        unchanged(r#"[{"file":"<eval>","line":11,"column":5}]"#, &frames);
        unchanged("prepared:2", &frames);
        // As source-map-support writes a stack it has mapped already.
        unchanged(
            &format!("Error: boom\n    at Ue ({HEADER}:41:13)\n    at {APP}:10:5"),
            &frames,
        );
    }

    #[test]
    fn leaves_the_text_alone_on_any_mismatch() {
        let text = "RangeError: Maximum call stack size exceeded\n    at f (<eval>:11:5)\n    at f (<eval>:11:5)";
        // V8 gives a stack overflow's frames before it has their positions.
        unchanged(text, &[frame(EVAL, 1, 1), frame(EVAL, 1, 1)]);
        unchanged(text, &[frame(EVAL, 11, 6), frame(EVAL, 11, 5)]);
        // A matched frame whose line names another script.
        unchanged(
            "Error: x\n    at Ue (bundle.js:11:5)",
            &[frame(EVAL, 11, 5)],
        );
        unchanged("Error: x\n    at Ue(<eval>:11:5)", &[frame(EVAL, 11, 5)]);
        unchanged("Error: x\n    at Ue (<eval>:11:5", &[frame(EVAL, 11, 5)]);
    }

    #[test]
    fn rewrites_identical_frames_of_a_recursion() {
        let text = "RangeError: Maximum call stack size exceeded\n    at f (<eval>:11:31)\n    at f (<eval>:11:31)\n    at f (<eval>:11:31)";
        let frames = [frame(EVAL, 11, 31); 3];
        assert_eq!(
            rewritten(text, &frames, OWN_LINE),
            format!(
                "RangeError: Maximum call stack size exceeded{}",
                format!("\n    at f ({HEADER}:51:3)").repeat(3)
            )
        );
    }

    #[test]
    fn leaves_frames_the_map_does_not_map() {
        let text = "Error: x\n    at prefix (<eval>:3:1)\n    at unmapped (<eval>:13:1)\n    at after (<eval>:14:1)\n    at Ue (<eval>:11:5)";
        let frames = [
            frame(EVAL, 3, 1),
            frame(EVAL, 13, 1),
            frame(EVAL, 14, 1),
            frame(EVAL, 11, 5),
        ];
        assert_eq!(
            rewritten(text, &frames, OWN_LINE),
            format!(
                "Error: x\n    at prefix (<eval>:3:1)\n    at unmapped (<eval>:13:1)\n    at after (<eval>:14:1)\n    at Ue ({HEADER}:41:13)"
            )
        );
    }

    #[test]
    fn maps_a_bundle_that_starts_mid_line() {
        let placement = Placement {
            line: 10,
            column: 17,
        };
        let text =
            "Error: x\n    at prefix (<eval>:11:10)\n    at Ue (<eval>:11:22)\n    at <eval>:12:1";
        let frames = [frame(EVAL, 11, 10), frame(EVAL, 11, 22), frame(EVAL, 12, 1)];
        assert_eq!(
            rewritten(text, &frames, placement),
            format!(
                "Error: x\n    at prefix (<eval>:11:10)\n    at Ue ({HEADER}:41:13)\n    at {APP}:10:5"
            )
        );
    }

    // V8 places a WebAssembly frame on line 1 at its byte offset plus one.
    #[test]
    fn pairs_webassembly_frames() {
        let text = "Error: under wasm\n    at jsf (<eval>:11:5)\n    at wasm://wasm/26fa11b2:wasm-function[1]:0x29\n    at $g (wasm://wasm/26fa11b2:wasm-function[2]:0x30)\n    at <eval>:12:1";
        let frames = [
            frame(EVAL, 11, 5),
            frame(23, 1, 42),
            frame(23, 1, 49),
            frame(EVAL, 12, 1),
        ];
        assert_eq!(
            rewritten(text, &frames, OWN_LINE),
            format!(
                "Error: under wasm\n    at jsf ({HEADER}:41:13)\n    at wasm://wasm/26fa11b2:wasm-function[1]:0x29\n    at $g (wasm://wasm/26fa11b2:wasm-function[2]:0x30)\n    at {APP}:10:5"
            )
        );
        unchanged(text, &[frame(EVAL, 11, 5), frame(23, 1, 41)]);
    }

    #[test]
    fn leaves_text_alone_without_frames_or_matches() {
        let text = "Error: x\n    at Ue (<eval>:11:5)";
        unchanged(text, &[]);
        unchanged("plain message", &[frame(EVAL, 11, 5)]);
        let frames = [frame(EVAL, 11, 5)];
        assert!(matches!(rewrite(text, &frames, &[]), Cow::Borrowed(same) if same == text));
        // A frame of a script with no bundle pairs but changes nothing.
        unchanged(text, &[frame(EVAL + 1, 11, 5)]);
    }
}
