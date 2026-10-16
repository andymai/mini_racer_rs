mod common;

use common::{Node, Tree, render};
use mini_racer_wire::{Limits, Writer, decode_value, decode_values};

fn body(body: &[u8]) -> String {
    render(&frame(body), Limits::default())
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
    for (input, message) in cases {
        assert_eq!(body(input), format!("error: {message}"), "{input:?}");
    }
    assert_eq!(
        render(&[0x00, 0x0F, b'0'], Limits::default()),
        "error: bad header"
    );
    assert_eq!(
        render(&[0xFF, 0x0C, b'0'], Limits::default()),
        "error: unsupported wire format version 12"
    );
    assert_eq!(body(b"^\x00"), "error: bad object reference 0");
}

#[test]
fn regexp_patterns_and_error_fields_count_as_nesting() {
    let flat = Limits {
        max_depth: 0,
        ..Limits::default()
    };
    assert_eq!(
        render(&frame(b"RS\x01a\x01"), flat),
        "error: too much recursion"
    );
    assert_eq!(
        render(&frame(b"rmS\x01x."), flat),
        "error: too much recursion"
    );
    assert_eq!(render(&frame(b"r."), flat), "error{}");
    assert_eq!(render(&frame(b"sS\x01a"), flat), "\"a\"");
}

#[test]
fn checks_run_in_mini_racers_order() {
    assert_eq!(body(b"c\x03ab"), "error: input too short");
    assert_eq!(body(b"c\x03abc"), "error: bad utf16 string size");
    assert_eq!(body(b"Z\x03\x00"), "error: bad bigint");
    assert_eq!(body(b"Z\x10\x01"), "error: input too short");
    let tight = Limits {
        max_bigint_bytes: 8,
        ..Limits::default()
    };
    assert_eq!(
        render(&frame(b"Z\x20\x01"), tight),
        "error: input too short"
    );
    assert_eq!(
        render(&frame(&[b"Z\x20".as_slice(), &[1; 16]].concat()), tight),
        "error: bigint too big"
    );
}

#[test]
fn varints_hold_at_most_64_bits() {
    let len = |last: u8| [b"S".as_slice(), &[0xFF; 9], &[last]].concat();
    assert_eq!(body(&len(0x01)), "error: input too short");
    assert_eq!(body(&len(0x02)), "error: bad varint");
    assert_eq!(
        body(&[b"S".as_slice(), &[0xFF; 10]].concat()),
        "error: bad varint"
    );
}

#[test]
fn versions_13_to_16_are_read() {
    assert_eq!(render(&[0xFF, 0x0D, b'0'], Limits::default()), "null");
    assert_eq!(render(&[0xFF, 0x10, b'0'], Limits::default()), "null");
    assert_eq!(
        render(&[0xFF, 0x11, b'0'], Limits::default()),
        "error: unsupported wire format version 17"
    );
}

#[test]
fn padding_and_object_counts_before_a_value_are_skipped() {
    assert_eq!(body(b"\x00\x00I\x02"), "1");
    assert_eq!(body(b"?\x03I\x02"), "1");
    assert_eq!(body(b"R\x00\x00S\x01a\x01"), "regexp(\"a\", 1)");
}

#[test]
fn every_nan_reads_as_the_quiet_nan() {
    for tag in *b"NnD" {
        let mut bytes = frame(&[tag]);
        bytes.extend_from_slice(&0x7FF0_0000_0000_0001u64.to_le_bytes());
        let mut tree = Tree::default();
        let root = decode_value(&bytes, Limits::default(), &mut tree).expect("decodes");
        let (Node::Number(n) | Node::Date(n)) = tree.nodes[root] else {
            panic!("not a number: {:?}", tree.nodes[root]);
        };
        assert_eq!(n.to_bits(), 0x7FF8_0000_0000_0000, "{}", char::from(tag));
    }
}

#[test]
fn views_follow_v8s_alignment_and_flag_rules() {
    assert_eq!(body(b"B\x04abcdVW\x02\x02\x00"), "buffer(61626364)");
    assert_eq!(body(b"B\x04abcdVW\x01\x02\x00"), "error: bad typed array");
    assert_eq!(body(b"B\x04abcdVD\x00\x02\x00"), "error: bad typed array");
    assert_eq!(body(b"B\x02abVB\x00\x02\x01"), "error: bad typed array");
    assert_eq!(body(b"~\x02\x04abVB\x00\x02\x03"), "buffer(6162)");
    assert_eq!(body(b"~\x02\x04abVB\x00\x02\x02"), "buffer(6162)");
    assert_eq!(body(b"~\x02\x04abVB\x00\x02\x01"), "error: bad typed array");
    assert_eq!(body(b"~\x02\x04abVB\x00\x02\x00"), "error: bad typed array");
    assert_eq!(body(b"~\x04\x02abcd"), "error: bad arraybuffer");
}

#[test]
fn regexp_flags_follow_v8s_rules() {
    assert_eq!(body(b"RS\x01a\x01"), "regexp(\"a\", 1)");
    assert_eq!(body(b"RS\x01a\x80\x04"), "error: bad regexp");
    assert_eq!(body(b"RS\x01a\x40"), "error: bad regexp");
    assert_eq!(body(b"RS\x01a\x90\x02"), "error: bad regexp");
}

#[test]
fn a_dense_array_longer_than_its_input_fails_first() {
    assert_eq!(body(b"A\x7f"), "error: input too short");
}

#[test]
fn an_error_reads_its_fields_in_v8s_order() {
    assert_eq!(
        body(b"rRmS\x01xc^\x00."),
        "#0=error{\"message\": \"x\", \"cause\": @0}"
    );
    assert_eq!(body(b"rcI\x02mS\x01x."), "error: bad error object");
    assert_eq!(body(b"rsS\x01xmS\x01y."), "error: bad error object");
    assert_eq!(body(b"rmsS\x01x."), "error: error .message is not a string");
}

#[test]
fn a_reference_inside_an_object_finds_it() {
    assert_eq!(body(b"oS\x01a^\x00{\x01"), "#0={\"a\": @0}");
    assert_eq!(body(b"oS\x01a^\x01{\x01"), "error: bad object reference 1");
}

#[test]
fn writer_ids_match_the_ids_v8_reads() {
    let mut bytes = Vec::new();
    let mut writer = Writer::new(&mut bytes);
    let array = writer.begin_array(5);
    let date = writer.date(1.0);
    let binary = writer.uint8_array(b"ab");
    writer.object_ref(date);
    writer.object_ref(binary);
    writer.object_ref(array);
    writer.end_array(5);
    assert_eq!((array, date, binary), (0, 1, 3));
    let mut tree = Tree::default();
    let root = decode_value(&bytes, Limits::default(), &mut tree).expect("decodes");
    assert_eq!(
        tree.render(root),
        "#0=[date(1.0), #1=buffer(6162), date(1.0), @1, @0]"
    );
    let Node::Container { items, .. } = &tree.nodes[root] else {
        panic!("not an array");
    };
    assert_eq!(items[0], items[2]);
}

#[test]
fn a_dense_array_keeps_its_holes() {
    assert_eq!(body(b"A\x02-I\x02$\x00\x02"), "[hole, 1]");
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
        encode(&|w| {
            w.date(f64::INFINITY);
        }),
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
        encode(&|w| {
            w.uint8_array(b"ab");
        }),
        frame(b"B\x02abVB\x00\x02\x00")
    );
    let mut out = Vec::new();
    assert!(Writer::new(&mut out).bigint(false, &[1; 3]).is_err());
}
