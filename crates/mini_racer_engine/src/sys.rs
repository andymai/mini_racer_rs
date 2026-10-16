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
    unsafe { isolate.try_into_shared() }.ok()
}
