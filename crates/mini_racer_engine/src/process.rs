pub mod platform;
pub(crate) mod watchdog;

use std::sync::Once;
use std::sync::atomic::{AtomicU64, Ordering};

static INIT: Once = Once::new();
static NEXT_THREAD: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static THREAD: u64 = NEXT_THREAD.fetch_add(1, Ordering::Relaxed);
}

/// Names the calling thread for the life of the process; never 0.
pub(crate) fn thread_token() -> u64 {
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
        let platform = v8::new_unprotected_default_platform(0, false).make_shared();
        v8::V8::initialize_platform(platform);
        v8::V8::initialize();
    });
}

#[must_use]
pub fn v8_version() -> &'static str {
    v8::V8::get_version()
}
