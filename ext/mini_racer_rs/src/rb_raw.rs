use std::ffi::{c_int, c_void};

use magnus::rb_sys::{FromRawValue, protect};
use magnus::{Error, RString, Ruby, Value};

use crate::guard::{Panicked, guard};

struct Call<F, R> {
    work: Option<F>,
    result: Option<Result<R, Panicked>>,
}

unsafe extern "C" fn call_without_gvl<F, R>(data: *mut c_void) -> *mut c_void
where
    F: FnOnce() -> R,
{
    // SAFETY: `data` is the `Call` that `nogvl` keeps alive on its stack for the
    // whole synchronous rb_nogvl call.
    let call = unsafe { &mut *data.cast::<Call<F, R>>() };
    if let Some(work) = call.work.take() {
        call.result = Some(guard(work));
    }
    std::ptr::null_mut()
}

unsafe extern "C" fn call_unblock<U: Fn()>(data: *mut c_void) {
    // SAFETY: `data` is the `&U` that `nogvl` borrows for the whole rb_nogvl call;
    // Ruby only calls the unblock function while that call is in progress.
    let unblock = unsafe { &*data.cast::<U>() };
    let _ = guard(unblock);
}

// RB_NOGVL_INTR_FAIL makes rb_nogvl return instead of raising when an interrupt
// is pending, so no Ruby exception can unwind through these Rust frames (I6).
// Returns None when Ruby skipped `work` because an interrupt was already pending.
pub fn nogvl<F, U, R>(_ruby: &Ruby, work: F, unblock: &U) -> Option<Result<R, Panicked>>
where
    F: FnOnce() -> R + Send,
    R: Send,
    U: Fn() + Sync,
{
    let mut call = Call {
        work: Some(work),
        result: None,
    };
    let flags = c_int::try_from(rb_sys::RB_NOGVL_INTR_FAIL).unwrap_or(1);
    // SAFETY: rb_nogvl runs both callbacks synchronously before returning, so
    // `call` and `unblock` outlive every access through the raw pointers.
    unsafe {
        rb_sys::rb_nogvl(
            Some(call_without_gvl::<F, R>),
            std::ptr::addr_of_mut!(call).cast::<c_void>(),
            Some(call_unblock::<U>),
            std::ptr::from_ref(unblock).cast_mut().cast::<c_void>(),
            flags,
        );
    }
    call.result
}

// Runs pending interrupts: signal traps return Ok, while Thread#raise, #kill,
// Timeout and SIGINT come back as the Err the caller must eventually return.
pub fn check_ints(_ruby: &Ruby) -> Result<(), Error> {
    protect(|| {
        // SAFETY: the `&Ruby` argument proves this thread holds the GVL, and
        // `protect` catches any non-local exit the interrupt handlers raise.
        unsafe { rb_sys::rb_thread_check_ints() };
        rb_sys::Qnil as rb_sys::VALUE
    })
    .map(|_| ())
}

pub fn dbl2big(_ruby: &Ruby, n: f64) -> Value {
    // SAFETY: rb_dbl2big allocates a Bignum and requires the GVL, which the
    // `&Ruby` argument proves this thread holds.
    let raw = unsafe { rb_sys::rb_dbl2big(n) };
    // SAFETY: `raw` is the live VALUE rb_dbl2big just returned.
    unsafe { Value::from_raw(raw) }
}

pub fn string_bytes(_ruby: &Ruby, s: RString) -> Vec<u8> {
    // SAFETY: the slice is copied before any Ruby code can run and move or free it.
    unsafe { s.as_slice() }.to_vec()
}
