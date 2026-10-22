use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::sync::OnceLock;

use magnus::gc::Marker;
use magnus::typed_data::Obj;
use magnus::value::{Lazy, Opaque, ReprValue};
use magnus::{Class, DataTypeFunctions, Error, Module, Object, RModule, Ruby, TypedData, Value};

use crate::rb_raw;

pub struct StaticRoot<T: ReprValue>(Lazy<T>);

impl<T: ReprValue> StaticRoot<T> {
    pub const fn new(init: fn(&Ruby) -> T) -> Self {
        Self(Lazy::new(init))
    }

    pub fn get(&self, ruby: &Ruby) -> T {
        ruby.get_inner(&self.0)
    }
}

/// A Ruby value held by a wrapped Rust object, kept alive and pinned by the
/// owner's mark function. Only touched under the GVL.
pub struct OwnedRoot<T: ReprValue>(Cell<Option<Opaque<T>>>);

impl<T: ReprValue> Default for OwnedRoot<T> {
    fn default() -> Self {
        Self(Cell::new(None))
    }
}

impl<T: ReprValue> OwnedRoot<T> {
    pub fn set(&self, _ruby: &Ruby, value: T) {
        self.0.set(Some(Opaque::from(value)));
    }

    pub fn get(&self, ruby: &Ruby) -> Option<T> {
        self.0.get().map(|value| ruby.get_inner(value))
    }

    pub fn take(&self, ruby: &Ruby) -> Option<T> {
        self.0.take().map(|value| ruby.get_inner(value))
    }

    pub fn mark(&self, marker: &Marker) {
        if let Some(value) = self.0.get() {
            marker.mark(value);
        }
    }
}

/// A Ruby value found after startup, such as a class defined by a later
/// require, kept alive for the life of the process once set.
pub struct OnceRoot<T: ReprValue>(OnceLock<Opaque<T>>);

impl<T: ReprValue> OnceRoot<T> {
    pub const fn new() -> Self {
        Self(OnceLock::new())
    }

    pub fn get(&self, ruby: &Ruby) -> Option<T> {
        self.0.get().map(|value| ruby.get_inner(*value))
    }

    pub fn set(&self, ruby: &Ruby, value: T) {
        if self.0.set(Opaque::from(value)).is_ok() {
            ruby.gc_register_mark_object(value);
        }
    }
}

/// Ruby values a Rust loop holds while it may call Ruby, which can collect and
/// compact. The holder's mark pins every value, so each stays alive at the
/// same address and can serve as an identity key. Only touched under the GVL,
/// and no borrow spans a Ruby call, so the mark always finds the list free.
#[derive(Default, TypedData)]
#[magnus(class = "MiniRacerRs::Pins", free_immediately, mark)]
pub struct Pins(RefCell<Vec<Opaque<Value>>>);

impl DataTypeFunctions for Pins {
    // A borrow at mark time would leave every value unpinned and free to move,
    // so it ends the process rather than corrupt the encode.
    fn mark(&self, marker: &Marker) {
        let Ok(values) = self.0.try_borrow() else {
            std::process::abort();
        };
        for value in values.iter() {
            marker.mark(*value);
        }
    }
}

impl Pins {
    /// Keep using the returned `Obj` while the values are needed; a reference
    /// into its data alone does not keep the holder alive.
    pub fn new(ruby: &Ruby) -> Obj<Self> {
        ruby.obj_wrap(Self::default())
    }

    pub fn push(&self, value: Value) -> usize {
        let mut values = self.0.borrow_mut();
        values.push(Opaque::from(value));
        values.len().saturating_sub(1)
    }

    pub fn get(&self, ruby: &Ruby, index: usize) -> Option<Value> {
        self.0
            .borrow()
            .get(index)
            .map(|value| ruby.get_inner(*value))
    }
}

/// Ruby values a decode keeps in order past what fits on the native stack.
/// The holder's mark pins every value. Only touched under the GVL; a borrow
/// that spans a Ruby call is a shared one, which the mark shares, and no
/// mutable borrow does.
#[derive(Default, TypedData)]
#[magnus(class = "MiniRacerRs::Spill", free_immediately, mark)]
pub struct Spill(RefCell<Vec<Opaque<Value>>>);

impl DataTypeFunctions for Spill {
    // As for Pins, a mutable borrow at mark time would leave values unmarked.
    fn mark(&self, marker: &Marker) {
        let Ok(values) = self.0.try_borrow() else {
            std::process::abort();
        };
        for value in values.iter() {
            marker.mark(*value);
        }
    }
}

impl Spill {
    /// Holds `values` first. Keep using the returned `Obj` while the values
    /// are needed; a reference into its data alone does not keep the holder
    /// alive.
    pub fn new(ruby: &Ruby, values: &[Value]) -> Obj<Self> {
        let held = values.iter().copied().map(Opaque::from).collect();
        ruby.obj_wrap(Self(RefCell::new(held)))
    }

    pub fn push(&self, value: Value) {
        self.0.borrow_mut().push(Opaque::from(value));
    }

    pub fn truncate(&self, len: usize) {
        self.0.borrow_mut().truncate(len);
    }

    pub fn get(&self, ruby: &Ruby, index: usize) -> Option<Value> {
        self.0
            .borrow()
            .get(index)
            .map(|value| ruby.get_inner(*value))
    }

    /// Runs `read` over the values in `range`; `read` may call Ruby.
    pub fn with_values<R>(
        &self,
        _ruby: &Ruby,
        range: Range<usize>,
        read: impl FnOnce(&[Value]) -> R,
    ) -> R {
        let values = self.0.borrow();
        let held = values.get(range).unwrap_or_default();
        // SAFETY: Opaque is a transparent wrapper, so a slice of them has the
        // layout of a slice of the values they wrap, which the `&Ruby`
        // argument proves this thread may use.
        let slice =
            unsafe { std::slice::from_raw_parts(held.as_ptr().cast::<Value>(), held.len()) };
        read(slice)
    }
}

// A hidden instance variable, which Ruby code cannot read or change.
const SPAWNED: &str = "spawned_with";

/// Starts a Ruby thread that runs `run` with `value`, which the thread object
/// keeps alive until the thread ends. `run` must handle whatever its Ruby
/// calls raise.
pub fn spawn<T: ReprValue + 'static>(
    ruby: &Ruby,
    value: T,
    run: impl FnOnce(&Ruby, T) + Send + 'static,
) -> Result<(), Error> {
    let held = Opaque::from(value);
    // The new thread waits for the GVL this one holds until the variable is
    // set, and `value` lives on this stack until then.
    let thread = rb_raw::thread_create(
        ruby,
        Box::new(move |ruby: &Ruby| run(ruby, ruby.get_inner(held))),
    )?;
    thread.ivar_set(SPAWNED, value.as_value())
}

// Each holder's class is looked up once, by a Ruby call whose failure panics;
// near the end of a fiber's stack Ruby fails it with SystemStackError, so it is
// looked up here instead, at load, with room to spare.
pub fn init(ruby: &Ruby, rs: RModule) -> Result<(), Error> {
    for name in ["Pins", "Spill"] {
        rs.define_class(name, ruby.class_object())?
            .undef_default_alloc_func();
    }
    Pins::class(ruby);
    Spill::class(ruby);
    Ok(())
}
