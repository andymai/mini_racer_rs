use std::ffi::c_void;

use magnus::rb_sys::FromRawValue;
use magnus::{RString, Ruby, Value};

use crate::guard::{Panicked, guard};

struct Call<F, R> {
    work: Option<F>,
    result: Option<Result<R, Panicked>>,
}

unsafe extern "C" fn call_without_gvl<F, R>(data: *mut c_void) -> *mut c_void
where
    F: FnOnce() -> R,
{
    // SAFETY: `data` is the `Call` that `without_gvl` keeps alive on its stack for
    // the whole synchronous rb_thread_call_without_gvl call.
    let call = unsafe { &mut *data.cast::<Call<F, R>>() };
    if let Some(work) = call.work.take() {
        call.result = Some(guard(work));
    }
    std::ptr::null_mut()
}

pub fn without_gvl<F, R>(_ruby: &Ruby, work: F) -> Result<R, Panicked>
where
    F: FnOnce() -> R + Send,
    R: Send,
{
    let mut call = Call {
        work: Some(work),
        result: None,
    };
    let data = std::ptr::addr_of_mut!(call).cast::<c_void>();
    // SAFETY: rb_thread_call_without_gvl runs the callback synchronously on this
    // thread before returning, so `call` outlives every access through `data`.
    unsafe {
        rb_sys::rb_thread_call_without_gvl(
            Some(call_without_gvl::<F, R>),
            data,
            None,
            std::ptr::null_mut(),
        );
    }
    call.result.unwrap_or(Err(Panicked))
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
