pub mod platform;
pub(crate) mod watchdog;

use std::sync::Once;

static INIT: Once = Once::new();

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
