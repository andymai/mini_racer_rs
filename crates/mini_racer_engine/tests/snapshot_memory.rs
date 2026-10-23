#![cfg(target_os = "linux")]
#![allow(
    clippy::expect_used,
    reason = "test helpers fail the test by panicking"
)]

// Its own test binary: it reads the resident memory of the whole process,
// which tests running alongside it in one binary would change.

use mini_racer_engine::{ContextHandle, Host, SnapshotBlob, WaitHandle, WaitOutcome};

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

fn resident_bytes() -> usize {
    let status = std::fs::read_to_string("/proc/self/status").expect("status");
    let kb = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .and_then(|kb| kb.trim().strip_suffix("kB")?.trim().parse::<usize>().ok())
        .expect("VmRSS");
    kb * 1024
}

// glibc and musl map an allocation past 32 MiB on its own and unmap it once
// it is freed, so a blob of about 40 MiB leaves resident memory as soon as its
// bytes are freed. ASan's quarantine keeps freed memory resident, so
// scripts/sanitizers.sh skips this test.
#[test]
fn a_weak_blob_keeps_none_of_its_bytes() {
    let built = ContextHandle::create_snapshot(
        "var doubles = new Float64Array(5 * 1024 * 1024).fill(0.5)".into(),
        None,
        &mut Parking,
    )
    .expect("snapshot");
    let blob = SnapshotBlob::new(&built).expect("a blob V8 wrote");
    drop(built);
    let len = blob.as_bytes().len();
    let weak = blob.downgrade();
    let held = resident_bytes();
    drop(blob);
    let freed = held.saturating_sub(resident_bytes());
    assert!(weak.upgrade().is_none());
    assert!(
        freed > len / 2,
        "dropping the last strong reference to a {len} byte blob freed {freed} bytes"
    );
}
