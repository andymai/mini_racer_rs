use std::sync::{Once, OnceLock};
use std::time::Duration;

use mini_racer_sync::WorkerQueue;

use crate::process::platform::{self, Mode};
use crate::sys::WorkerTask;

// V8 and rusty_v8 hold a platform's pool to this many threads.
const MOST_THREADS: usize = 16;
// V8's own workers get the system's default thread stack, 8 MiB on Linux.
const MIN_STACK_BYTES: usize = 8 << 20;
const STACK_HEADROOM_BYTES: usize = 1 << 20;

#[derive(Clone, Copy)]
struct Size {
    threads: usize,
    stack_bytes: usize,
}

// Set before V8 initializes, so V8 sizes its jobs by this pool from the start,
// and read without the flags lock, which a fork may leave held.
static SIZE: OnceLock<Size> = OnceLock::new();

/// Sizes every process's pool as V8 would have sized its own for the
/// platform mode: what `rusty_v8` makes of a request for the default, or the
/// one thread the single-threaded platform keeps.
pub fn configure() {
    let threads = match platform::mode() {
        Mode::SingleThreaded => 1,
        Mode::MultiThreaded => crate::sys::processors_online().clamp(1, MOST_THREADS),
    };
    // V8 lets a task on a worker thread use `--stack_size` of its stack, as it
    // lets a runner.
    let stack_bytes = platform::stack_size_kb().map_or(MIN_STACK_BYTES, |kb| {
        kb.saturating_mul(1024)
            .saturating_add(STACK_HEADROOM_BYTES)
            .max(MIN_STACK_BYTES)
    });
    let _ = SIZE.set(Size {
        threads,
        stack_bytes,
    });
}

/// How many worker threads a process runs, which V8 reads as its platform's.
pub fn threads() -> usize {
    SIZE.get().map_or(1, |size| size.threads)
}

/// Where V8's worker tasks go: the current process's pool. A child never
/// posts to its parent's, whose threads it does not have.
pub fn post(task: WorkerTask, delay: Duration) {
    super::fork::current().workers().post(task, delay);
}

/// One process's worker threads, started at its first worker task.
pub struct Workers {
    queue: WorkerQueue<WorkerTask>,
    started: Once,
}

impl Default for Workers {
    fn default() -> Self {
        Self {
            queue: WorkerQueue::default(),
            started: Once::new(),
        }
    }
}

impl Workers {
    fn post(&'static self, task: WorkerTask, delay: Duration) {
        self.queue.post(task, delay);
        self.started.call_once(|| self.start());
    }

    fn start(&'static self) {
        let Some(&Size {
            threads,
            stack_bytes,
        }) = SIZE.get()
        else {
            return;
        };
        for _ in 0..threads {
            let spawned = std::thread::Builder::new()
                .name("mini_racer_rs worker".to_owned())
                .stack_size(stack_bytes)
                .spawn(move || {
                    loop {
                        self.queue.run_next(WorkerTask::run);
                    }
                });
            if spawned.is_err() {
                break;
            }
        }
    }

    /// Starts no task until `resume`, and waits up to `bound` for the
    /// running ones to end. Returns whether none is running.
    pub fn pause(&self, bound: Duration) -> bool {
        self.queue.pause(bound)
    }

    pub fn resume(&self) {
        self.queue.resume();
    }
}

/// Taken by every test that fills or times the process's pool, so tests
/// sharing a process take turns with it.
#[cfg(test)]
pub static TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::sync::{Arc, Barrier};
    use std::time::Duration;

    use super::Workers;
    use crate::sys::testing::worker_task;

    // A pause waits for the task running, holds a task posted meanwhile, and
    // the resume lets it run.
    #[test]
    fn a_pause_waits_for_the_running_task_and_holds_new_ones_until_resumed() {
        crate::process::ensure_initialized();
        let workers: &'static Workers = Box::leak(Box::default());
        let started = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let (ran, order) = mpsc::channel();
        {
            let (started, release, ran) = (Arc::clone(&started), Arc::clone(&release), ran.clone());
            workers.post(
                worker_task(Box::new(move || {
                    started.wait();
                    release.wait();
                    std::thread::sleep(Duration::from_millis(20));
                    ran.send("running").expect("receiver");
                })),
                Duration::ZERO,
            );
        }
        started.wait();
        let pausing = std::thread::spawn(|| workers.pause(Duration::from_secs(10)));
        std::thread::sleep(Duration::from_millis(30));
        assert!(!pausing.is_finished());
        workers.post(
            worker_task(Box::new(move || ran.send("posted").expect("receiver"))),
            Duration::ZERO,
        );
        release.wait();
        assert!(pausing.join().expect("pause"));
        assert_eq!(order.try_recv(), Ok("running"));
        assert_eq!(
            order.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        workers.resume();
        assert_eq!(order.recv_timeout(Duration::from_secs(10)), Ok("posted"));
    }
}
