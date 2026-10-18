pub mod code_cache;
pub mod fork;
pub mod platform;
pub(crate) mod tasks;
pub(crate) mod watchdog;
pub(crate) mod workers;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Once, OnceLock};

static INIT: Once = Once::new();
// The platform points into it for the life of the process.
static PLATFORM_VTABLE: OnceLock<Box<crate::sys::PlatformVtable>> = OnceLock::new();
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
        workers::configure();
        // Unprotected: on PKU-capable CPUs the protected platform requires every
        // V8 thread to descend from the initializing thread, which Ruby threads
        // and per-context runners do not.
        // Worker tasks go to `workers`, whose threads each process starts for
        // itself, as threads do not survive a fork. V8's own pool keeps the one
        // thread rusty_v8 cannot go below, idle for good.
        let mut platform = v8::new_custom_platform(1, false, true, tasks::Foreground);
        let vtable = PLATFORM_VTABLE.get_or_init(|| crate::sys::worker_vtable(&platform));
        crate::sys::use_vtable(&mut platform, vtable);
        v8::V8::initialize_platform(platform.make_shared());
        v8::V8::initialize();
        crate::sys::register_fork_handlers();
    });
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
