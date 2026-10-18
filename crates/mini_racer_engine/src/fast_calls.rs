/// A value an eval source carries as a literal, which a fast call hands its
/// template instead of compiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hoisted<'a> {
    /// JSON text for an array or object, which V8's JSON parser decides on.
    Json(&'a [u8]),
    /// What a single-quoted string literal with no escapes holds.
    Text(&'a [u8]),
}

/// A source split around the literals of the values it carries: the source
/// is `around[0]`, the first value's literal, `around[1]`, and so on.
#[derive(Debug, PartialEq, Eq)]
pub struct Shape<'a> {
    pub around: Vec<&'a [u8]>,
    pub values: Vec<Hoisted<'a>>,
    /// The code a render runs before and after its call. The template reads
    /// its values as the source would only if the call runs once, at the
    /// wrapper's top level, so each must leave the call alone
    /// (`leaves_the_call_alone`) and compile alone as a function body.
    pub code: Vec<&'a [u8]>,
}

impl Shape<'_> {
    /// The source with each value's literal replaced by `<reader>(<index>)`,
    /// unless the source already names `reader`.
    pub fn template(&self, reader: &[u8]) -> Option<Vec<u8>> {
        let mut text = Vec::new();
        for (index, piece) in self.around.iter().enumerate() {
            if find(piece, reader).is_some() {
                return None;
            }
            if let Some(value) = index.checked_sub(1) {
                text.extend_from_slice(reader);
                text.extend_from_slice(format!("({value})").as_bytes());
            }
            text.extend_from_slice(piece);
        }
        Some(text)
    }
}

/// The shape of `source` if it is, in UTF-8, one of the two evals sent once
/// per call: `ExecJS` 2.10.2's `call`, `(<path>.apply(this, <JSON array>))`
/// with `<path>` ASCII identifiers joined by dots, or a react-rails 3.2.1 or
/// 3.3.1 render, `(` and `compose_js`'s wrapper around its one
/// `this.ReactRailsUJS.serverRender('<function>', '<component>', <props>)`
/// call, with JSON props on one line. Anything else is `None`.
pub fn recognize(source: &[u8]) -> Option<Shape<'_>> {
    call(source)
        .or_else(|| render(source))
        .filter(|_| std::str::from_utf8(source).is_ok())
}

const APPLY: &[u8] = b".apply(this, ";

fn call(source: &[u8]) -> Option<Shape<'_>> {
    let inner = source.strip_prefix(b"(")?.strip_suffix(b"))")?;
    let mut path = 0;
    let args = loop {
        path += identifier_len(inner.get(path..)?)?;
        let rest = inner.get(path..)?;
        if let Some(args) = rest.strip_prefix(APPLY) {
            break args;
        }
        if rest.first() != Some(&b'.') {
            return None;
        }
        path += 1;
    };
    if args.first() != Some(&b'[') || json_extent(args)? != args.len() {
        return None;
    }
    let args_at = 1 + path + APPLY.len();
    Some(Shape {
        around: vec![source.get(..args_at)?, source.get(args_at + args.len()..)?],
        values: vec![Hoisted::Json(args)],
        code: Vec::new(),
    })
}

fn identifier_len(text: &[u8]) -> Option<usize> {
    let name = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$';
    if text
        .first()
        .is_none_or(|first| first.is_ascii_digit() || !name(*first))
    {
        return None;
    }
    Some(
        text.iter()
            .position(|&byte| !name(byte))
            .unwrap_or(text.len()),
    )
}

// react-rails' `compose_js` heredoc, indentation and all, inside ExecJS's
// parentheses; its `before` and `after` code sits between these.
const OPEN: &[u8] = b"(          (function () {\n            ";
const CALL: &[u8] = b"\n            var result = this.ReactRailsUJS.serverRender(";
const RESULT: &[u8] = b");\n            ";
const CLOSE: &[u8] = b"\n            return result;\n          })()\n)";
const SERVER_RENDER: &[u8] = b"ReactRailsUJS.serverRender";

fn render(source: &[u8]) -> Option<Shape<'_>> {
    let body = source.strip_prefix(OPEN)?.strip_suffix(CLOSE)?;
    let call = find(body, CALL)?;
    let before = body.get(..call)?;
    if find(before, SERVER_RENDER).is_some() {
        return None;
    }
    let function_at = OPEN.len() + call + CALL.len();
    let function = literal(source.get(function_at..)?)?;
    let component_at = function_at + function.len() + 4;
    let component = literal(source.get(component_at..)?)?;
    let props_at = component_at + component.len() + 4;
    let props_end = props_at + json_extent(source.get(props_at..)?)?;
    let tail = source.get(props_end..)?;
    let after = tail.strip_prefix(RESULT)?.strip_suffix(CLOSE)?;
    let between = [
        source.get(function_at + function.len() + 2..component_at)?,
        source.get(component_at + component.len() + 2..props_at)?,
    ];
    let props = source.get(props_at..props_end)?;
    if between != [b", ", b", "] || find(after, SERVER_RENDER).is_some() || breaks_line(props) {
        return None;
    }
    Some(Shape {
        around: vec![source.get(..function_at)?, between[0], between[1], tail],
        values: vec![
            Hoisted::Text(function),
            Hoisted::Text(component),
            Hoisted::Json(props),
        ],
        code: vec![before, after],
    })
}

/// Whether a render's `code` can neither run its call again away from its
/// values nor share what V8 keeps per script between renders: it holds no
/// `arguments` other than as a property name (its `callee` is the wrapper),
/// no `eval` or `caller`, no escape that could spell one of them, and no
/// template literal, as V8 keeps a tagged one's strings per script.
pub fn leaves_the_call_alone(code: &[u8]) -> bool {
    !(code.contains(&b'`')
        || find(code, b"\\u").is_some()
        || names(code, b"eval", false)
        || names(code, b"caller", false)
        || names(code, b"arguments", true))
}

// Whether `code` holds `name` as a whole word, unless `as_property` lets one
// read as a property after a single `.` pass. A byte past ASCII never joins a
// word, as V8 takes some of those for spaces.
fn names(code: &[u8], name: &[u8], as_property: bool) -> bool {
    let joins = |byte: &u8| byte.is_ascii_alphanumeric() || *byte == b'_' || *byte == b'$';
    let mut from = 0;
    while let Some(at) = code.get(from..).and_then(|rest| find(rest, name)) {
        let at = from + at;
        from = at + 1;
        let start = at.checked_sub(1).and_then(|before| code.get(before));
        if start.is_some_and(joins) || code.get(at + name.len()).is_some_and(joins) {
            continue;
        }
        let mut before = code.get(..at).unwrap_or_default().iter().rev();
        let property = before.find(|byte| !byte.is_ascii_whitespace()) == Some(&b'.')
            && before.next() != Some(&b'.');
        if !(as_property && property) {
            return true;
        }
    }
    false
}

// A string literal that holds its own text: no escapes, and no line
// terminator, which would move every line after it.
fn literal(text: &[u8]) -> Option<&[u8]> {
    let text = text.strip_prefix(b"'")?;
    let content = text.get(..text.iter().position(|&byte| byte == b'\'')?)?;
    (!content.contains(&b'\\') && !breaks_line(content)).then_some(content)
}

// V8 numbers lines at U+2028 and U+2029 as well, inside literals too.
fn breaks_line(text: &[u8]) -> bool {
    text.iter().enumerate().any(|(at, &byte)| {
        byte == b'\n'
            || byte == b'\r'
            || (byte == 0xE2 && matches!(text.get(at + 1..at + 3), Some([0x80, 0xA8 | 0xA9])))
    })
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

// V8's JSON parser keeps its own stack where its JavaScript parser recurses,
// so a value nested deeper than this takes the normal path, which fails on it
// the way mini_racer does once the nesting overflows the stack.
const MAX_DEPTH: usize = 128;

// The length of the JSON array or object `text` starts with, if it nests no
// deeper than `MAX_DEPTH` and cannot name a `__proto__` key, which an object
// literal makes its prototype and `JSON.parse` an own property. It reads
// `text` as JSON; V8's parser decides whether it is, though a byte JSON never
// has outside a string ends it here, cheaper than V8's error.
fn json_extent(text: &[u8]) -> Option<usize> {
    if !matches!(text.first(), Some(b'[' | b'{')) {
        return None;
    }
    let mut depth = 0_usize;
    let mut at = 0;
    while let Some(&byte) = text.get(at) {
        match CLASSES[usize::from(byte)] {
            Class::Quote => {
                at = string_end(text, at + 1)?;
                continue;
            }
            Class::Open => {
                depth += 1;
                if depth > MAX_DEPTH {
                    return None;
                }
            }
            Class::Close => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(at + 1);
                }
            }
            Class::Plain => {}
            Class::Other => return None,
        }
        at += 1;
    }
    None
}

#[derive(Clone, Copy)]
enum Class {
    Quote,
    Open,
    Close,
    // Whitespace, `:`, `,`, what numbers are written with, and the letters of
    // `true`, `false` and `null`.
    Plain,
    Other,
}

const CLASSES: [Class; 256] = {
    let mut classes = [Class::Other; 256];
    let plain = b" \t\n\r:,0123456789-+.Eaeflnrstu";
    let mut at = 0;
    while at < plain.len() {
        classes[plain[at] as usize] = Class::Plain;
        at += 1;
    }
    classes[b'"' as usize] = Class::Quote;
    classes[b'[' as usize] = Class::Open;
    classes[b'{' as usize] = Class::Open;
    classes[b']' as usize] = Class::Close;
    classes[b'}' as usize] = Class::Close;
    classes
};

// The index after the closing quote of the string whose contents start at
// `at`, unless it holds `__proto__` or an escaped letter, digit, `_` or `$`,
// which could spell it.
fn string_end(text: &[u8], mut at: usize) -> Option<usize> {
    let mut plain = 0;
    loop {
        // Most strings are short keys, which a word at a time only slows.
        if plain == 16 {
            at = plain_run(text, at);
            plain = 0;
        }
        match *text.get(at)? {
            b'"' => return Some(at + 1),
            b'\\' if text.get(at + 1) == Some(&b'u') => {
                let unit = std::str::from_utf8(text.get(at + 2..at + 6)?).ok()?;
                let unit = u8::try_from(u32::from_str_radix(unit, 16).ok()?).unwrap_or(0x80);
                if unit.is_ascii_alphanumeric() || unit == b'_' || unit == b'$' {
                    return None;
                }
                at += 6;
            }
            b'\\' => at += 2,
            b'_' if text
                .get(at..)
                .is_some_and(|rest| rest.starts_with(b"__proto__")) =>
            {
                return None;
            }
            _ => {
                at += 1;
                plain += 1;
            }
        }
    }
}

// Past eight bytes at a time that hold no quote, backslash or `_`: most of a
// long string, read a word at a time.
fn plain_run(text: &[u8], mut at: usize) -> usize {
    const ONES: u64 = u64::from_ne_bytes([1; 8]);
    const HIGHS: u64 = u64::from_ne_bytes([0x80; 8]);
    let holds = |word: u64, byte: u8| {
        let matched = word ^ (ONES * u64::from(byte));
        matched.wrapping_sub(ONES) & !matched & HIGHS != 0
    };
    while let Some(word) = text
        .get(at..at + 8)
        .and_then(|bytes| <[u8; 8]>::try_from(bytes).ok())
        .map(u64::from_ne_bytes)
    {
        if holds(word, b'"') || holds(word, b'\\') || holds(word, b'_') {
            break;
        }
        at += 8;
    }
    at
}

#[cfg(test)]
mod tests {
    use super::{Hoisted, MAX_DEPTH, Shape, leaves_the_call_alone, recognize};

    // Code a renderer runs around each render, as react-rails' console
    // replay does.
    const BEFORE: &str = "var logged = [];\nconsole.history = logged;\n";
    const AFTER: &str = "if (logged.length > 0) {\n  result += \"\\n<!--\" + \
        logged.map(function (entry) {\n    \
        return \" \" + entry.level + \" \" + JSON.stringify(entry.arguments);\n  \
        }).join(\"\") + \" -->\";\n}\n";

    // What react-rails 3.2.1 and 3.3.1 hand ExecJS's eval, which wraps it in
    // parentheses.
    fn compose_js(before: &str, main: &str, after: &str) -> String {
        format!(
            "(          (function () {{\n            {before}\n            var result = {main};\n            \
             {after}\n            return result;\n          }})()\n)"
        )
    }

    fn render(function: &str, component: &str, props: &str) -> String {
        compose_js(
            BEFORE,
            &format!("this.ReactRailsUJS.serverRender('{function}', '{component}', {props})"),
            AFTER,
        )
    }

    fn values(source: &str) -> Option<Vec<Hoisted<'_>>> {
        recognize(source.as_bytes()).map(|shape| shape.values)
    }

    fn json(text: &str) -> Hoisted<'_> {
        Hoisted::Json(text.as_bytes())
    }

    fn text(text: &str) -> Hoisted<'_> {
        Hoisted::Text(text.as_bytes())
    }

    fn nested(depth: usize) -> String {
        format!("{}{}", "[".repeat(depth), "]".repeat(depth))
    }

    #[test]
    fn an_execjs_call_hoists_its_arguments() {
        let source = "(add.apply(this, [1,\"two\",{\"3\":null}]))";
        assert_eq!(
            recognize(source.as_bytes()),
            Some(Shape {
                around: vec![b"(add.apply(this, ", b"))"],
                values: vec![json("[1,\"two\",{\"3\":null}]")],
                code: vec![],
            })
        );
    }

    #[test]
    fn a_call_names_its_function_by_a_dotted_path() {
        for path in [
            "a.b.c",
            "$_.x9",
            "_",
            "$",
            "foo.apply",
            "this.add",
            "arguments",
        ] {
            let source = format!("({path}.apply(this, []))");
            let shape = recognize(source.as_bytes());
            assert_eq!(
                shape.map(|shape| shape.around[0].to_vec()),
                Some(format!("({path}.apply(this, ").into_bytes()),
                "{path}"
            );
        }
    }

    #[test]
    fn a_call_template_reads_its_arguments_back() {
        let shape = recognize(b"(add.apply(this, [1]))");
        assert_eq!(
            shape.and_then(|shape| shape.template(b"read")),
            Some(b"(add.apply(this, read(0)))".to_vec())
        );
    }

    #[test]
    fn a_source_that_names_the_reader_has_no_template() {
        for source in ["(read.apply(this, [1]))", "(a.read.apply(this, [1]))"] {
            let shape = recognize(source.as_bytes());
            assert!(shape.is_some(), "{source}");
            assert_eq!(shape.and_then(|shape| shape.template(b"read")), None);
        }
    }

    #[test]
    fn near_misses_of_a_call_are_not_recognized() {
        for source in [
            " (add.apply(this, [1]))",
            "( add.apply(this, [1]))",
            "(add .apply(this, [1]))",
            "(add.apply( this, [1]))",
            "(add.apply(this,  [1]))",
            "(add.apply(this,[1]))",
            "(add.apply(this, [1]) )",
            "(add.apply(this, [1]));",
            "add.apply(this, [1])",
            "(add.call(this, [1]))",
            "(add.apply(globalThis, [1]))",
            "(a[0].apply(this, [1]))",
            "(a['b'].apply(this, [1]))",
            "(a..b.apply(this, [1]))",
            "(.a.apply(this, [1]))",
            "(9a.apply(this, [1]))",
            "(\u{e4}.apply(this, [1]))",
            "(f().apply(this, [1]))",
            "(add.apply(this, \"[1]\"))",
            "(add.apply(this, {\"a\":1}))",
            "(add.apply(this, [1]) + add.apply(this, [2]))",
            "(add.apply(this, [1] ))",
            "(add.apply(this, [1]\n))",
            "(add.apply(this, [1))",
            "(add.apply(this, [\"1]))",
        ] {
            assert_eq!(values(source), None, "{source:?}");
        }
    }

    #[test]
    fn brackets_and_parentheses_in_strings_do_not_end_the_arguments() {
        let source = "(f.apply(this, [\"])\\\"[\",\"))\"]))";
        assert_eq!(values(source), Some(vec![json("[\"])\\\"[\",\"))\"]")]));
    }

    #[test]
    fn a_proto_key_in_any_spelling_takes_the_normal_path() {
        for args in [
            "[{\"__proto__\":{}}]",
            "[\"__proto__\"]",
            "[{\"\\u005f_proto__\":1}]",
            "[{\"_\\u005Fproto__\":1}]",
            "[{\"__\\u0070roto__\":1}]",
            "[{\"__pr\\u006Fto__\":1}]",
        ] {
            assert_eq!(values(&format!("(f.apply(this, {args}))")), None, "{args}");
        }
        let escaped = "[\"\\u003c\\u2028\\\\u005f\\\"\"]";
        assert_eq!(
            values(&format!("(f.apply(this, {escaped}))")),
            Some(vec![json(escaped)])
        );
    }

    #[test]
    fn nesting_past_the_limit_takes_the_normal_path() {
        let deepest = nested(MAX_DEPTH);
        let source = format!("(f.apply(this, {deepest}))");
        assert_eq!(values(&source), Some(vec![json(&deepest)]));
        assert_eq!(
            values(&format!("(f.apply(this, {}))", nested(MAX_DEPTH + 1))),
            None
        );
    }

    #[test]
    fn a_source_that_is_not_utf8_is_not_recognized() {
        assert_eq!(recognize(b"(f.apply(this, [\"\xff\"]))"), None);
    }

    #[test]
    fn a_react_rails_render_hoists_its_function_component_and_props() {
        let source = render(
            "renderToString",
            "Components.Hello",
            "{\"name\":\"W\u{e9}\"}",
        );
        let shape = recognize(source.as_bytes());
        assert_eq!(
            shape.as_ref().map(|shape| shape.values.clone()),
            Some(vec![
                text("renderToString"),
                text("Components.Hello"),
                json("{\"name\":\"W\u{e9}\"}"),
            ])
        );
        let expected = compose_js(
            BEFORE,
            "this.ReactRailsUJS.serverRender(read(0), read(1), read(2))",
            AFTER,
        );
        assert_eq!(
            shape.and_then(|shape| shape.template(b"read")),
            Some(expected.into_bytes())
        );
    }

    #[test]
    fn a_render_with_no_code_around_its_call_is_recognized() {
        let source = compose_js(
            "",
            "this.ReactRailsUJS.serverRender('renderToStaticMarkup', 'A', [])",
            "",
        );
        assert_eq!(
            values(&source),
            Some(vec![text("renderToStaticMarkup"), text("A"), json("[]")])
        );
    }

    #[test]
    fn a_renders_code_is_what_runs_before_and_after_its_call() {
        let source = render("renderToString", "A", "{}");
        let shape = recognize(source.as_bytes());
        assert_eq!(
            shape.map(|shape| shape.code),
            Some(vec![BEFORE.as_bytes(), AFTER.as_bytes()])
        );
        assert_eq!(
            recognize(b"(f.apply(this, []))").map(|shape| shape.code),
            Some(vec![])
        );
    }

    #[test]
    fn code_that_could_reach_past_the_call_does_not_leave_it_alone() {
        for code in [
            "var s = `x`;",
            "eval('1');",
            "var f = window.eval;",
            "f.caller;",
            "var a = arguments;",
            "arguments.callee;",
            "f(...arguments);",
            "x = (arguments);",
            "\\u0061rguments;",
            "ev\\u0061l('1');",
            "\u{a0}arguments;",
        ] {
            assert!(!leaves_the_call_alone(code.as_bytes()), "{code}");
        }
    }

    #[test]
    fn longer_names_and_properties_called_arguments_leave_the_call_alone() {
        for code in [
            BEFORE,
            AFTER,
            "msg.arguments;",
            "msg . arguments;",
            "myarguments;",
            "arguments_;",
            "$eval();",
            "evaluate();",
            "callers;",
            "'\\n';",
        ] {
            assert!(leaves_the_call_alone(code.as_bytes()), "{code}");
        }
    }

    #[test]
    fn near_misses_of_a_render_are_not_recognized() {
        let call = "this.ReactRailsUJS.serverRender('renderToString', 'A', {})";
        let second = "this.ReactRailsUJS.serverRender('renderToString', 'B', {});";
        let sources = [
            compose_js(BEFORE, call, AFTER).replacen("(          (", "(         (", 1),
            compose_js(BEFORE, call, AFTER).replacen("var result = ", "var result  = ", 1),
            compose_js(BEFORE, call, AFTER).replacen("return result;\n", "return result; \n", 1),
            compose_js(BEFORE, call, AFTER).replacen("{});", "{} );", 1),
            compose_js(BEFORE, call, AFTER).replacen("'A', {}", "'A',{}", 1),
            compose_js(BEFORE, call, AFTER)
                .trim_end_matches(')')
                .to_owned(),
            compose_js(second, call, AFTER),
            compose_js(BEFORE, call, second),
            compose_js(BEFORE, &format!("{call} + {call}"), AFTER),
            compose_js(
                BEFORE,
                "ReactRailsUJS.serverRender('renderToString', 'A', {})",
                AFTER,
            ),
            render("renderToString", "He'llo", "{}"),
            render("renderToString", "A\\u0041", "{}"),
            render("renderToString", "A\u{2028}", "{}"),
            render("render\nToString", "A", "{}"),
            render("renderToString", "A", "{name: 'js'} || {}"),
            render("renderToString", "A", "null"),
            render("renderToString", "A", "\"{}\""),
            render("renderToString", "A", "{\n  \"n\": 4\n}"),
            render("renderToString", "A", "{\"n\":\"\u{2029}\"}"),
            render("renderToString", "A", "{\"__proto__\":{}}"),
        ];
        for source in &sources {
            assert_eq!(values(source), None, "{source:?}");
        }
    }

    #[test]
    fn props_with_a_byte_json_keeps_in_strings_take_the_normal_path() {
        for props in [
            "{name: 'js'}",
            "{\"a\":undefined}",
            "{\"a\":1}/*x*/",
            "{\"a\":`b`}",
        ] {
            assert_eq!(
                values(&render("renderToString", "A", props)),
                None,
                "{props}"
            );
        }
    }

    // Whether props are JSON at all is V8's to decide.
    #[test]
    fn props_of_json_bytes_are_left_to_the_json_parser() {
        let source = render("renderToString", "A", "{n: true}");
        assert_eq!(
            values(&source),
            Some(vec![text("renderToString"), text("A"), json("{n: true}")])
        );
    }

    // Past its first 16 bytes a string is read eight bytes at a time, so each
    // byte that matters is tried before that and at every offset into a word.
    #[test]
    fn quotes_escapes_and_proto_are_found_at_any_offset_in_a_string() {
        for offset in 0..48 {
            let pad = "x".repeat(offset);
            let plain = format!("[\"{pad}\",\"{pad}\\\"]\"]");
            let source = format!("(f.apply(this, {plain}))");
            assert_eq!(values(&source), Some(vec![json(&plain)]), "{offset}");
            for args in [
                format!("[{{\"{pad}__proto__\":1}}]"),
                format!("[\"{pad}\\u005f\"]"),
                format!("[\"{pad}]"),
            ] {
                assert_eq!(values(&format!("(f.apply(this, {args}))")), None, "{args}");
            }
        }
    }
}
