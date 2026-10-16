use std::ffi::{c_int, c_void};

use magnus::rb_sys::{AsRawId, AsRawValue, FromRawValue, protect};
use magnus::value::{IntoId, LazyId, ReprValue};
use magnus::{Error, RArray, RString, Ruby, Value};

static CALL: LazyId = LazyId::new("call");

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

// Mutex#lock releases the GVL while it waits and runs interrupts; magnus wraps
// it in protect, so a Thread#kill comes back as the Err (I6).
pub fn mutex_lock(_ruby: &Ruby, mutex: magnus::Mutex) -> Result<(), Error> {
    mutex.lock()
}

// Ruby's GC cannot see an isolate's native memory, so it is reported here;
// without it, contexts left to finalizers pile up between collections.
pub fn adjust_memory(_ruby: &Ruby, bytes: isize) {
    // SAFETY: the `&Ruby` argument proves this thread holds the GVL.
    unsafe { rb_sys::rb_gc_adjust_memory_usage(rb_sys::ssize_t::try_from(bytes).unwrap_or(0)) };
}

/// For a free function, which runs with the GVL held but no `Ruby` handle.
/// Only decreases are allowed there, and they never start a collection.
pub fn release_memory(bytes: usize) {
    let diff = rb_sys::ssize_t::try_from(bytes).map_or(0, |n| -n);
    // SAFETY: free functions run during GC with the GVL held, and a negative
    // adjustment only lowers the malloc counter without triggering GC.
    unsafe { rb_sys::rb_gc_adjust_memory_usage(diff) };
}

pub fn dbl2big(_ruby: &Ruby, n: f64) -> Value {
    // SAFETY: rb_dbl2big allocates a Bignum and requires the GVL, which the
    // `&Ruby` argument proves this thread holds.
    let raw = unsafe { rb_sys::rb_dbl2big(n) };
    // SAFETY: `raw` is the live VALUE rb_dbl2big just returned.
    unsafe { Value::from_raw(raw) }
}

// mini_racer checks its String arguments with Check_Type, whose TypeError
// names the argument's type as Ruby itself does.
pub fn expect_string(ruby: &Ruby, value: Value) -> Result<RString, Error> {
    let raw = value.as_raw();
    protect(|| {
        // SAFETY: the `&Ruby` argument proves this thread holds the GVL, and
        // `protect` catches the TypeError rb_check_type raises.
        unsafe { rb_sys::rb_check_type(raw, rb_sys::ruby_value_type::RUBY_T_STRING as c_int) };
        raw
    })?;
    RString::from_value(value)
        .ok_or_else(|| Error::new(ruby.exception_type_error(), "expected String"))
}

/// `digits` is a `BigInt` magnitude as little-endian 64-bit words.
pub fn integer_unpack(_ruby: &Ruby, digits: &[u8], negative: bool) -> Result<Value, Error> {
    let words = (digits.len() / 8).try_into().unwrap_or_default();
    let sign = if negative {
        rb_sys::INTEGER_PACK_NEGATIVE
    } else {
        0
    };
    let flags = c_int::try_from(rb_sys::INTEGER_PACK_LITTLE_ENDIAN | sign).unwrap_or(0);
    let raw = protect(|| {
        // SAFETY: `digits` holds `words` whole 8-byte words and outlives the
        // call, which copies them; `protect` catches an allocation failure.
        unsafe { rb_sys::rb_integer_unpack(digits.as_ptr().cast(), words, 8, 0, flags) }
    })?;
    // SAFETY: `raw` is the live Integer rb_integer_unpack just returned.
    Ok(unsafe { Value::from_raw(raw) })
}

/// The builtin types `mini_racer`'s encoder tells apart; `Other` carries the
/// number `TYPE()` gives, which it prints in hex.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RubyType {
    Object,
    Float,
    String,
    Array,
    Hash,
    Bignum,
    Data,
    Nil,
    True,
    False,
    Symbol,
    Fixnum,
    Other(u32),
}

pub fn ruby_type(_ruby: &Ruby, value: Value) -> RubyType {
    use rb_sys::ruby_value_type as T;
    // SAFETY: the `&Ruby` argument proves this thread holds the GVL, and
    // `value` is a live VALUE the caller holds.
    let kind = unsafe { rb_sys::RB_TYPE(value.as_raw()) };
    match kind {
        T::RUBY_T_OBJECT => RubyType::Object,
        T::RUBY_T_FLOAT => RubyType::Float,
        T::RUBY_T_STRING => RubyType::String,
        T::RUBY_T_ARRAY => RubyType::Array,
        T::RUBY_T_HASH => RubyType::Hash,
        T::RUBY_T_BIGNUM => RubyType::Bignum,
        T::RUBY_T_DATA => RubyType::Data,
        T::RUBY_T_NIL => RubyType::Nil,
        T::RUBY_T_TRUE => RubyType::True,
        T::RUBY_T_FALSE => RubyType::False,
        T::RUBY_T_SYMBOL => RubyType::Symbol,
        T::RUBY_T_FIXNUM => RubyType::Fixnum,
        other => RubyType::Other(other as u32),
    }
}

/// An object's address, the identity `mini_racer`'s encoder keys repeated
/// containers by; stable only while the object is pinned.
pub fn identity(value: Value) -> u64 {
    value.as_raw()
}

/// `receiver.call(*args)`, with Ruby reading the arguments from `args`
/// itself, so they stay where the GC sees them for the whole call.
pub fn call_with_array(ruby: &Ruby, receiver: Value, args: RArray) -> Result<Value, Error> {
    let call = (*CALL).into_id_with(ruby);
    let raw = protect(|| {
        // SAFETY: the `&Ruby` argument proves this thread holds the GVL, both
        // values are live on this stack, and `protect` catches any non-local
        // exit the call raises.
        unsafe { rb_sys::rb_apply(receiver.as_raw(), call.as_raw(), args.as_raw()) }
    })?;
    // SAFETY: `raw` is the live value the call returned.
    Ok(unsafe { Value::from_raw(raw) })
}

/// How Ruby's own type errors name `value`: nil, true and false by value,
/// anything else by its class, never a singleton class.
pub fn type_name(ruby: &Ruby, value: Value) -> String {
    if value.is_nil() {
        return "nil".to_owned();
    }
    if value.is_kind_of(ruby.class_true_class()) {
        return "true".to_owned();
    }
    if value.is_kind_of(ruby.class_false_class()) {
        return "false".to_owned();
    }
    // SAFETY: the `&Ruby` argument proves this thread holds the GVL, and
    // rb_obj_class only reads the value's class.
    let raw = unsafe { rb_sys::rb_obj_class(value.as_raw()) };
    // SAFETY: `raw` is the live class rb_obj_class returned.
    let class = unsafe { Value::from_raw(raw) };
    class
        .funcall::<_, _, String>("to_s", ())
        .unwrap_or_else(|_| class_name(ruby, value))
}

/// The struct name a typed data object was wrapped under, which Ruby's
/// typed data check names it by.
pub fn typed_data_name(ruby: &Ruby, value: Value) -> Option<String> {
    if !matches!(ruby_type(ruby, value), RubyType::Data) {
        return None;
    }
    // SAFETY: `value` is a live T_DATA object and the GVL is held.
    if !unsafe { rb_sys::RTYPEDDATA_P(value.as_raw()) } {
        return None;
    }
    // SAFETY: as above; the type of a typed data object is static data.
    let data_type = unsafe { rb_sys::RTYPEDDATA_TYPE(value.as_raw()) };
    // SAFETY: `data_type` points at the object's static rb_data_type_t.
    let name = unsafe { (*data_type).wrap_struct_name };
    if name.is_null() {
        return None;
    }
    // SAFETY: wrap_struct_name is a NUL-terminated static string.
    Some(
        unsafe { std::ffi::CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned(),
    )
}

/// `rb_str_dup`: a copy of the same class, without `String#dup`'s instance
/// variables.
pub fn str_dup(_ruby: &Ruby, s: RString) -> RString {
    // SAFETY: the `&Ruby` argument proves this thread holds the GVL; rb_str_dup
    // returns a new live String.
    let raw = unsafe { rb_sys::rb_str_dup(s.as_raw()) };
    // SAFETY: `raw` is the String rb_str_dup returned.
    RString::from_value(unsafe { Value::from_raw(raw) }).unwrap_or(s)
}

/// A Symbol's name as a String, without calling `Symbol#to_s`.
pub fn sym2str(ruby: &Ruby, symbol: Value) -> RString {
    // SAFETY: the `&Ruby` argument proves this thread holds the GVL, and the
    // caller passes a Symbol, for which rb_sym2str returns its interned name.
    let raw = unsafe { rb_sys::rb_sym2str(symbol.as_raw()) };
    // SAFETY: `raw` is the live String rb_sym2str returned.
    let name = unsafe { Value::from_raw(raw) };
    RString::from_value(name).unwrap_or_else(|| ruby.str_new(""))
}

/// Runs `read` over the string's bytes without copying them; `read` must not
/// call Ruby, which could change or move the string.
pub fn with_string_bytes<R>(_ruby: &Ruby, s: RString, read: impl FnOnce(&[u8]) -> R) -> R {
    // SAFETY: the bytes are only read by `read`, which runs no Ruby code.
    read(unsafe { s.as_slice() })
}

/// The magnitude of an Integer as little-endian 64-bit words, and whether it is
/// negative; `None` past `max_words`.
pub fn integer_words(
    _ruby: &Ruby,
    value: Value,
    max_words: usize,
) -> Result<Option<(bool, Vec<u8>)>, Error> {
    let raw = value.as_raw();
    let mut count = 0;
    protect(|| {
        // SAFETY: the `&Ruby` argument proves this thread holds the GVL, and
        // `protect` catches what rb_absint_numwords raises for a non-Integer.
        count = unsafe { rb_sys::rb_absint_numwords(raw, 64, std::ptr::null_mut()) };
        rb_sys::Qnil as rb_sys::VALUE
    })?;
    let Some(words) = usize::try_from(count)
        .ok()
        .filter(|&words| words <= max_words)
    else {
        return Ok(None);
    };
    let mut digits = vec![0u64; words];
    let flags = c_int::try_from(rb_sys::INTEGER_PACK_LITTLE_ENDIAN).unwrap_or(0);
    let mut sign = 0;
    protect(|| {
        // SAFETY: `digits` holds `words` words, which rb_integer_pack fills.
        sign =
            unsafe { rb_sys::rb_integer_pack(raw, digits.as_mut_ptr().cast(), count, 8, 0, flags) };
        rb_sys::Qnil as rb_sys::VALUE
    })?;
    if !(-1..=1).contains(&sign) {
        return Ok(None);
    }
    Ok(Some((
        sign < 0,
        digits.iter().flat_map(|word| word.to_le_bytes()).collect(),
    )))
}

/// The name `rb_class2name` gives the class of `value`, as `mini_racer` prints it.
pub fn class_name(_ruby: &Ruby, value: Value) -> String {
    use magnus::Class as _;
    use magnus::value::ReprValue as _;
    let class = value.class();
    // SAFETY: the name is copied before any Ruby code can run and free it.
    unsafe { class.name() }.into_owned()
}

pub fn string_bytes(_ruby: &Ruby, s: RString) -> Vec<u8> {
    // SAFETY: the slice is copied before any Ruby code can run and move or free it.
    unsafe { s.as_slice() }.to_vec()
}
