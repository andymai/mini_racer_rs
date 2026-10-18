use std::time::{Duration, Instant};

use crate::lock::{lock, wait, wait_timeout};
use crate::primitives::{Condvar, Mutex};
use crate::tasks::Queue;

struct State<T> {
    queue: Queue<T>,
    paused: bool,
    running: usize,
    idle: usize,
}

/// One process's worker tasks, as V8's own pool keeps them: run once due,
/// in posting order, by whichever worker is free.
///
/// A fork pauses the queue: `pause` stops tasks from starting and returns
/// once none is running, so the child finds no task halfway through, and
/// `resume` lets the workers go on. A forked child never touches its parent's
/// queue, so it does not matter that the workers sleeping on it are gone.
pub struct WorkerQueue<T> {
    state: Mutex<State<T>>,
    work: Condvar,
    quiet: Condvar,
}

impl<T> Default for WorkerQueue<T> {
    fn default() -> Self {
        Self {
            state: Mutex::new(State {
                queue: Queue::default(),
                paused: false,
                running: 0,
                idle: 0,
            }),
            work: Condvar::new(),
            quiet: Condvar::new(),
        }
    }
}

// Ends a task's run, even one that unwinds, so a pause never waits for it.
struct Running<'a, T>(&'a WorkerQueue<T>);

impl<T> Drop for Running<'_, T> {
    fn drop(&mut self) {
        let mut state = lock(&self.0.state);
        state.running -= 1;
        let quiet = state.paused && state.running == 0;
        drop(state);
        if quiet {
            self.0.quiet.notify_all();
        }
    }
}

impl<T> WorkerQueue<T> {
    /// Queues `task` to run once `delay` has passed.
    pub fn post(&self, task: T, delay: Duration) {
        let mut state = lock(&self.state);
        state.queue.push(task, true, delay);
        let wake = state.idle > 0 && !state.paused;
        drop(state);
        if !wake {
            return;
        }
        // Every idle worker then waits for the earliest deadline, so one is
        // still waiting for the next after another takes the first.
        if delay.is_zero() {
            self.work.notify_one();
        } else {
            self.work.notify_all();
        }
    }

    /// Waits until a task is due and the queue is not paused, then hands it
    /// to `run`. A pause waits for `run` to return.
    pub fn run_next(&self, run: impl FnOnce(T)) {
        let task = self.take();
        let _running = Running(self);
        run(task);
    }

    fn take(&self) -> T {
        let mut state = lock(&self.state);
        loop {
            let timeout = if state.paused {
                None
            } else {
                let now = Instant::now();
                if let Some(task) = state.queue.pop(now, false) {
                    state.running += 1;
                    drop(state);
                    return task;
                }
                state
                    .queue
                    .next_due()
                    .map(|due| due.saturating_duration_since(now))
            };
            state.idle += 1;
            state = match timeout {
                Some(timeout) => wait_timeout(&self.work, state, timeout),
                None => wait(&self.work, state),
            };
            state.idle -= 1;
        }
    }

    /// Starts no task until `resume`, and waits up to `bound` for the running
    /// ones to end. Returns whether none is running.
    pub fn pause(&self, bound: Duration) -> bool {
        let started = Instant::now();
        let mut state = lock(&self.state);
        state.paused = true;
        let quiet = loop {
            if state.running == 0 {
                break true;
            }
            let left = bound.saturating_sub(started.elapsed());
            if left.is_zero() {
                break false;
            }
            state = wait_timeout(&self.quiet, state, left);
        };
        drop(state);
        quiet
    }

    pub fn resume(&self) {
        lock(&self.state).paused = false;
        self.work.notify_all();
    }
}

#[cfg(test)]
#[cfg(not(loom))]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::sync::{Arc, Barrier};
    use std::thread;

    use super::*;

    const NOW: Duration = Duration::ZERO;

    fn worker(queue: &Arc<WorkerQueue<u32>>, ran: &mpsc::Sender<u32>) -> thread::JoinHandle<()> {
        let (queue, ran) = (Arc::clone(queue), ran.clone());
        thread::spawn(move || {
            loop {
                let mut last = false;
                queue.run_next(|task| {
                    last = task == 0;
                    ran.send(task).expect("receiver");
                });
                if last {
                    break;
                }
            }
        })
    }

    #[test]
    fn runs_tasks_in_posting_order_then_delayed_ones_by_deadline() {
        let queue = Arc::new(WorkerQueue::default());
        let (ran, order) = mpsc::channel();
        queue.post(3, Duration::from_millis(60));
        queue.post(2, Duration::from_millis(30));
        for task in [1, 4, 5] {
            queue.post(task, NOW);
        }
        let started = Instant::now();
        let thread = worker(&queue, &ran);
        let first: Vec<u32> = order.iter().take(3).collect();
        assert_eq!(first, [1, 4, 5]);
        assert_eq!(order.recv().expect("task"), 2);
        assert!(started.elapsed() >= Duration::from_millis(25));
        assert_eq!(order.recv().expect("task"), 3);
        assert!(started.elapsed() >= Duration::from_millis(55));
        queue.post(0, NOW);
        thread.join().expect("worker");
    }

    #[test]
    fn a_task_posted_to_idle_workers_runs() {
        let queue = Arc::new(WorkerQueue::default());
        let (ran, order) = mpsc::channel();
        let threads = [worker(&queue, &ran), worker(&queue, &ran)];
        thread::sleep(Duration::from_millis(20));
        queue.post(7, NOW);
        assert_eq!(order.recv().expect("task"), 7);
        queue.post(0, NOW);
        queue.post(0, NOW);
        for thread in threads {
            thread.join().expect("worker");
        }
    }

    // A pause waits for the running task, starts nothing until the resume,
    // and the resume runs what was posted meanwhile.
    #[test]
    fn a_pause_waits_for_the_running_task_and_holds_the_rest() {
        let queue = Arc::new(WorkerQueue::<u32>::default());
        let started = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let finished = Arc::new(AtomicBool::new(false));
        let (ran, order) = mpsc::channel();
        let thread = {
            let (queue, started, release) = (
                Arc::clone(&queue),
                Arc::clone(&started),
                Arc::clone(&release),
            );
            let finished = Arc::clone(&finished);
            thread::spawn(move || {
                queue.run_next(|_| {
                    started.wait();
                    release.wait();
                    thread::sleep(Duration::from_millis(20));
                    finished.store(true, Ordering::SeqCst);
                });
                queue.run_next(|task| ran.send(task).expect("receiver"));
            })
        };
        queue.post(1, NOW);
        started.wait();
        let pausing = {
            let queue = Arc::clone(&queue);
            thread::spawn(move || queue.pause(Duration::from_secs(10)))
        };
        thread::sleep(Duration::from_millis(20));
        assert!(!pausing.is_finished());
        queue.post(2, NOW);
        release.wait();
        assert!(pausing.join().expect("pause"));
        assert!(finished.load(Ordering::SeqCst));
        assert_eq!(
            order.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        queue.resume();
        assert_eq!(order.recv().expect("task"), 2);
        thread.join().expect("worker");
    }

    #[test]
    fn a_pause_past_its_bound_reports_the_running_task() {
        let queue = Arc::new(WorkerQueue::<u32>::default());
        let started = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let thread = {
            let (queue, started, release) = (
                Arc::clone(&queue),
                Arc::clone(&started),
                Arc::clone(&release),
            );
            thread::spawn(move || {
                queue.run_next(|_| {
                    started.wait();
                    release.wait();
                });
            })
        };
        queue.post(1, NOW);
        started.wait();
        let before = Instant::now();
        assert!(!queue.pause(Duration::from_millis(30)));
        assert!(before.elapsed() >= Duration::from_millis(25));
        release.wait();
        thread.join().expect("worker");
        assert!(queue.pause(Duration::from_millis(30)));
        queue.resume();
    }

    #[test]
    fn a_pause_with_nothing_running_returns_at_once() {
        let queue = WorkerQueue::<u32>::default();
        queue.post(1, NOW);
        assert!(queue.pause(Duration::from_secs(10)));
        queue.resume();
    }
}
