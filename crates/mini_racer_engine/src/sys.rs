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
/// without being held between requests. `None` if V8 refuses.
pub fn share(isolate: v8::OwnedIsolate) -> Option<v8::SharedIsolate> {
    // SAFETY: the runner calls this on the thread that created `isolate`, with
    // no reference from `Global::open` live, and the only slot it sets, an
    // `Arc<Runtime>`, is Send.
    let shared = unsafe { isolate.try_into_shared() };
    // A refused isolate comes back to be dropped here, on its runner.
    shared.map_err(|refused| drop(refused.into_isolate())).ok()
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
