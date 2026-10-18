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
    for blob in [&built, &empty, &warmed] {
        assert_eq!(SnapshotBlob::check(blob), Ok(()));
    }
    assert_eq!(SnapshotBlob::check(&[]), Ok(()));
}

// V8 aborts the process on each of these, or reads past the end of the blob;
// on a damaged read-only section, only when no isolate of the process came first.
#[test]
fn refuses_a_blob_v8_would_abort_on() {
    let blob = snapshot("var a = 1", None).expect("snapshot");
    let len = blob.len();
    let last = word(&blob, 88 + 4 * (word(&blob, 0) - 1));
    let (startup, shared_heap) = (word(&blob, 80), word(&blob, 84));
    let mut refused = vec![
        (changed(&blob, |b| b[16] ^= 1), InvalidSnapshot::Version),
        (b"garbage".to_vec(), InvalidSnapshot::Corrupt),
        (blob[..81].to_vec(), InvalidSnapshot::Corrupt),
        (blob[..80].to_vec(), InvalidSnapshot::Corrupt),
        (
            changed(&blob, |b| set_word(b, 0, 0)),
            InvalidSnapshot::Corrupt,
        ),
        (
            changed(&blob, |b| set_word(b, 0, u32::MAX)),
            InvalidSnapshot::Corrupt,
        ),
        (
            changed(&blob, |b| set_word(b, 4, 2)),
            InvalidSnapshot::Corrupt,
        ),
        (
            changed(&blob, |b| set_word(b, 84, 1 << 30)),
            InvalidSnapshot::Corrupt,
        ),
        (
            changed(&blob, |b| {
                set_word(b, 80, u32::try_from(shared_heap).expect("an offset"));
                set_word(b, 84, u32::try_from(startup).expect("an offset"));
            }),
            InvalidSnapshot::Corrupt,
        ),
        (changed(&blob, |b| b[96] ^= 1), InvalidSnapshot::Corrupt),
        (changed(&blob, |b| b[last] ^= 1), InvalidSnapshot::Corrupt),
    ];
    for cut in [len / 2, last + 8, last + 4, len - 1] {
        refused.push((blob[..cut].to_vec(), InvalidSnapshot::Corrupt));
    }
    let mut random = vec![0_u8; len];
    let mut state = 0x9E37_79B9_u32;
    for byte in &mut random {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        *byte = state.to_le_bytes()[0];
    }
    refused.push((random, InvalidSnapshot::Version));
    for (index, (bytes, invalid)) in refused.iter().enumerate() {
        assert_eq!(SnapshotBlob::check(bytes), Err(*invalid), "case {index}");
        assert_eq!(
            SnapshotBlob::new(bytes).err(),
            Some(*invalid),
            "case {index}"
        );
    }
}

// V8 trusts each section's own payload length, so what lies past it, or a
// damaged byte inside it, is V8's to read, as it is in mini_racer.
#[test]
fn leaves_what_a_section_holds_to_v8() {
    let blob = snapshot("var a = 41", None).expect("snapshot");
    let longer = changed(&blob, |b| b.extend_from_slice(b"\n"));
    assert_eq!(eval_in(&longer, "a + 1"), 42);
    let startup = word(&blob, 80);
    let damaged = changed(&blob, |b| b[startup + 4096] ^= 1);
    assert_eq!(SnapshotBlob::check(&damaged), Ok(()));
}

#[test]
fn a_warmup_from_an_empty_blob_is_refused() {
    let refused =
        ContextHandle::create_snapshot("1".into(), Some(SnapshotBlob::default()), &mut Parking);
    assert!(matches!(
        refused,
        Err(EngineError::InvalidSnapshot(InvalidSnapshot::Empty))
    ));
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
