mod common;

use common::Tree;
use mini_racer_wire::{Limits, Writer, decode_value, decode_values};

fn render(bytes: &[u8], limits: Limits) -> String {
    let mut tree = Tree::default();
    match decode_value(bytes, limits, &mut tree) {
        Ok(root) => tree.render(root),
        Err(error) => format!("error: {error}"),
    }
}

fn frame(body: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0x0F];
    bytes.extend_from_slice(body);
    bytes
}

fn nested(levels: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut writer = Writer::new(&mut bytes);
    for _ in 0..levels {
        writer.begin_array(1);
    }
    writer.integer(1);
    for _ in 0..levels {
        writer.end_array(1);
    }
    bytes
}

#[test]
fn nesting_stops_after_97_levels_like_mini_racer() {
    assert!(render(&nested(96), Limits::default()).starts_with("[[["));
    assert_eq!(
        render(&nested(97), Limits::default()),
        "error: too much recursion"
    );
}

#[test]
fn bigints_past_the_limit_are_refused() {
    let mut bytes = Vec::new();
    Writer::new(&mut bytes)
        .bigint(false, &[1; 16])
        .expect("whole words");
    let tight = Limits {
        max_bigint_bytes: 8,
        ..Limits::default()
    };
    assert_eq!(render(&bytes, tight), "error: bigint too big");
    assert!(render(&bytes, Limits::default()).ends_with('n'));
}

#[test]
fn malformed_input_reports_mini_racers_messages() {
    let cases: &[(&[u8], &str)] = &[
        (b"c\x03abc", "bad utf16 string size"),
        (b"q", "bad tag: q"),
        (b"\x01", "bad tag: 01"),
        (b"o\"\x01a\x49\x02{\x02", "object properties count mismatch"),
        (b"B\x02xyV\x42\x01\x02\x00", "bad typed array"),
        (b"rm\x49\x02.", "error .message is not a string"),
        (b"Z\x01", "negative zero bigint"),
        (b"Z\x06\x00", "bad bigint"),
        (b"\x22\x05ab", "input too short"),
    ];
    for (body, message) in cases {
        assert_eq!(
            render(&frame(body), Limits::default()),
            format!("error: {message}"),
            "{body:?}"
        );
    }
    assert_eq!(
        render(&[0x00, 0x0F, b'0'], Limits::default()),
        "error: bad header"
    );
    assert_eq!(
        render(&[0xFF, 0x0C, b'0'], Limits::default()),
        "error: unsupported wire format version 12"
    );
    assert_eq!(
        render(&frame(b"^\x05"), Limits::default()),
        "error: no object 5"
    );
}

#[test]
fn a_dense_array_keeps_its_holes() {
    assert_eq!(
        render(&frame(b"A\x02-I\x02$\x00\x02"), Limits::default()),
        "[hole, 1]"
    );
}

#[test]
fn values_of_one_frame_share_object_ids() {
    let mut bytes = Vec::new();
    let mut writer = Writer::new(&mut bytes);
    writer.begin_object();
    writer.end_object(0);
    writer.object_ref(0);
    let mut tree = Tree::default();
    let root = decode_values(&bytes, Limits::default(), &mut tree).expect("decodes");
    assert_eq!(tree.render(root), "values(#0={}, @0)");
}

#[test]
fn writer_matches_mini_racers_encoder() {
    let encode = |f: &dyn Fn(&mut Writer<'_>)| {
        let mut bytes = Vec::new();
        f(&mut Writer::new(&mut bytes));
        bytes
    };
    assert_eq!(encode(&|w| w.integer(-2)), frame(b"I\x03"));
    assert_eq!(encode(&|w| w.integer(1 << 31)).get(2), Some(&b'N'));
    assert_eq!(
        encode(&|w| w.integer(i64::MAX)),
        frame(b"Z\x10\xff\xff\xff\xff\xff\xff\xff\x7f")
    );
    assert_eq!(
        encode(&|w| w.integer(i64::MIN)),
        frame(b"Z\x11\x00\x00\x00\x00\x00\x00\x00\x80")
    );
    assert_eq!(
        encode(&|w| w.number(f64::NAN)),
        frame(b"N\x00\x00\x00\x00\x00\x00\xf8\x7f")
    );
    assert_eq!(
        encode(&|w| w.date(f64::INFINITY)),
        frame(b"D\x00\x00\x00\x00\x00\x00\xf8\x7f")
    );
    assert_eq!(
        encode(&|w| w.bigint(false, &[0; 8]).expect("word")),
        frame(b"Z\x00")
    );
    assert_eq!(
        encode(&|w| w
            .bigint(true, &[5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])
            .expect("words")),
        frame(b"Z\x11\x05\x00\x00\x00\x00\x00\x00\x00")
    );
    assert_eq!(
        encode(&|w| w.uint8_array(b"ab")),
        frame(b"B\x02abVB\x00\x02\x00")
    );
    let mut out = Vec::new();
    assert!(Writer::new(&mut out).bigint(false, &[1; 3]).is_err());
}
