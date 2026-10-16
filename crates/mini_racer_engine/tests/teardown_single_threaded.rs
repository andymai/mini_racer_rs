//! The same teardown with V8 in single-threaded mode, which changes how the
//! platform runs its background work. Flags freeze at initialization, so this
//! needs its own process.

use mini_racer_engine::process::platform;

mod common;

#[test]
fn explicit_teardown_leaves_nothing_behind_single_threaded() {
    platform::push_flag(b"single_threaded", None).expect("flag");
    common::churn();
}
