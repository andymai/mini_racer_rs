use std::cell::{Cell, RefCell};
use std::sync::OnceLock;

use magnus::gc::Marker;
use magnus::typed_data::Obj;
use magnus::value::{Lazy, Opaque, ReprValue};
use magnus::{Class, DataTypeFunctions, Error, Module, RModule, Ruby, TypedData, Value};

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

pub fn init(ruby: &Ruby, rs: RModule) -> Result<(), Error> {
    let class = rs.define_class("Pins", ruby.class_object())?;
    class.undef_default_alloc_func();
    Ok(())
}
