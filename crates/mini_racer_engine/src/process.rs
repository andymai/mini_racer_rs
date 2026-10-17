pub mod code_cache;
pub mod fork;
pub mod platform;
pub(crate) mod tasks;
pub(crate) mod watchdog;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Once, OnceLock};

static INIT: Once = Once::new();
// With the fork epoch it began in: its worker threads run only in that
// process, never in a forked child.
static PLATFORM: OnceLock<(v8::SharedRef<v8::Platform>, u64)> = OnceLock::new();
static NEXT_THREAD: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static THREAD: u64 = NEXT_THREAD.fetch_add(1, Ordering::Relaxed);
}

/// Names the calling thread for the life of the process; never 0.
#[must_use]
pub fn thread_token() -> u64 {
    THREAD.with(|token| *token)
}

pub fn ensure_initialized() {
    INIT.call_once(|| {
        let _ = v8::icu::set_common_data_78(deno_core_icudata::ICU_DATA);
        for flag in platform::freeze() {
            v8::V8::set_flags_from_string(&flag);
        }
        // Unprotected: on PKU-capable CPUs the protected platform requires every
        // V8 thread to descend from the initializing thread, which Ruby threads
        // and per-context runners do not.
        // rusty_v8 runs at least one worker, and under `--single-threaded`
        // the scavenger still hands its job to it, one participant at a
        // time, so one is enough; a fork has to hold it.
        let workers = match platform::mode() {
            platform::Mode::SingleThreaded => 1,
            platform::Mode::MultiThreaded => 0,
        };
        let platform =
            v8::new_custom_platform(workers, false, true, tasks::Foreground).make_shared();
        let _ = PLATFORM.set((platform.clone(), fork::epoch()));
        v8::V8::initialize_platform(platform);
        v8::V8::initialize();
        crate::sys::register_fork_handlers();
    });
}

/// V8's platform, if this process runs its worker threads.
pub(crate) fn platform_with_workers() -> Option<&'static v8::SharedRef<v8::Platform>> {
    PLATFORM
        .get()
        .filter(|(_, began)| *began == fork::epoch())
        .map(|(platform, _)| platform)
}

/// Tasks V8 posted for an isolate no runner had registered, dropped unrun.
/// Snapshot builds post some; a context's isolate should never get one.
#[must_use]
pub fn unserved_tasks() -> u64 {
    tasks::unserved()
}

#[must_use]
pub fn v8_version() -> &'static str {
    v8::V8::get_version()
}
