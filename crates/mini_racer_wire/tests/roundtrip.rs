//! What the writer produces, the decoder reads back as the same values.

mod common;

use common::{Tree, hex, number};
use mini_racer_wire::{Limits, Writer, decode_value, text};
use proptest::prelude::*;

#[derive(Debug, Clone)]
enum Value {
    Null,
    Undefined,
    Bool(bool),
    Int(i64),
    Float(f64),
    Utf8(String),
    Latin1(Vec<u8>),
    Date(f64),
    Bytes(Vec<u8>),
    Array(Vec<Self>),
    Object(Vec<(String, Self)>),
}

fn value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        Just(Value::Undefined),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::Int),
        (-1_000_000i64..1_000_000).prop_map(Value::Int),
        any::<f64>()
            .prop_filter("finite", |f| f.is_finite())
            .prop_map(Value::Float),
        ".{0,12}".prop_map(Value::Utf8),
        proptest::collection::vec(any::<u8>(), 0..12).prop_map(Value::Latin1),
        (-8.64e15f64..8.64e15).prop_map(Value::Date),
        proptest::collection::vec(any::<u8>(), 0..12).prop_map(Value::Bytes),
    ];
    leaf.prop_recursive(4, 48, 6, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
            proptest::collection::vec(("[a-z]{1,6}", inner), 0..6).prop_map(Value::Object),
        ]
    })
}

fn write(writer: &mut Writer<'_>, value: &Value) {
    match value {
        Value::Null => writer.null(),
        Value::Undefined => writer.undefined(),
        Value::Bool(b) => writer.bool(*b),
        Value::Int(n) => writer.integer(*n),
        Value::Float(f) => writer.number(*f),
        Value::Utf8(s) => writer.utf8(s.as_bytes()),
        Value::Latin1(bytes) => writer.latin1(bytes),
        Value::Date(time) => {
            writer.date(*time);
        }
        Value::Bytes(bytes) => {
            writer.uint8_array(bytes);
        }
        Value::Array(items) => {
            let len = u32::try_from(items.len()).expect("short");
            writer.begin_array(len);
            for item in items {
                write(writer, item);
            }
            writer.end_array(len);
        }
        Value::Object(entries) => {
            writer.begin_object();
            for (key, value) in entries {
                writer.utf8(key.as_bytes());
                write(writer, value);
            }
            writer.end_object(u32::try_from(entries.len()).expect("short"));
        }
    }
}

// The rendering `Tree::render` gives for what the writer means to encode.
fn expected(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Undefined => "undefined".to_owned(),
        Value::Bool(b) => b.to_string(),
        Value::Int(n) => match i32::try_from(*n) {
            Ok(small) => small.to_string(),
            #[expect(clippy::cast_precision_loss, reason = "mirrors the writer's rounding")]
            Err(_) if *n > i64::MIN / 1024 && *n <= i64::MAX / 1024 => {
                format!("num({})", number(*n as f64))
            }
            Err(_) => {
                let sign = if *n < 0 { "-" } else { "" };
                format!("{sign}0x{}n", hex(n.unsigned_abs().to_be_bytes()))
            }
        },
        Value::Float(f) => format!("num({})", number(*f)),
        Value::Utf8(s) => format!("{s:?}"),
        Value::Latin1(bytes) => format!("{:?}", text::latin1_to_utf8(bytes)),
        Value::Date(time) => format!("date({})", number(*time)),
        Value::Bytes(bytes) => format!("buffer({})", hex(bytes.iter().copied())),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(expected).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(entries) => format!(
            "{{{}}}",
            entries
                .iter()
                .map(|(k, v)| format!("{k:?}: {}", expected(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    #[test]
    fn writer_output_decodes_to_what_was_written(value in value()) {
        let mut bytes = Vec::new();
        write(&mut Writer::new(&mut bytes), &value);
        let mut tree = Tree::default();
        let root = decode_value(&bytes, Limits::default(), &mut tree).expect("decodes");
        prop_assert_eq!(tree.render(root), expected(&value));
    }
}
