use std::collections::HashMap;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mini_racer_sync::{TaskQueue, lock};

use crate::process::fork;

pub type Tasks = TaskQueue<v8::Task>;

/// The queues V8's foreground tasks go to in one process, by the address V8
/// names each isolate by.
#[derive(Default)]
pub struct TaskRegistry {
    queues: Mutex<HashMap<usize, Arc<Tasks>>>,
}

impl TaskRegistry {
    /// Sends the isolate's tasks to `tasks`. An entry a disposed isolate at the
    /// same address left behind is replaced.
    pub fn register(&self, isolate: usize, tasks: &Arc<Tasks>) {
        lock(&self.queues).insert(isolate, Arc::clone(tasks));
    }

    /// Stops sending tasks to `tasks`, unless a newer isolate at the same
    /// address has taken the entry over.
    pub fn unregister(&self, isolate: usize, tasks: &Arc<Tasks>) {
        let mut queues = lock(&self.queues);
        if queues
            .get(&isolate)
            .is_some_and(|queued| Arc::ptr_eq(queued, tasks))
        {
            queues.remove(&isolate);
        }
    }

    fn find(&self, isolate: usize) -> Option<Arc<Tasks>> {
        lock(&self.queues).get(&isolate).cloned()
    }
}

/// Takes V8's foreground tasks off its platform, so a runner can sleep until
/// one is due and a termination can wake it.
pub struct Foreground;

impl v8::PlatformImpl for Foreground {
    fn post_task(&self, isolate: *mut c_void, task: v8::Task) {
        post(isolate, task, true, 0.0);
    }

    fn post_non_nestable_task(&self, isolate: *mut c_void, task: v8::Task) {
        post(isolate, task, false, 0.0);
    }

    fn post_delayed_task(&self, isolate: *mut c_void, task: v8::Task, delay: f64) {
        post(isolate, task, true, delay);
    }

    fn post_non_nestable_delayed_task(&self, isolate: *mut c_void, task: v8::Task, delay: f64) {
        post(isolate, task, false, delay);
    }

    // The platform is built without idle task support, so V8 posts none.
    fn post_idle_task(&self, _isolate: *mut c_void, task: v8::IdleTask) {
        drop(task);
    }
}

static UNSERVED: AtomicU64 = AtomicU64::new(0);

pub fn unserved() -> u64 {
    UNSERVED.load(Ordering::Relaxed)
}

// Called from any thread. A task for an isolate no runner serves (one being
// built into a snapshot, made or torn down) is dropped where it was posted,
// as nothing would ever run it.
fn post(isolate: *mut c_void, task: v8::Task, nestable: bool, delay: f64) {
    let posted = catch_unwind(AssertUnwindSafe(|| {
        let delay = Duration::try_from_secs_f64(delay).unwrap_or(Duration::MAX);
        let Some(tasks) = fork::current().tasks().find(isolate.addr()) else {
            UNSERVED.fetch_add(1, Ordering::Relaxed);
            drop(task);
            return;
        };
        if let Err(refused) = tasks.post(task, nestable, delay) {
            drop(refused);
        }
    }));
    if posted.is_err() {
        std::process::abort();
    }
}
