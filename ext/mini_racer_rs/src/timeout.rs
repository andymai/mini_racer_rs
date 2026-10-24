use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use magnus::value::{IntoId, LazyId, ReprValue, StaticSymbol};
use magnus::{
    Class, DataTypeFunctions, Error, Module, Object, RClass, RModule, Ruby, TryConvert, TypedData,
    Value,
};

use crate::context::{option_error, option_value};
use crate::gc_root::StaticRoot;
use crate::{export, options, rb_raw};

// Scopes open in this process. While there are none, a dispatch reads no
// fiber storage, so code that never opens one pays only this load.
static OPEN: AtomicUsize = AtomicUsize::new(0);

static KEY: LazyId = LazyId::new("mini_racer_rs_timeout");
static AREF: LazyId = LazyId::new("[]");
static ASET: LazyId = LazyId::new("[]=");

#[expect(
    clippy::expect_used,
    reason = "Ruby defines Fiber, and a root initializer cannot return an error"
)]
static FIBER: StaticRoot<RClass> = StaticRoot::new(|ruby| {
    ruby.class_object()
        .const_get("Fiber")
        .expect("Ruby defines Fiber")
});

struct Scope {
    budget: Duration,
    open: AtomicBool,
    // The scope open on this fiber when this one opened. A fiber started
    // inside a scope inherits it through fiber storage and may outlive it,
    // so a scope it opens can outlast the one it encloses.
    enclosing: Option<Arc<Self>>,
}

impl Scope {
    fn budget(&self) -> Option<Duration> {
        let mut budget: Option<Duration> = None;
        let mut scope = Some(self);
        while let Some(current) = scope {
            if current.open.load(Ordering::Relaxed) {
                budget = Some(budget.map_or(current.budget, |b| b.min(current.budget)));
            }
            scope = current.enclosing.as_deref();
        }
        budget
    }

    fn close(&self) {
        if self.open.swap(false, Ordering::Relaxed) {
            OPEN.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

#[derive(TypedData)]
#[magnus(class = "MiniRacerRs::TimeoutScope", free_immediately, size)]
struct TimeoutScope(Arc<Scope>);

impl DataTypeFunctions for TimeoutScope {}

impl Drop for TimeoutScope {
    // A fiber collected while suspended inside the block never returns to
    // close its scope.
    fn drop(&mut self) {
        self.0.close();
    }
}

pub fn init(ruby: &Ruby, rs: RModule) -> Result<(), Error> {
    rs.define_class("TimeoutScope", ruby.class_object())?
        .undef_default_alloc_func();
    TimeoutScope::class(ruby);
    FIBER.get(ruby);
    rs.define_singleton_method("timeout", export!(function timeout, 1))?;
    Ok(())
}

/// The timeout the scopes open on the current fiber give a script it starts.
pub fn current(ruby: &Ruby) -> Result<Option<Duration>, Error> {
    if OPEN.load(Ordering::Relaxed) == 0 {
        return Ok(None);
    }
    let stored: Value = FIBER.get(ruby).funcall(*AREF, (*KEY,))?;
    Ok(<&TimeoutScope>::try_convert(stored)
        .ok()
        .and_then(|scope| scope.0.budget()))
}

/// Takes the scope out of the current fiber's storage, for a thread the
/// extension starts, which serves no request and starts with a copy of its
/// creator's storage. Ruby removes it before it checks for interrupts as the
/// call returns, so an interrupt raised then leaves the fiber outside it too.
pub fn leave_inherited(ruby: &Ruby) -> Result<(), Error> {
    if OPEN.load(Ordering::Relaxed) == 0 {
        return Ok(());
    }
    FIBER
        .get(ruby)
        .funcall::<_, _, Value>(*ASET, (*KEY, ruby.qnil()))
        .map(drop)
}

// The scope lives in fiber storage, which fibers and threads started inside
// the block inherit; a copy that outlives the block finds the scope closed.
fn timeout(ruby: &Ruby, milliseconds: Value) -> Result<Value, Error> {
    let budget = options::timeout(&option_value(ruby, milliseconds))
        .map_err(|error| option_error(ruby, &error))?;
    if !ruby.block_given() {
        return Err(Error::new(
            ruby.exception_local_jump_error(),
            "no block given (yield)",
        ));
    }
    let Some(budget) = budget else {
        return ruby.yield_values(());
    };
    let fiber = FIBER.get(ruby);
    let previous: Value = fiber.funcall(*AREF, (*KEY,))?;
    let enclosing = <&TimeoutScope>::try_convert(previous)
        .ok()
        .map(|scope| Arc::clone(&scope.0));
    let scope = Arc::new(Scope {
        budget,
        open: AtomicBool::new(true),
        enclosing,
    });
    let stored = ruby.obj_wrap(TimeoutScope(Arc::clone(&scope)));
    OPEN.fetch_add(1, Ordering::Relaxed);
    let key = StaticSymbol::from((*KEY).into_id_with(ruby)).as_value();
    let result = rb_raw::yield_setting(
        ruby,
        fiber.as_value(),
        *ASET,
        key,
        (stored.as_value(), previous),
    );
    scope.close();
    result
}
