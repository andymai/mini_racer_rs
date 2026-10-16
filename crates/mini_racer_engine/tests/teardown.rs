//! Every context is disposed explicitly, so when the process exits no isolate,
//! runner or control block should remain; the sanitizer job runs this with
//! leak detection on.

mod common;

#[test]
fn explicit_teardown_leaves_nothing_behind() {
    common::churn();
}
