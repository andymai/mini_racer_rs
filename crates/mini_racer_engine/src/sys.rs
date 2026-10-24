use std::ffi::{c_char, c_void};
use std::mem::MaybeUninit;
use std::ptr::NonNull;
use std::sync::Arc;
use std::time::Duration;

use crate::process::sources::Units;

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
    // no reference from `Global::open` live, and the only slots it sets, an
    // `Arc<Runtime>` and its `Collections`, are Send.
    let shared = unsafe { isolate.try_into_shared() };
    shared.map_err(v8::IntoSharedError::into_isolate)
}

/// V8's view of `blob`, its own bytes rather than a copy. Whoever starts an
/// isolate from it keeps `blob` until that isolate is gone: a runner's control
/// block holds its context's blob, and a snapshot build holds its base.
pub fn startup_data(blob: &crate::SnapshotBlob) -> v8::StartupData {
    let bytes = blob.as_bytes();
    // SAFETY: a blob's bytes never move or change while it lives, and V8 reads
    // them only while an isolate started from them lives, which the blob
    // outlives as the doc comment requires of every caller.
    let bytes: &'static [u8] = unsafe { std::slice::from_raw_parts(bytes.as_ptr(), bytes.len()) };
    v8::StartupData::from(bytes)
}

/// A string V8 reads from `units` in place. It holds a reference to them,
/// which V8 hands back through `release_one_byte` or `release_two_byte` once,
/// on whatever thread it runs that on: when the string dies in a collection
/// or its isolate's teardown, or when V8 internalizes it while it lives and
/// gives the units to the internalized string, to release when that one dies,
/// or releases them at once if that string has its own (`MigrateExternalString`
/// in V8's `string.cc`). `None` past V8's length limit.
pub fn external_string<'s>(
    scope: &v8::PinScope<'s, '_>,
    units: &Units,
) -> Option<v8::Local<'s, v8::String>> {
    // V8 refuses a resource past its limit without disposing of it, which
    // would keep the reference for good, so it never gets one.
    match units {
        Units::OneByte(bytes) if bytes.len() <= v8::String::MAX_LENGTH => {
            let data = Arc::into_raw(Arc::clone(bytes));
            // SAFETY: the bytes stay where they are, unchanged, while the
            // reference made here lives; V8 only reads them, and hands the
            // reference back through `release_one_byte`, with this pointer and
            // length, once the string dies.
            unsafe {
                v8::String::new_external_onebyte_raw(
                    scope,
                    data.cast::<c_char>().cast_mut(),
                    bytes.len(),
                    release_one_byte,
                )
            }
        }
        Units::TwoByte(units) if units.len() <= v8::String::MAX_LENGTH => {
            let data = Arc::into_raw(Arc::clone(units));
            // SAFETY: as for one byte a unit, through `release_two_byte`.
            unsafe {
                v8::String::new_external_twobyte_raw(
                    scope,
                    data.cast::<u16>().cast_mut(),
                    units.len(),
                    release_two_byte,
                )
            }
        }
        Units::OneByte(_) | Units::TwoByte(_) => None,
    }
}

// Each runs once per string `external_string` made, and only releases its
// reference: the list in `process::sources` holds another for as long as any
// string reads them, so the units are never freed here.
unsafe extern "C" fn release_one_byte(data: *mut c_char, length: usize) {
    let released = std::panic::catch_unwind(|| {
        let bytes = std::ptr::slice_from_raw_parts(data.cast::<u8>().cast_const(), length);
        // SAFETY: V8 passes back the pointer and length `external_string`
        // gave it, once, so this releases the one reference it made.
        unsafe { Arc::decrement_strong_count(bytes) };
    });
    if released.is_err() {
        std::process::abort();
    }
}

unsafe extern "C" fn release_two_byte(data: *mut u16, length: usize) {
    let released = std::panic::catch_unwind(|| {
        let units = std::ptr::slice_from_raw_parts(data.cast_const(), length);
        // SAFETY: as in `release_one_byte`.
        unsafe { Arc::decrement_strong_count(units) };
    });
    if released.is_err() {
        std::process::abort();
    }
}

/// The UTF-16 units `bytes` hold, in the host's order as V8's deserializer
/// reads them, without a copy; `None` when they do not start where a unit
/// can be read from or hold an odd byte.
#[must_use]
pub fn units(bytes: &[u8]) -> Option<&[u16]> {
    // SAFETY: every bit pattern is a valid u16, and `align_to` hands out only
    // the aligned middle of `bytes`, borrowed for as long as `bytes` is.
    let (head, units, tail) = unsafe { bytes.align_to::<u16>() };
    (head.is_empty() && tail.is_empty()).then_some(units)
}

/// Writes the Latin-1 bytes of `text` at the start of `out` when `text` is
/// valid UTF-8 and every character in it is at most U+00FF, and returns how
/// many; `None` otherwise, or when `out` is shorter than `text`.
pub fn utf8_to_latin1(text: &[u8], out: &mut [u8]) -> Option<usize> {
    if out.len() < text.len() {
        return None;
    }
    // SAFETY: simdutf writes at most one byte per byte of `text`, which `out`
    // has room for.
    let converted = unsafe { v8::simdutf::convert_utf8_to_latin1_with_errors(text, out) };
    converted.is_ok().then_some(converted.count)
}

/// Writes the UTF-16 units of `text` at the start of `out` when `text` is
/// valid UTF-8, and returns how many; `None` otherwise, or when `out` is
/// shorter than `text`.
pub fn utf8_to_utf16(text: &[u8], out: &mut [u16]) -> Option<usize> {
    if out.len() < text.len() {
        return None;
    }
    // SAFETY: simdutf writes at most one unit per byte of `text`, which `out`
    // has room for.
    let converted = unsafe { v8::simdutf::convert_utf8_to_utf16le_with_errors(text, out) };
    converted.is_ok().then_some(converted.count)
}

/// A shared slice of a length fixed when it is made, written in place from its
/// first element on, a run at a time, and handed out once every one is
/// written.
pub struct Filling<T> {
    slots: Arc<[MaybeUninit<T>]>,
    written: usize,
}

impl<T: Copy> Filling<T> {
    #[must_use]
    pub fn new(len: usize) -> Self {
        Self {
            slots: Arc::new_uninit_slice(len),
            written: 0,
        }
    }

    /// Writes `run` after the elements already written; `None`, writing
    /// nothing, when it does not fit.
    pub fn append(&mut self, run: &[T]) -> Option<()> {
        let end = self.written.checked_add(run.len())?;
        let slots = Arc::get_mut(&mut self.slots)?.get_mut(self.written..end)?;
        for (slot, &unit) in slots.iter_mut().zip(run) {
            slot.write(unit);
        }
        self.written = end;
        Some(())
    }

    /// The slice, once every element is written.
    #[must_use]
    pub fn filled(self) -> Option<Arc<[T]>> {
        if self.written != self.slots.len() {
            return None;
        }
        // SAFETY: `append` writes the elements in order from the first and
        // counts only those it wrote, so every one is initialized.
        Some(unsafe { self.slots.assume_init() })
    }
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

// V8 tags a small integer's word with a clear low bit and a heap object's with
// a set one.
const SMI_TAG_MASK: usize = 1;

/// Whether `value` is a small integer, which V8's serializer writes as an
/// int32; a heap number it writes as a double, even one holding an int32.
#[must_use]
pub const fn is_smi(value: &v8::Value) -> bool {
    let slot: *const v8::Value = value;
    // SAFETY: rusty_v8 builds V8 without direct handles, so a reference to a
    // value is the address of its handle slot, one aligned tagged word that
    // lives as long as the reference.
    let tagged = unsafe { slot.cast::<usize>().read() };
    tagged & SMI_TAG_MASK == 0
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

/// Has V8 ask `block` before it aborts the process at the isolate's heap
/// limit. `block` must stay where it is for as long as the isolate can run:
/// a runner holds its context's block until it has dropped the isolate, and a
/// forked child that takes the isolate over registers its own block instead.
pub fn watch_heap_limit(isolate: &mut v8::Isolate, block: &crate::control::ControlBlock) {
    let data = std::ptr::from_ref(block).cast_mut().cast::<c_void>();
    isolate.add_near_heap_limit_callback(near_heap_limit, data);
}

/// Undoes `watch_heap_limit`, which must have been called, and with a
/// `limit` other than 0 puts the old generation's limit back to it, though
/// V8 keeps it no lower than the heap's live size and a quarter.
pub fn unwatch_heap_limit(isolate: &mut v8::Isolate, limit: usize) {
    isolate.remove_near_heap_limit_callback(near_heap_limit, limit);
}

unsafe extern "C" fn near_heap_limit(data: *mut c_void, current: usize, initial: usize) -> usize {
    let raised = std::panic::catch_unwind(|| {
        // SAFETY: `data` is the control block `watch_heap_limit` registered,
        // which outlives the registration as its doc comment requires.
        let block = unsafe { &*data.cast::<crate::control::ControlBlock>() };
        crate::runner::near_heap_limit(block, current, initial)
    });
    raised.unwrap_or_else(|_| std::process::abort())
}

/// Has V8 report the start and end of each collection on `isolate` for its
/// runner's counters: scavenges, minor mark-sweeps and mark-compacts, not the
/// start of incremental marking or the processing of weak callbacks, which
/// collect nothing.
pub fn count_collections(isolate: &mut v8::Isolate) {
    let collections = v8::GCType::kGCTypeScavenge
        | v8::GCType::kGCTypeMinorMarkSweep
        | v8::GCType::kGCTypeMarkSweepCompact;
    isolate.add_gc_prologue_callback(collection_began, std::ptr::null_mut(), collections);
    isolate.add_gc_epilogue_callback(collection_ended, std::ptr::null_mut(), collections);
}

unsafe extern "C" fn collection_began(
    isolate: v8::UnsafeRawIsolatePtr,
    _type: v8::GCType,
    _flags: v8::GCCallbackFlags,
    _data: *mut c_void,
) {
    let counted = std::panic::catch_unwind(|| {
        // SAFETY: V8 calls a GC prologue callback on the thread that has the
        // isolate entered, with its own non-null pointer to it.
        let isolate = unsafe { v8::Isolate::ref_from_raw_isolate_ptr(&isolate) };
        crate::runner::collection_began(isolate);
    });
    if counted.is_err() {
        std::process::abort();
    }
}

unsafe extern "C" fn collection_ended(
    isolate: v8::UnsafeRawIsolatePtr,
    _type: v8::GCType,
    _flags: v8::GCCallbackFlags,
    _data: *mut c_void,
) {
    let counted = std::panic::catch_unwind(|| {
        // SAFETY: as in `collection_began`, for the epilogue.
        let isolate = unsafe { v8::Isolate::ref_from_raw_isolate_ptr(&isolate) };
        crate::runner::collection_ended(isolate);
    });
    if counted.is_err() {
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
const POST_DELAYED_TASK_ON_WORKER_THREAD_IMPL: usize = 21;
// The words before the first slot: the offset to the top and the type info.
const VTABLE_PREFIX: usize = 2;

/// `rusty_v8`'s `CustomPlatform` vtable as words, the two before its first slot
/// included; `nm -S` sizes it at 0xc0 bytes.
pub type PlatformVtable = [usize; VTABLE_PREFIX + 22];

/// A copy of `platform`'s vtable whose worker thread entries lead to
/// `process::workers`, which V8 then also sizes its jobs by.
#[must_use]
pub fn worker_vtable(platform: &v8::Platform) -> Box<PlatformVtable> {
    let this: *const v8::Platform = platform;
    // SAFETY: `platform` is a live C++ v8::Platform, whose first word points
    // at its vtable's first slot.
    let first_slot = unsafe { *this.cast::<*const usize>() };
    // SAFETY: a vtable keeps the offset to the top and the type info right
    // before the slot its objects point at.
    let start = unsafe { first_slot.sub(VTABLE_PREFIX) };
    // SAFETY: the vtable is a whole `PlatformVtable` of initialized words
    // from `start`, never written once the library is loaded.
    let mut vtable = Box::new(unsafe { start.cast::<PlatformVtable>().read() });
    let entries = [
        (
            NUMBER_OF_WORKER_THREADS,
            number_of_worker_threads as *const (),
        ),
        (
            POST_TASK_ON_WORKER_THREAD_IMPL,
            post_task_on_worker_thread as *const (),
        ),
        (
            POST_DELAYED_TASK_ON_WORKER_THREAD_IMPL,
            post_delayed_task_on_worker_thread as *const (),
        ),
    ];
    for (slot, entry) in entries {
        vtable[VTABLE_PREFIX + slot] = entry.addr();
    }
    vtable
}

/// Has every later virtual call on `platform` go through `vtable`.
pub fn use_vtable(platform: &mut v8::Platform, vtable: &'static PlatformVtable) {
    let this: *mut v8::Platform = platform;
    let first_slot = vtable[VTABLE_PREFIX..].as_ptr();
    // SAFETY: `platform` is a live C++ object whose first word is its vtable
    // pointer, not yet handed to V8, and `vtable` lives as long as the
    // process with the platform's own entries in every other slot.
    unsafe { this.cast::<*const usize>().write(first_slot) };
}

extern "C" fn number_of_worker_threads(_platform: *const c_void) -> libc::c_int {
    let threads = std::panic::catch_unwind(crate::process::workers::threads);
    threads.map_or_else(
        |_| std::process::abort(),
        |threads| libc::c_int::try_from(threads).unwrap_or(libc::c_int::MAX),
    )
}

// V8's libc++ passes a std::unique_ptr<Task> in a register, and the callee
// owns the task from then on. The priority is ignored, as rusty_v8's platform
// ignores it in V8's default `PriorityMode::kDontApply`.
extern "C" fn post_task_on_worker_thread(
    _platform: *const c_void,
    _priority: u8,
    task: *mut c_void,
    _location: *const c_void,
) {
    if std::panic::catch_unwind(|| post_worker_task(task, Duration::ZERO)).is_err() {
        std::process::abort();
    }
}

extern "C" fn post_delayed_task_on_worker_thread(
    _platform: *const c_void,
    _priority: u8,
    task: *mut c_void,
    delay_in_seconds: f64,
    _location: *const c_void,
) {
    let posted = std::panic::catch_unwind(|| {
        let delay = Duration::try_from_secs_f64(delay_in_seconds.max(0.0)).unwrap_or(Duration::MAX);
        post_worker_task(task, delay);
    });
    if posted.is_err() {
        std::process::abort();
    }
}

fn post_worker_task(task: *mut c_void, delay: Duration) {
    if let Some(task) = NonNull::new(task) {
        crate::process::workers::post(WorkerTask(task), delay);
    }
}

// rusty_v8's own entry points for a task V8 handed over, which it also runs
// the runners' foreground tasks with.
unsafe extern "C" {
    fn v8__Task__Run(task: *mut c_void);
    fn v8__Task__DELETE(task: *mut c_void);
}

/// A `v8::Task` posted for a worker thread, owned from the post on: run once,
/// then deleted.
pub struct WorkerTask(NonNull<c_void>);

// SAFETY: V8 posts worker tasks from any thread for any worker thread to run.
unsafe impl Send for WorkerTask {}

impl WorkerTask {
    pub fn run(self) {
        // SAFETY: the task is V8's, whole and not yet run.
        unsafe { v8__Task__Run(self.0.as_ptr()) };
    }
}

impl Drop for WorkerTask {
    fn drop(&mut self) {
        // SAFETY: the task is V8's and deleted once, here.
        unsafe { v8__Task__DELETE(self.0.as_ptr()) };
    }
}

/// The processors online, which `rusty_v8` sizes a platform's default pool by.
#[must_use]
pub fn processors_online() -> usize {
    // SAFETY: sysconf only reads the system's configuration.
    let online = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) };
    usize::try_from(online).unwrap_or(0)
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

/// `v8::Task`s made in Rust, and the platform's worker thread entries called
/// the way V8 calls them.
#[cfg(test)]
pub mod testing {
    use std::ffi::c_void;
    use std::ptr::NonNull;

    use super::{
        NUMBER_OF_WORKER_THREADS, POST_DELAYED_TASK_ON_WORKER_THREAD_IMPL,
        POST_TASK_ON_WORKER_THREAD_IMPL, WorkerTask,
    };

    // v8::TaskPriority::kUserBlocking.
    const USER_BLOCKING: u8 = 2;

    type TaskMethod = unsafe extern "C" fn(*mut RustTask);
    // Apple's arm64 ABI has a complete destructor return `this`; elsewhere
    // the returned value is ignored.
    type TaskDestructor = unsafe extern "C" fn(*mut RustTask) -> *mut RustTask;

    // A C++ vtable as clang lays one out for v8::Task: the object points past
    // the first two words, at the destructors and then `Run`.
    #[repr(C)]
    struct TaskVtable {
        offset_to_top: isize,
        type_info: *const c_void,
        complete_destructor: TaskDestructor,
        deleting_destructor: TaskMethod,
        run: TaskMethod,
    }

    // A constant reference, promoted to memory that lives as long as the
    // program, as a vtable must.
    const TASK_VTABLE: &TaskVtable = &TaskVtable {
        offset_to_top: 0,
        type_info: std::ptr::null(),
        complete_destructor: destroy_task,
        deleting_destructor: delete_task,
        run: run_task,
    };

    #[repr(C)]
    struct RustTask {
        vtable: *const TaskMethod,
        run: Option<Box<dyn FnOnce() + Send>>,
    }

    unsafe extern "C" fn destroy_task(task: *mut RustTask) -> *mut RustTask {
        let destroyed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // SAFETY: the task's owner destroys it once, and it was built in
            // `task`.
            unsafe { std::ptr::drop_in_place(task) };
        }));
        if destroyed.is_err() {
            std::process::abort();
        }
        task
    }

    unsafe extern "C" fn delete_task(task: *mut RustTask) {
        let deleted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // SAFETY: the task's owner deletes it once; it came from
            // `Box::into_raw`.
            drop(unsafe { Box::from_raw(task) });
        }));
        if deleted.is_err() {
            std::process::abort();
        }
    }

    unsafe extern "C" fn run_task(task: *mut RustTask) {
        let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // SAFETY: the task's owner runs it on one thread, before deleting
            // it.
            let run = unsafe { (*task).run.take() };
            if let Some(run) = run {
                run();
            }
        }));
        if ran.is_err() {
            std::process::abort();
        }
    }

    fn task(run: Box<dyn FnOnce() + Send>) -> NonNull<RustTask> {
        NonNull::from(Box::leak(Box::new(RustTask {
            vtable: (&raw const TASK_VTABLE.complete_destructor).cast(),
            run: Some(run),
        })))
    }

    /// A worker task that runs `run`, as if V8 had posted it.
    pub fn worker_task(run: Box<dyn FnOnce() + Send>) -> WorkerTask {
        WorkerTask(task(run).cast())
    }

    const fn platform_method(platform: &v8::Platform, slot: usize) -> *const c_void {
        let this: *const v8::Platform = platform;
        // SAFETY: `platform` is a live C++ v8::Platform, whose first word
        // points at its vtable.
        let vtable = unsafe { *this.cast::<*const *const c_void>() };
        // SAFETY: v8::Platform declares more virtual functions than `slot`,
        // so the entry is inside the vtable.
        let entry = unsafe { vtable.add(slot) };
        // SAFETY: the entry is an initialized function pointer of the vtable.
        unsafe { *entry }
    }

    /// The platform's `NumberOfWorkerThreads()`.
    pub fn worker_threads(platform: &v8::Platform) -> usize {
        let method = platform_method(platform, NUMBER_OF_WORKER_THREADS);
        // SAFETY: the slot holds `int Platform::NumberOfWorkerThreads()`,
        // called with `this` first, as clang passes it.
        let method = unsafe {
            std::mem::transmute::<
                *const c_void,
                unsafe extern "C" fn(*const v8::Platform) -> libc::c_int,
            >(method)
        };
        // SAFETY: `platform` is the live object the method belongs to.
        let count = unsafe { method(platform) };
        usize::try_from(count).unwrap_or(0)
    }

    /// Posts `run` through the platform's `PostTaskOnWorkerThreadImpl`, as
    /// V8's jobs do.
    pub fn post_on_worker_thread(platform: &v8::Platform, run: Box<dyn FnOnce() + Send>) {
        // v8::SourceLocation holds one pointer, which V8 reads as no location
        // when null.
        let location: *const c_void = std::ptr::null();
        let method = platform_method(platform, POST_TASK_ON_WORKER_THREAD_IMPL);
        // SAFETY: the slot holds `void Platform::PostTaskOnWorkerThreadImpl(
        // TaskPriority, std::unique_ptr<Task>, const SourceLocation&)`. V8's
        // libc++ passes a unique_ptr in a register, as its own callers do, and
        // the callee owns it from then on.
        let method = unsafe {
            std::mem::transmute::<
                *const c_void,
                unsafe extern "C" fn(*const v8::Platform, u8, *mut RustTask, *const *const c_void),
            >(method)
        };
        let task = task(run).as_ptr();
        // SAFETY: `platform` is the live object the method belongs to, the
        // task is a whole v8::Task, and `location` outlives the call.
        unsafe { method(platform, USER_BLOCKING, task, &raw const location) };
    }

    /// Posts `run` through the platform's `PostDelayedTaskOnWorkerThreadImpl`
    /// to run after `delay` seconds.
    pub fn post_delayed_on_worker_thread(
        platform: &v8::Platform,
        delay: f64,
        run: Box<dyn FnOnce() + Send>,
    ) {
        let location: *const c_void = std::ptr::null();
        let method = platform_method(platform, POST_DELAYED_TASK_ON_WORKER_THREAD_IMPL);
        // SAFETY: the slot holds `void PostDelayedTaskOnWorkerThreadImpl(
        // TaskPriority, std::unique_ptr<Task>, double, const SourceLocation&)`,
        // the unique_ptr passed as in `post_on_worker_thread`.
        let method = unsafe {
            std::mem::transmute::<
                *const c_void,
                unsafe extern "C" fn(
                    *const v8::Platform,
                    u8,
                    *mut RustTask,
                    f64,
                    *const *const c_void,
                ),
            >(method)
        };
        let task = task(run).as_ptr();
        // SAFETY: as in `post_on_worker_thread`.
        unsafe { method(platform, USER_BLOCKING, task, delay, &raw const location) };
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::mpsc;
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::{Duration, Instant};

    use super::testing::{post_delayed_on_worker_thread, post_on_worker_thread, worker_threads};

    // The slots are V8's own worker thread entries in the V8 this crate
    // links: a platform left as V8 builds it reports the pool it was asked
    // for and runs tasks posted through them on its own threads.
    #[test]
    fn the_worker_slots_are_v8s_own() {
        let platform = v8::new_custom_platform(3, false, true, crate::process::tasks::Foreground);
        assert_eq!(worker_threads(&platform), 3);
        let (ran, order) = mpsc::channel();
        let record = |label: &'static str| -> Box<dyn FnOnce() + Send> {
            let ran = ran.clone();
            Box::new(move || {
                let pool = std::thread::current().name() == Some("mini_racer_rs worker");
                ran.send((label, pool, Instant::now())).expect("receiver");
            })
        };
        let posted = Instant::now();
        post_delayed_on_worker_thread(&platform, 0.05, record("delayed"));
        post_on_worker_thread(&platform, record("now"));
        let first = order.recv_timeout(Duration::from_secs(10)).expect("a task");
        let second = order.recv_timeout(Duration::from_secs(10)).expect("a task");
        assert_eq!((first.0, first.1), ("now", false));
        assert_eq!((second.0, second.1), ("delayed", false));
        assert!(second.2 - posted >= Duration::from_millis(50));
        drop(platform);
    }

    // Patched, the platform reports this process's pool, and tasks posted
    // through it run on every pool thread, each on its own, and are deleted.
    // Each waits for the rest to start, so one thread cannot run them all.
    #[test]
    fn runs_a_task_on_each_worker_thread() {
        let _turn = mini_racer_sync::lock(&crate::process::workers::TESTS);
        crate::process::ensure_initialized();
        let platform = v8::V8::get_current_platform();
        let threads = worker_threads(&platform);
        assert_eq!(threads, crate::process::workers::threads());
        assert_eq!(threads, super::processors_online().clamp(1, 16));
        let started = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
        for _ in 0..threads {
            let started = Arc::clone(&started);
            post_on_worker_thread(
                &platform,
                Box::new(move || {
                    let (seen, all) = &*started;
                    let mut seen = seen.lock().expect("lock");
                    let thread = std::thread::current();
                    seen.push((thread.id(), thread.name().map(str::to_owned)));
                    all.notify_all();
                    let deadline = Instant::now() + Duration::from_secs(10);
                    while seen.len() < threads && Instant::now() < deadline {
                        seen = all
                            .wait_timeout(seen, Duration::from_millis(10))
                            .expect("wait")
                            .0;
                    }
                    drop(seen);
                }),
            );
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while Arc::strong_count(&started) > 1 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        let seen = std::mem::take(&mut *started.0.lock().expect("lock"));
        assert_eq!(seen.len(), threads);
        assert_eq!(
            seen.iter().map(|(id, _)| id).collect::<HashSet<_>>().len(),
            threads
        );
        assert!(
            seen.iter()
                .all(|(_, name)| name.as_deref() == Some("mini_racer_rs worker"))
        );
        assert_eq!(Arc::strong_count(&started), 1);
    }

    #[test]
    fn runs_delayed_tasks_by_deadline_and_not_before_their_delay() {
        let _turn = mini_racer_sync::lock(&crate::process::workers::TESTS);
        crate::process::ensure_initialized();
        let platform = v8::V8::get_current_platform();
        let ran = Arc::new(Mutex::new(Vec::new()));
        let record = |label: &'static str| -> Box<dyn FnOnce() + Send> {
            let ran = Arc::clone(&ran);
            Box::new(move || ran.lock().expect("lock").push((label, Instant::now())))
        };
        let posted = Instant::now();
        post_delayed_on_worker_thread(&platform, 0.12, record("later"));
        post_delayed_on_worker_thread(&platform, 0.06, record("sooner"));
        post_on_worker_thread(&platform, record("now"));
        let deadline = Instant::now() + Duration::from_secs(10);
        while ran.lock().expect("lock").len() < 3 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        let ran = std::mem::take(&mut *ran.lock().expect("lock"));
        let labels: Vec<_> = ran.iter().map(|(label, _)| *label).collect();
        assert_eq!(labels, ["now", "sooner", "later"]);
        assert!(ran[1].1 - posted >= Duration::from_millis(60));
        assert!(ran[2].1 - posted >= Duration::from_millis(120));
    }
}
