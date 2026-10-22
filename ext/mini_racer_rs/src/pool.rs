use std::cell::{Cell, OnceCell};

use magnus::error::ErrorType;
use magnus::gc::Marker;
use magnus::scan_args::{get_kwargs, scan_args};
use magnus::typed_data::Obj;
use magnus::value::ReprValue;
use magnus::{
    Class, DataTypeFunctions, Error, Exception, ExceptionClass, Integer, KwArgs, Module, RClass,
    RHash, RModule, Ruby, TypedData, Value, kwargs,
};
use mini_racer_engine::process::fork;

use crate::context::{self, Context};
use crate::gc_root::{self, OnceRoot, OwnedRoot};
use crate::options::{self, OptionValue};
use crate::{errors, export, rb_raw};

const SHUT_DOWN: &str = "the context pool is shut down";
// How long a waiting checkout sleeps before it looks at the slots again
// unwoken: a checkout that ended in a break, a throw or Thread#kill leaves its
// context idle without waking anyone.
const RESCAN_SECONDS: f64 = 0.1;

static SHUTDOWN_ERROR: OnceRoot<ExceptionClass> = OnceRoot::new();

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Phase {
    // Waiting for a thread of the pool's own to dispose the context it holds,
    // if any, and build a new one.
    #[default]
    Pending,
    Building,
    Idle,
    InUse,
    // The last build raised; the next checkout raises that and starts another.
    Failed,
    Closed,
}

#[derive(Default)]
struct Slot {
    phase: Cell<Phase>,
    // The context while idle, in use or pending disposal; the exception while
    // failed.
    value: OwnedRoot<Value>,
    // The fiber that checked the context out, whose nested checkouts reuse it.
    holder: OwnedRoot<Value>,
    uses: Cell<u64>,
    spent: Cell<bool>,
}

/// `MiniRacerRs::Pool`: contexts built ahead, checked out one per fiber, and
/// replaced on threads of the pool's own. Only touched under the GVL.
#[derive(Default, TypedData)]
#[magnus(
    class = "MiniRacerRs::Pool",
    name = "mini_racer_rs/pool",
    free_immediately,
    mark
)]
pub struct Pool {
    slots: OnceCell<Box<[Slot]>>,
    max_uses: Cell<Option<u64>>,
    options: OwnedRoot<RHash>,
    setup: OwnedRoot<Value>,
    // A Thread::Queue that only wakes waiting checkouts, since Queue#pop
    // parks a fiber under a scheduler and the thread otherwise; the slots
    // say which contexts are free.
    wakeups: OwnedRoot<Value>,
    epoch: Cell<u64>,
    shut_down: Cell<bool>,
}

impl DataTypeFunctions for Pool {
    fn mark(&self, marker: &Marker) {
        self.options.mark(marker);
        self.setup.mark(marker);
        self.wakeups.mark(marker);
        for slot in self.slots.get().iter().flat_map(|slots| slots.iter()) {
            slot.value.mark(marker);
            slot.holder.mark(marker);
        }
    }
}

pub fn init(ruby: &Ruby, rs: RModule) -> Result<(), Error> {
    let class = rs.define_class("Pool", ruby.class_object())?;
    class.define_alloc_func::<Pool>();
    class.define_method("initialize", export!(method Pool::initialize, -1))?;
    class.define_method("with", export!(method Pool::with, 0))?;
    class.define_method("shutdown", export!(method Pool::shutdown, 0))?;
    let base: RClass = ruby
        .class_object()
        .const_get::<_, RModule>("MiniRacer")?
        .const_get("Error")?;
    let error = class.define_class("ShutdownError", base)?;
    if let Some(error) = ExceptionClass::from_value(error.as_value()) {
        SHUTDOWN_ERROR.set(ruby, error);
    }
    Ok(())
}

fn uninitialized(ruby: &Ruby) -> Error {
    errors::internal(ruby, "pool is not initialized")
}

fn shut_down(ruby: &Ruby) -> Error {
    SHUTDOWN_ERROR.get(ruby).map_or_else(
        || errors::internal(ruby, SHUT_DOWN),
        |class| Error::new(class, SHUT_DOWN),
    )
}

// After a captured break, throw or Thread#kill, Ruby resumes from what the
// execution context holds, which a Ruby call could overwrite.
fn jumped(error: &Error) -> bool {
    matches!(error.error_type(), ErrorType::Jump(_))
}

fn dispose(context: Value) -> Result<(), Error> {
    context.funcall::<_, _, Value>("dispose", ()).map(drop)
}

// As `Context.new` reads its integers, except that a Bignum is out of range
// rather than of the wrong type.
fn limit_value(ruby: &Ruby, value: Value) -> Result<OptionValue, Error> {
    Ok(match context::option_value(ruby, value) {
        OptionValue::Other(_) if Integer::from_value(value).is_some() => {
            if value.funcall::<_, _, bool>("negative?", ())? {
                OptionValue::Fixnum(i64::MIN)
            } else {
                OptionValue::Fixnum(i64::MAX)
            }
        }
        other => other,
    })
}

fn thread_queue(ruby: &Ruby) -> Result<Value, Error> {
    ruby.class_object()
        .const_get::<_, RClass>("Thread")?
        .const_get::<_, RClass>("Queue")?
        .new_instance(())
}

impl Pool {
    fn slots(&self, ruby: &Ruby) -> Result<&[Slot], Error> {
        self.slots
            .get()
            .map(|slots| &**slots)
            .ok_or_else(|| uninitialized(ruby))
    }

    fn slot(&self, index: usize) -> Option<&Slot> {
        self.slots.get().and_then(|slots| slots.get(index))
    }

    fn initialize(ruby: &Ruby, rb_self: Obj<Self>, args: &[Value]) -> Result<(), Error> {
        if rb_self.slots.get().is_some() {
            return Err(errors::internal(ruby, "pool is already initialized"));
        }
        let args = scan_args::<(), (), (), (), RHash, ()>(args)?;
        let keywords = ruby.hash_new();
        keywords.update(args.keywords)?;
        let keywords =
            get_kwargs::<_, (Value,), (Option<Value>,), RHash>(keywords, &["size"], &["max_uses"])?;
        let (size,) = keywords.required;
        let (max_uses,) = keywords.optional;
        let max_uses = match max_uses {
            Some(value) => limit_value(ruby, value)?,
            None => OptionValue::Nil,
        };
        let limits = options::pool(&limit_value(ruby, size)?, &max_uses)
            .map_err(|error| context::option_error(ruby, &error))?;
        let options = keywords.splat;
        options.freeze();
        rb_self.options.set(ruby, options);
        if ruby.block_given() {
            rb_self.setup.set(ruby, ruby.block_proc()?.as_value());
        }
        rb_self.max_uses.set(limits.max_uses);
        rb_self.epoch.set(fork::epoch());
        rb_self.wakeups.set(ruby, thread_queue(ruby)?);
        let slots = rb_self.slots.get_or_init(|| {
            (0..limits.size)
                .map(|_| Slot {
                    phase: Cell::new(Phase::Building),
                    ..Slot::default()
                })
                .collect()
        });
        for slot in slots {
            match rb_self.build(ruby) {
                Ok(context) => {
                    slot.value.set(ruby, context);
                    slot.phase.set(Phase::Idle);
                }
                Err(error) => {
                    rb_self.abandon(ruby, &error);
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    // A pool whose first contexts did not all build disposes those that did.
    fn abandon(&self, ruby: &Ruby, error: &Error) {
        self.shut_down.set(true);
        for slot in self.slots.get().iter().flat_map(|slots| slots.iter()) {
            slot.phase.set(Phase::Closed);
            if let Some(context) = slot.value.take(ruby)
                && !jumped(error)
            {
                let _ = dispose(context);
            }
        }
    }

    // The setup block runs for every context, replacements included.
    fn build(&self, ruby: &Ruby) -> Result<Value, Error> {
        let options = self.options.get(ruby).ok_or_else(|| uninitialized(ruby))?;
        let context: Value = Context::class(ruby).funcall("new", (KwArgs(options),))?;
        let Some(setup) = self.setup.get(ruby) else {
            return Ok(context);
        };
        match setup.funcall::<_, _, Value>("call", (context,)) {
            Ok(_) => Ok(context),
            Err(error) => {
                if !jumped(&error) {
                    let _ = dispose(context);
                }
                Err(error)
            }
        }
    }

    fn refuse_if_shut_down(&self, ruby: &Ruby) -> Result<(), Error> {
        if self.shut_down.get() {
            Err(shut_down(ruby))
        } else {
            Ok(())
        }
    }

    // A forked child forgets every context its parent built, disposing none,
    // since they belong to the parent's runners, and builds its own.
    fn reconcile(&self, ruby: &Ruby) -> Result<(), Error> {
        let epoch = fork::epoch();
        if self.epoch.replace(epoch) == epoch {
            return Ok(());
        }
        let phase = if self.shut_down.get() {
            Phase::Closed
        } else {
            Phase::Pending
        };
        for slot in self.slots.get().iter().flat_map(|slots| slots.iter()) {
            slot.value.take(ruby);
            slot.holder.take(ruby);
            slot.uses.set(0);
            slot.spent.set(false);
            slot.phase.set(phase);
        }
        self.wakeups.get(ruby).map_or(Ok(()), |wakeups| {
            wakeups.funcall::<_, _, Value>("clear", ()).map(drop)
        })
    }

    // A nested checkout on the fiber that holds a context gets that context,
    // so it cannot wait on itself, and a callback that reenters the pool runs
    // on the context already running it.
    fn with(ruby: &Ruby, rb_self: Obj<Self>) -> Result<Value, Error> {
        if !ruby.block_given() {
            return Err(Error::new(
                ruby.exception_local_jump_error(),
                "no block given (yield)",
            ));
        }
        let slots = rb_self.slots(ruby)?;
        rb_self.reconcile(ruby)?;
        rb_self.refuse_if_shut_down(ruby)?;
        let fiber = ruby.fiber_current().as_value();
        let held = slots.iter().find(|slot| {
            slot.phase.get() == Phase::InUse
                && slot
                    .holder
                    .get(ruby)
                    .is_some_and(|holder| rb_raw::identity(holder) == rb_raw::identity(fiber))
        });
        if let Some(slot) = held {
            return Self::nested(ruby, slot);
        }
        let (index, context) = rb_self.checkout(ruby, rb_self, fiber)?;
        let epoch = fork::epoch();
        let result = rb_raw::yield_ensuring(ruby, context, &mut |ruby, raised| {
            rb_self.checkin(ruby, index, epoch, raised);
        });
        // What the checkin left to do takes Ruby calls, which would overwrite
        // a break, throw or Thread#kill under way; the next checkout or a
        // waiting one's rescan does it then.
        match &result {
            Err(error) if jumped(error) => result,
            _ => rb_self.settle(ruby, rb_self).and(result),
        }
    }

    fn nested(ruby: &Ruby, slot: &Slot) -> Result<Value, Error> {
        let context = slot
            .value
            .get(ruby)
            .ok_or_else(|| errors::internal(ruby, "the checked-out context is missing"))?;
        let epoch = fork::epoch();
        rb_raw::yield_ensuring(ruby, context, &mut |ruby, raised| {
            if fork::epoch() == epoch
                && raised.is_some_and(|exception| errors::spends_context(ruby, exception))
            {
                slot.spent.set(true);
            }
        })
    }

    fn checkout(
        &self,
        ruby: &Ruby,
        rb_self: Obj<Self>,
        fiber: Value,
    ) -> Result<(usize, Value), Error> {
        loop {
            self.reconcile(ruby)?;
            self.refuse_if_shut_down(ruby)?;
            if let Some(taken) = self.take(ruby, rb_self, fiber)? {
                return Ok(taken);
            }
            let wakeups = self.wakeups.get(ruby).ok_or_else(|| uninitialized(ruby))?;
            // Shutdown closes the queue, whose pop then returns nil at once.
            let popped =
                wakeups.funcall::<_, _, Value>("pop", (kwargs!("timeout" => RESCAN_SECONDS),));
            if let Err(error) = popped {
                // Ruby may raise into this checkout after its pop took a
                // wakeup, which then passes to the next waiter.
                if !jumped(&error) {
                    self.settle(ruby, rb_self)?;
                }
                return Err(error);
            }
        }
    }

    // A failed build is raised to the next checkout, after another is started.
    fn take(
        &self,
        ruby: &Ruby,
        rb_self: Obj<Self>,
        fiber: Value,
    ) -> Result<Option<(usize, Value)>, Error> {
        let slots = self.slots(ruby)?;
        let mut idle = None;
        for (index, slot) in slots.iter().enumerate() {
            match slot.phase.get() {
                Phase::Pending => self.start(ruby, rb_self, index),
                Phase::Failed => {
                    let raised = slot.value.take(ruby).and_then(Exception::from_value);
                    self.start(ruby, rb_self, index);
                    return Err(raised.map_or_else(
                        || errors::internal(ruby, "a pool context failed to build"),
                        Error::from,
                    ));
                }
                Phase::Idle if idle.is_none() => idle = Some((index, slot)),
                _ => {}
            }
        }
        let Some((index, slot)) = idle else {
            return Ok(None);
        };
        let Some(context) = slot.value.get(ruby) else {
            self.start(ruby, rb_self, index);
            return Ok(None);
        };
        slot.phase.set(Phase::InUse);
        slot.holder.set(ruby, fiber);
        slot.uses.set(slot.uses.get().saturating_add(1));
        slot.spent.set(false);
        Ok(Some((index, context)))
    }

    // Runs inside the ensure of the checkout's block, where a Ruby method
    // call could take an interrupt that nothing would then raise, so it only
    // marks the slot; `settle` does the rest.
    fn checkin(&self, ruby: &Ruby, index: usize, epoch: u64, raised: Option<Exception>) {
        // A forked child forgets what its parent checked out.
        if fork::epoch() != epoch {
            return;
        }
        let Some(slot) = self.slot(index) else {
            return;
        };
        if slot.phase.get() != Phase::InUse {
            return;
        }
        slot.holder.take(ruby);
        let spent = slot.spent.get()
            || raised.is_some_and(|exception| errors::spends_context(ruby, exception))
            || slot
                .value
                .get(ruby)
                .is_none_or(|value| context::is_disposed(ruby, value))
            || self
                .max_uses
                .get()
                .is_some_and(|max| slot.uses.get() >= max);
        slot.phase.set(if spent || self.shut_down.get() {
            Phase::Pending
        } else {
            Phase::Idle
        });
    }

    // Starts a thread for each pending slot, or after shutdown disposes its
    // context here, then wakes a waiting checkout if one can take a slot.
    fn settle(&self, ruby: &Ruby, rb_self: Obj<Self>) -> Result<(), Error> {
        self.reconcile(ruby)?;
        let slots = self.slots(ruby)?;
        let mut failed: Option<Error> = None;
        for (index, slot) in slots.iter().enumerate() {
            if slot.phase.get() != Phase::Pending {
                continue;
            }
            if !self.shut_down.get() {
                self.start(ruby, rb_self, index);
            } else if !failed.as_ref().is_some_and(jumped) {
                slot.phase.set(Phase::Closed);
                if let Some(context) = slot.value.take(ruby)
                    && let Err(error) = dispose(context)
                {
                    failed.get_or_insert(error);
                }
            }
        }
        if let Some(error) = failed {
            return Err(error);
        }
        if self.shut_down.get()
            || !slots
                .iter()
                .any(|slot| matches!(slot.phase.get(), Phase::Idle | Phase::Failed))
        {
            return Ok(());
        }
        let wakeups = self.wakeups.get(ruby).ok_or_else(|| uninitialized(ruby))?;
        let waiting = wakeups.funcall::<_, _, usize>("num_waiting", ())?;
        let woken = wakeups.funcall::<_, _, usize>("size", ())?;
        if waiting > woken {
            wakeups.funcall::<_, _, Value>("push", (true,))?;
        }
        Ok(())
    }

    // Replacements are built on a thread of the pool's own, off the request
    // path: it waits on the runners with the GVL released, and a fork leaves
    // it behind, so a child rebuilds whatever its parent's threads were
    // building.
    fn start(&self, ruby: &Ruby, rb_self: Obj<Self>, index: usize) {
        let Some(slot) = self.slot(index) else {
            return;
        };
        slot.phase.set(Phase::Building);
        let spawned = gc_root::spawn(ruby, rb_self, move |ruby, pool: Obj<Self>| {
            pool.replace(ruby, pool, index);
        });
        if let Err(error) = spawned {
            slot.value.take(ruby);
            match error.error_type() {
                ErrorType::Exception(exception) => {
                    slot.value.set(ruby, exception.as_value());
                    slot.phase.set(Phase::Failed);
                }
                _ => slot.phase.set(Phase::Pending),
            }
        }
    }

    // On the pool's thread: dispose what the slot retired, then build its
    // replacement unless the pool has shut down meanwhile. A thread killed
    // midway leaves the slot pending for the next checkout.
    fn replace(&self, ruby: &Ruby, rb_self: Obj<Self>, index: usize) {
        let Some(slot) = self.slot(index) else {
            return;
        };
        if let Some(retired) = slot.value.take(ruby)
            && dispose(retired).is_err_and(|error| jumped(&error))
        {
            slot.phase.set(Phase::Pending);
            return;
        }
        if self.shut_down.get() {
            slot.phase.set(Phase::Closed);
            return;
        }
        let built = self.build(ruby);
        if self.shut_down.get() {
            slot.phase.set(Phase::Closed);
            if let Ok(context) = built {
                let _ = dispose(context);
            }
            return;
        }
        match built {
            Ok(context) => {
                slot.value.set(ruby, context);
                slot.uses.set(0);
                slot.phase.set(Phase::Idle);
            }
            Err(error) => {
                if let ErrorType::Exception(exception) = error.error_type() {
                    slot.value.set(ruby, exception.as_value());
                    slot.phase.set(Phase::Failed);
                } else {
                    slot.phase.set(Phase::Pending);
                    return;
                }
            }
        }
        let _ = self.settle(ruby, rb_self);
    }

    // Idle contexts, and retired ones not yet disposed, are disposed now;
    // those in use when their checkout ends, and those being built once built.
    fn shutdown(ruby: &Ruby, rb_self: Obj<Self>) -> Result<(), Error> {
        let slots = rb_self.slots(ruby)?;
        rb_self.reconcile(ruby)?;
        // Called again, it finishes disposing what an interrupt stopped.
        if !rb_self.shut_down.replace(true) {
            for slot in slots {
                match slot.phase.get() {
                    Phase::Idle => slot.phase.set(Phase::Pending),
                    Phase::Failed => {
                        slot.value.take(ruby);
                        slot.phase.set(Phase::Closed);
                    }
                    Phase::Pending | Phase::Building | Phase::InUse | Phase::Closed => {}
                }
            }
            if let Some(wakeups) = rb_self.wakeups.get(ruby) {
                wakeups.funcall::<_, _, Value>("close", ())?;
            }
        }
        rb_self.settle(ruby, rb_self)
    }
}
