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

// V8 names its worker threads "V8 DefaultWorkerThreadsTaskRunner WorkerThread"
// and cuts every thread name to 15 bytes.
#[cfg(any(target_os = "linux", target_os = "macos"))]
const V8_WORKER: &[u8] = b"V8 DefaultWorke";

/// V8's worker threads, and how many of them are asleep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Workers {
    pub total: usize,
    pub asleep: usize,
}

/// V8's worker threads now, or `None` when the process cannot tell.
#[cfg(target_os = "linux")]
#[must_use]
pub fn v8_workers() -> Option<Workers> {
    let mut workers = Workers {
        total: 0,
        asleep: 0,
    };
    for task in std::fs::read_dir("/proc/self/task").ok()? {
        let path = task.ok()?.path();
        // A thread that has exited since the listing is skipped.
        let Ok(name) = std::fs::read(path.join("comm")) else {
            continue;
        };
        if !name.starts_with(V8_WORKER) {
            continue;
        }
        let Ok(stat) = std::fs::read_to_string(path.join("stat")) else {
            continue;
        };
        // The state follows the name, which is in parentheses and may hold any
        // byte, so the last parenthesis ends it.
        let state = stat
            .rsplit_once(") ")
            .and_then(|(_, rest)| rest.chars().next());
        workers.total += 1;
        workers.asleep += usize::from(state == Some('S'));
    }
    Some(workers)
}

/// V8's worker threads now, or `None` when the process cannot tell.
#[cfg(target_os = "macos")]
#[must_use]
pub fn v8_workers() -> Option<Workers> {
    // The proc_info flavor that lists a process's thread handles.
    const PROC_PIDLISTTHREADS: libc::c_int = 6;
    let pid = libc::pid_t::try_from(std::process::id()).ok()?;
    let mut threads = vec![0_u64; 1024];
    let capacity = libc::c_int::try_from(std::mem::size_of_val(threads.as_slice())).ok()?;
    // SAFETY: the buffer has `capacity` writable bytes for the handles.
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            PROC_PIDLISTTHREADS,
            0,
            threads.as_mut_ptr().cast(),
            capacity,
        )
    };
    // libproc reports a failure as 0.
    let count =
        usize::try_from(written).ok().filter(|&bytes| bytes > 0)? / std::mem::size_of::<u64>();
    // A full buffer may have left threads out.
    if count >= threads.len() {
        return None;
    }
    let size = libc::c_int::try_from(std::mem::size_of::<libc::proc_threadinfo>()).ok()?;
    let mut workers = Workers {
        total: 0,
        asleep: 0,
    };
    for &thread in threads.get(..count)? {
        let mut info = libc::proc_threadinfo::default();
        // SAFETY: `info` has room for one `proc_threadinfo`, `size` bytes.
        let read = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTHREADINFO,
                thread,
                (&raw mut info).cast(),
                size,
            )
        };
        // A thread that has exited since the listing is skipped.
        if read != size {
            continue;
        }
        let name = info.pth_name.map(libc::c_char::cast_unsigned);
        if name.starts_with(V8_WORKER) {
            workers.total += 1;
            workers.asleep += usize::from(info.pth_run_state == libc::TH_STATE_WAITING);
        }
    }
    Some(workers)
}

/// V8's worker threads now, or `None` when the process cannot tell.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[must_use]
pub const fn v8_workers() -> Option<Workers> {
    None
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

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    // A V8 that renames its worker threads would turn the wait before a fork
    // off without a word; this fails first.
    #[test]
    fn finds_v8s_worker_threads() {
        crate::process::ensure_initialized();
        let workers = super::v8_workers();
        assert!(
            workers.is_some_and(|workers| workers.total > 0),
            "{workers:?}"
        );
    }
}
