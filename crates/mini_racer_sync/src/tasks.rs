use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::lock::lock;
use crate::parker::Parker;
use crate::primitives::Mutex;

// V8 asks for delays of seconds; anything longer is held to this, so every
// deadline fits the clock.
const LONGEST_DELAY: Duration = Duration::from_hours(365 * 24);

struct Entry<T> {
    task: T,
    nestable: bool,
}

struct Delayed<T> {
    due: Instant,
    entry: Entry<T>,
}

pub struct Queue<T> {
    ready: VecDeque<Entry<T>>,
    // In posting order, which breaks ties between equal deadlines.
    delayed: Vec<Delayed<T>>,
    closed: bool,
}

impl<T> Default for Queue<T> {
    fn default() -> Self {
        Self {
            ready: VecDeque::new(),
            delayed: Vec::new(),
            closed: false,
        }
    }
}

impl<T> Queue<T> {
    pub fn push(&mut self, task: T, nestable: bool, delay: Duration) {
        let entry = Entry { task, nestable };
        if delay.is_zero() {
            self.ready.push_back(entry);
        } else {
            let now = Instant::now();
            let due = now.checked_add(delay.min(LONGEST_DELAY)).unwrap_or(now);
            self.delayed.push(Delayed { due, entry });
        }
    }

    pub fn pop(&mut self, now: Instant, nested: bool) -> Option<T> {
        self.promote(now);
        let index = self.position(nested)?;
        self.ready.remove(index).map(|entry| entry.task)
    }

    // Delayed tasks that have come due join the ready ones, earliest first.
    fn promote(&mut self, now: Instant) {
        if !self.delayed.iter().any(|delayed| delayed.due <= now) {
            return;
        }
        let (mut due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.delayed)
            .into_iter()
            .partition(|delayed| delayed.due <= now);
        due.sort_by_key(|delayed| delayed.due);
        self.delayed = later;
        self.ready
            .extend(due.into_iter().map(|delayed| delayed.entry));
    }

    fn position(&self, nested: bool) -> Option<usize> {
        self.ready
            .iter()
            .position(|entry| !nested || entry.nestable)
    }

    pub fn next_due(&self) -> Option<Instant> {
        self.delayed.iter().map(|delayed| delayed.due).min()
    }

    fn drain(&mut self) -> Vec<T> {
        let mut delayed = std::mem::take(&mut self.delayed);
        delayed.sort_by_key(|delayed| delayed.due);
        std::mem::take(&mut self.ready)
            .into_iter()
            .chain(delayed.into_iter().map(|delayed| delayed.entry))
            .map(|entry| entry.task)
            .collect()
    }
}

/// One isolate's foreground tasks, as V8's own task runner keeps them: run in
/// posting order once due, and a non-nestable task never runs inside another.
///
/// Any thread may post. The runner pops and runs one task at a time, and
/// sleeps in `wait` until a task is due or `ring` is called, which is how a
/// termination reaches a runner awaiting a promise. A post or ring that lands
/// before the runner sleeps ends that sleep at once, so neither is lost.
pub struct TaskQueue<T> {
    queue: Mutex<Queue<T>>,
    doorbell: Parker,
}

impl<T> Default for TaskQueue<T> {
    fn default() -> Self {
        Self {
            queue: Mutex::new(Queue::default()),
            doorbell: Parker::default(),
        }
    }
}

impl<T> TaskQueue<T> {
    /// Queues `task` to run once `delay` has passed. A closed queue refuses
    /// it, and the caller drops it.
    pub fn post(&self, task: T, nestable: bool, delay: Duration) -> Result<(), T> {
        let mut queue = lock(&self.queue);
        if queue.closed {
            drop(queue);
            return Err(task);
        }
        queue.push(task, nestable, delay);
        drop(queue);
        self.doorbell.unpark();
        Ok(())
    }

    /// The oldest task that is due. `nested`, for a pump inside a running
    /// task, passes over non-nestable ones.
    pub fn pop(&self, nested: bool) -> Option<T> {
        let mut queue = lock(&self.queue);
        let task = queue.pop(Instant::now(), nested);
        drop(queue);
        task
    }

    /// Sleeps until a task is due, a task is posted, or `ring` is called;
    /// returns at once if a task is due already or the queue is closed. May
    /// return early: callers check again. Only an outermost dispatch waits,
    /// so every task counts.
    pub fn wait(&self) {
        let timeout = {
            let mut queue = lock(&self.queue);
            let now = Instant::now();
            queue.promote(now);
            if queue.closed || !queue.ready.is_empty() {
                return;
            }
            queue
                .next_due()
                .map(|due| due.saturating_duration_since(now))
        };
        self.doorbell.park(timeout);
    }

    /// Ends a `wait`, or the next one if none is under way.
    pub fn ring(&self) {
        self.doorbell.unpark();
    }

    /// Refuses every later post and hands back what is queued, for its owner
    /// to drop where the tasks belong.
    pub fn close(&self) -> Vec<T> {
        let mut queue = lock(&self.queue);
        queue.closed = true;
        let tasks = queue.drain();
        drop(queue);
        self.doorbell.unpark();
        tasks
    }

    /// What a forked child takes over with an idle isolate: the queued tasks,
    /// moved into a new queue and out of this one. `None` while the lock is
    /// held, which in a child means by a thread that is gone.
    #[must_use]
    pub fn inherited(&self) -> Option<Self> {
        let mut queue = self.queue.try_lock().ok()?;
        let taken = Queue {
            ready: std::mem::take(&mut queue.ready),
            delayed: std::mem::take(&mut queue.delayed),
            closed: queue.closed,
        };
        drop(queue);
        Some(Self {
            queue: Mutex::new(taken),
            doorbell: Parker::default(),
        })
    }
}

impl<T> Drop for TaskQueue<T> {
    // Tasks are dropped only by the thread their isolate runs on, which takes
    // them with `close`. Any left here are leaked.
    fn drop(&mut self) {
        let leftover = lock(&self.queue).drain();
        if !leftover.is_empty() {
            #[expect(
                clippy::mem_forget,
                reason = "tasks may be dropped only where their isolate runs"
            )]
            std::mem::forget(leftover);
        }
    }
}

#[cfg(test)]
#[cfg(not(loom))]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    const NOW: Duration = Duration::ZERO;

    fn drained(queue: &TaskQueue<u32>, nested: bool) -> Vec<u32> {
        std::iter::from_fn(|| queue.pop(nested)).collect()
    }

    #[test]
    fn runs_tasks_in_posting_order() {
        let queue = TaskQueue::default();
        for task in 0..4 {
            queue.post(task, true, NOW).expect("open");
        }
        assert_eq!(drained(&queue, false), [0, 1, 2, 3]);
    }

    #[test]
    fn holds_a_delayed_task_until_it_is_due() {
        let queue = TaskQueue::default();
        queue
            .post(1, true, Duration::from_millis(30))
            .expect("open");
        queue.post(2, true, NOW).expect("open");
        assert_eq!(drained(&queue, false), [2]);
        let started = Instant::now();
        let mut waits = 0;
        while queue.pop(false).is_none() {
            queue.wait();
            waits += 1;
        }
        assert!(started.elapsed() >= Duration::from_millis(25));
        // The posts left the doorbell rung once; then one sleep to the deadline.
        assert!(waits <= 3, "woke {waits} times");
    }

    // As V8's runner moves expired delayed tasks: behind those already ready,
    // earliest deadline first.
    #[test]
    fn due_tasks_join_behind_ready_ones_by_deadline() {
        let queue = TaskQueue::default();
        queue
            .post(1, true, Duration::from_millis(20))
            .expect("open");
        queue
            .post(2, true, Duration::from_millis(10))
            .expect("open");
        queue
            .post(3, true, Duration::from_millis(10))
            .expect("open");
        std::thread::sleep(Duration::from_millis(25));
        queue.post(4, true, NOW).expect("open");
        assert_eq!(drained(&queue, false), [4, 2, 3, 1]);
    }

    #[test]
    fn a_nested_pop_passes_over_non_nestable_tasks() {
        let queue = TaskQueue::default();
        queue.post(1, false, NOW).expect("open");
        queue.post(2, true, NOW).expect("open");
        assert_eq!(drained(&queue, true), [2]);
        assert_eq!(drained(&queue, false), [1]);
    }

    #[test]
    fn close_hands_back_everything_and_refuses_later_posts() {
        let queue = TaskQueue::default();
        queue.post(1, true, Duration::from_secs(60)).expect("open");
        queue.post(2, false, NOW).expect("open");
        assert_eq!(queue.close(), [2, 1]);
        assert_eq!(queue.post(3, true, NOW), Err(3));
        assert_eq!(queue.pop(false), None);
        queue.wait();
    }

    #[test]
    fn a_ring_ends_a_wait_with_nothing_due() {
        let queue = Arc::new(TaskQueue::<u32>::default());
        let ringer = Arc::clone(&queue);
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            ringer.ring();
        });
        queue.wait();
        thread.join().expect("ringer");
    }

    #[test]
    fn an_absurd_delay_is_held_to_a_year() {
        let queue = TaskQueue::default();
        queue.post(1, true, Duration::MAX).expect("open");
        assert_eq!(queue.pop(false), None);
        assert_eq!(queue.close(), [1]);
    }

    #[test]
    fn a_child_takes_the_tasks_over() {
        let queue = TaskQueue::default();
        queue.post(1, true, NOW).expect("open");
        queue.post(2, true, Duration::from_secs(60)).expect("open");
        let inherited = queue.inherited().expect("unlocked");
        assert_eq!(queue.pop(false), None);
        assert_eq!(inherited.pop(false), Some(1));
        assert_eq!(inherited.close(), [2]);
    }

    #[test]
    fn a_held_lock_leaves_nothing_to_take_over() {
        let queue = TaskQueue::<u32>::default();
        let held = lock(&queue.queue);
        assert!(queue.inherited().is_none());
        drop(held);
    }

    struct Counted(Arc<AtomicUsize>);

    impl Drop for Counted {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn tasks_left_in_a_dropped_queue_are_leaked() {
        let drops = Arc::new(AtomicUsize::new(0));
        let queue = TaskQueue::default();
        queue
            .post(Counted(Arc::clone(&drops)), true, NOW)
            .map_err(drop)
            .expect("open");
        drop(queue);
        assert_eq!(drops.load(Ordering::SeqCst), 0);
    }
}
