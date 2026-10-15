use magnus::Ruby;
use magnus::value::{Lazy, ReprValue};

pub struct StaticRoot<T: ReprValue>(Lazy<T>);

impl<T: ReprValue> StaticRoot<T> {
    pub const fn new(init: fn(&Ruby) -> T) -> Self {
        Self(Lazy::new(init))
    }

    pub fn get(&self, ruby: &Ruby) -> T {
        ruby.get_inner(&self.0)
    }
}
