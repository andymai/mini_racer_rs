use std::borrow::Cow;
use std::cell::RefCell;
use std::fmt;

use memchr::memmem::Finder;
use serde::Deserialize;
use serde::de::{self, DeserializeSeed, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};

use crate::vlq;

// A map is input from the application, so what it may hold is bounded: well
// past the largest measured (MUI's 8 MB bundle maps 118 thousand lines with
// 410 thousand segments), and short of letting a map of tens of megabytes
// make one hold gigabytes. A segment kept takes 16 bytes.
const MAX_LINES: usize = 1 << 22;
const MAX_SEGMENTS: usize = 1 << 22;
const MAX_SOURCES: usize = 1 << 20;
const MAX_NAMES: usize = 1 << 22;
// A source and the source root, each, in bytes: a rewritten frame prints one
// of each.
const MAX_URL: usize = 1 << 16;
// The map whole, before it is read: the bound the process already puts on
// the large sources and code caches it keeps, twenty times the largest map
// measured, sourcesContent included.
const MAX_MAP: usize = 256 << 20;

#[derive(Debug, thiserror::Error)]
pub enum MapError {
    #[error("the bundle is empty")]
    EmptyBundle,
    #[error("the source map is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("the source map is not a JSON object")]
    NotAnObject,
    #[error("index source maps (with sections) are not supported")]
    IndexMap,
    #[error("the source map's version is not 3")]
    Version,
    #[error("the source map has no {0}")]
    Missing(&'static str),
    #[error("the source map's mappings are malformed at byte {0}")]
    Mappings(usize),
    #[error(
        "the source map's mappings refer to source {index}, outside its {count} sources, at byte {at}"
    )]
    Source { index: i64, count: usize, at: usize },
    #[error(
        "the source map's mappings refer to name {index}, outside its {count} names, at byte {at}"
    )]
    Name { index: i64, count: usize, at: usize },
    #[error("the source map maps line {line}, but the bundle has {lines} lines")]
    Lines { line: usize, lines: usize },
    #[error(
        "the source map maps column {column} of line {line}, past the line's {length} UTF-16 units"
    )]
    Column {
        line: usize,
        column: u32,
        length: u32,
    },
    #[error("there are more than {limit} {what}")]
    Limit { what: &'static str, limit: usize },
    #[error("a source or the source root of the source map holds a control character")]
    Control,
    #[error("the bundle or its source map is too large")]
    TooLarge,
}

// The fields a map is read for; `sourcesContent` and every other field are
// skipped as they are read, keeping none of it. Each is optional here so that
// an index map is told apart before the fields it lacks, and a field given
// twice keeps its last value, as `JSON.parse` does.
#[derive(Default)]
struct Raw<'a> {
    version: Option<f64>,
    sections: bool,
    source_root: Option<String>,
    sources: Option<Vec<Option<String>>>,
    names: usize,
    mappings: Option<Cow<'a, str>>,
}

// serde borrows a string from the input only for a field typed `Cow<str>`
// itself, so the mappings, a few megabytes, are read in place through this.
#[derive(Deserialize)]
struct Text<'a>(#[serde(borrow)] Cow<'a, str>);

// A read stopped at a cap or a control character carries only serde's own
// error out, so the reason waits here.
type Refusal = RefCell<Option<MapError>>;

fn refuse<E: de::Error>(refusal: &Refusal, error: MapError) -> E {
    let text = error.to_string();
    *refusal.borrow_mut() = Some(error);
    E::custom(text)
}

#[derive(Clone, Copy)]
struct Fields<'r>(&'r Refusal);

impl<'de> Visitor<'de> for Fields<'_> {
    type Value = Raw<'de>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a source map object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Raw<'de>, A::Error> {
        let mut raw = Raw::default();
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "version" => raw.version = map.next_value()?,
                // ECMA-426 reads a map with `sections` as an index map, even
                // a null one.
                "sections" => {
                    map.next_value::<IgnoredAny>()?;
                    raw.sections = true;
                }
                "sourceRoot" => raw.source_root = map.next_value_seed(Url(self.0))?,
                "sources" => raw.sources = Some(map.next_value_seed(Sources(self.0))?),
                "names" => raw.names = map.next_value_seed(Names(self.0))?,
                "mappings" => raw.mappings = Some(map.next_value::<Text<'de>>()?.0),
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(raw)
    }
}

// Each source as it arrives, so the count and each length stop the read
// before the rest is kept.
struct Sources<'r>(&'r Refusal);

impl<'de> DeserializeSeed<'de> for Sources<'_> {
    type Value = Vec<Option<String>>;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for Sources<'_> {
    type Value = Vec<Option<String>>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an array of sources")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut sources = Vec::new();
        while let Some(source) = seq.next_element_seed(Url(self.0))? {
            if sources.len() >= MAX_SOURCES {
                let limit = MapError::Limit {
                    what: "sources in the source map",
                    limit: MAX_SOURCES,
                };
                return Err(refuse(self.0, limit));
            }
            sources.push(source);
        }
        Ok(sources)
    }
}

struct Names<'r>(&'r Refusal);

impl<'de> DeserializeSeed<'de> for Names<'_> {
    type Value = usize;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<usize, D::Error> {
        deserializer.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for Names<'_> {
    type Value = usize;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an array of names")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<usize, A::Error> {
        let mut count = 0_usize;
        while seq.next_element::<IgnoredAny>()?.is_some() {
            if count >= MAX_NAMES {
                let limit = MapError::Limit {
                    what: "names in the source map",
                    limit: MAX_NAMES,
                };
                return Err(refuse(self.0, limit));
            }
            count = count.saturating_add(1);
        }
        Ok(count)
    }
}

// A source or the root: a string or null, printed into a backtrace line, so
// bounded and free of anything that would add a line or disguise one.
#[derive(Clone, Copy)]
struct Url<'r>(&'r Refusal);

impl Url<'_> {
    fn checked<E: de::Error>(self, text: &str) -> Result<Option<String>, E> {
        if text.len() > MAX_URL {
            let limit = MapError::Limit {
                what: "bytes in a source or the source root",
                limit: MAX_URL,
            };
            return Err(refuse(self.0, limit));
        }
        if text.chars().any(unprintable) {
            return Err(refuse(self.0, MapError::Control));
        }
        Ok(Some(text.to_owned()))
    }
}

impl<'de> DeserializeSeed<'de> for Url<'_> {
    type Value = Option<String>;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_option(self)
    }
}

impl<'de> Visitor<'de> for Url<'_> {
    type Value = Option<String>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a string or null")
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(None)
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_str(self)
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<Self::Value, E> {
        self.checked(text)
    }
}

// Control characters, the line and paragraph separators, and the bidi
// embeddings, overrides and isolates.
fn unprintable(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{2028}' | '\u{2029}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

const UNMAPPED: u32 = u32::MAX;

#[derive(Clone, Copy, Debug)]
struct Segment {
    column: u32,
    source: u32,
    line: u32,
    original_column: u32,
}

// A line break V8 counts and a map's generator does not (a lone CR, U+2028
// or U+2029): the V8 line it starts, zero-based in the bundle, and the column
// of the map's line it starts at.
#[derive(Clone, Copy, Debug)]
struct Break {
    line: u32,
    column: u32,
}

/// Where a bundle starts in a script that holds it: V8's line, zero-based,
/// and its column in UTF-16 units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub line: u32,
    pub column: u32,
}

/// A position in an original source, whose name is `root` then `source`;
/// `line` and `column` are zero-based, as in the map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Original<'a> {
    pub root: &'a str,
    pub source: &'a str,
    pub line: u32,
    pub column: u32,
}

/// A bundle with its parsed source map, checked against each other.
pub struct SourceMap {
    // The bundle's first bytes, and the bundle whole, its only copy, for a
    // search linear however it repeats itself; built here, as a forked child
    // may share the map and must never wait on one built on demand.
    head: Finder<'static>,
    whole: Finder<'static>,
    // Joined to each source when one is printed, so a long root costs once.
    root: Box<str>,
    sources: Box<[Option<Box<str>>]>,
    // Where each generated line's segments start, then where the last ends.
    starts: Box<[u32]>,
    segments: Box<[Segment]>,
    breaks: Box<[Break]>,
    // The bundle's last V8 line and its length in UTF-16 units.
    end: Placement,
}

// The bundle's first bytes, which a search looks for before comparing it
// whole.
const HEAD: usize = 64;

/// Refuses a map past 256 MiB by its length alone, before it is copied or
/// read.
pub const fn check_length(length: usize) -> Result<(), MapError> {
    within("bytes in the source map", length, MAX_MAP)
}

impl SourceMap {
    /// Parses `map`, a regular version 3 source map, for `bundle`. Its
    /// generated lines are counted at each LF, as source map generators do;
    /// the lines V8 counts also at a lone CR, U+2028 and U+2029 are mapped
    /// back to the map's. The work is linear in the bytes of both.
    pub fn new(bundle: &str, map: &[u8]) -> Result<Self, MapError> {
        if bundle.is_empty() {
            return Err(MapError::EmptyBundle);
        }
        check_length(map.len())?;
        // serde would read a struct from a JSON array too.
        let first = map
            .iter()
            .find(|byte| !matches!(byte, b' ' | b'\t' | b'\n' | b'\r'));
        if first != Some(&b'{') {
            return Err(MapError::NotAnObject);
        }
        let refusal = Refusal::default();
        let mut json = serde_json::Deserializer::from_slice(map);
        let raw = match json
            .deserialize_map(Fields(&refusal))
            .and_then(|raw| json.end().map(|()| raw))
        {
            Ok(raw) => raw,
            Err(error) => return Err(refusal.into_inner().unwrap_or(MapError::Json(error))),
        };
        if raw.sections {
            return Err(MapError::IndexMap);
        }
        if raw.version != Some(3.0) {
            return Err(MapError::Version);
        }
        let Some(mappings) = raw.mappings else {
            return Err(MapError::Missing("mappings"));
        };
        let Some(listed) = raw.sources else {
            return Err(MapError::Missing("sources"));
        };
        let (starts, segments) = decode(mappings.as_bytes(), listed.len(), raw.names)?;
        let shape = Shape::of(bundle, starts.len().saturating_sub(1))?;
        check(&starts, &segments, &shape)?;
        // Joined with a slash unless it ends in one, as ECMA-426 joins them,
        // but an empty root, which generators write for none, is none here
        // where ECMA-426 would still add the slash.
        let root = match raw.source_root.as_deref() {
            None | Some("") => String::new(),
            Some(root) if root.ends_with('/') => root.to_owned(),
            Some(root) => format!("{root}/"),
        };
        let head = bundle
            .as_bytes()
            .get(..HEAD.min(bundle.len()))
            .unwrap_or_default();
        Ok(Self {
            head: Finder::new(head).into_owned(),
            whole: Finder::new(bundle.as_bytes()).into_owned(),
            root: root.into_boxed_str(),
            sources: listed
                .into_iter()
                .map(|source| source.map(String::into_boxed_str))
                .collect(),
            starts: starts.into_boxed_slice(),
            segments: segments.into_boxed_slice(),
            breaks: shape.breaks.into_boxed_slice(),
            end: shape.end,
        })
    }

    /// The bundle's bytes, UTF-8.
    #[must_use]
    pub fn bundle(&self) -> &[u8] {
        self.whole.needle()
    }

    /// Where the bundle first starts in `source`, if `source` holds it whole
    /// after text that is valid UTF-8, as the bundle is. The search is linear
    /// in both however the bundle repeats itself.
    #[must_use]
    pub fn find(&self, source: &[u8]) -> Option<Placement> {
        let bundle = self.bundle();
        // Almost always the bundle starts where its first bytes first occur.
        let first = self.head.find(source)?;
        let at = if source.get(first..)?.starts_with(bundle) {
            first
        } else {
            let after = first.checked_add(1)?;
            after.checked_add(self.whole.find(source.get(after..)?)?)?
        };
        let prefix = std::str::from_utf8(source.get(..at)?).ok()?;
        Shape::end_of(prefix, bundle.first().copied())
    }

    /// The original position the map gives V8's one-based `line` and `column`
    /// in a script that holds the bundle at `placement`: that of the last
    /// segment at or before the column on the same generated line, or the
    /// first of several at one column, each found by binary search. `None`
    /// outside the bundle, before a line's first segment, and at a segment
    /// that maps to no source.
    #[must_use]
    pub fn original(&self, placement: Placement, line: u32, column: u32) -> Option<Original<'_>> {
        let line = line.checked_sub(1)?.checked_sub(placement.line)?;
        let column = column.checked_sub(1)?;
        let column = if line == 0 {
            column.checked_sub(placement.column)?
        } else {
            column
        };
        if line > self.end.line || (line == self.end.line && column >= self.end.column) {
            return None;
        }
        let before = self.breaks.partition_point(|at| at.line <= line);
        let map_line = line.checked_sub(u32::try_from(before).ok()?)?;
        let base = before
            .checked_sub(1)
            .and_then(|at| self.breaks.get(at))
            .filter(|at| at.line == line)
            .map_or(0, |at| at.column);
        let column = base.checked_add(column)?;
        let index = usize::try_from(map_line).ok()?;
        let start = usize::try_from(*self.starts.get(index)?).ok()?;
        let end = usize::try_from(*self.starts.get(index.checked_add(1)?)?).ok()?;
        let segments = self.segments.get(start..end)?;
        let after = segments.partition_point(|segment| segment.column <= column);
        let found = segments.get(after.checked_sub(1)?)?.column;
        let first = segments.partition_point(|segment| segment.column < found);
        let segment = segments.get(first)?;
        let source = self
            .sources
            .get(usize::try_from(segment.source).ok()?)?
            .as_deref()?;
        Some(Original {
            root: &self.root,
            source,
            line: segment.line,
            column: segment.original_column,
        })
    }
}

const fn within(what: &'static str, count: usize, limit: usize) -> Result<(), MapError> {
    if count > limit {
        return Err(MapError::Limit { what, limit });
    }
    Ok(())
}

// Each generated line's segments, sorted by column, and where each line's
// segments start in the list.
fn decode(
    mappings: &[u8],
    sources: usize,
    names: usize,
) -> Result<(Vec<u32>, Vec<Segment>), MapError> {
    let count = |len: usize| u32::try_from(len).map_err(|_| MapError::TooLarge);
    let lines = memchr::memchr_iter(b';', mappings)
        .count()
        .saturating_add(1);
    within("lines in the source map's mappings", lines, MAX_LINES)?;
    // Each segment ends at a comma, a semicolon or the end of the mappings.
    let separators = memchr::memchr_iter(b',', mappings)
        .count()
        .saturating_add(lines);
    let mut starts = Vec::with_capacity(lines.saturating_add(1));
    starts.push(0);
    let mut segments: Vec<Segment> = Vec::with_capacity(separators.min(MAX_SEGMENTS));
    // The generated column restarts on each line; the other fields carry on.
    let mut state = [0_i64; 5];
    let mut at = 0_usize;
    let mut line_start = 0_usize;
    let mut after_comma = false;
    loop {
        match mappings.get(at) {
            None | Some(b';') => {
                if after_comma {
                    return Err(MapError::Mappings(at));
                }
                sort_line(&mut segments, line_start);
                line_start = segments.len();
                starts.push(count(segments.len())?);
                if at >= mappings.len() {
                    return Ok((starts, segments));
                }
                state[0] = 0;
                at = at.saturating_add(1);
            }
            Some(b',') => {
                if after_comma || segments.len() == line_start {
                    return Err(MapError::Mappings(at));
                }
                after_comma = true;
                at = at.saturating_add(1);
            }
            Some(_) => {
                after_comma = false;
                within(
                    "segments in the source map's mappings",
                    segments.len().saturating_add(1),
                    MAX_SEGMENTS,
                )?;
                let start = at;
                let mut fields = [0_i64; 5];
                let mut taken = 0_usize;
                while let Some(&byte) = mappings.get(at)
                    && byte != b','
                    && byte != b';'
                {
                    let field = fields.get_mut(taken).ok_or(MapError::Mappings(at))?;
                    let rest = mappings.get(at..).unwrap_or_default();
                    let (value, width) = vlq::decode(rest).ok_or(MapError::Mappings(at))?;
                    *field = i64::from(value);
                    taken = taken.saturating_add(1);
                    at = at.saturating_add(width);
                }
                if !matches!(taken, 1 | 4 | 5) {
                    return Err(MapError::Mappings(start));
                }
                for (total, delta) in state.iter_mut().zip(fields).take(taken) {
                    *total = total.saturating_add(delta);
                }
                segments.push(segment(state, taken, (sources, names), start)?);
            }
        }
    }
}

fn segment(
    [column, source, line, original_column, name]: [i64; 5],
    fields: usize,
    (sources, names): (usize, usize),
    at: usize,
) -> Result<Segment, MapError> {
    let malformed = |_| MapError::Mappings(at);
    let column = u32::try_from(column).map_err(malformed)?;
    if fields == 1 {
        return Ok(Segment {
            column,
            source: UNMAPPED,
            line: 0,
            original_column: 0,
        });
    }
    let indexes =
        |index: i64, count: usize| usize::try_from(index).is_ok_and(|index| index < count);
    if !indexes(source, sources) {
        return Err(MapError::Source {
            index: source,
            count: sources,
            at,
        });
    }
    if fields == 5 && !indexes(name, names) {
        return Err(MapError::Name {
            index: name,
            count: names,
            at,
        });
    }
    let source = u32::try_from(source)
        .ok()
        .filter(|&source| source != UNMAPPED)
        .ok_or(MapError::TooLarge)?;
    Ok(Segment {
        column,
        source,
        line: u32::try_from(line).map_err(malformed)?,
        original_column: u32::try_from(original_column).map_err(malformed)?,
    })
}

// ECMA-426 sorts mappings by generated position. Generators write each line
// in order, so this sorts only a line written out of order, once.
fn sort_line(segments: &mut [Segment], start: usize) {
    if let Some(line) = segments.get_mut(start..)
        && !line.is_sorted_by_key(|segment| segment.column)
    {
        line.sort_by_key(|segment| segment.column);
    }
}

// The bundle's lines as its map counts them, at each LF: how many, and the
// length in UTF-16 units of each the map has; the breaks only V8 counts; and
// where its last V8 line ends.
struct Shape {
    lines: usize,
    lengths: Vec<u32>,
    breaks: Vec<Break>,
    end: Placement,
}

impl Shape {
    // Lengths only of the map's lines, so a bundle of many short lines costs
    // no more than the map does.
    fn of(bundle: &str, mapped: usize) -> Result<Self, MapError> {
        let too_large = || MapError::TooLarge;
        let mut shape = Self {
            lines: 0,
            lengths: Vec::with_capacity(
                mapped.min(
                    memchr::memchr_iter(b'\n', bundle.as_bytes())
                        .count()
                        .saturating_add(1),
                ),
            ),
            breaks: Vec::new(),
            end: Placement { line: 0, column: 0 },
        };
        let mut lines = bundle.split('\n').peekable();
        while let Some(text) = lines.next() {
            let last = lines.peek().is_none();
            let end = &mut shape.end;
            let length = if plain(text) {
                end.column = u32::try_from(text.len()).map_err(|_| too_large())?;
                end.column
            } else {
                let mut length = 0_u32;
                end.column = 0;
                let mut chars = text.chars().peekable();
                while let Some(c) = chars.next() {
                    let width = u32::try_from(c.len_utf16()).map_err(|_| too_large())?;
                    length = length.checked_add(width).ok_or_else(too_large)?;
                    if v8_only(c, chars.peek().is_some() || last) {
                        within(
                            "lone CR, U+2028 and U+2029 line breaks in the bundle",
                            shape.breaks.len().saturating_add(1),
                            MAX_LINES,
                        )?;
                        end.line = end.line.checked_add(1).ok_or_else(too_large)?;
                        end.column = 0;
                        shape.breaks.push(Break {
                            line: end.line,
                            column: length,
                        });
                    } else {
                        end.column = end.column.checked_add(width).ok_or_else(too_large)?;
                    }
                }
                length
            };
            if shape.lines < mapped {
                shape.lengths.push(length);
            }
            shape.lines = shape.lines.saturating_add(1);
            if !last {
                shape.end.line = shape.end.line.checked_add(1).ok_or_else(too_large)?;
                shape.end.column = 0;
            }
        }
        Ok(shape)
    }

    // Where V8 counts the end of `text`, which a bundle starting with `next`
    // follows.
    fn end_of(text: &str, next: Option<u8>) -> Option<Placement> {
        let mut end = Placement { line: 0, column: 0 };
        let mut lines = text.split('\n').peekable();
        while let Some(text) = lines.next() {
            let last = lines.peek().is_none();
            if plain(text) {
                end.column = u32::try_from(text.len()).ok()?;
            } else {
                end.column = 0;
                let mut chars = text.chars().peekable();
                while let Some(c) = chars.next() {
                    let apart = chars.peek().is_some() || (last && next != Some(b'\n'));
                    if v8_only(c, apart) {
                        end.line = end.line.checked_add(1)?;
                        end.column = 0;
                    } else {
                        let width = u32::try_from(c.len_utf16()).ok()?;
                        end.column = end.column.checked_add(width)?;
                    }
                }
            }
            if !last {
                end.line = end.line.checked_add(1)?;
                end.column = 0;
            }
        }
        Some(end)
    }
}

// Most lines of a bundle are ASCII with no CR: a UTF-16 unit a byte.
fn plain(line: &str) -> bool {
    line.is_ascii() && memchr::memchr(b'\r', line.as_bytes()).is_none()
}

// Whether V8 breaks a line at `c` where an LF-counting generator does not. A
// CR right before an LF is CRLF, one break to both; `apart` says whether
// something other than that LF follows it.
const fn v8_only(c: char, apart: bool) -> bool {
    match c {
        '\r' => apart,
        '\u{2028}' | '\u{2029}' => true,
        _ => false,
    }
}

// A map made for another bundle maps lines or columns this one does not have.
fn check(starts: &[u32], segments: &[Segment], shape: &Shape) -> Result<(), MapError> {
    for (line, bounds) in starts.windows(2).enumerate() {
        let &[start, end] = bounds else {
            continue;
        };
        let range = usize::try_from(start).unwrap_or(usize::MAX)..usize::try_from(end).unwrap_or(0);
        let Some(last) = segments.get(range).and_then(<[Segment]>::last) else {
            continue;
        };
        let Some(&length) = shape.lengths.get(line) else {
            return Err(MapError::Lines {
                line: line.saturating_add(1),
                lines: shape.lines,
            });
        };
        if last.column > length {
            return Err(MapError::Column {
                line: line.saturating_add(1),
                column: last.column,
                length,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
pub mod tests {
    #![allow(
        clippy::arithmetic_side_effects,
        clippy::indexing_slicing,
        reason = "test helpers"
    )]

    use std::time::{Duration, Instant};

    use super::{
        MAX_LINES, MAX_NAMES, MAX_SEGMENTS, MAX_SOURCES, MAX_URL, MapError, Original, Placement,
        Shape, SourceMap,
    };
    use crate::vlq::tests::encode;

    /// A segment by its absolute fields: mapped, as generated column,
    /// source, original line and original column; or unmapped, as its
    /// generated column.
    #[derive(Clone, Copy)]
    pub enum Seg {
        M(i64, i64, i64, i64),
        U(i64),
    }

    /// The mappings a generator writes for `lines`, each field relative to
    /// the one before.
    pub fn mappings(lines: &[&[Seg]]) -> String {
        let mut out = String::new();
        let (mut source, mut line, mut column) = (0, 0, 0);
        for (index, segments) in lines.iter().enumerate() {
            if index > 0 {
                out.push(';');
            }
            let mut generated = 0;
            for (at, segment) in segments.iter().enumerate() {
                if at > 0 {
                    out.push(',');
                }
                match *segment {
                    Seg::U(at) => {
                        out += &encode(at - generated);
                        generated = at;
                    }
                    Seg::M(at, s, l, c) => {
                        for (value, previous) in
                            [(at, generated), (s, source), (l, line), (c, column)]
                        {
                            out += &encode(value - previous);
                        }
                        (generated, source, line, column) = (at, s, l, c);
                    }
                }
            }
        }
        out
    }

    pub fn json(mappings: &str) -> Vec<u8> {
        format!(r#"{{"version":3,"sources":["a.js","b.js",null],"names":["n"],"mappings":"{mappings}"}}"#)
            .into_bytes()
    }

    const ROW: &str = "0123456789012345678901234567890123456789";

    fn bundle(lines: usize) -> String {
        format!("{}\n", vec![ROW; lines].join("\n"))
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "compared with lookups, which return an Option"
    )]
    fn original(source: &str, line: u32, column: u32) -> Option<Original<'_>> {
        Some(Original {
            root: "",
            source,
            line,
            column,
        })
    }

    fn named(found: Original<'_>) -> String {
        format!("{}{}", found.root, found.source)
    }

    const AT_START: Placement = Placement { line: 0, column: 0 };

    #[test]
    fn parses_a_map_and_skips_its_sources_content() {
        let text = format!(
            r#"{{"version":3,"file":"out.js","sources":["a.js"],"sourcesContent":[null,{{"not":["a","string"]}},3],"names":[],"x_google_ignoreList":[0],"mappings":"{}"}}"#,
            mappings(&[&[Seg::M(2, 0, 4, 6)]])
        );
        let map = SourceMap::new(&bundle(1), text.as_bytes()).unwrap();
        assert_eq!(map.original(AT_START, 1, 3), original("a.js", 4, 6));
        assert_eq!(map.bundle(), bundle(1).as_bytes());
    }

    #[test]
    fn reads_mappings_written_with_escapes() {
        let text = r#"{"version":3,"sources":["a.js"],"mappings":"AAAA"}"#;
        let map = SourceMap::new(&bundle(1), text.as_bytes()).unwrap();
        assert_eq!(map.original(AT_START, 1, 1), original("a.js", 0, 0));
    }

    #[test]
    fn refuses_versions_other_than_3() {
        for version in ["2", "4", "3.5", "null"] {
            let text = format!(r#"{{"version":{version},"sources":[],"mappings":""}}"#);
            let error = SourceMap::new(&bundle(1), text.as_bytes()).err();
            assert!(
                matches!(error, Some(MapError::Version)),
                "{version}: {error:?}"
            );
        }
        let error = SourceMap::new(&bundle(1), br#"{"sources":[],"mappings":""}"#).err();
        assert!(matches!(error, Some(MapError::Version)), "{error:?}");
        let error =
            SourceMap::new(&bundle(1), br#"{"version":"3","sources":[],"mappings":""}"#).err();
        assert!(matches!(error, Some(MapError::Json(_))), "{error:?}");
        assert!(
            SourceMap::new(&bundle(1), br#"{"version":3.0,"sources":[],"mappings":""}"#).is_ok()
        );
    }

    #[test]
    fn refuses_index_maps() {
        for text in [
            r#"{"version":3,"sections":[{"offset":{"line":0,"column":0},"map":{"version":3,"sources":[],"mappings":""}}]}"#,
            r#"{"version":3,"sections":null,"sources":[],"mappings":""}"#,
            r#"{"version":2,"sections":[]}"#,
        ] {
            let error = SourceMap::new(&bundle(1), text.as_bytes()).err();
            assert!(
                matches!(error, Some(MapError::IndexMap)),
                "{text}: {error:?}"
            );
        }
    }

    #[test]
    fn refuses_what_is_not_a_map() {
        for (text, missing) in [
            (r#"{"version":3,"sources":[]}"#, "mappings"),
            (r#"{"version":3,"mappings":""}"#, "sources"),
        ] {
            let error = SourceMap::new(&bundle(1), text.as_bytes()).err();
            assert!(
                matches!(error, Some(MapError::Missing(field)) if field == missing),
                "{error:?}"
            );
        }
        for text in ["", "  ", "[]", "[3, [], \"\"]", "3", "null", "\u{feff}{}"] {
            let error = SourceMap::new(&bundle(1), text.as_bytes()).err();
            assert!(
                matches!(error, Some(MapError::NotAnObject)),
                "{text}: {error:?}"
            );
        }
        for text in [
            "{",
            r#"{"version":3,"sources":[1],"mappings":""}"#,
            r#"{"version":3,"sources":[],"mappings":7}"#,
            r#" {"version":3,"sources":[],"mappings":""} x"#,
        ] {
            let error = SourceMap::new(&bundle(1), text.as_bytes()).err();
            assert!(
                matches!(error, Some(MapError::Json(_))),
                "{text}: {error:?}"
            );
        }
        assert!(
            SourceMap::new(
                &bundle(1),
                b"\n\t {\"version\":3,\"sources\":[],\"mappings\":\"\"}\n"
            )
            .is_ok()
        );
        assert!(matches!(
            SourceMap::new("", &json("")),
            Err(MapError::EmptyBundle)
        ));
    }

    #[test]
    fn refuses_malformed_mappings() {
        for (mappings, at) in [
            ("AAAA,,AAAA", 5),
            (",AAAA", 0),
            ("AAAA,", 5),
            ("AAAA;,AAAA", 5),
            ("AA", 0),
            ("AAA", 0),
            ("AAAAAA", 5),
            ("AAAA,AA!A", 7),
            ("AAAAg", 4),
            ("AAAA;ggggggE", 5),
        ] {
            let error = SourceMap::new(&bundle(2), &json(mappings)).err();
            assert!(
                matches!(error, Some(MapError::Mappings(found)) if found == at),
                "{mappings}: {error:?}"
            );
        }
    }

    #[test]
    fn refuses_sources_and_names_out_of_range() {
        let error = SourceMap::new(&bundle(1), &json(&mappings(&[&[Seg::M(0, 3, 0, 0)]]))).err();
        assert!(
            matches!(
                error,
                Some(MapError::Source {
                    index: 3,
                    count: 3,
                    at: 0
                })
            ),
            "{error:?}"
        );
        let error = SourceMap::new(&bundle(1), &json(&mappings(&[&[Seg::M(0, -1, 0, 0)]]))).err();
        assert!(
            matches!(error, Some(MapError::Source { index: -1, .. })),
            "{error:?}"
        );
        // The fifth field names the name; "C" is 1, past the one name.
        let error = SourceMap::new(&bundle(1), &json("AAAAC")).err();
        assert!(
            matches!(
                error,
                Some(MapError::Name {
                    index: 1,
                    count: 1,
                    at: 0
                })
            ),
            "{error:?}"
        );
        let text = r#"{"version":3,"sources":["a.js"],"mappings":"AAAAA"}"#;
        let error = SourceMap::new(&bundle(1), text.as_bytes()).err();
        assert!(
            matches!(
                error,
                Some(MapError::Name {
                    index: 0,
                    count: 0,
                    ..
                })
            ),
            "{error:?}"
        );
        assert!(SourceMap::new(&bundle(1), &json("AAAAA")).is_ok());
    }

    #[test]
    fn refuses_negative_positions() {
        let error = SourceMap::new(&bundle(1), &json("CAAA,FAAA")).err();
        assert!(matches!(error, Some(MapError::Mappings(5))), "{error:?}");
        let error = SourceMap::new(&bundle(1), &json(&mappings(&[&[Seg::M(0, 0, -1, 0)]]))).err();
        assert!(matches!(error, Some(MapError::Mappings(0))), "{error:?}");
        let error = SourceMap::new(&bundle(1), &json(&mappings(&[&[Seg::M(0, 0, 0, -3)]]))).err();
        assert!(matches!(error, Some(MapError::Mappings(0))), "{error:?}");
    }

    // The fields after the generated column carry on from line to line.
    #[test]
    fn carries_fields_across_lines() {
        let text = mappings(&[&[Seg::M(1, 1, 7, 3)], &[], &[Seg::M(5, 1, 9, 0), Seg::U(8)]]);
        let map = SourceMap::new(&bundle(3), &json(&text)).unwrap();
        assert_eq!(map.original(AT_START, 1, 2), original("b.js", 7, 3));
        assert_eq!(map.original(AT_START, 3, 6), original("b.js", 9, 0));
        assert_eq!(map.original(AT_START, 3, 9), None);
    }

    #[test]
    fn looks_up_on_the_same_line_only() {
        let text = mappings(&[
            &[
                Seg::M(4, 0, 10, 0),
                Seg::M(10, 0, 11, 2),
                Seg::M(20, 1, 3, 4),
            ],
            &[],
            &[Seg::M(6, 0, 30, 1)],
        ]);
        let map = SourceMap::new(&bundle(3), &json(&text)).unwrap();
        assert_eq!(
            map.original(AT_START, 1, 4),
            None,
            "before the line's first segment"
        );
        assert_eq!(map.original(AT_START, 1, 5), original("a.js", 10, 0));
        assert_eq!(map.original(AT_START, 1, 10), original("a.js", 10, 0));
        assert_eq!(map.original(AT_START, 1, 11), original("a.js", 11, 2));
        assert_eq!(map.original(AT_START, 1, 21), original("b.js", 3, 4));
        assert_eq!(map.original(AT_START, 1, 40), original("b.js", 3, 4));
        assert_eq!(
            map.original(AT_START, 2, 1),
            None,
            "a line with no segments"
        );
        assert_eq!(
            map.original(AT_START, 3, 5),
            None,
            "the line above is not read"
        );
        assert_eq!(map.original(AT_START, 3, 7), original("a.js", 30, 1));
        assert_eq!(map.original(AT_START, 4, 1), None, "past the map's lines");
        assert_eq!(map.original(AT_START, 0, 1), None);
        assert_eq!(map.original(AT_START, 1, 0), None);
    }

    #[test]
    fn takes_the_first_segment_at_a_column_and_none_for_unmapped() {
        let text = mappings(&[&[
            Seg::M(2, 0, 1, 1),
            Seg::M(2, 1, 5, 5),
            Seg::U(6),
            Seg::M(9, 2, 0, 0),
        ]]);
        let map = SourceMap::new(&bundle(1), &json(&text)).unwrap();
        assert_eq!(map.original(AT_START, 1, 3), original("a.js", 1, 1));
        assert_eq!(map.original(AT_START, 1, 7), None, "an unmapped segment");
        assert_eq!(map.original(AT_START, 1, 10), None, "a source that is null");
    }

    #[test]
    fn sorts_a_line_written_out_of_order() {
        let text = mappings(&[
            &[Seg::M(20, 1, 2, 0), Seg::M(5, 0, 1, 0)],
            &[Seg::M(3, 0, 9, 0)],
        ]);
        let map = SourceMap::new(&bundle(2), &json(&text)).unwrap();
        assert_eq!(map.original(AT_START, 1, 6), original("a.js", 1, 0));
        assert_eq!(map.original(AT_START, 1, 25), original("b.js", 2, 0));
        assert_eq!(map.original(AT_START, 2, 4), original("a.js", 9, 0));
    }

    #[test]
    fn joins_the_source_root() {
        let text = mappings(&[&[Seg::M(0, 0, 0, 0), Seg::M(5, 1, 0, 0), Seg::M(9, 2, 0, 0)]]);
        for (root, a) in [
            (r#","sourceRoot":"webpack://app""#, "webpack://app/a.js"),
            (r#","sourceRoot":"src/""#, "src/a.js"),
            (r#","sourceRoot":"""#, "a.js"),
            (r#","sourceRoot":null"#, "a.js"),
            ("", "a.js"),
        ] {
            let json = format!(
                r#"{{"version":3,"sources":["a.js","./b.js",null]{root},"mappings":"{text}"}}"#
            );
            let map = SourceMap::new(&bundle(1), json.as_bytes()).unwrap();
            assert_eq!(
                map.original(AT_START, 1, 1).map(named).as_deref(),
                Some(a),
                "{root}"
            );
            let b = map.original(AT_START, 1, 6).map(named).unwrap();
            assert!(b.ends_with("./b.js"), "{root}: {b}");
            assert_eq!(map.original(AT_START, 1, 10), None, "{root}");
        }
    }

    #[test]
    fn finds_the_bundle_after_a_prefix() {
        let bundle = "(()=>{throw new Error('x')})();\nvar after = 1;\n";
        let map = SourceMap::new(bundle, &json("")).unwrap();
        let at = |line, column| Some(Placement { line, column });
        for (prefix, expected) in [
            ("", at(0, 0)),
            (
                "var global = global || this;\nvar self = self || this;\n",
                at(2, 0),
            ),
            ("var a = 1; ", at(0, 11)),
            ("var é = '\u{1F600}'; ", at(0, 14)),
            ("a\r\nb\rc\u{2028}d\u{2029}", at(4, 0)),
            ("x\ny\r", at(2, 0)),
        ] {
            let source = format!("{prefix}{bundle}tail()");
            assert_eq!(map.find(source.as_bytes()), expected, "{prefix:?}");
        }
        assert_eq!(map.find(bundle.as_bytes()), at(0, 0));
        assert_eq!(map.find(&bundle.as_bytes()[1..]), None, "shorter");
        assert_eq!(
            map.find(format!("{}#", &bundle[..bundle.len() - 1]).as_bytes()),
            None,
            "differs"
        );
        let mut invalid = b"\xFF;".to_vec();
        invalid.extend_from_slice(bundle.as_bytes());
        assert_eq!(map.find(&invalid), None, "a prefix that is not UTF-8");
    }

    // A banner of one repeated pattern matches its own first bytes a period
    // later, so the bundle can start inside a candidate that fails.
    #[test]
    fn finds_a_bundle_whose_first_bytes_repeat() {
        let bundle = format!("{}Z();\n", "ab".repeat(40));
        let map = SourceMap::new(&bundle, &json("")).unwrap();
        let source = format!("ab{bundle}");
        assert_eq!(
            map.find(source.as_bytes()),
            Some(Placement { line: 0, column: 2 })
        );
        let banner = format!("/{}/\n", "*".repeat(70));
        let map = SourceMap::new(&banner, &json("")).unwrap();
        let source = format!("/**{banner}");
        assert_eq!(
            map.find(source.as_bytes()),
            Some(Placement { line: 0, column: 3 })
        );
        assert_eq!(
            map.find(format!("/**{}", &banner[..60]).repeat(9).as_bytes()),
            None
        );
    }

    // A CR that ends the prefix, before a bundle that starts with LF, is
    // CRLF to V8: one break, counted at the LF inside the bundle, so the CR
    // takes a column on the bundle's first line.
    #[test]
    fn counts_crlf_across_the_start_of_the_bundle() {
        let bundle = "\nthrow 1;\n";
        let map = SourceMap::new(bundle, &json(&mappings(&[&[], &[Seg::M(0, 0, 4, 0)]]))).unwrap();
        let placement = map.find(b"a\r\nthrow 1;\n").unwrap();
        assert_eq!(placement, Placement { line: 0, column: 2 });
        assert_eq!(map.original(placement, 2, 1), original("a.js", 4, 0));
    }

    #[test]
    fn maps_through_where_the_bundle_starts() {
        let text = mappings(&[
            &[Seg::M(0, 0, 1, 0), Seg::M(4, 0, 2, 0)],
            &[Seg::M(0, 1, 3, 0)],
        ]);
        let map = SourceMap::new(&bundle(2), &json(&text)).unwrap();
        let own_line = Placement {
            line: 10,
            column: 0,
        };
        assert_eq!(map.original(own_line, 11, 5), original("a.js", 2, 0));
        assert_eq!(map.original(own_line, 12, 1), original("b.js", 3, 0));
        assert_eq!(map.original(own_line, 10, 5), None, "in the prefix");
        assert_eq!(map.original(own_line, 13, 1), None, "after the bundle");
        let mid_line = Placement {
            line: 3,
            column: 17,
        };
        assert_eq!(map.original(mid_line, 4, 18), original("a.js", 1, 0));
        assert_eq!(map.original(mid_line, 4, 22), original("a.js", 2, 0));
        assert_eq!(
            map.original(mid_line, 4, 17),
            None,
            "in the prefix, on the bundle's line"
        );
        assert_eq!(
            map.original(mid_line, 5, 1),
            original("b.js", 3, 0),
            "the next line starts at 0"
        );
    }

    #[test]
    fn corrects_for_line_breaks_only_v8_counts() {
        // As a generator counts lines, at each LF: "ab\rcd\u{2028}ef" then
        // "g\r\nh" then "i\u{2029}j". V8 also breaks at the lone CR, U+2028
        // and U+2029, and once for the CRLF.
        let bundle = "ab\rcd\u{2028}ef\ng\r\nh\ni\u{2029}j";
        let text = mappings(&[
            &[
                Seg::M(0, 0, 0, 0),
                Seg::M(3, 0, 1, 0),
                Seg::M(4, 0, 1, 1),
                Seg::M(6, 0, 2, 0),
            ],
            &[Seg::M(0, 1, 0, 0)],
            &[Seg::M(0, 1, 1, 0)],
            &[Seg::M(0, 1, 2, 0)],
        ]);
        let map = SourceMap::new(bundle, &json(&text)).unwrap();
        let v8 = |line, column| map.original(AT_START, line, column);
        assert_eq!(v8(1, 1), original("a.js", 0, 0), "a");
        assert_eq!(v8(2, 1), original("a.js", 1, 0), "c, after the lone CR");
        assert_eq!(v8(2, 2), original("a.js", 1, 1), "d");
        assert_eq!(v8(3, 1), original("a.js", 2, 0), "e, after U+2028");
        assert_eq!(v8(4, 1), original("b.js", 0, 0), "g");
        assert_eq!(v8(5, 1), original("b.js", 1, 0), "h, after the CRLF");
        assert_eq!(v8(6, 1), original("b.js", 2, 0), "i");
        assert_eq!(
            v8(7, 1),
            original("b.js", 2, 0),
            "j, after U+2029, past the line's last segment"
        );
        assert_eq!(v8(7, 2), None, "past the end of the bundle");
    }

    #[test]
    fn counts_columns_in_utf16_units() {
        let bundle = "\u{1F600}é x\n";
        let map = SourceMap::new(bundle, &json(&mappings(&[&[Seg::M(4, 0, 0, 9)]]))).unwrap();
        assert_eq!(map.original(AT_START, 1, 4), None);
        assert_eq!(map.original(AT_START, 1, 5), original("a.js", 0, 9));
        let error = SourceMap::new(bundle, &json(&mappings(&[&[Seg::M(6, 0, 0, 0)]]))).err();
        assert!(
            matches!(
                error,
                Some(MapError::Column {
                    line: 1,
                    column: 6,
                    length: 5
                })
            ),
            "{error:?}"
        );
    }

    #[test]
    fn refuses_a_map_made_for_another_bundle() {
        let error = SourceMap::new(
            &bundle(2),
            &json(&mappings(&[&[], &[], &[], &[Seg::M(0, 0, 0, 0)]])),
        )
        .err();
        assert!(
            matches!(error, Some(MapError::Lines { line: 4, lines: 3 })),
            "{error:?}"
        );
        let error = SourceMap::new(&bundle(1), &json(&mappings(&[&[Seg::M(41, 0, 0, 0)]]))).err();
        assert!(
            matches!(
                error,
                Some(MapError::Column {
                    line: 1,
                    column: 41,
                    length: 40
                })
            ),
            "{error:?}"
        );
        // A segment may sit at the LF that ends its line, and a map need not
        // reach the bundle's last lines.
        assert!(
            SourceMap::new(
                &bundle(3),
                &json(&mappings(&[&[Seg::M(40, 0, 0, 0)], &[], &[]]))
            )
            .is_ok()
        );
        assert!(
            SourceMap::new(&bundle(3), &json(";;;;;")).is_ok(),
            "empty lines past the bundle"
        );
    }

    // Each worst case is linear, so it takes a fraction of a second even
    // unoptimized, and a few seconds on a loaded CI runner; quadratic work at
    // these sizes would take hours.
    const BOUND: Duration = Duration::from_secs(30);

    fn timed<T>(label: &str, work: impl FnOnce() -> T) -> T {
        let started = Instant::now();
        let out = work();
        assert!(
            started.elapsed() < BOUND,
            "{label} took {:?}",
            started.elapsed()
        );
        out
    }

    fn limited(error: Option<&MapError>, limit: usize) -> bool {
        matches!(error, Some(MapError::Limit { limit: found, .. }) if *found == limit)
    }

    #[test]
    fn reads_one_line_of_a_million_segments() {
        let count = 1_000_000;
        let mut text = String::from("AAAA");
        text.push_str(&",CAAA".repeat(count - 1));
        let bundle = format!("{}\n", "x".repeat(count));
        let map = timed("ascending", || {
            SourceMap::new(&bundle, &json(&text)).unwrap()
        });
        assert_eq!(map.segments.len(), count);
        let found = timed("lookups", || {
            (1..=1_000_u32)
                .filter_map(|at| map.original(AT_START, 1, at * 997))
                .count()
        });
        assert_eq!(found, 1_000);
        // Written backwards, the line is sorted once.
        let mut text = mappings(&[&[Seg::M(i64::try_from(count).unwrap() - 1, 0, 0, 0)]]);
        text.push_str(&",DAAA".repeat(count - 1));
        let map = timed("descending", || {
            SourceMap::new(&bundle, &json(&text)).unwrap()
        });
        assert_eq!(map.original(AT_START, 1, 1), original("a.js", 0, 0));
        // At one column, the first of them.
        let mut text = String::from("AAAA");
        text.push_str(&",AACA".repeat(count - 1));
        let map = timed("one column", || {
            SourceMap::new(&bundle, &json(&text)).unwrap()
        });
        assert_eq!(map.original(AT_START, 1, 1), original("a.js", 0, 0));
    }

    #[test]
    fn caps_the_lines_of_the_mappings() {
        let most = ";".repeat(MAX_LINES - 1);
        let map = timed("most lines", || {
            SourceMap::new(&bundle(1), &json(&most)).unwrap()
        });
        assert_eq!(map.starts.len(), MAX_LINES + 1);
        let more = format!("{most};");
        let error = timed("too many lines", || {
            SourceMap::new(&bundle(1), &json(&more)).err()
        });
        assert!(limited(error.as_ref(), MAX_LINES));
    }

    #[test]
    fn caps_the_segments_of_the_mappings() {
        let most = format!("A{}", ",A".repeat(MAX_SEGMENTS - 1));
        assert!(timed("most segments", || SourceMap::new(
            &bundle(1),
            &json(&most)
        )
        .is_ok()));
        let error = timed("too many segments", || {
            SourceMap::new(&bundle(1), &json(&format!("{most},A"))).err()
        });
        assert!(limited(error.as_ref(), MAX_SEGMENTS));
    }

    #[test]
    fn refuses_a_long_run_of_continuation_digits() {
        let text = format!("A,{}A", "g".repeat(5_000_000));
        let error = timed("continuations", || {
            SourceMap::new(&bundle(1), &json(&text)).err()
        });
        assert!(matches!(error, Some(MapError::Mappings(2))), "{error:?}");
    }

    // Garbage after the first source past a cap shows the cap was found as
    // the sources arrived, before the rest was read.
    #[test]
    fn refuses_sources_as_they_arrive() {
        let long = "r".repeat(MAX_URL + 1);
        let text = format!(r#"{{"version":3,"sources":["{long}", !garbage"#);
        let error = SourceMap::new(&bundle(1), text.as_bytes()).err();
        assert!(limited(error.as_ref(), MAX_URL), "{error:?}");
        let nulls = vec!["null"; MAX_SOURCES + 1].join(",");
        let text = format!(r#"{{"version":3,"sources":[{nulls}, !garbage"#);
        let error = timed("too many sources", || {
            SourceMap::new(&bundle(1), text.as_bytes()).err()
        });
        assert!(limited(error.as_ref(), MAX_SOURCES), "{error:?}");
    }

    #[test]
    fn refuses_a_map_past_256_mib_before_reading_it() {
        let mut text = br#"{"version":3,"sources":[],"mappings":""}"#.to_vec();
        text.resize((256 << 20) + 1, b' ');
        let error = timed("past the cap", || SourceMap::new(&bundle(1), &text).err());
        assert!(limited(error.as_ref(), 256 << 20), "{error:?}");
    }

    // Each source and the root are printed into a backtrace line, where a
    // line break would add entries and an escape or a bidi control would
    // disguise one.
    #[test]
    fn refuses_control_characters_in_sources_and_the_root() {
        for text in [
            "x\r\u{1b}[2K/app/models/user.rb:42:in `save'",
            "a\nb",
            "a\tb",
            "a\u{7f}b",
            "a\u{85}b",
            "a\u{2028}b",
            "a\u{2029}b",
            "a\u{202a}b",
            "a\u{202e}b",
            "a\u{2066}b",
            "a\u{2069}b",
        ] {
            let quoted = serde_json::to_string(text).unwrap();
            for json in [
                format!(r#"{{"version":3,"sources":[{quoted}],"mappings":"AAAA"}}"#),
                format!(
                    r#"{{"version":3,"sourceRoot":{quoted},"sources":["a.js"],"mappings":"AAAA"}}"#
                ),
            ] {
                let error = SourceMap::new(&bundle(1), json.as_bytes()).err();
                assert!(
                    error
                        .as_ref()
                        .is_some_and(|error| error.to_string().contains("control character")),
                    "{text:?}: {error:?}"
                );
            }
        }
        let fine = r#"{"version":3,"sourceRoot":"webpack://app/","sources":["./src/Café ☕.jsx",null],"mappings":"AAAA"}"#;
        assert!(SourceMap::new(&bundle(1), fine.as_bytes()).is_ok());
    }

    // Sources are read one by one and names only counted, so arrays far
    // past what the mappings use stop at their caps, and the map keeps one
    // copy of each source and of the root, joined only when one is printed.
    #[test]
    fn caps_names_and_sources_far_past_what_the_mappings_use() {
        let names = vec!["\"n\""; MAX_NAMES].join(",");
        let text =
            format!(r#"{{"version":3,"sources":["a.js"],"names":[{names}],"mappings":"AAAAA"}}"#);
        assert!(timed("most names", || SourceMap::new(
            &bundle(1),
            text.as_bytes()
        )
        .is_ok()));
        let text = format!(
            r#"{{"version":3,"sources":["a.js"],"names":[{names},"n"],"mappings":"AAAAA"}}"#
        );
        let error = timed("too many names", || {
            SourceMap::new(&bundle(1), text.as_bytes()).err()
        });
        assert!(limited(error.as_ref(), MAX_NAMES));
        let sources = vec!["null"; MAX_SOURCES + 1].join(",");
        let text = format!(r#"{{"version":3,"sources":[{sources}],"mappings":""}}"#);
        let error = timed("too many sources", || {
            SourceMap::new(&bundle(1), text.as_bytes()).err()
        });
        assert!(limited(error.as_ref(), MAX_SOURCES));
        // Joined to each of a million sources up front, this root would take
        // 64 GiB.
        let root = "r".repeat(MAX_URL);
        let sources = vec!["\"\""; MAX_SOURCES].join(",");
        let text = format!(
            r#"{{"version":3,"sourceRoot":"{root}","sources":[{sources}],"mappings":"AAAA"}}"#
        );
        let map = timed("a long root", || {
            SourceMap::new(&bundle(1), text.as_bytes()).unwrap()
        });
        assert_eq!(
            map.original(AT_START, 1, 1).map(|at| at.root.len()),
            Some(MAX_URL + 1)
        );
        for text in [
            format!(r#"{{"version":3,"sourceRoot":"{root}r","sources":[],"mappings":""}}"#),
            format!(r#"{{"version":3,"sources":["{root}r"],"mappings":""}}"#),
        ] {
            let error = SourceMap::new(&bundle(1), text.as_bytes()).err();
            assert!(limited(error.as_ref(), MAX_URL));
        }
    }

    #[test]
    fn keeps_line_lengths_only_for_the_maps_lines() {
        let bundle = "\n".repeat(10_000_000);
        let shape = timed("shape", || Shape::of(&bundle, 3).unwrap());
        assert_eq!((shape.lengths.len(), shape.lines), (3, 10_000_001));
        let error =
            SourceMap::new(&bundle, &json(&format!("{}AAAA", ";".repeat(10_000_000)))).err();
        assert!(limited(error.as_ref(), MAX_LINES));
        let breaks = "\r".repeat(MAX_LINES + 1);
        let error = timed("breaks", || SourceMap::new(&breaks, &json("")).err());
        assert!(limited(error.as_ref(), MAX_LINES));
    }

    // A bundle that repeats itself, found or not in text that nearly holds
    // it at every offset.
    #[test]
    fn finds_a_repetitive_bundle_in_linear_time() {
        let bundle = format!("{}b", "a".repeat(100_000));
        let map = SourceMap::new(&bundle, &json("")).unwrap();
        let near = "a".repeat(2_000_000);
        assert_eq!(timed("absent", || map.find(near.as_bytes())), None);
        let source = format!("{near}{bundle}");
        assert_eq!(
            timed("found", || map.find(source.as_bytes())),
            Some(Placement {
                line: 0,
                column: 2_000_000
            })
        );
    }
}
