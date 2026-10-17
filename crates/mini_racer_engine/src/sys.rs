use std::ffi::c_void;

// The prebuilt V8 archive marks every heap mapping MADV_DONTFORK, so a forked
// child would lose the isolate's memory. `-Wl,--wrap=madvise` (build.rs) routes
// V8's calls here; this drops that one advice and forwards the rest unchanged.
#[cfg(all(target_os = "linux", not(feature = "no-madvise-wrap")))]
#[unsafe(no_mangle)]
pub extern "C" fn __wrap_madvise(
    addr: *mut libc::c_void,
    len: libc::size_t,
    advice: libc::c_int,
) -> libc::c_int {
    let forwarded = std::panic::catch_unwind(|| {
        if advice == libc::MADV_DONTFORK {
            return 0;
        }
        // SAFETY: the arguments are the caller's own madvise arguments, passed
        // through unchanged to the kernel, which validates them.
        let result = unsafe { libc::syscall(libc::SYS_madvise, addr, len, advice) };
        libc::c_int::try_from(result).unwrap_or(-1)
    });
    forwarded.unwrap_or_else(|_| std::process::abort())
}

/// Turns a runner's isolate into one any thread can lock, so it can be served
/// without being held between requests. A refused isolate comes back for its
/// runner to drop.
pub fn share(isolate: v8::OwnedIsolate) -> Result<v8::SharedIsolate, v8::OwnedIsolate> {
    // SAFETY: the runner calls this on the thread that created `isolate`, with
    // no reference from `Global::open` live, and the only slot it sets, an
    // `Arc<Runtime>`, is Send.
    let shared = unsafe { isolate.try_into_shared() };
    shared.map_err(v8::IntoSharedError::into_isolate)
}

/// The address V8 names `isolate` by when it posts the isolate a task.
#[must_use]
pub fn isolate_key(isolate: &v8::Isolate) -> usize {
    // SAFETY: the pointer is only compared as an address, never dereferenced.
    let raw = unsafe { isolate.as_raw_isolate_ptr() };
    // SAFETY: `UnsafeRawIsolatePtr` is a transparent wrapper around the
    // pointer V8 passes to its task runners.
    let pointer = unsafe { std::mem::transmute::<v8::UnsafeRawIsolatePtr, *mut c_void>(raw) };
    pointer.addr()
}

/// Has V8 report each garbage collection on `isolate`, so its runner can
/// check the heap against `max_memory`.
pub fn watch_heap(isolate: &mut v8::Isolate) {
    isolate.add_gc_epilogue_callback(after_gc, std::ptr::null_mut(), v8::GCType::kGCTypeAll);
}

unsafe extern "C" fn after_gc(
    mut isolate: v8::UnsafeRawIsolatePtr,
    _type: v8::GCType,
    _flags: v8::GCCallbackFlags,
    _data: *mut c_void,
) {
    let checked = std::panic::catch_unwind(move || {
        // SAFETY: V8 calls a GC epilogue callback on the thread that has the
        // isolate entered, with its own non-null pointer to it.
        let isolate = unsafe { v8::Isolate::ref_from_raw_isolate_ptr_mut(&mut isolate) };
        crate::runner::check_heap(isolate);
    });
    if checked.is_err() {
        std::process::abort();
    }
}

/// Asks the isolate's runner to park for a pending fork the next time V8
/// checks for interrupts, where JavaScript may be running for a long time.
pub fn interrupt_for_fork(handle: &v8::IsolateHandle) {
    handle.request_interrupt(park_for_fork, std::ptr::null_mut());
}

extern "C" fn park_for_fork(isolate: v8::UnsafeRawIsolatePtr, _data: *mut std::ffi::c_void) {
    let parked = std::panic::catch_unwind(|| {
        // SAFETY: V8 calls an interrupt callback on the thread that has the
        // isolate entered, with its own non-null pointer to it.
        let isolate = unsafe { v8::Isolate::ref_from_raw_isolate_ptr(&isolate) };
        crate::runner::park_for_fork(isolate);
    });
    if parked.is_err() {
        std::process::abort();
    }
}

/// Returns the free pages of every glibc malloc arena in the process to the
/// system. glibc keeps what is freed for later allocations, and parsing a
/// large script frees most of what it took: about half of a loaded bundle's
/// resident memory, which Ruby's collector never hands back.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub fn release_free_memory() {
    // SAFETY: malloc_trim only reads and updates glibc's allocator state,
    // under the locks glibc takes for it.
    let _ = unsafe { libc::malloc_trim(0) };
}

/// A no-op where malloc is not glibc's.
#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
pub const fn release_free_memory() {}

// Slots of v8::Platform's vtable, in v8-platform.h's declaration order after
// the destructor's two; the v8-bump skill checks them against each new V8.
const NUMBER_OF_WORKER_THREADS: usize = 6;
const POST_TASK_ON_WORKER_THREAD_IMPL: usize = 20;
// v8::TaskPriority::kUserBlocking.
const USER_BLOCKING: u8 = 2;

type TaskMethod = unsafe extern "C" fn(*mut WorkerTask);
// Apple's arm64 ABI has a complete destructor return `this`; elsewhere the
// returned value is ignored.
type TaskDestructor = unsafe extern "C" fn(*mut WorkerTask) -> *mut WorkerTask;

// A C++ vtable as clang lays one out for v8::Task: the object points past the
// first two words, at the destructors and then `Run`.
#[repr(C)]
struct TaskVtable {
    offset_to_top: isize,
    type_info: *const c_void,
    complete_destructor: TaskDestructor,
    deleting_destructor: TaskMethod,
    run: TaskMethod,
}

// A constant reference, promoted to memory that lives as long as the program,
// as a vtable must.
const TASK_VTABLE: &TaskVtable = &TaskVtable {
    offset_to_top: 0,
    type_info: std::ptr::null(),
    complete_destructor: destroy_task,
    deleting_destructor: delete_task,
    run: run_task,
};

// A v8::Task made in Rust, which V8 runs once on a worker thread and deletes.
#[repr(C)]
struct WorkerTask {
    vtable: *const TaskMethod,
    run: Option<Box<dyn FnOnce() + Send>>,
}

unsafe extern "C" fn destroy_task(task: *mut WorkerTask) -> *mut WorkerTask {
    let destroyed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: V8 destroys a task it owns once, and it was built in
        // `post_on_worker_thread`.
        unsafe { std::ptr::drop_in_place(task) };
    }));
    if destroyed.is_err() {
        std::process::abort();
    }
    task
}

unsafe extern "C" fn delete_task(task: *mut WorkerTask) {
    let deleted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: V8 deletes a task it owns once; it came from `Box::into_raw`.
        drop(unsafe { Box::from_raw(task) });
    }));
    if deleted.is_err() {
        std::process::abort();
    }
}

unsafe extern "C" fn run_task(task: *mut WorkerTask) {
    let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: V8 runs a task it owns, on one thread, before deleting it.
        let run = unsafe { (*task).run.take() };
        if let Some(run) = run {
            run();
        }
    }));
    if ran.is_err() {
        std::process::abort();
    }
}

const fn platform_method(platform: &v8::Platform, slot: usize) -> *const c_void {
    let this: *const v8::Platform = platform;
    // SAFETY: `platform` is a live C++ v8::Platform, whose first word points
    // at its vtable.
    let vtable = unsafe { *this.cast::<*const *const c_void>() };
    // SAFETY: v8::Platform declares more virtual functions than `slot`, so the
    // entry is inside the vtable.
    let entry = unsafe { vtable.add(slot) };
    // SAFETY: the entry is an initialized function pointer of the vtable.
    unsafe { *entry }
}

/// How many worker threads V8's platform runs.
#[must_use]
pub fn worker_threads(platform: &v8::Platform) -> usize {
    let method = platform_method(platform, NUMBER_OF_WORKER_THREADS);
    // SAFETY: the slot holds `int Platform::NumberOfWorkerThreads()`, called
    // with `this` first, as clang passes it.
    let method = unsafe {
        std::mem::transmute::<*const c_void, unsafe extern "C" fn(*const v8::Platform) -> libc::c_int>(
            method,
        )
    };
    // SAFETY: `platform` is the live object the method belongs to.
    let count = unsafe { method(platform) };
    usize::try_from(count).unwrap_or(0)
}

/// Has V8 run `run` once on one of its worker threads.
pub fn post_on_worker_thread(platform: &v8::Platform, run: Box<dyn FnOnce() + Send>) {
    let task = Box::into_raw(Box::new(WorkerTask {
        vtable: (&raw const TASK_VTABLE.complete_destructor).cast(),
        run: Some(run),
    }));
    // v8::SourceLocation holds one pointer, which V8 reads as no location when
    // null.
    let location: *const c_void = std::ptr::null();
    let method = platform_method(platform, POST_TASK_ON_WORKER_THREAD_IMPL);
    // SAFETY: the slot holds `void Platform::PostTaskOnWorkerThreadImpl(
    // TaskPriority, std::unique_ptr<Task>, const SourceLocation&)`. V8's
    // libc++ passes a unique_ptr in a register, as its own callers do, and the
    // callee owns it from then on.
    let method = unsafe {
        std::mem::transmute::<
            *const c_void,
            unsafe extern "C" fn(*const v8::Platform, u8, *mut WorkerTask, *const *const c_void),
        >(method)
    };
    // SAFETY: `platform` is the live object the method belongs to, `task` is
    // a whole v8::Task, and `location` outlives the call.
    unsafe { method(platform, USER_BLOCKING, task, &raw const location) };
}

/// Runs the engine's fork handlers inside every `fork()` of the process.
pub fn register_fork_handlers() {
    // SAFETY: the handlers are plain functions that live as long as the
    // process and never unwind.
    let registered =
        unsafe { libc::pthread_atfork(Some(on_prepare), Some(on_parent), Some(on_child)) };
    debug_assert_eq!(registered, 0, "pthread_atfork");
}

extern "C" fn on_prepare() {
    if std::panic::catch_unwind(crate::process::fork::prepare).is_err() {
        std::process::abort();
    }
}

extern "C" fn on_parent() {
    if std::panic::catch_unwind(crate::process::fork::parent).is_err() {
        std::process::abort();
    }
}

extern "C" fn on_child() {
    if std::panic::catch_unwind(crate::process::fork::child).is_err() {
        std::process::abort();
    }
}

/// Has V8 run `run` on one of its worker threads after `delay` seconds, as
/// V8's own delayed worker tasks do.
#[cfg(test)]
pub fn post_delayed_on_worker_thread(
    platform: &v8::Platform,
    delay: f64,
    run: Box<dyn FnOnce() + Send>,
) {
    // v8::Platform::PostDelayedTaskOnWorkerThreadImpl, after the immediate one.
    const SLOT: usize = POST_TASK_ON_WORKER_THREAD_IMPL + 1;
    let task = Box::into_raw(Box::new(WorkerTask {
        vtable: (&raw const TASK_VTABLE.complete_destructor).cast(),
        run: Some(run),
    }));
    let location: *const c_void = std::ptr::null();
    let method = platform_method(platform, SLOT);
    // SAFETY: the slot holds `void PostDelayedTaskOnWorkerThreadImpl(
    // TaskPriority, std::unique_ptr<Task>, double, const SourceLocation&)`,
    // the unique_ptr passed as in `post_on_worker_thread`.
    let method = unsafe {
        std::mem::transmute::<
            *const c_void,
            unsafe extern "C" fn(
                *const v8::Platform,
                u8,
                *mut WorkerTask,
                f64,
                *const *const c_void,
            ),
        >(method)
    };
    // SAFETY: as in `post_on_worker_thread`.
    unsafe { method(platform, USER_BLOCKING, task, delay, &raw const location) };
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::{Arc, Condvar, Mutex};
    use std::thread::ThreadId;
    use std::time::{Duration, Instant};

    // The vtable slots match the V8 this crate links: tasks posted through
    // them run on every worker thread, each on its own, and are deleted.
    // Each waits for the rest to start, so one worker cannot run them all.
    #[test]
    fn runs_a_task_on_each_of_v8s_worker_threads() {
        let _turn = mini_racer_sync::lock(&crate::process::fork::WORKER_TESTS);
        crate::process::ensure_initialized();
        let platform = crate::process::platform_with_workers().expect("initialized");
        let workers = super::worker_threads(platform);
        assert!(workers > 0);
        let started = Arc::new((Mutex::new(Vec::<ThreadId>::new()), Condvar::new()));
        for _ in 0..workers {
            let started = Arc::clone(&started);
            super::post_on_worker_thread(
                platform,
                Box::new(move || {
                    let (threads, all) = &*started;
                    let mut threads = threads.lock().expect("lock");
                    threads.push(std::thread::current().id());
                    all.notify_all();
                    let deadline = Instant::now() + Duration::from_secs(10);
                    while threads.len() < workers && Instant::now() < deadline {
                        threads = all
                            .wait_timeout(threads, Duration::from_millis(10))
                            .expect("wait")
                            .0;
                    }
                    drop(threads);
                }),
            );
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while Arc::strong_count(&started) > 1 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        let threads = started.0.lock().expect("lock");
        assert_eq!(threads.len(), workers);
        assert_eq!(threads.iter().collect::<HashSet<_>>().len(), workers);
        assert_eq!(Arc::strong_count(&started), 1);
    }
}
