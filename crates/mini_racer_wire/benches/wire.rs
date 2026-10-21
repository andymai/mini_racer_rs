//! Decoding, writing and transcoding V8's wire format, without V8 or Ruby, on
//! the shapes the extension sees: seeded server rendering props, and runs of
//! numbers and strings. Strings take the forms V8's serializer gives them.

use std::convert::Infallible;
use std::hint::black_box;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use mini_racer_wire::{Begun, Container, Limits, Scalar, Visitor, Writer, decode_value, text};

const WORDS: [&str; 12] = [
    "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "theta", "kappa", "lambda", "sigma",
    "omega", "omicron",
];
const INTL: [&str; 5] = ["café", "naïve", "Zürich", "東京", "😀 party"];
// The words of INTL that V8 keeps one byte a character.
const LATIN1: [&str; 3] = ["café", "naïve", "Zürich"];

/// Reads every value and keeps nothing, so the decoder's own work is what
/// is measured.
struct Discard;

impl Visitor for Discard {
    type Value = ();
    type Frame = ();
    type Error = Infallible;

    fn scalar(&mut self, scalar: Scalar<'_>) -> Result<(), Infallible> {
        black_box(scalar);
        Ok(())
    }

    fn begin(&mut self, container: Container) -> Result<Begun<(), ()>, Infallible> {
        black_box(container);
        Ok(Begun {
            frame: (),
            value: None,
        })
    }

    fn push(&mut self, (): &mut (), (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    fn insert(&mut self, (): &mut (), (): (), (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    fn end(&mut self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    fn register(&mut self, id: u32, (): ()) -> Result<(), Infallible> {
        black_box(id);
        Ok(())
    }

    fn back_ref(&mut self, id: u32) -> Result<(), Infallible> {
        black_box(id);
        Ok(())
    }
}

enum Node {
    Int(i64),
    Number(f64),
    Bool(bool),
    Latin1(Vec<u8>),
    Utf16(Vec<u16>),
    Object(Vec<(Vec<u8>, Self)>),
    Array(Vec<Self>),
}

impl Node {
    fn text(text: &str) -> Self {
        let latin1: Option<Vec<u8>> = text
            .chars()
            .map(|c| u8::try_from(u32::from(c)).ok())
            .collect();
        latin1.map_or_else(|| Self::Utf16(text.encode_utf16().collect()), Self::Latin1)
    }

    fn object(entries: Vec<(&str, Self)>) -> Self {
        Self::Object(
            entries
                .into_iter()
                .map(|(key, value)| (key.as_bytes().to_vec(), value))
                .collect(),
        )
    }

    fn write(&self, writer: &mut Writer<'_>) {
        match self {
            Self::Int(n) => writer.integer(*n),
            Self::Number(n) => writer.number(*n),
            Self::Bool(b) => writer.bool(*b),
            Self::Latin1(bytes) => writer.latin1(bytes),
            Self::Utf16(units) => writer.utf16_aligned(units),
            Self::Object(entries) => {
                writer.begin_object();
                for (key, value) in entries {
                    writer.latin1(key);
                    value.write(writer);
                }
                writer.end_object(count(entries.len()));
            }
            Self::Array(items) => {
                writer.begin_array(count(items.len()));
                for item in items {
                    item.write(writer);
                }
                writer.end_array(count(items.len()));
            }
        }
    }

    fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write(&mut Writer::versioned(&mut out, 15));
        out
    }
}

fn count(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

/// xorshift64, so the payload is the same on every machine.
struct Seeded(u64);

impl Seeded {
    const fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: usize) -> usize {
        usize::try_from(self.next()).unwrap_or(0) % bound
    }

    fn words(&mut self, count: usize) -> Node {
        self.words_from(count, &INTL)
    }

    fn words_from(&mut self, count: usize, intl: &[&str]) -> Node {
        let words: Vec<&str> = (0..count)
            .map(|_| {
                if self.below(10) == 0 {
                    intl[self.below(intl.len())]
                } else {
                    WORDS[self.below(WORDS.len())]
                }
            })
            .collect();
        Node::text(&words.join(" "))
    }

    fn cents(&mut self) -> f64 {
        f64::from(u32::try_from(self.below(100_000)).unwrap_or(0)) / 100.0
    }
}

/// About 1 MB of props, the size the end-to-end benchmark sends.
fn props() -> Node {
    let mut seeded = Seeded(42);
    let items = (0..5_000)
        .map(|id| {
            let length = 3 + seeded.below(6);
            Node::object(vec![
                ("id", Node::Int(id)),
                (
                    "sku",
                    Node::text(&format!("SKU-{}", seeded.below(1_000_000))),
                ),
                ("title", seeded.words(length)),
                ("price", Node::Number(seeded.cents())),
                ("rating", Node::Number(seeded.cents() / 200.0)),
                ("inStock", Node::Bool(seeded.below(4) != 0)),
                (
                    "tags",
                    Node::Array(vec![seeded.words(1), seeded.words(1), seeded.words(1)]),
                ),
                (
                    "seller",
                    Node::object(vec![
                        ("name", seeded.words(2)),
                        (
                            "reviews",
                            Node::Int(i64::try_from(seeded.below(5_000)).unwrap_or(0)),
                        ),
                    ]),
                ),
            ])
        })
        .collect();
    Node::object(vec![
        (
            "page",
            Node::object(vec![
                ("title", Node::text("Products")),
                ("locale", Node::text("en-US")),
            ]),
        ),
        ("items", Node::Array(items)),
    ])
}

fn numbers() -> Node {
    let mut seeded = Seeded(7);
    Node::Array((0..10_000).map(|_| Node::Number(seeded.cents())).collect())
}

fn latin1_strings() -> Node {
    let mut seeded = Seeded(9);
    Node::Array((0..10_000).map(|_| seeded.words_from(2, &LATIN1)).collect())
}

fn two_byte_strings() -> Node {
    Node::Array(
        (0..1_000)
            .map(|n| Node::text(&format!("東京の天気は晴れです {n}")))
            .collect(),
    )
}

fn decode(c: &mut Criterion) {
    let mut group = c.benchmark_group("decode");
    for (name, node) in [
        ("ssr_props", props()),
        ("dense_numbers", numbers()),
        ("latin1_strings", latin1_strings()),
        ("two_byte_strings", two_byte_strings()),
    ] {
        let bytes = node.bytes();
        group.throughput(Throughput::Bytes(
            bytes.len().try_into().unwrap_or(u64::MAX),
        ));
        group.bench_function(name, |b| {
            b.iter(|| decode_value(black_box(&bytes), Limits::default(), &mut Discard));
        });
    }
    group.finish();
}

fn write(c: &mut Criterion) {
    let node = props();
    let mut out = Vec::new();
    node.write(&mut Writer::new(&mut out));
    let mut group = c.benchmark_group("write");
    group.throughput(Throughput::Bytes(out.len().try_into().unwrap_or(u64::MAX)));
    group.bench_function("ssr_props", |b| {
        b.iter(|| {
            let mut writer = Writer::new(&mut out);
            black_box(&node).write(&mut writer);
        });
    });
    group.finish();
}

fn transcode(c: &mut Criterion) {
    let latin1: Vec<u8> = "Le café de la naïveté à Zürich, déjà vu. "
        .chars()
        .filter_map(|ch| u8::try_from(u32::from(ch)).ok())
        .cycle()
        .take(64 << 10)
        .collect();
    let utf16: Vec<u8> = "東京の天気は晴れです。"
        .encode_utf16()
        .cycle()
        .take(32 << 10)
        .flat_map(u16::to_le_bytes)
        .collect();
    let mut out = vec![0; 192 << 10];
    let mut group = c.benchmark_group("transcode");
    group.throughput(Throughput::Bytes(
        latin1.len().try_into().unwrap_or(u64::MAX),
    ));
    group.bench_function("latin1_to_utf8", |b| {
        b.iter(|| text::latin1_to_utf8_into(black_box(&latin1), &mut out));
    });
    group.throughput(Throughput::Bytes(
        utf16.len().try_into().unwrap_or(u64::MAX),
    ));
    group.bench_function("utf16le_to_utf8", |b| {
        b.iter(|| text::utf16le_to_utf8_into(black_box(&utf16), &mut out));
    });
    group.finish();
}

criterion_group!(benches, decode, write, transcode);
criterion_main!(benches);
