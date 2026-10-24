mod common;

use common::{Kind, Node, Tree, render, render_keys, render_runs};
use mini_racer_wire::{Limits, Writer, decode_value};

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
fn bigints_up_to_the_limit_are_read_and_past_it_refused() {
    let mut bytes = Vec::new();
    Writer::new(&mut bytes)
        .bigint(false, &[1; 16])
        .expect("whole words");
    let tight = Limits {
        max_bigint_bytes: 8,
        ..Limits::default()
    };
    assert_eq!(render(&bytes, tight), "error: bigint too big");
    let exact = Limits {
        max_bigint_bytes: 16,
        ..Limits::default()
    };
    assert_eq!(render(&bytes, exact), "0x01010101010101010101010101010101n");
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
    assert_eq!(body(b"~\x02\x02ab"), "buffer(6162)");
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

// An int32-keyed property after the last element is encoded like two more
// elements, so only the array's length ends a run there.
#[test]
fn a_dense_run_stops_at_the_arrays_length() {
    assert_eq!(body(b"A\x01I\x02I\x00I\x04$\x01\x01"), "[1, 0: 2]");
    let elements = [double(1.5), double(2.5)].concat();
    let property = [int32(0), double(3.5)].concat();
    assert_eq!(
        body(&[b"A\x02".as_slice(), &elements, &property, b"$\x01\x02"].concat()),
        "[num(1.5), num(2.5), 0: num(3.5)]"
    );
}

fn varint(mut value: u64) -> Vec<u8> {
    let mut bytes = Vec::new();
    loop {
        let low = u8::try_from(value & 0x7F).unwrap_or_default();
        value >>= 7;
        if value == 0 {
            bytes.push(low);
            return bytes;
        }
        bytes.push(low | 0x80);
    }
}

// The bytes `write` puts after a frame's header.
fn element(write: impl FnOnce(&mut Writer<'_>)) -> Vec<u8> {
    let mut bytes = Vec::new();
    write(&mut Writer::new(&mut bytes));
    bytes.split_off(2)
}

fn int32(n: i32) -> Vec<u8> {
    element(|writer| writer.integer(n.into()))
}

fn double(n: f64) -> Vec<u8> {
    element(|writer| writer.number(n))
}

fn dense(elements: &[Vec<u8>]) -> Vec<u8> {
    let length = varint(u64::try_from(elements.len()).unwrap_or_default());
    [
        b"A".as_slice(),
        &length,
        &elements.concat(),
        b"$\x00",
        &length,
    ]
    .concat()
}

fn sparse(length: u64, entries: &[[Vec<u8>; 2]]) -> Vec<u8> {
    let count = varint(u64::try_from(entries.len()).unwrap_or_default());
    let length = varint(length);
    let entries = entries.concat().concat();
    [b"a".as_slice(), &length, &entries, b"@", &count, &length].concat()
}

#[test]
fn runs_of_numbers_span_the_buffer() {
    let ints: Vec<_> = (0..150).map(|n| int32(n * 1000 - 70_000)).collect();
    let (rendered, runs) = render_runs(&frame(&dense(&ints)), Limits::default());
    let expected: Vec<_> = (0..150).map(|n| (n * 1000 - 70_000).to_string()).collect();
    assert_eq!(rendered, format!("[{}]", expected.join(", ")));
    assert_eq!(runs, [("int32s", 64), ("int32s", 64), ("int32s", 22)]);

    let doubles: Vec<_> = (0..129).map(|n| double(f64::from(n) + 0.5)).collect();
    let (rendered, runs) = render_runs(&frame(&dense(&doubles)), Limits::default());
    let expected: Vec<_> = (0..129).map(|n| format!("num({n}.5)")).collect();
    assert_eq!(rendered, format!("[{}]", expected.join(", ")));
    assert_eq!(runs, [("numbers", 64), ("numbers", 64), ("numbers", 1)]);

    let entries: Vec<_> = (0..130).map(|n| [int32(n), int32(-n)]).collect();
    let (rendered, runs) = render_runs(&frame(&sparse(130, &entries)), Limits::default());
    let expected: Vec<_> = (0..130).map(|n| format!("{n}: {}", -n)).collect();
    assert_eq!(rendered, format!("sparse[{}]", expected.join(", ")));
    assert_eq!(
        runs,
        [
            ("int32 entries", 64),
            ("int32 entries", 64),
            ("int32 entries", 2)
        ]
    );
}

#[test]
fn a_run_ends_at_any_other_element() {
    let elements = [
        int32(1),
        int32(2),
        double(1.5),
        double(-0.0),
        b"\"\x01a".to_vec(),
        int32(3),
        b"0".to_vec(),
        double(4.5),
        b"-".to_vec(),
        int32(5),
        int32(6),
    ];
    let (rendered, runs) = render_runs(&frame(&dense(&elements)), Limits::default());
    assert_eq!(
        rendered,
        "[1, 2, num(1.5), num(-0), \"a\", 3, null, num(4.5), hole, 5, 6]"
    );
    assert_eq!(
        runs,
        [
            ("int32s", 2),
            ("numbers", 2),
            ("int32s", 1),
            ("numbers", 1),
            ("int32s", 2)
        ]
    );
}

#[test]
fn padding_before_an_element_leaves_it_to_the_general_path() {
    let padded = [b"\x00".as_slice(), &int32(2)].concat();
    let elements = [int32(1), padded, int32(3), int32(4)];
    let (rendered, runs) = render_runs(&frame(&dense(&elements)), Limits::default());
    assert_eq!(rendered, "[1, 2, 3, 4]");
    assert_eq!(runs, [("int32s", 1), ("int32s", 2)]);
}

fn fails_after_runs(input: &[&[u8]], message: &str, handed: &[(&str, usize)]) {
    let (rendered, runs) = render_runs(&frame(&input.concat()), Limits::default());
    assert_eq!(rendered, format!("error: {message}"), "{input:?}");
    assert_eq!(runs, handed, "{input:?}");
}

#[test]
fn a_run_hands_over_what_it_read_before_the_input_fails() {
    let (one, two) = (int32(1), int32(2));
    fails_after_runs(
        &[b"A\x04", &one, &two, b"I"],
        "input too short",
        &[("int32s", 2)],
    );
    let (half, more) = (double(1.5), double(2.5));
    fails_after_runs(
        &[b"A\x03", &half, &more, b"N\x00\x00"],
        "input too short",
        &[("numbers", 2)],
    );
    fails_after_runs(
        &[b"A\x03", &one, b"I", &[0xFF; 10]],
        "bad varint",
        &[("int32s", 1)],
    );
    fails_after_runs(
        &[b"A\x03", &one, b"I\x80\x80\x80\x80\x10"],
        "bad varint",
        &[("int32s", 1)],
    );
    let zero = int32(0);
    fails_after_runs(
        &[b"a\x03", &zero, &one, &one, b"I"],
        "input too short",
        &[("int32 entries", 1)],
    );
}

#[test]
fn every_prefix_of_a_run_is_too_short() {
    let elements: Vec<_> = (0..70)
        .map(|n| match n % 7 {
            3 => double(f64::from(n) / 4.0),
            5 => element(|writer| writer.utf8(b"text")),
            _ => int32(n * 40_000),
        })
        .collect();
    let entries: Vec<_> = (0..70)
        .zip(&elements)
        .map(|(n, element)| [int32(n), element.clone()])
        .collect();
    for whole in [frame(&dense(&elements)), frame(&sparse(70, &entries))] {
        assert!(!render(&whole, Limits::default()).starts_with("error"));
        for end in 0..whole.len() {
            assert_eq!(
                render(&whole[..end], Limits::default()),
                "error: input too short",
                "{end}"
            );
        }
    }
}

#[test]
fn a_run_checks_the_depth_as_its_first_element_would() {
    let flat = Limits {
        max_depth: 0,
        ..Limits::default()
    };
    let too_deep = "error: too much recursion";
    assert_eq!(
        render(&frame(&dense(&[int32(1), int32(2)])), flat),
        too_deep
    );
    assert_eq!(render(&frame(&dense(&[double(1.5)])), flat), too_deep);
    let entry = [int32(0), double(1.5)];
    assert_eq!(render(&frame(&sparse(1, &[entry])), flat), too_deep);
    let keyed = [int32(0), b"S\x01x".to_vec()];
    assert_eq!(render(&frame(&sparse(1, &[keyed])), flat), too_deep);
    let holes = [b"-".to_vec(), b"-".to_vec()];
    assert_eq!(render(&frame(&dense(&holes)), flat), "[hole, hole]");
}

#[test]
fn counts_after_a_run_are_checked() {
    let two = [int32(1), int32(2)].concat();
    assert_eq!(
        body(&[b"A\x02".as_slice(), &two, b"$\x00\x03"].concat()),
        "error: array element count mismatch"
    );
    let entries = [int32(0), int32(1), int32(1), int32(2)].concat();
    assert_eq!(
        body(&[b"a\x02".as_slice(), &entries, b"@\x03\x02"].concat()),
        "error: element count mismatch"
    );
}

#[test]
fn sparse_runs_take_int32_keys_with_number_values() {
    let entries = [
        [int32(0), int32(1)],
        [int32(1), int32(2)],
        [b"S\x01x".to_vec(), int32(3)],
        [int32(4), int32(5)],
        [int32(1000), double(-0.0)],
        [int32(-3), double(2.5)],
        [int32(7), b"S\x01y".to_vec()],
        [int32(8), [b"\x00".as_slice(), &int32(9)].concat()],
    ];
    let (rendered, runs) = render_runs(&frame(&sparse(1001, &entries)), Limits::default());
    assert_eq!(
        rendered,
        "sparse[0: 1, 1: 2, \"x\": 3, 4: 5, 1000: num(-0), -3: num(2.5), 7: \"y\", 8: 9]"
    );
    assert_eq!(
        runs,
        [
            ("int32 entries", 2),
            ("int32 entries", 1),
            ("number entries", 2)
        ]
    );
}

#[test]
fn string_keys_reach_key_with_their_container() {
    let object =
        b"o\"\x01aI\x02S\x01b;\"\x01cI\x04I\x06I\x08:\x04I\x02c\x02\x00\x01\x00c\x02A\x00T{\x04";
    let (rendered, keys) = render_keys(&frame(object), Limits::default());
    assert_eq!(
        rendered,
        "{\"a\": 1, \"b\": map{\"c\": 2, 3: 4}, 1: \"\u{100}\", \"A\": true}"
    );
    let named = |kind, key: &str| (kind, key.to_owned());
    assert_eq!(
        keys,
        [
            named(Kind::Object, "a"),
            named(Kind::Object, "b"),
            named(Kind::Map, "c"),
            named(Kind::Object, "A"),
        ]
    );
    let arrays = b"A\x01a\x01\"\x01yI\x02@\x01\x01\"\x01xI\x04$\x01\x01";
    let (rendered, keys) = render_keys(&frame(arrays), Limits::default());
    assert_eq!(rendered, "[sparse[\"y\": 1], \"x\": 2]");
    assert_eq!(keys, [named(Kind::Sparse, "y"), named(Kind::Dense, "x")]);
    let (rendered, keys) = render_keys(&frame(b"rmS\x01xsS\x01y."), Limits::default());
    assert_eq!(rendered, "error{\"message\": \"x\", \"stack\": \"y\"}");
    assert_eq!(keys, []);
}

#[test]
fn a_key_past_the_depth_limit_fails_before_it_is_read() {
    let shallow = Limits {
        max_depth: 0,
        ..Limits::default()
    };
    let (rendered, keys) = render_keys(&frame(b"o\"\x01aI\x02{\x01"), shallow);
    assert_eq!(rendered, "error: too much recursion");
    assert_eq!(keys, []);
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
    // mini_racer writes a double above INT64_MIN / 1024, which is -(2**53),
    // and up to INT64_MAX / 1024, which is 2**53 - 1.
    let safe = (1 << 53) - 1;
    assert_eq!(
        encode(&|w| w.integer(safe)),
        frame(b"N\xff\xff\xff\xff\xff\xff\x3f\x43")
    );
    assert_eq!(
        encode(&|w| w.integer(-safe)),
        frame(b"N\xff\xff\xff\xff\xff\xff\x3f\xc3")
    );
    assert_eq!(
        encode(&|w| w.integer(safe + 1)),
        frame(b"Z\x10\x00\x00\x00\x00\x00\x00\x20\x00")
    );
    assert_eq!(
        encode(&|w| w.integer(-safe - 1)),
        frame(b"Z\x11\x00\x00\x00\x00\x00\x00\x20\x00")
    );
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

#[test]
fn a_versioned_frame_carries_its_version() {
    let mut bytes = Vec::new();
    Writer::versioned(&mut bytes, 16).null();
    assert_eq!(bytes, [0xFF, 0x10, b'0']);
    assert_eq!(render(&bytes, Limits::default()), "null");
}

// V8 pads before a two-byte string whose characters would start at an odd
// offset, which turns on where the string begins and how long its length is.
#[test]
fn aligned_two_byte_strings_pad_where_v8_pads_and_decode_through_it() {
    for (units, padded) in [
        (1, false),
        (63, false),
        (64, true),
        (8191, true),
        (8192, false),
    ] {
        let text: Vec<u16> = std::iter::repeat_n(0x0100, units).collect();
        let mut bytes = Vec::new();
        Writer::new(&mut bytes).utf16_aligned(&text);
        assert_eq!(bytes.get(2) == Some(&0x00), padded, "{units} units");
        let start = bytes
            .iter()
            .position(|&byte| byte == b'c')
            .expect("a two-byte tag");
        let length_bytes = bytes.len() - start - 1 - 2 * units;
        assert!(
            (start + 1 + length_bytes).is_multiple_of(2),
            "{units} units"
        );
        assert_eq!(
            render(&bytes, Limits::default()),
            format!("{:?}", "\u{100}".repeat(units))
        );
    }
    let mut bytes = Vec::new();
    let mut writer = Writer::new(&mut bytes);
    writer.begin_array(1);
    writer.utf16_aligned(&[0x0100]);
    writer.end_array(1);
    assert_eq!(render(&bytes, Limits::default()), "[\"\u{100}\"]");
}
