use std::cell::{Cell, OnceCell, RefCell};
use std::collections::VecDeque;

use magnus::error::ErrorType;
use magnus::gc::Marker;
use magnus::scan_args::{get_kwargs, scan_args};
use magnus::typed_data::Obj;
use magnus::value::ReprValue;
use magnus::{
    Class, DataTypeFunctions, Error, Exception, ExceptionClass, Integer, KwArgs, Module, RArray,
    RClass, RHash, RModule, Ruby, TypedData, Value, kwargs,
};
use mini_racer_engine::process::fork;

use crate::context::{self, Context};
use crate::gc_root::{self, OnceRoot, OwnedRoot};
use crate::options::{self, OptionValue};
use crate::{errors, export, rb_raw};

const SHUT_DOWN: &str = "the context pool is shut down";
// How long a waiting checkout sleeps before it looks at the slots again
// unwoken: a checkout that ended in a break, a throw or Thread#kill leaves its
// context idle without handing it on.
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
    // Handed to the waiting checkout whose queue `holder` holds.
    Reserved,
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
    // The fiber that checked the context out, whose nested checkouts reuse
    // it, or the queue of the waiting checkout it is reserved for.
    holder: OwnedRoot<Value>,
    // The fiber of the waiting checkout it is reserved for.
    waiter: OwnedRoot<Value>,
    uses: Cell<u64>,
    spent: Cell<bool>,
}

#[derive(Default)]
struct Waiter {
    queue: OwnedRoot<Value>,
    fiber: OwnedRoot<Value>,
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
    // Waiting checkouts in arrival order, each by a Thread::Queue of its own
    // that it pops, which parks a fiber under a scheduler and the thread
    // otherwise. A freed context goes to the first of them, and a checkout
    // that finds any waiting waits behind them, so none is passed over.
    waiting: RefCell<VecDeque<Waiter>>,
    // Queues of waits a trap handler's checkout displaced, owed a wakeup.
    owed: RefCell<Vec<OwnedRoot<Value>>>,
    epoch: Cell<u64>,
    shut_down: Cell<bool>,
}

impl DataTypeFunctions for Pool {
    fn mark(&self, marker: &Marker) {
        self.options.mark(marker);
        self.setup.mark(marker);
        // As for gc_root::Pins: a borrow at mark time would leave a queue
        // unmarked, and no borrow spans a Ruby call.
        let Ok(waiting) = self.waiting.try_borrow() else {
            std::process::abort();
        };
        for waiter in waiting.iter() {
            waiter.queue.mark(marker);
            waiter.fiber.mark(marker);
        }
        let Ok(owed) = self.owed.try_borrow() else {
            std::process::abort();
        };
        for queue in owed.iter() {
            queue.mark(marker);
        }
        for slot in self.slots.get().iter().flat_map(|slots| slots.iter()) {
            slot.value.mark(marker);
            slot.holder.mark(marker);
            slot.waiter.mark(marker);
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
    // since they belong to the parent's runners, and builds its own; the
    // checkouts that waited there are gone with the parent's other threads.
    fn reconcile(&self, ruby: &Ruby) {
        let epoch = fork::epoch();
        if self.epoch.replace(epoch) == epoch {
            return;
        }
        let phase = if self.shut_down.get() {
            Phase::Closed
        } else {
            Phase::Pending
        };
        for slot in self.slots.get().iter().flat_map(|slots| slots.iter()) {
            slot.value.take(ruby);
            slot.holder.take(ruby);
            slot.waiter.take(ruby);
            slot.uses.set(0);
            slot.spent.set(false);
            slot.phase.set(phase);
        }
        self.waiting.borrow_mut().clear();
        self.owed.borrow_mut().clear();
    }

    // The first waiting checkout's queue and fiber.
    fn first_waiting(&self, ruby: &Ruby) -> Option<(Value, Value)> {
        let waiting = self.waiting.borrow();
        let first = waiting.front()?;
        Some((first.queue.get(ruby)?, first.fiber.get(ruby)?))
    }

    fn enlist(&self, ruby: &Ruby, queue: Value, fiber: Value) {
        let waiter = Waiter::default();
        waiter.queue.set(ruby, queue);
        waiter.fiber.set(ruby, fiber);
        self.waiting.borrow_mut().push_back(waiter);
    }

    fn delist(&self, ruby: &Ruby, queue: Value) -> bool {
        let mut waiting = self.waiting.borrow_mut();
        let found = waiting.iter().position(|waiter| {
            waiter
                .queue
                .get(ruby)
                .is_some_and(|listed| same(listed, queue))
        });
        found.and_then(|index| waiting.remove(index)).is_some()
    }

    fn replace_waiting(&self, ruby: &Ruby, outer: Value, queue: Value) {
        let waiting = self.waiting.borrow();
        let listed = waiting.iter().find(|waiter| {
            waiter
                .queue
                .get(ruby)
                .is_some_and(|listed| same(listed, outer))
        });
        if let Some(waiter) = listed {
            waiter.queue.set(ruby, queue);
        }
    }

    // The queue of a wait `fiber` already has, in line or with a context
    // reserved for it.
    fn place_of(&self, ruby: &Ruby, fiber: Value) -> Option<Value> {
        let listed = self.waiting.borrow().iter().find_map(|waiter| {
            waiter
                .fiber
                .get(ruby)
                .is_some_and(|listed| same(listed, fiber))
                .then(|| waiter.queue.get(ruby))
                .flatten()
        });
        listed.or_else(|| {
            self.slots.get()?.iter().find_map(|slot| {
                (slot.phase.get() == Phase::Reserved
                    && slot
                        .waiter
                        .get(ruby)
                        .is_some_and(|waiter| same(waiter, fiber)))
                .then(|| slot.holder.get(ruby))
                .flatten()
            })
        })
    }

    fn reserved_for(&self, ruby: &Ruby, queue: Value) -> Option<(usize, &Slot)> {
        self.slots.get()?.iter().enumerate().find(|(_, slot)| {
            slot.phase.get() == Phase::Reserved
                && slot
                    .holder
                    .get(ruby)
                    .is_some_and(|holder| same(holder, queue))
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
        rb_self.reconcile(ruby);
        rb_self.refuse_if_shut_down(ruby)?;
        let fiber = ruby.fiber_current().as_value();
        let held = slots.iter().find(|slot| {
            slot.phase.get() == Phase::InUse
                && slot
                    .holder
                    .get(ruby)
                    .is_some_and(|holder| same(holder, fiber))
        });
        if let Some(slot) = held {
            return Self::nested(ruby, slot);
        }
        let (index, context, displaced) = rb_self.checkout(ruby, rb_self, fiber)?;
        let epoch = fork::epoch();
        let result = rb_raw::yield_ensuring(ruby, context, &mut |ruby, raised| {
            rb_self.checkin(ruby, index, epoch, raised);
        });
        rb_self.owe(ruby, displaced);
        // What the checkin left to do takes Ruby calls, which would overwrite
        // a break, throw or Thread#kill under way; the next settle, on any
        // thread, or a waiting checkout's rescan does it then.
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

    // Once a slot is marked in use, nothing calls a Ruby method before the
    // block's ensure is in place, so no interrupt can strand the slot.
    // Also the queue of a wait this checkout displaced, which its caller
    // wakes once the checkout is over.
    fn checkout(
        &self,
        ruby: &Ruby,
        rb_self: Obj<Self>,
        fiber: Value,
    ) -> Result<(usize, Value, Option<Value>), Error> {
        self.start_pending(ruby, rb_self)?;
        if self.waiting.borrow().is_empty() {
            let slots = self.slots(ruby)?;
            if let Some(index) = slots
                .iter()
                .position(|slot| slot.phase.get() == Phase::Failed)
            {
                return Err(self.failed(ruby, rb_self, index));
            }
            if let Some(index) = slots
                .iter()
                .position(|slot| slot.phase.get() == Phase::Idle)
            {
                let (index, context) = self.take(ruby, index, fiber)?;
                return Ok((index, context, None));
            }
        }
        // A trap handler or a finalizer that checks out while its own fiber
        // waits here takes that wait's place in line, and any context
        // reserved for it: the wait cannot go on until the handler returns,
        // and then finds its place gone and takes a new one. The handler
        // waits on a queue of its own, so the wait's stays as it slept on it.
        let displaced = self.place_of(ruby, fiber);
        if let Some((index, _)) = displaced.and_then(|outer| self.reserved_for(ruby, outer)) {
            let (index, context) = self.take(ruby, index, fiber)?;
            return Ok((index, context, displaced));
        }
        let queue = thread_queue(ruby)?;
        match displaced {
            Some(outer) => self.replace_waiting(ruby, outer, queue),
            None => self.enlist(ruby, queue, fiber),
        }
        let waited = self.wait_in_turn(ruby, rb_self, fiber, queue);
        // A context handed to this checkout that it did not take goes to the
        // next in line, unless a break, throw or Thread#kill is under way,
        // when the others' rescans find it.
        let handed_back = self.leave(ruby, queue);
        if waited.is_err() {
            self.owe(ruby, displaced);
        }
        match waited {
            Err(error) if jumped(&error) => Err(error),
            Err(error) if handed_back || displaced.is_some() => {
                Err(self.settle(ruby, rb_self).err().unwrap_or(error))
            }
            waited => waited.map(|(index, context)| (index, context, displaced)),
        }
    }

    fn owe(&self, ruby: &Ruby, displaced: Option<Value>) {
        if let Some(queue) = displaced {
            let root = OwnedRoot::default();
            root.set(ruby, queue);
            self.owed.borrow_mut().push(root);
        }
    }

    // A displaced wait sleeps in a timed pop, which misses a push made while
    // a trap handler runs on its thread, since the handler's return puts the
    // thread back to sleep; so a thread of the pool's own pushes, once the
    // GVL is free.
    fn wake_owed(&self, ruby: &Ruby) -> Result<(), Error> {
        let owed: Vec<Value> = self
            .owed
            .borrow_mut()
            .drain(..)
            .filter_map(|queue| queue.get(ruby))
            .collect();
        if owed.is_empty() {
            return Ok(());
        }
        let queues = ruby.ary_new_capa(owed.len());
        for queue in owed {
            queues.push(queue)?;
        }
        gc_root::spawn(ruby, queues, |_ruby, queues: RArray| {
            while let Ok(Some(queue)) = queues.shift::<Option<Value>>() {
                let _ = queue.funcall::<_, _, Value>("push", (true,));
            }
        })
    }

    fn wait_in_turn(
        &self,
        ruby: &Ruby,
        rb_self: Obj<Self>,
        fiber: Value,
        queue: Value,
    ) -> Result<(usize, Value), Error> {
        loop {
            self.reconcile(ruby);
            self.refuse_if_shut_down(ruby)?;
            if self.reserved_for(ruby, queue).is_none() && !self.is_listed(ruby, queue) {
                self.enlist(ruby, queue, fiber);
            }
            self.start_pending(ruby, rb_self)?;
            self.hand_off(ruby)?;
            if let Some((index, _)) = self.reserved_for(ruby, queue) {
                return self.take(ruby, index, fiber);
            }
            if self
                .first_waiting(ruby)
                .is_some_and(|(first, _)| same(first, queue))
                && let Some(index) = self
                    .slots(ruby)?
                    .iter()
                    .position(|slot| slot.phase.get() == Phase::Failed)
            {
                return Err(self.failed(ruby, rb_self, index));
            }
            queue.funcall::<_, _, Value>("pop", (kwargs!("timeout" => RESCAN_SECONDS),))?;
        }
    }

    fn is_listed(&self, ruby: &Ruby, queue: Value) -> bool {
        self.waiting.borrow().iter().any(|waiter| {
            waiter
                .queue
                .get(ruby)
                .is_some_and(|listed| same(listed, queue))
        })
    }

    // Makes no Ruby call. Whether it gave back a context handed to it.
    fn leave(&self, ruby: &Ruby, queue: Value) -> bool {
        self.delist(ruby, queue);
        let Some((_, slot)) = self.reserved_for(ruby, queue) else {
            return false;
        };
        slot.holder.take(ruby);
        slot.waiter.take(ruby);
        slot.phase.set(Phase::Idle);
        true
    }

    // Gives each free context to the first waiting checkout and wakes it; a
    // failed build wakes the first to raise it.
    fn hand_off(&self, ruby: &Ruby) -> Result<(), Error> {
        if self.shut_down.get() {
            return Ok(());
        }
        let slots = self.slots(ruby)?;
        while let Some((first, fiber)) = self.first_waiting(ruby) {
            if let Some(slot) = slots.iter().find(|slot| slot.phase.get() == Phase::Idle) {
                self.delist(ruby, first);
                slot.holder.set(ruby, first);
                slot.waiter.set(ruby, fiber);
                slot.phase.set(Phase::Reserved);
            } else if !slots.iter().any(|slot| slot.phase.get() == Phase::Failed) {
                return Ok(());
            }
            first.funcall::<_, _, Value>("push", (true,))?;
            if slots.iter().any(|slot| slot.phase.get() == Phase::Failed) {
                return Ok(());
            }
        }
        Ok(())
    }

    fn take(&self, ruby: &Ruby, index: usize, fiber: Value) -> Result<(usize, Value), Error> {
        let slot = self.slot(index).ok_or_else(|| uninitialized(ruby))?;
        let context = slot
            .value
            .get(ruby)
            .ok_or_else(|| errors::internal(ruby, "the pool's context is missing"))?;
        slot.phase.set(Phase::InUse);
        slot.holder.set(ruby, fiber);
        slot.waiter.take(ruby);
        slot.uses.set(slot.uses.get().saturating_add(1));
        slot.spent.set(false);
        Ok((index, context))
    }

    // A failed build is raised to the checkout that finds it, after another
    // is started.
    fn failed(&self, ruby: &Ruby, rb_self: Obj<Self>, index: usize) -> Error {
        let raised = self
            .slot(index)
            .and_then(|slot| slot.value.take(ruby))
            .and_then(Exception::from_value);
        if let Some(slot) = self.slot(index) {
            slot.phase.set(Phase::Pending);
        }
        if let Err(error) = self.start_pending(ruby, rb_self) {
            return error;
        }
        raised.map_or_else(
            || errors::internal(ruby, "a pool context failed to build"),
            Error::from,
        )
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

    // Starts the threads pending slots wait for, or after shutdown disposes
    // their contexts here, then hands what is free to waiting checkouts.
    fn settle(&self, ruby: &Ruby, rb_self: Obj<Self>) -> Result<(), Error> {
        self.reconcile(ruby);
        self.start_pending(ruby, rb_self)?;
        self.hand_off(ruby)?;
        self.wake_owed(ruby)
    }

    fn start_pending(&self, ruby: &Ruby, rb_self: Obj<Self>) -> Result<(), Error> {
        let slots = self.slots(ruby)?;
        let mut failed: Option<Error> = None;
        for (index, slot) in slots.iter().enumerate() {
            if slot.phase.get() != Phase::Pending || failed.as_ref().is_some_and(jumped) {
                continue;
            }
            let started = if self.shut_down.get() {
                slot.phase.set(Phase::Closed);
                slot.value.take(ruby).map_or(Ok(()), dispose)
            } else {
                self.start(ruby, rb_self, index)
            };
            if let Err(error) = started {
                failed.get_or_insert(error);
            }
        }
        failed.map_or(Ok(()), Err)
    }

    // Replacements are built on a thread of the pool's own, off the request
    // path: it waits on the runners with the GVL released, and a fork leaves
    // it behind, so a child rebuilds whatever its parent's threads were
    // building.
    fn start(&self, ruby: &Ruby, rb_self: Obj<Self>, index: usize) -> Result<(), Error> {
        let Some(slot) = self.slot(index) else {
            return Ok(());
        };
        slot.phase.set(Phase::Building);
        let spawned = gc_root::spawn(ruby, rb_self, move |ruby, pool: Obj<Self>| {
            pool.replace(ruby, pool, index);
        });
        let Err(error) = spawned else {
            return Ok(());
        };
        let refused = match error.error_type() {
            ErrorType::Exception(exception)
                if exception.is_kind_of(ruby.exception_thread_error()) =>
            {
                Some(*exception)
            }
            _ => None,
        };
        let Some(refused) = refused else {
            // An interrupt raised while the thread was made; the slot waits
            // for the next checkout.
            slot.phase.set(Phase::Pending);
            return Err(error);
        };
        // Ruby could not start a thread: the next checkout raises why, after
        // the retired context, if any, is disposed here.
        let retired = slot.value.take(ruby);
        slot.value.set(ruby, refused.as_value());
        slot.phase.set(Phase::Failed);
        retired.map_or(Ok(()), dispose)
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
        rb_self.reconcile(ruby);
        // Called again, it finishes disposing what an interrupt stopped.
        if !rb_self.shut_down.replace(true) {
            for slot in slots {
                match slot.phase.get() {
                    Phase::Idle | Phase::Reserved => {
                        slot.holder.take(ruby);
                        slot.waiter.take(ruby);
                        slot.phase.set(Phase::Pending);
                    }
                    Phase::Failed => {
                        slot.value.take(ruby);
                        slot.phase.set(Phase::Closed);
                    }
                    Phase::Pending | Phase::Building | Phase::InUse | Phase::Closed => {}
                }
            }
        }
        let waiting: Vec<Value> = rb_self
            .waiting
            .borrow()
            .iter()
            .filter_map(|waiter| waiter.queue.get(ruby))
            .collect();
        for queue in waiting {
            queue.funcall::<_, _, Value>("push", (true,))?;
        }
        rb_self.start_pending(ruby, rb_self)
    }
}

// By identity; both values are pinned, by the pool's mark or this stack.
fn same(left: Value, right: Value) -> bool {
    rb_raw::identity(left) == rb_raw::identity(right)
}
