#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers fail the test by panicking"
)]

use std::time::{Duration, Instant};

use mini_racer_engine::{
    CallRequest, CallbackReply, ContextConfig, ContextHandle, Counts, EngineError, Host,
    JsErrorKind, JsText, TerminationCause, Terminations, WaitHandle, WaitOutcome,
};
use mini_racer_wire::Writer;

fn parked(wait: &WaitHandle<'_>) {
    while !wait.ready() {
        wait.park();
    }
}

struct Parking;

impl Host for Parking {
    fn wait(&mut self, wait: &WaitHandle<'_>) -> WaitOutcome {
        parked(wait);
        WaitOutcome::Ready
    }

    fn wait_blocking(&mut self, wait: &WaitHandle<'_>) {
        parked(wait);
    }
}

// Answers each callback with what the closure makes of its request's bytes.
struct Answering<F>(F);

impl<F: FnMut(&[u8]) -> CallbackReply> Host for Answering<F> {
    fn wait(&mut self, wait: &WaitHandle<'_>) -> WaitOutcome {
        parked(wait);
        WaitOutcome::Ready
    }

    fn wait_blocking(&mut self, wait: &WaitHandle<'_>) {
        parked(wait);
    }

    fn callback(&mut self, request: &[u8]) -> CallbackReply {
        (self.0)(request)
    }
}

// Gives the wait up at the deadline, as a Ruby timeout would.
struct GivingUp(Instant);

impl Host for GivingUp {
    fn wait(&mut self, wait: &WaitHandle<'_>) -> WaitOutcome {
        while !wait.spin(Duration::from_millis(1)) {
            if Instant::now() >= self.0 {
                return WaitOutcome::Cancel;
            }
        }
        WaitOutcome::Ready
    }

    fn wait_blocking(&mut self, wait: &WaitHandle<'_>) {
        parked(wait);
    }
}

fn spawn() -> ContextHandle {
    spawn_with(ContextConfig::default())
}

fn spawn_with(config: ContextConfig) -> ContextHandle {
    ContextHandle::spawn(config, &mut Parking).expect("spawn")
}

fn eval(context: &ContextHandle, source: &str, host: &mut impl Host) -> Vec<u8> {
    context.eval("<eval>", source, host).expect("eval succeeds")
}

fn integer(value: i64) -> Vec<u8> {
    let mut out = Vec::new();
    Writer::new(&mut out).integer(value);
    out
}

// A call's or an attach's request: the name, then integers.
fn request(name: &str, values: &[i64]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut writer = Writer::new(&mut out);
    let length = u32::try_from(values.len() + 1).expect("short");
    writer.begin_array(length);
    writer.utf8(name.as_bytes());
    for value in values {
        writer.integer(*value);
    }
    writer.end_array(length);
    out
}

fn attached(name: &str) -> ContextHandle {
    let context = spawn();
    context
        .attach(request(name, &[0]), &mut Parking)
        .expect("attach");
    context
}

fn length(bytes: usize) -> u64 {
    u64::try_from(bytes).expect("fits")
}

fn terminated_by(result: Result<Vec<u8>, EngineError>) -> TerminationCause {
    match result {
        Err(EngineError::Js(js)) => match js.kind {
            JsErrorKind::Terminated(cause) => cause,
            other => panic!("expected a termination, got {other:?}"),
        },
        other => panic!("expected a termination, got {other:?}"),
    }
}

fn since(before: Counts, after: Counts) -> Counts {
    let caused = |before: u64, after: u64| after - before;
    let (was, now) = (before.terminated, after.terminated);
    Counts {
        dispatches: after.dispatches - before.dispatches,
        callbacks: after.callbacks - before.callbacks,
        bytes_to_js: after.bytes_to_js - before.bytes_to_js,
        bytes_from_js: after.bytes_from_js - before.bytes_from_js,
        js_ns: after.js_ns - before.js_ns,
        gc_count: after.gc_count - before.gc_count,
        gc_ns: after.gc_ns - before.gc_ns,
        terminated: Terminations {
            stop: caused(was.stop, now.stop),
            timeout: caused(was.timeout, now.timeout),
            interrupt: caused(was.interrupt, now.interrupt),
            max_memory: caused(was.max_memory, now.max_memory),
            heap_exhausted: caused(was.heap_exhausted, now.heap_exhausted),
        },
    }
}

#[test]
fn a_new_context_counts_none_of_its_own_setup() {
    let counts = spawn().counters();

    assert_eq!(
        (
            counts.dispatches,
            counts.bytes_to_js,
            counts.bytes_from_js,
            counts.js_ns
        ),
        (0, 0, 0, 0)
    );
}

#[test]
fn each_operation_is_one_dispatch_with_its_bytes_each_way() {
    let context = spawn();
    let mut to_js = 0;
    let mut from_js = 0;

    let source = "function id(x) { return x } 1 + 1";
    to_js += source.len();
    from_js += eval(&context, source, &mut Parking).len();
    let call = request("id", &[7]);
    to_js += call.len();
    let request_call = CallRequest {
        bytes: call,
        objects: 0,
        properties: 0,
    };
    from_js += context
        .call(request_call, &mut Parking)
        .expect("call")
        .len();
    let attach = request("callback", &[0]);
    to_js += attach.len();
    context.attach(attach, &mut Parking).expect("attach");
    let units: Vec<u16> = "'\u{e9}t\u{e9}'".encode_utf16().collect();
    to_js += 2 * units.len();
    from_js += context
        .eval("<eval>", JsText::Utf16(units), &mut Parking)
        .expect("eval")
        .len();
    from_js += context.heap_stats(&mut Parking).expect("stats").len();
    from_js += context.heap_snapshot(&mut Parking).expect("snapshot").len();
    context
        .low_memory_notification(&mut Parking)
        .expect("collection");
    context
        .perform_microtask_checkpoint(&mut Parking)
        .expect("checkpoint");
    context.pump_message_loop(&mut Parking).expect("pump");
    from_js += 1;

    let counts = context.counters();
    assert_eq!(counts.dispatches, 9);
    assert_eq!(counts.bytes_to_js, length(to_js));
    assert_eq!(counts.bytes_from_js, length(from_js));
    assert_eq!(counts.callbacks, 0);
    assert_eq!(counts.terminated, Terminations::default());
}

#[test]
fn a_callback_and_the_dispatch_it_nests_count_with_their_bytes() {
    let context = attached("inner");
    let before = context.counters();
    let mut asked = 0;
    let mut nested = Vec::new();
    let mut host = Answering(|request: &[u8]| {
        asked += request.len();
        nested = eval(&context, "6 * 7", &mut Parking);
        CallbackReply::Value(nested.clone())
    });
    let outer = eval(&context, "inner(1, 2) + 1", &mut host);

    let counted = since(before, context.counters());
    assert_eq!(counted.dispatches, 2);
    assert_eq!(counted.callbacks, 1);
    assert_eq!(
        counted.bytes_to_js,
        length("inner(1, 2) + 1".len() + "6 * 7".len() + nested.len())
    );
    assert_eq!(
        counted.bytes_from_js,
        length(asked + nested.len() + outer.len())
    );
}

#[test]
fn a_raised_callback_hands_javascript_its_message() {
    let context = attached("inner");
    let before = context.counters();
    let source = "try { inner() } catch (e) { 0 }";
    eval(
        &context,
        source,
        &mut Answering(|_: &[u8]| CallbackReply::Raised(JsText::from("nope"))),
    );

    let counted = since(before, context.counters());
    assert_eq!(counted.callbacks, 1);
    assert_eq!(counted.bytes_to_js, length(source.len() + "nope".len()));
}

#[test]
fn a_stop_counts_once_for_the_script_it_ends() {
    let context = spawn();
    context.stop().expect("live");
    context.stop().expect("live");

    assert_eq!(
        terminated_by(context.eval("<eval>", "1", &mut Parking)),
        TerminationCause::Stop
    );
    eval(&context, "1", &mut Parking);
    assert_eq!(
        context.counters().terminated,
        Terminations {
            stop: 1,
            ..Terminations::default()
        }
    );
}

#[test]
fn a_timeout_counts_for_the_script_it_ends() {
    let context = spawn_with(ContextConfig {
        timeout: Some(Duration::from_millis(20)),
        ..ContextConfig::default()
    });

    assert_eq!(
        terminated_by(context.eval("<eval>", "while (true) {}", &mut Parking)),
        TerminationCause::Timeout
    );
    assert_eq!(
        context.counters().terminated,
        Terminations {
            timeout: 1,
            ..Terminations::default()
        }
    );
}

#[test]
fn an_abandoned_wait_counts_as_an_interrupt() {
    let context = spawn();
    let deadline = Instant::now() + Duration::from_millis(50);

    assert_eq!(
        terminated_by(context.eval("<eval>", "while (true) {}", &mut GivingUp(deadline))),
        TerminationCause::Cancelled
    );
    assert_eq!(
        context.counters().terminated,
        Terminations {
            interrupt: 1,
            ..Terminations::default()
        }
    );
}

#[test]
fn an_aborted_callback_counts_as_an_interrupt() {
    let context = attached("aborts");
    let mut host = Answering(|_: &[u8]| CallbackReply::Abort);

    assert_eq!(
        terminated_by(context.eval("<eval>", "aborts(); while (true) {}", &mut host)),
        TerminationCause::Cancelled
    );
    let counts = context.counters();
    assert_eq!(counts.callbacks, 1);
    assert_eq!(
        counts.terminated,
        Terminations {
            interrupt: 1,
            ..Terminations::default()
        }
    );
}

#[test]
fn a_heap_past_max_memory_counts_as_max_memory() {
    let context = spawn_with(ContextConfig {
        max_memory: Some(50_000_000),
        ..ContextConfig::default()
    });
    let grown = context.eval(
        "<eval>",
        "let a = []; while (true) { a.push(new Array(10000).fill(0)) }",
        &mut Parking,
    );

    assert_eq!(terminated_by(grown), TerminationCause::MaxMemory);
    let counts = context.counters();
    assert!(counts.gc_count > 0, "{counts:?}");
    assert_eq!(
        counts.terminated,
        Terminations {
            max_memory: 1,
            ..Terminations::default()
        }
    );
}

#[test]
fn a_heap_past_its_limit_counts_as_heap_exhausted() {
    let context = spawn_with(ContextConfig {
        heap_limit: Some(32 << 20),
        ..ContextConfig::default()
    });
    for _ in 0..2 {
        let grown = context.eval(
            "<eval>",
            "let a = []; while (true) { a.push(new Array(10000).fill(0)) }",
            &mut Parking,
        );
        assert_eq!(terminated_by(grown), TerminationCause::HeapExhausted);
    }

    assert_eq!(
        context.counters().terminated,
        Terminations {
            heap_exhausted: 2,
            ..Terminations::default()
        }
    );
}

// The engine ends an abandoned nested dispatch without recording a cause, so
// the dispatch around it runs on.
#[test]
fn an_abandoned_nested_dispatch_counts_as_an_interrupt() {
    let context = attached("inner");
    let mut nested = None;
    let mut host = Answering(|_: &[u8]| {
        let deadline = Instant::now() + Duration::from_millis(50);
        nested = Some(context.eval("<eval>", "while (true) {}", &mut GivingUp(deadline)));
        CallbackReply::Value(integer(1))
    });
    eval(&context, "inner() + 1", &mut host);

    assert!(matches!(
        nested,
        Some(Err(EngineError::Js(ref js))) if matches!(js.kind, JsErrorKind::Terminated(_))
    ));
    assert_eq!(
        context.counters().terminated,
        Terminations {
            interrupt: 1,
            ..Terminations::default()
        }
    );
}

#[test]
fn a_stop_counts_for_each_dispatch_it_ends_nested_ones_included() {
    let context = attached("inner");
    let mut nested = None;
    let mut host = Answering(|_: &[u8]| {
        context.stop().expect("live");
        nested = Some(context.eval("<eval>", "1", &mut Parking));
        CallbackReply::Value(integer(1))
    });

    assert_eq!(
        terminated_by(context.eval("<eval>", "inner(); 1", &mut host)),
        TerminationCause::Stop
    );
    assert_eq!(terminated_by(nested.expect("ran")), TerminationCause::Stop);
    assert_eq!(
        context.counters().terminated,
        Terminations {
            stop: 2,
            ..Terminations::default()
        }
    );
}

// Nothing times a pump, so a script nested in one gets a timeout of its own,
// which ends that script alone.
#[test]
fn a_script_timed_on_its_own_inside_a_pump_counts_its_timeout() {
    let context = spawn_with(ContextConfig {
        timeout: Some(Duration::from_millis(50)),
        ..ContextConfig::default()
    });
    context
        .attach(request("spin", &[0]), &mut Parking)
        .expect("attach");
    eval(
        &context,
        "Atomics.waitAsync(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 5).value \
           .then(() => spin()); 0",
        &mut Parking,
    );
    std::thread::sleep(Duration::from_millis(30));
    let mut nested = None;
    let mut host = Answering(|_: &[u8]| {
        nested = Some(context.eval("<eval>", "while (true) {}", &mut Parking));
        CallbackReply::Value(integer(7))
    });

    assert!(context.pump_message_loop(&mut host).expect("pump"));
    assert_eq!(
        terminated_by(nested.expect("ran")),
        TerminationCause::Timeout
    );
    assert_eq!(
        context.counters().terminated,
        Terminations {
            timeout: 1,
            ..Terminations::default()
        }
    );
}

fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).expect("fits")
}

#[test]
fn javascript_time_counts_the_script_and_not_the_callback_it_waits_on() {
    let context = attached("slow");
    let before = context.counters().js_ns;
    let started = Instant::now();
    eval(
        &context,
        "const until = Date.now() + 50; while (Date.now() < until) {} 0",
        &mut Parking,
    );
    let wall = started.elapsed();
    let busy = context.counters().js_ns;
    let ran = busy - before;
    assert!(ran >= nanos(Duration::from_millis(49)), "{ran}");
    assert!(ran <= nanos(wall), "{ran} past {wall:?}");

    let mut host = Answering(|_: &[u8]| {
        std::thread::sleep(Duration::from_millis(300));
        CallbackReply::Value(integer(1))
    });
    eval(&context, "slow()", &mut host);
    let waited = context.counters().js_ns - busy;
    assert!(waited < nanos(Duration::from_millis(100)), "{waited}");
}

#[test]
fn javascript_time_leaves_out_an_awaits_sleep() {
    let context = spawn();
    let settled = context
        .eval_await(
            "<eval>",
            "Atomics.waitAsync(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 300).value",
            &mut Parking,
        )
        .expect("settles");

    assert!(settled.ends_with(b"timed-out"));
    let js = context.counters().js_ns;
    assert!(js > 0);
    assert!(js < nanos(Duration::from_millis(100)), "{js}");
}

const SPIN: &str = "const until = Date.now() + 50; while (Date.now() < until) {}";

fn spun(before: Counts, after: Counts) -> u64 {
    since(before, after).js_ns
}

// The task resolves the promise, and the pump runs its reaction after it.
#[test]
fn a_pumped_task_is_javascript_time() {
    let context = spawn();
    eval(
        &context,
        &format!(
            "Atomics.waitAsync(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 5).value \
               .then(() => {{ {SPIN} }}); 0"
        ),
        &mut Parking,
    );
    std::thread::sleep(Duration::from_millis(30));
    let before = context.counters();

    assert!(context.pump_message_loop(&mut Parking).expect("pump"));
    let ran = spun(before, context.counters());
    assert!(ran >= nanos(Duration::from_millis(49)), "{ran}");
}

// Drained from a callback, the microtask runs while the script around it
// waits for Ruby, which is not timed.
#[test]
fn a_microtask_checkpoint_is_javascript_time() {
    let context = attached("drain");
    let before = context.counters();
    let mut host = Answering(|_: &[u8]| {
        context
            .perform_microtask_checkpoint(&mut Parking)
            .expect("checkpoint");
        CallbackReply::Value(integer(0))
    });
    eval(
        &context,
        &format!("Promise.resolve().then(() => {{ {SPIN} }}); drain(); 0"),
        &mut host,
    );

    let ran = spun(before, context.counters());
    assert!(ran >= nanos(Duration::from_millis(49)), "{ran}");
}

// Attach reads each segment of its path but the last, so a getter there runs.
#[test]
fn an_attach_is_javascript_time() {
    let context = spawn();
    eval(
        &context,
        &format!(
            "Object.defineProperty(globalThis, 'slow', {{ get() {{ {SPIN}; return {{}} }} }}); 0"
        ),
        &mut Parking,
    );
    let before = context.counters();
    context
        .attach(request("slow.callback", &[0]), &mut Parking)
        .expect("attach");

    let ran = spun(before, context.counters());
    assert!(ran >= nanos(Duration::from_millis(49)), "{ran}");
}

#[test]
fn heap_statistics_snapshots_and_collections_are_not_javascript_time() {
    let context = spawn();
    eval(
        &context,
        "globalThis.junk = []; for (let i = 0; i < 1e5; i++) junk.push({ i }); junk = null; 0",
        &mut Parking,
    );
    let before = context.counters();
    context.heap_stats(&mut Parking).expect("stats");
    context.heap_snapshot(&mut Parking).expect("snapshot");
    context
        .low_memory_notification(&mut Parking)
        .expect("collection");

    let counted = since(before, context.counters());
    assert_eq!(counted.dispatches, 3);
    assert_eq!(counted.js_ns, 0);
    assert!(counted.gc_count >= 1, "{counted:?}");
    assert!(counted.gc_ns > 0, "{counted:?}");
}

// Polls until the script is dispatched, then reads while it loops, so a
// loaded machine cannot make the read come before or after it.
#[test]
fn counts_read_from_another_thread_while_a_script_runs() {
    let context = spawn();
    let (during, ended) = std::thread::scope(|scope| {
        let running = scope.spawn(|| context.eval("<eval>", "while (true) {}", &mut Parking));
        let deadline = Instant::now() + Duration::from_secs(10);
        while context.counters().dispatches == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        let during = context.counters();
        context.stop().expect("live");
        (during, running.join().expect("eval thread"))
    });

    assert_eq!(
        (during.dispatches, during.bytes_from_js, during.js_ns),
        (1, 0, 0)
    );
    assert_eq!(terminated_by(ended), TerminationCause::Stop);
    let after = context.counters();
    assert!(after.js_ns > 0, "{after:?}");
    assert_eq!(after.terminated.stop, 1);
}

#[test]
fn a_disposed_context_keeps_its_last_counts() {
    let context = spawn();
    eval(&context, "1 + 1", &mut Parking);
    let last = context.counters();
    context.begin_dispose();
    context.dispose(&mut Parking).expect("dispose");

    assert!(matches!(
        context.eval("<eval>", "1", &mut Parking),
        Err(EngineError::Disposed)
    ));
    assert_eq!(context.counters(), last);
}
