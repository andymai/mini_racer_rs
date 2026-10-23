#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers fail the test by panicking"
)]

use std::time::{Duration, Instant};

use mini_racer_engine::{
    ContextConfig, ContextHandle, EngineError, Host, InvalidSnapshot, SnapshotBlob, WaitHandle,
    WaitOutcome,
};
use mini_racer_wire::{Begun, Container, Limits, Scalar, Visitor, decode_value};

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

struct Int;

impl Visitor for Int {
    type Value = i64;
    type Frame = ();
    type Error = String;

    fn scalar(&mut self, scalar: Scalar<'_>) -> Result<i64, String> {
        match scalar {
            Scalar::Int32(n) => Ok(n.into()),
            other => Err(format!("unexpected {other:?}")),
        }
    }

    fn begin(&mut self, container: Container) -> Result<Begun<(), i64>, String> {
        Err(format!("unexpected {container:?}"))
    }

    fn push(&mut self, (): &mut (), _value: i64) -> Result<(), String> {
        Ok(())
    }

    fn insert(&mut self, (): &mut (), _key: i64, _value: i64) -> Result<(), String> {
        Ok(())
    }

    fn end(&mut self, (): ()) -> Result<i64, String> {
        Err("unexpected container".to_owned())
    }

    fn register(&mut self, _id: u32, _value: i64) -> Result<(), String> {
        Ok(())
    }

    fn back_ref(&mut self, id: u32) -> Result<i64, String> {
        Err(format!("unexpected reference {id}"))
    }
}

fn snapshot(code: &str, base: Option<Vec<u8>>) -> Result<Vec<u8>, EngineError> {
    let base = base.map(|blob| SnapshotBlob::new(&blob).expect("a blob V8 wrote"));
    ContextHandle::create_snapshot(code.into(), base, &mut Parking)
}

fn spawn_from(blob: &SnapshotBlob) -> ContextHandle {
    let config = ContextConfig {
        snapshot: Some(blob.clone()),
        ..ContextConfig::default()
    };
    ContextHandle::spawn(config, &mut Parking).expect("spawn")
}

fn eval(context: &ContextHandle, source: &str) -> i64 {
    let bytes = context
        .eval("<eval>".to_owned(), source.to_owned(), &mut Parking)
        .expect("eval");
    decode_value(&bytes, Limits::default(), &mut Int).expect("an integer")
}

fn eval_in(blob: &[u8], source: &str) -> i64 {
    let context = spawn_from(&SnapshotBlob::new(blob).expect("a blob V8 wrote"));
    eval(&context, source)
}

fn word(blob: &[u8], at: usize) -> usize {
    let bytes = blob.get(at..at + 4).expect("a header word");
    u32::from_le_bytes(bytes.try_into().expect("four bytes")) as usize
}

fn set_word(blob: &mut [u8], at: usize, value: u32) {
    blob.get_mut(at..at + 4)
        .expect("a header word")
        .copy_from_slice(&value.to_le_bytes());
}

fn changed(blob: &[u8], change: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let mut changed = blob.to_vec();
    change(&mut changed);
    changed
}

// V8's checksum of what follows the checksum's own word, with the modulus of
// zlib's Adler-32 or of V8's own Fletcher-32.
fn checksum(blob: &[u8], modulus: u32) -> u32 {
    let (mut low, mut high) = (0_u32, 0_u32);
    for &byte in &blob[12..] {
        low = (low + u32::from(byte)) % modulus;
        high = (high + low) % modulus;
    }
    high << 16 | low
}

const ADLER: u32 = 65_521;
const FLETCHER: u32 = 65_535;

// Changed, then given the checksum that matches, as a blob damaged on purpose
// would be.
fn resummed(blob: &[u8], change: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let mut changed = changed(blob, change);
    let sum = checksum(&changed, FLETCHER);
    set_word(&mut changed, 8, sum);
    changed
}

#[test]
fn a_context_starts_from_its_snapshot() {
    let blob =
        snapshot("var base = 40; function add(x) { return base + x }", None).expect("snapshot");
    assert_eq!(eval_in(&blob, "add(2)"), 42);
}

#[test]
fn a_warmup_keeps_the_snapshot_and_none_of_its_own_state() {
    let blob = snapshot("var a = 5; function f(x) { return x * 2 }", None).expect("snapshot");
    let warmed = snapshot("f(21); var a = 7; var b = 1;", Some(blob)).expect("warmup");
    assert_eq!(eval_in(&warmed, "f(a)"), 10);
    assert_eq!(eval_in(&warmed, "typeof b === 'undefined' ? 1 : 0"), 1);
}

#[test]
fn a_failing_script_reports_mini_racers_message_and_place() {
    let Err(EngineError::Snapshot(message)) = snapshot("var foo = bar;", None) else {
        panic!("expected a snapshot error");
    };
    assert_eq!(
        message,
        "Uncaught ReferenceError: bar is not defined\n<snapshot>:1:10"
    );
    let Err(EngineError::Snapshot(message)) = snapshot("(", None) else {
        panic!("expected a snapshot error");
    };
    assert_eq!(
        message,
        "Uncaught SyntaxError: Unexpected end of input\n<snapshot>:1:1"
    );
}

#[test]
fn every_blob_v8_writes_is_accepted() {
    let built = snapshot("var a = 1", None).expect("snapshot");
    let empty = snapshot("", None).expect("snapshot");
    let warmed = snapshot("a + 1", Some(built.clone())).expect("warmup");
    let from_v8s_own = snapshot("1", Some(Vec::new())).expect("warmup");
    for blob in [&built, &empty, &warmed, &from_v8s_own] {
        assert_eq!(SnapshotBlob::check(blob), Ok(()));
        assert_eq!(word(blob, 8), checksum(blob, FLETCHER) as usize);
    }
    assert_eq!(SnapshotBlob::check(&[]), Ok(()));
}

struct Layout {
    blob: Vec<u8>,
    startup: usize,
    read_only: usize,
    shared_heap: usize,
    last: usize,
}

fn layout() -> Layout {
    let blob = snapshot("var a = 1", None).expect("snapshot");
    let contexts = word(&blob, 0);
    Layout {
        startup: (88 + 4 * contexts).next_multiple_of(8),
        read_only: word(&blob, 80),
        shared_heap: word(&blob, 84),
        last: word(&blob, 88 + 4 * (contexts - 1)),
        blob,
    }
}

fn offset(at: usize) -> u32 {
    u32::try_from(at).expect("an offset")
}

// A section between `start` and `end` whose recorded payload runs a byte past
// it, with the checksum that matches.
fn overlong(blob: &[u8], start: usize, end: usize) -> Vec<u8> {
    resummed(blob, |b| {
        set_word(b, start + 4, offset(end - start - 8 + 1));
    })
}

fn assert_refused(refused: &[(Vec<u8>, InvalidSnapshot)]) {
    for (index, (bytes, invalid)) in refused.iter().enumerate() {
        assert_eq!(SnapshotBlob::check(bytes), Err(*invalid), "case {index}");
        assert_eq!(
            SnapshotBlob::new(bytes).err(),
            Some(*invalid),
            "case {index}"
        );
    }
}

// mini_racer's checks of the length, the version, the header and the offsets,
// in its order and with its messages.
#[test]
fn refuses_a_header_as_mini_racer_does() {
    let Layout {
        blob,
        startup,
        read_only,
        shared_heap,
        ..
    } = layout();
    let len = blob.len();
    let half = len / 2;
    let cut_in_half = if read_only > half {
        InvalidSnapshot::ReadOnly
    } else if shared_heap > half {
        InvalidSnapshot::SharedHeap
    } else {
        InvalidSnapshot::Context
    };
    let mut random = vec![0_u8; len];
    let mut state = 0x9E37_79B9_u32;
    for byte in &mut random {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        *byte = state.to_le_bytes()[0];
    }
    assert_refused(&[
        (changed(&blob, |b| b[16] ^= 1), InvalidSnapshot::Version),
        (random, InvalidSnapshot::Version),
        (b"garbage".to_vec(), InvalidSnapshot::TooShort),
        (blob[..87].to_vec(), InvalidSnapshot::TooShort),
        (blob[..88].to_vec(), InvalidSnapshot::ContextTable),
        (
            changed(&blob, |b| set_word(b, 4, 2)),
            InvalidSnapshot::Header,
        ),
        (
            changed(&blob, |b| set_word(b, 0, 0)),
            InvalidSnapshot::NoContexts,
        ),
        (
            changed(&blob, |b| set_word(b, 0, u32::MAX)),
            InvalidSnapshot::ContextTable,
        ),
        (
            changed(&blob, |b| set_word(b, 80, offset(startup))),
            InvalidSnapshot::ReadOnly,
        ),
        (
            changed(&blob, |b| set_word(b, 84, 1 << 30)),
            InvalidSnapshot::SharedHeap,
        ),
        (
            changed(&blob, |b| {
                set_word(b, 80, offset(shared_heap));
                set_word(b, 84, offset(read_only));
            }),
            InvalidSnapshot::SharedHeap,
        ),
        (
            changed(&blob, |b| set_word(b, 88, offset(len))),
            InvalidSnapshot::Context,
        ),
        (blob[..half].to_vec(), cut_in_half),
    ]);
}

// mini_racer's checksum; then, past it, a section V8 aborts on or reads past
// the end of, whose checksum anyone can make match, which mini_racer hands V8.
#[test]
fn refuses_a_damaged_blob_as_mini_racer_does_and_one_v8_aborts_on() {
    let Layout {
        blob,
        startup,
        read_only,
        shared_heap,
        last,
    } = layout();
    let len = blob.len();
    let mut refused = vec![
        (
            changed(&blob, |b| b[startup] ^= 1),
            InvalidSnapshot::Checksum,
        ),
        (changed(&blob, |b| b[last] ^= 1), InvalidSnapshot::Checksum),
        (changed(&blob, |b| b[8] ^= 1), InvalidSnapshot::Checksum),
        (
            changed(&blob, |b| b.extend_from_slice(b"\n")),
            InvalidSnapshot::Checksum,
        ),
        (
            resummed(&blob, |b| b[startup + 3] ^= 1),
            InvalidSnapshot::Startup,
        ),
        (
            resummed(&blob, |b| b[read_only + 3] ^= 1),
            InvalidSnapshot::ReadOnly,
        ),
        (
            resummed(&blob, |b| set_word(b, read_only + 4, 1 << 31)),
            InvalidSnapshot::ReadOnly,
        ),
        (
            overlong(&blob, startup, read_only),
            InvalidSnapshot::Startup,
        ),
        (
            overlong(&blob, read_only, shared_heap),
            InvalidSnapshot::ReadOnly,
        ),
        (
            overlong(&blob, shared_heap, word(&blob, 88)),
            InvalidSnapshot::SharedHeap,
        ),
        (overlong(&blob, last, len), InvalidSnapshot::Context),
        (
            resummed(&blob, |b| b[shared_heap] ^= 1),
            InvalidSnapshot::SharedHeap,
        ),
        (resummed(&blob, |b| b[last] ^= 1), InvalidSnapshot::Context),
    ];
    for cut in [last + 8, last + 4, len - 1] {
        refused.push((blob[..cut].to_vec(), InvalidSnapshot::Checksum));
    }
    assert_refused(&refused);
}

// Once its checksum matches, what a section holds within its bounds is V8's
// to read, as it is in mini_racer: bytes past the last section's payload, a
// payload length up to the section's end, a damaged byte. Either checksum
// mini_racer accepts will do.
#[test]
fn leaves_what_a_section_holds_to_v8_once_its_checksum_matches() {
    let blob = snapshot("var a = 41", None).expect("snapshot");
    let longer = resummed(&blob, |b| b.extend_from_slice(b"\n"));
    assert_eq!(eval_in(&longer, "a + 1"), 42);
    let last = word(&blob, 88 + 4 * (word(&blob, 0) - 1));
    let filled = resummed(&blob, |b| {
        b.extend_from_slice(&[0; 16]);
        let room = offset(b.len() - last - 8);
        set_word(b, last + 4, room);
    });
    assert_eq!(SnapshotBlob::check(&filled), Ok(()));
    assert_eq!(eval_in(&filled, "a + 1"), 42);
    let read_only = word(&blob, 80);
    let damaged = resummed(&blob, |b| b[read_only + 4096] ^= 1);
    assert_eq!(SnapshotBlob::check(&damaged), Ok(()));
    let adler = changed(&blob, |b| set_word(b, 8, checksum(&blob, ADLER)));
    assert_eq!(eval_in(&adler, "a + 1"), 42);
}

// mini_racer warms an empty snapshot up from V8's own, keeping none of the
// warmup's state.
#[test]
fn a_warmup_from_an_empty_blob_starts_from_v8s_own() {
    let warmed =
        snapshot("var b = 1; function g() { return 2 } g()", Some(Vec::new())).expect("warmup");
    assert_ne!(warmed.len(), 0);
    assert_eq!(eval_in(&warmed, "typeof b === 'undefined' ? 1 : 0"), 1);
    assert_eq!(eval_in(&warmed, "1 + 1"), 2);
}

fn refused(code: &str) -> String {
    match snapshot(code, None) {
        Err(EngineError::Snapshot(message)) => message,
        other => panic!("{code}: expected a snapshot error, got {other:?}"),
    }
}

// mini_racer refuses a script that reads or writes the global Intl, whose
// objects V8 cannot write into a blob, and puts Intl back after one that does
// not; the locale methods of other built-ins never read it.
#[test]
fn a_script_may_not_touch_intl() {
    const REFUSED: &str = "Intl is not supported in Snapshot.new scripts";
    for code in [
        "typeof Intl",
        "Intl = 1",
        "globalThis.x = new Intl.DateTimeFormat('en')",
        "let c = new Intl.Collator('en')",
    ] {
        assert_eq!(refused(code), REFUSED, "{code}");
    }
    assert_eq!(
        refused("Intl; throw 1"),
        "Uncaught 1\n<snapshot>:1:6",
        "the script's own error comes first"
    );
    for code in [
        "(1234.5).toLocaleString('en')",
        "'a'.localeCompare('b', 'en')",
        "var d = Object.getOwnPropertyDescriptor(globalThis, 'Intl')",
    ] {
        let blob = snapshot(code, None).expect(code);
        let restored = "var d = Object.getOwnPropertyDescriptor(globalThis, 'Intl'); \
                        d.writable && !d.enumerable && d.configurable && \
                        typeof Intl.DateTimeFormat === 'function' ? 1 : 0";
        assert_eq!(eval_in(&blob, restored), 1, "{code}");
    }
    let deleted = snapshot("delete globalThis.Intl", None).expect("delete");
    assert_eq!(eval_in(&deleted, "typeof Intl === 'undefined' ? 1 : 0"), 1);
    let replaced = snapshot(
        "Object.defineProperty(globalThis, 'Intl', { configurable: true, get() { return 7 } })",
        None,
    )
    .expect("replace");
    assert_eq!(eval_in(&replaced, "Intl"), 7);
    let kept = snapshot(
        "globalThis.d = Object.getOwnPropertyDescriptor(globalThis, 'Intl')",
        None,
    )
    .expect("descriptor");
    assert_eq!(eval_in(&kept, "d.get() === Intl ? 1 : 0"), 1);
}

#[test]
fn a_warmup_may_touch_intl() {
    let blob = snapshot("function f() { return 1 }", None).expect("snapshot");
    let warmed = snapshot(
        "var formatter = new Intl.DateTimeFormat('en'); f()",
        Some(blob),
    )
    .expect("warmup");
    assert_eq!(
        eval_in(&warmed, "typeof formatter === 'undefined' ? f() : 0"),
        1
    );
}

// Every context reads the one blob, which outlives whoever made it and goes
// with the last context. V8 reads a freed blob without faulting as often as
// not, so the test counts its holders instead.
#[test]
fn contexts_share_their_blob_and_keep_it_alive() {
    let built =
        snapshot("var base = 40; function add(x) { return base + x }", None).expect("snapshot");
    let blob = SnapshotBlob::new(&built).expect("a blob V8 wrote");
    drop(built);
    let contexts: Vec<_> = (0..4).map(|_| spawn_from(&blob)).collect();
    let weak = blob.downgrade();
    drop(blob);
    for (n, context) in (0..).zip(&contexts) {
        assert_eq!(eval(context, &format!("add({n})")), 40 + n);
    }
    assert!(weak.upgrade().is_some(), "contexts hold the blob V8 reads");
    std::thread::scope(|scope| {
        for context in contexts {
            scope.spawn(move || {
                assert_eq!(eval(&context, "add(2)"), 42);
                context.begin_dispose();
                context.dispose(&mut Parking).expect("dispose");
            });
        }
    });
    // A runner lets go of its control block just after the dispose returns.
    let deadline = Instant::now() + Duration::from_secs(10);
    while weak.upgrade().is_some() {
        assert!(Instant::now() < deadline, "the last context frees the blob");
        std::thread::sleep(Duration::from_millis(5));
    }
}
