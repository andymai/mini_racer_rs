use std::cell::Cell;

use magnus::Ruby;
use magnus::gc::Marker;
use magnus::value::{Lazy, Opaque, ReprValue};

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

    pub fn mark(&self, marker: &Marker) {
        if let Some(value) = self.0.get() {
            marker.mark(value);
        }
    }
}
