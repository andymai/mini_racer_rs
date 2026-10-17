use crate::lock::lock;
use crate::primitives::Mutex;

/// Why V8 stopped running a dispatch's JavaScript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminationCause {
    Stop,
    Timeout,
    Cancelled,
    MaxMemory,
    HeapExhausted,
}

impl TerminationCause {
    // Most important first: the memory causes raise a different Ruby error
    // than the rest, so they win when several land on one dispatch.
    const BY_PRIORITY: [Self; 5] = [
        Self::HeapExhausted,
        Self::MaxMemory,
        Self::Cancelled,
        Self::Timeout,
        Self::Stop,
    ];

    const fn bit(self) -> u8 {
        match self {
            Self::Stop => 1,
            Self::Timeout => 1 << 1,
            Self::Cancelled => 1 << 2,
            Self::MaxMemory => 1 << 3,
            Self::HeapExhausted => 1 << 4,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct CauseSet(u8);

impl CauseSet {
    // The causes that outlive the dispatch they arrived in, as upstream's
    // non-watchdog termination bit does.
    const PENDING: Self = Self(
        TerminationCause::Stop.bit()
            | TerminationCause::MaxMemory.bit()
            | TerminationCause::HeapExhausted.bit(),
    );

    // The causes upstream records in its request flag beside the one-shot
    // interrupt, which V8 can use up while the flag stays.
    const SPENDABLE: Self = Self(TerminationCause::Stop.bit() | TerminationCause::MaxMemory.bit());

    const fn with(self, cause: TerminationCause) -> Self {
        Self(self.0 | cause.bit())
    }

    const fn without(self, cause: TerminationCause) -> Self {
        Self(self.0 & !cause.bit())
    }

    const fn contains(self, cause: TerminationCause) -> bool {
        self.0 & cause.bit() != 0
    }

    const fn is_empty(self) -> bool {
        self.0 == 0
    }

    const fn intersect(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    const fn minus(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    // What outlives a dispatch that does not report it: a stop or memory
    // cause, unless a cancel cleared everything, as upstream's cancel path
    // stores zero.
    const fn carried(self) -> Self {
        if self.contains(TerminationCause::Cancelled) {
            return Self(0);
        }
        self.intersect(Self::PENDING)
    }

    fn first(self) -> Option<TerminationCause> {
        TerminationCause::BY_PRIORITY
            .into_iter()
            .find(|cause| self.contains(*cause))
    }
}

/// Identifies one outermost dispatch, so a late timeout cannot hit the next one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DispatchId(u64);

/// How a dispatch treats pending causes, following `mini_racer` 0.22.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchKind {
    /// `eval`, `call` and their await forms, snapshot creation and warmup:
    /// report the cause; outermost, consume it.
    Script,
    /// `pump_message_loop`: never report; outermost, consume silently.
    Pump,
    /// Isolate setup, `attach`, heap statistics, heap snapshots, the low
    /// memory notification and the microtask checkpoint: never report; leave
    /// a stop or memory cause for the next script.
    Maintenance,
}

impl DispatchKind {
    fn report(self, causes: CauseSet) -> Option<TerminationCause> {
        match self {
            Self::Script => causes.first(),
            Self::Pump | Self::Maintenance => None,
        }
    }

    // A cancelled dispatch clears everything, so an abandoned wait leaves no
    // stale termination behind.
    const fn remaining(self, causes: CauseSet) -> CauseSet {
        match self {
            Self::Script | Self::Pump => CauseSet(0),
            Self::Maintenance => causes.carried(),
        }
    }
}

/// A request to terminate. Timeouts and cancellations name the dispatch they
/// belong to and are dropped if it has already ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interrupt {
    Stop,
    Timeout(DispatchId),
    Cancel(DispatchId),
    MaxMemory,
    HeapExhausted,
}

impl Interrupt {
    const fn cause(self) -> TerminationCause {
        match self {
            Self::Stop => TerminationCause::Stop,
            Self::Timeout(_) => TerminationCause::Timeout,
            Self::Cancel(_) => TerminationCause::Cancelled,
            Self::MaxMemory => TerminationCause::MaxMemory,
            Self::HeapExhausted => TerminationCause::HeapExhausted,
        }
    }

    const fn dispatch(self) -> Option<DispatchId> {
        match self {
            Self::Timeout(id) | Self::Cancel(id) => Some(id),
            Self::Stop | Self::MaxMemory | Self::HeapExhausted => None,
        }
    }
}

/// What a forked child takes over with a context's termination state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Inherited {
    pending: CauseSet,
    pub disposing: bool,
}

/// How many requests a cell had seen, taken as a dispatch's own work ends, so
/// `spend` leaves alone any that arrived after.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestMark(u64);

/// Why a dispatch must not report its own result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Halt {
    Terminated(TerminationCause),
    Disposed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TermError {
    #[error("the context is being disposed")]
    Disposing,
    #[error("a dispatch is already armed")]
    AlreadyArmed,
}

/// Stops and resumes the isolate's JavaScript from any thread. `terminate`
/// must also wake the runner if it sleeps waiting for tasks, or a stop during
/// an await would wait for the next task to be noticed.
pub trait Terminator {
    fn terminate(&self);
    fn cancel(&self);
}

#[derive(Clone, Copy)]
struct Armed {
    id: DispatchId,
    kind: DispatchKind,
    terminated: bool,
    // `settle` has given the verdict, and the result is being serialized.
    settled: bool,
}

struct State<T> {
    handle: Option<T>,
    armed: Option<Armed>,
    causes: CauseSet,
    // A carried stop or memory cause is still to be reported, but V8 no
    // longer holds its termination: mini_racer's request flag stays set
    // after its one-shot interrupt is used up or dropped, so the next script
    // runs and reports the cause when it ends.
    spent: bool,
    requests: u64,
    disposing: bool,
    suspended: bool,
    // A script nested in an untimed dispatch, timed on its own.
    nested_timer: Option<DispatchId>,
    next: u64,
}

impl<T: Terminator> State<T> {
    fn finish(&mut self, id: DispatchId) -> Option<Halt> {
        match self.armed {
            Some(armed) if armed.id == id && armed.settled => {}
            _ => return self.end(id),
        }
        self.cancel_armed();
        self.suspended = false;
        self.nested_timer = None;
        self.armed = None;
        if self.disposing {
            return Some(Halt::Disposed);
        }
        let causes = self.causes.carried();
        self.set_causes(causes);
        None
    }

    // The causes V8 still has a termination for.
    const fn live(&self) -> CauseSet {
        if self.spent {
            self.causes.minus(CauseSet::SPENDABLE)
        } else {
            self.causes
        }
    }

    const fn set_causes(&mut self, causes: CauseSet) {
        self.causes = causes;
        if causes.carried().is_empty() {
            self.spent = false;
        }
    }

    fn terminate_armed(&mut self) {
        if self.suspended {
            return;
        }
        if let (Some(armed), Some(handle)) = (&mut self.armed, &self.handle) {
            handle.terminate();
            armed.terminated = true;
        }
    }

    fn cancel_armed(&mut self) {
        if let (Some(armed), Some(handle)) = (&mut self.armed, &self.handle)
            && armed.terminated
        {
            handle.cancel();
            armed.terminated = false;
        }
    }

    fn end(&mut self, id: DispatchId) -> Option<Halt> {
        if self.armed.map(|armed| armed.id) != Some(id) {
            return None;
        }
        self.cancel_armed();
        self.suspended = false;
        self.nested_timer = None;
        let armed = self.armed.take()?;
        if self.disposing {
            return Some(Halt::Disposed);
        }
        let causes = self.causes;
        self.set_causes(armed.kind.remaining(causes));
        armed.kind.report(causes).map(Halt::Terminated)
    }
}

/// A context's termination state.
///
/// V8 is told to terminate only while an outermost dispatch is armed, and
/// `end` cancels that under the same lock, so a cause that arrives late is
/// either recorded for later or dropped, never left behind to kill the next
/// dispatch.
pub struct TermCell<T> {
    state: Mutex<State<T>>,
}

impl<T> Default for TermCell<T> {
    fn default() -> Self {
        Self {
            state: Mutex::new(State {
                handle: None,
                armed: None,
                causes: CauseSet::default(),
                spent: false,
                requests: 0,
                disposing: false,
                suspended: false,
                nested_timer: None,
                next: 0,
            }),
        }
    }
}

impl<T: Terminator> TermCell<T> {
    pub fn install(&self, handle: T) {
        lock(&self.state).handle = Some(handle);
    }

    /// Taken back before the isolate is dropped, so no later request reaches
    /// a dead isolate.
    pub fn uninstall(&self) -> Option<T> {
        lock(&self.state).handle.take()
    }

    /// Arms an outermost dispatch; nested dispatches run inside it. A cause
    /// still pending terminates it at once, as upstream's queued termination
    /// kills the next entry into JavaScript of any kind, unless that
    /// termination is spent.
    pub fn begin(&self, kind: DispatchKind) -> Result<DispatchId, TermError> {
        let mut state = lock(&self.state);
        if state.disposing {
            return Err(TermError::Disposing);
        }
        if state.armed.is_some() {
            return Err(TermError::AlreadyArmed);
        }
        let id = DispatchId(state.next);
        state.next = state.next.wrapping_add(1);
        state.armed = Some(Armed {
            id,
            kind,
            terminated: false,
            settled: false,
        });
        if !state.live().is_empty() {
            state.terminate_armed();
        }
        drop(state);
        Ok(id)
    }

    /// Records `interrupt` and terminates the armed dispatch. Returns false
    /// when the interrupt names a dispatch that is no longer armed.
    pub fn request(&self, interrupt: Interrupt) -> bool {
        let mut state = lock(&self.state);
        if let Some(id) = interrupt.dispatch()
            && state.armed.map(|armed| armed.id) != Some(id)
            && !(matches!(interrupt, Interrupt::Timeout(_)) && state.nested_timer == Some(id))
        {
            return false;
        }
        state.causes = state.causes.with(interrupt.cause());
        state.spent = false;
        state.requests = state.requests.wrapping_add(1);
        state.terminate_armed();
        true
    }

    #[must_use]
    pub fn mark(&self) -> RequestMark {
        RequestMark(lock(&self.state).requests)
    }

    /// V8 used up a termination during work that ended at `since`, as
    /// `mini_racer`'s attach or a microtask does with its one-shot interrupt:
    /// a carried stop or memory cause is left for the next script to report,
    /// and that script runs. A request after `since` keeps its own
    /// termination. For a dispatch nested in a callback, the termination is
    /// lifted so the script around it runs on, as in `mini_racer`, unless a
    /// request arrived after `since`, a dispose began, the heap is exhausted,
    /// which `mini_racer` has no counterpart for, or a timeout ended a script
    /// that has its verdict: its leftover microtasks would then run with no
    /// limit. Returns whether it lifted.
    pub fn spend(&self, since: RequestMark, nested: bool) -> bool {
        let mut state = lock(&self.state);
        if state.requests != since.0 {
            return false;
        }
        if !state.causes.carried().is_empty() {
            state.spent = true;
        }
        let lift = nested
            && !state.disposing
            && !state.causes.contains(TerminationCause::HeapExhausted)
            && (!state.causes.contains(TerminationCause::Timeout)
                || state.armed.is_none_or(|armed| !armed.settled));
        if lift {
            state.cancel_armed();
        }
        lift
    }

    /// A single-threaded runner has locked its isolate again for a request,
    /// and V8 drops every termination it had not acted on: the carried
    /// causes are spent, as they are when `mini_racer` enters its locker.
    pub fn relocked(&self) {
        let mut state = lock(&self.state);
        if !state.causes.carried().is_empty() {
            state.spent = true;
        }
    }

    /// A nested script that reports a cause hands the termination back to the
    /// dispatch around it, as `mini_racer`'s `restore_termination` does, even
    /// one an earlier dispatch had spent.
    pub fn renew(&self) {
        let mut state = lock(&self.state);
        if !state.causes.is_empty() {
            state.spent = false;
            state.terminate_armed();
        }
    }

    /// Whether a dispatch of `kind` must not start: V8 would end it at its
    /// first step, or a dispose has begun. A spent cause lets it run.
    #[must_use]
    pub fn refuses(&self, kind: DispatchKind) -> bool {
        let state = lock(&self.state);
        state.disposing || kind.report(state.live()).is_some()
    }

    /// Takes over what a context's cell held in the parent of a fork.
    pub fn adopt(&self, inherited: Inherited) {
        lock(&self.state).causes = inherited.pending;
    }

    /// What a forked child takes over with the context: the causes pending
    /// for the next script, and whether a dispose had begun. `None` while the
    /// lock is held, which in a child means by a thread that is gone.
    #[must_use]
    pub fn inherited(&self) -> Option<Inherited> {
        let state = self.state.try_lock().ok()?;
        Some(Inherited {
            pending: state.causes.intersect(CauseSet::PENDING),
            disposing: state.disposing,
        })
    }

    /// Marks the context as disposing: the armed dispatch, if any, is
    /// terminated and halts with `Disposed`, and no new one can begin.
    pub fn dispose(&self) -> bool {
        let mut state = lock(&self.state);
        state.disposing = true;
        state.terminate_armed();
        state.armed.is_some()
    }

    /// What a dispatch of `kind` must report instead of its result, without
    /// consuming anything: runners ask before running script, and a nested
    /// dispatch asks before replying, leaving the cause to its enclosing one.
    #[must_use]
    pub fn check(&self, kind: DispatchKind) -> Option<Halt> {
        let state = lock(&self.state);
        if state.disposing {
            return Some(Halt::Disposed);
        }
        kind.report(state.causes).map(Halt::Terminated)
    }

    /// Lifts the armed dispatch's termination so a nested dispatch can encode
    /// its reply. Causes that arrive meanwhile are recorded, and `resume`
    /// terminates again if any are pending.
    pub fn suspend(&self) {
        let mut state = lock(&self.state);
        state.suspended = true;
        state.cancel_armed();
    }

    pub fn resume(&self) {
        let mut state = lock(&self.state);
        state.suspended = false;
        if !state.causes.is_empty() || state.disposing {
            state.terminate_armed();
        }
    }

    /// Gives a script nested in an untimed dispatch (a pump or a microtask
    /// checkpoint, through a callback) a timeout of its own, as `mini_racer`
    /// arms its watchdog for any script when none is armed: an id the timeout
    /// names. `None` when the armed dispatch is a script, whose own timeout
    /// covers what it runs, or when an enclosing nested script has one.
    pub fn time_nested(&self) -> Option<DispatchId> {
        let mut state = lock(&self.state);
        let armed = state.armed?;
        if armed.kind == DispatchKind::Script || state.nested_timer.is_some() {
            return None;
        }
        let id = DispatchId(state.next);
        state.next = state.next.wrapping_add(1);
        state.nested_timer = Some(id);
        drop(state);
        Some(id)
    }

    /// Ends a nested script's own timeout. One that fired is dropped, and the
    /// dispatch around the script carries on unless another cause needs the
    /// termination, as `mini_racer` lifts its watchdog's.
    pub fn untime_nested(&self, id: DispatchId) {
        let mut state = lock(&self.state);
        if state.nested_timer != Some(id) {
            return;
        }
        state.nested_timer = None;
        if state.causes.contains(TerminationCause::Timeout) {
            let causes = state.causes.without(TerminationCause::Timeout);
            state.set_causes(causes);
            if state.causes.is_empty() && !state.disposing {
                state.cancel_armed();
            }
        }
    }

    /// Terminates the armed dispatch's script for an abandoned nested dispatch
    /// without recording a cause, so the dispatch it runs in carries on once
    /// `release` lifts it.
    pub fn interrupt(&self) {
        lock(&self.state).terminate_armed();
    }

    /// Lifts an `interrupt` once its nested dispatch has responded, unless a
    /// recorded cause or a dispose still needs the termination.
    pub fn release(&self) {
        let mut state = lock(&self.state);
        if state.causes.is_empty() && !state.disposing {
            state.cancel_armed();
        }
    }

    #[must_use]
    pub fn is_disposing(&self) -> bool {
        lock(&self.state).disposing
    }

    /// Whether the armed dispatch has its verdict (`settle`) and is finishing
    /// its reply.
    #[must_use]
    pub fn is_settled(&self) -> bool {
        lock(&self.state).armed.is_some_and(|armed| armed.settled)
    }

    /// The armed dispatch, for a cancellation from a nested wait.
    #[must_use]
    pub fn armed(&self) -> Option<DispatchId> {
        lock(&self.state).armed.map(|armed| armed.id)
    }

    /// Disarms `id` and settles its outcome. The runner calls it after the
    /// dispatch's last V8 work, and a halt it returns replaces that work's
    /// result; a cause arriving later waits for the next dispatch. A no-op
    /// returning `None` when `id` is not armed.
    pub fn end(&self, id: DispatchId) -> Option<Halt> {
        lock(&self.state).end(id)
    }

    /// `mini_racer`'s last check before an outermost dispatch serializes its
    /// result: reports and consumes the causes so far, as `end` would, and
    /// lifts the termination so the result can be serialized. The dispatch
    /// stays armed, so a cause that lands meanwhile still stops a getter.
    pub fn settle(&self, id: DispatchId) -> Option<Halt> {
        let mut state = lock(&self.state);
        let armed = state.armed.filter(|armed| armed.id == id)?;
        if state.disposing {
            return Some(Halt::Disposed);
        }
        state.cancel_armed();
        let causes = state.causes;
        state.set_causes(armed.kind.remaining(causes));
        if let Some(armed) = &mut state.armed {
            armed.settled = true;
        }
        drop(state);
        armed.kind.report(causes).map(Halt::Terminated)
    }

    /// Disarms `id` once its reply is complete. After `settle`, a cause that
    /// landed while the result serialized, without stopping it, leaves the
    /// result alone: a timeout or cancel is dropped, and a stop or memory
    /// cause waits for the next script, as `mini_racer` leaves them. A dispatch
    /// that was never settled ends as `end` ends it.
    pub fn finish(&self, id: DispatchId) -> Option<Halt> {
        lock(&self.state).finish(id)
    }

    /// Disarms `id` once the leftover microtasks of its terminated script ran
    /// without V8 acting on a cause. One still live fails `mini_racer`'s reply,
    /// whose retry reports it with this script, as `end` does; with none, a
    /// spent stop or memory cause is carried, as `finish` leaves it.
    pub fn end_drained(&self, id: DispatchId) -> Option<Halt> {
        let mut state = lock(&self.state);
        if state.live().is_empty() {
            state.finish(id)
        } else {
            state.end(id)
        }
    }
}

#[cfg(all(test, not(loom)))]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use super::*;

    #[derive(Default)]
    struct Calls {
        terminate: Cell<u32>,
        cancel: Cell<u32>,
    }

    struct Probe(Rc<Calls>);

    impl Terminator for Probe {
        fn terminate(&self) {
            self.0.terminate.set(self.0.terminate.get() + 1);
        }

        fn cancel(&self) {
            self.0.cancel.set(self.0.cancel.get() + 1);
        }
    }

    fn cell() -> (TermCell<Probe>, Rc<Calls>) {
        let calls = Rc::new(Calls::default());
        let cell = TermCell::default();
        cell.install(Probe(Rc::clone(&calls)));
        (cell, calls)
    }

    fn run(cell: &TermCell<Probe>, kind: DispatchKind) -> Option<Halt> {
        let id = cell.begin(kind).expect("nothing armed");
        cell.end(id)
    }

    const STOPPED: Option<Halt> = Some(Halt::Terminated(TerminationCause::Stop));

    #[test]
    fn a_stop_while_idle_is_reported_by_the_next_script_only() {
        let (cell, calls) = cell();
        assert!(cell.request(Interrupt::Stop));
        assert_eq!(calls.terminate.get(), 0);

        assert_eq!(run(&cell, DispatchKind::Script), STOPPED);
        assert_eq!(run(&cell, DispatchKind::Script), None);
    }

    #[test]
    fn a_pending_stop_terminates_the_next_dispatch_of_any_kind() {
        let (cell, calls) = cell();
        cell.request(Interrupt::Stop);
        let id = cell
            .begin(DispatchKind::Maintenance)
            .expect("nothing armed");
        assert_eq!(calls.terminate.get(), 1);

        assert_eq!(cell.end(id), None);
        assert_eq!(calls.cancel.get(), 1);
        assert_eq!(run(&cell, DispatchKind::Script), STOPPED);
    }

    #[test]
    fn a_stop_during_a_script_terminates_it_and_is_cancelled_at_end() {
        let (cell, calls) = cell();
        let id = cell.begin(DispatchKind::Script).expect("nothing armed");
        cell.request(Interrupt::Stop);
        assert_eq!(calls.terminate.get(), 1);

        assert_eq!(cell.end(id), STOPPED);
        assert_eq!(calls.cancel.get(), 1);
        assert_eq!(cell.check(DispatchKind::Script), None);
    }

    #[test]
    fn a_nested_script_reports_a_stop_and_leaves_it_pending() {
        let (cell, _) = cell();
        let id = cell.begin(DispatchKind::Script).expect("nothing armed");
        cell.request(Interrupt::Stop);

        assert_eq!(cell.check(DispatchKind::Script), STOPPED);
        assert_eq!(cell.check(DispatchKind::Pump), None);
        assert_eq!(cell.check(DispatchKind::Maintenance), None);
        assert_eq!(cell.end(id), STOPPED);
    }

    #[test]
    fn an_interrupt_for_a_nested_dispatch_is_lifted_and_reports_nothing() {
        let (cell, calls) = cell();
        let id = cell.begin(DispatchKind::Script).expect("nothing armed");
        cell.interrupt();
        assert_eq!(calls.terminate.get(), 1);
        assert_eq!(cell.check(DispatchKind::Script), None);
        cell.release();
        assert_eq!(calls.cancel.get(), 1);
        assert_eq!(cell.end(id), None);
    }

    #[test]
    fn a_cause_recorded_meanwhile_keeps_the_interrupt() {
        let (cell, calls) = cell();
        let id = cell.begin(DispatchKind::Script).expect("nothing armed");
        cell.interrupt();
        cell.request(Interrupt::Stop);
        cell.release();
        assert_eq!(calls.cancel.get(), 0);
        assert_eq!(cell.end(id), STOPPED);
    }

    #[test]
    fn a_pump_consumes_a_stop_silently() {
        let (cell, _) = cell();
        cell.request(Interrupt::Stop);

        assert_eq!(run(&cell, DispatchKind::Pump), None);
        assert_eq!(run(&cell, DispatchKind::Script), None);
    }

    #[test]
    fn maintenance_leaves_a_stop_pending() {
        let (cell, _) = cell();
        let id = cell
            .begin(DispatchKind::Maintenance)
            .expect("nothing armed");
        cell.request(Interrupt::Stop);
        assert_eq!(cell.check(DispatchKind::Maintenance), None);
        assert_eq!(cell.end(id), None);

        assert_eq!(run(&cell, DispatchKind::Script), STOPPED);
    }

    #[test]
    fn a_timeout_after_its_dispatch_ended_is_dropped() {
        let (cell, calls) = cell();
        let first = cell.begin(DispatchKind::Script).expect("nothing armed");
        cell.end(first);
        let second = cell.begin(DispatchKind::Script).expect("nothing armed");

        assert!(!cell.request(Interrupt::Timeout(first)));
        assert_eq!(calls.terminate.get(), 0);
        assert_eq!(cell.end(second), None);
    }

    #[test]
    fn a_timeout_does_not_outlive_its_dispatch() {
        let (cell, _) = cell();
        let id = cell
            .begin(DispatchKind::Maintenance)
            .expect("nothing armed");
        assert!(cell.request(Interrupt::Timeout(id)));
        assert_eq!(cell.end(id), None);

        assert_eq!(run(&cell, DispatchKind::Script), None);
    }

    #[test]
    fn a_cancel_clears_every_cause() {
        let (cell, _) = cell();
        cell.request(Interrupt::Stop);
        let id = cell.begin(DispatchKind::Script).expect("nothing armed");
        cell.request(Interrupt::Cancel(id));

        assert_eq!(
            cell.end(id),
            Some(Halt::Terminated(TerminationCause::Cancelled))
        );
        assert_eq!(run(&cell, DispatchKind::Script), None);
    }

    #[test]
    fn check_and_end_agree_on_the_cause() {
        let (cell, _) = cell();
        let id = cell.begin(DispatchKind::Script).expect("nothing armed");
        cell.request(Interrupt::Cancel(id));
        cell.request(Interrupt::MaxMemory);
        let memory = Some(Halt::Terminated(TerminationCause::MaxMemory));

        assert_eq!(cell.check(DispatchKind::Script), memory);
        assert_eq!(cell.end(id), memory);
        assert_eq!(run(&cell, DispatchKind::Script), None);
    }

    #[test]
    fn memory_causes_outrank_a_stop() {
        let (cell, _) = cell();
        cell.request(Interrupt::Stop);
        cell.request(Interrupt::MaxMemory);

        assert_eq!(
            run(&cell, DispatchKind::Script),
            Some(Halt::Terminated(TerminationCause::MaxMemory))
        );
    }

    #[test]
    fn a_suspended_dispatch_is_terminated_again_on_resume() {
        let (cell, calls) = cell();
        let id = cell.begin(DispatchKind::Script).expect("nothing armed");
        cell.request(Interrupt::Stop);
        cell.suspend();
        assert_eq!(calls.cancel.get(), 1);

        cell.request(Interrupt::Stop);
        assert_eq!(calls.terminate.get(), 1);
        cell.resume();
        assert_eq!(calls.terminate.get(), 2);
        assert_eq!(cell.end(id), STOPPED);
        assert_eq!(calls.cancel.get(), 2);
    }

    #[test]
    fn resume_without_causes_leaves_the_dispatch_running() {
        let (cell, calls) = cell();
        let id = cell.begin(DispatchKind::Script).expect("nothing armed");
        cell.suspend();
        cell.resume();

        assert_eq!(calls.terminate.get(), 0);
        assert_eq!(cell.end(id), None);
    }

    #[test]
    fn disposing_halts_the_armed_dispatch_and_refuses_new_ones() {
        let (cell, calls) = cell();
        let id = cell
            .begin(DispatchKind::Maintenance)
            .expect("nothing armed");
        assert!(cell.dispose());
        assert_eq!(calls.terminate.get(), 1);
        assert_eq!(cell.check(DispatchKind::Maintenance), Some(Halt::Disposed));

        assert_eq!(cell.end(id), Some(Halt::Disposed));
        assert_eq!(calls.cancel.get(), 1);
        assert_eq!(cell.begin(DispatchKind::Script), Err(TermError::Disposing));
    }

    #[test]
    fn one_outermost_dispatch_at_a_time() {
        let (cell, _) = cell();
        let id = cell.begin(DispatchKind::Script).expect("nothing armed");

        assert_eq!(
            cell.begin(DispatchKind::Script),
            Err(TermError::AlreadyArmed)
        );
        assert_eq!(cell.armed(), Some(id));
        cell.end(id);
        assert_eq!(cell.end(id), None);
        assert_eq!(cell.armed(), None);
    }

    #[test]
    fn nothing_is_cancelled_when_nothing_was_terminated() {
        let (cell, calls) = cell();
        run(&cell, DispatchKind::Script);

        assert_eq!(calls.cancel.get(), 0);
    }

    #[test]
    fn settling_reports_and_consumes_what_came_before_serializing() {
        let (cell, calls) = cell();
        let id = cell.begin(DispatchKind::Script).expect("nothing armed");
        cell.request(Interrupt::Stop);

        assert_eq!(cell.settle(id), STOPPED);
        assert_eq!(calls.cancel.get(), 1);
        assert_eq!(cell.armed(), Some(id));
        assert_eq!(cell.end(id), None);
        assert_eq!(run(&cell, DispatchKind::Script), None);
    }

    #[test]
    fn a_timeout_while_serializing_leaves_the_result_and_the_next_dispatch() {
        let (cell, calls) = cell();
        let id = cell.begin(DispatchKind::Script).expect("nothing armed");
        assert_eq!(cell.settle(id), None);
        assert!(cell.request(Interrupt::Timeout(id)));
        assert_eq!(calls.terminate.get(), 1);

        assert_eq!(cell.finish(id), None);
        assert_eq!(calls.cancel.get(), 1);
        assert_eq!(run(&cell, DispatchKind::Script), None);
    }

    #[test]
    fn a_stop_while_serializing_is_reported_by_the_next_script() {
        let (cell, _) = cell();
        let id = cell.begin(DispatchKind::Script).expect("nothing armed");
        assert_eq!(cell.settle(id), None);
        cell.request(Interrupt::Stop);

        assert_eq!(cell.finish(id), None);
        assert_eq!(run(&cell, DispatchKind::Script), STOPPED);
    }

    #[test]
    fn a_cause_that_stops_the_serializing_is_reported_by_end() {
        let (cell, _) = cell();
        let id = cell.begin(DispatchKind::Script).expect("nothing armed");
        assert_eq!(cell.settle(id), None);
        cell.request(Interrupt::Timeout(id));

        assert_eq!(
            cell.end(id),
            Some(Halt::Terminated(TerminationCause::Timeout))
        );
        assert_eq!(run(&cell, DispatchKind::Script), None);
    }

    #[test]
    fn a_dispose_while_serializing_still_halts_the_dispatch() {
        let (cell, _) = cell();
        let id = cell.begin(DispatchKind::Script).expect("nothing armed");
        assert_eq!(cell.settle(id), None);
        cell.dispose();

        assert_eq!(cell.finish(id), Some(Halt::Disposed));
    }

    #[test]
    fn finishing_an_unsettled_dispatch_ends_it() {
        let (cell, _) = cell();
        cell.request(Interrupt::Stop);
        let id = cell.begin(DispatchKind::Pump).expect("nothing armed");

        assert_eq!(cell.finish(id), None);
        assert_eq!(run(&cell, DispatchKind::Script), None);
    }

    #[test]
    fn a_script_nested_in_an_untimed_dispatch_is_timed_on_its_own() {
        let (cell, calls) = cell();
        let pump = cell.begin(DispatchKind::Pump).expect("nothing armed");
        let nested = cell.time_nested().expect("an untimed dispatch");
        assert_eq!(cell.time_nested(), None);

        assert!(cell.request(Interrupt::Timeout(nested)));
        assert_eq!(calls.terminate.get(), 1);
        assert_eq!(
            cell.check(DispatchKind::Script),
            Some(Halt::Terminated(TerminationCause::Timeout))
        );
        cell.untime_nested(nested);
        assert_eq!(calls.cancel.get(), 1);
        assert_eq!(cell.check(DispatchKind::Script), None);
        assert!(!cell.request(Interrupt::Timeout(nested)));
        assert_eq!(cell.end(pump), None);
    }

    #[test]
    fn a_nested_timeout_keeps_a_stop_that_came_with_it() {
        let (cell, calls) = cell();
        let checkpoint = cell
            .begin(DispatchKind::Maintenance)
            .expect("nothing armed");
        let nested = cell.time_nested().expect("an untimed dispatch");
        cell.request(Interrupt::Timeout(nested));
        cell.request(Interrupt::Stop);

        cell.untime_nested(nested);
        assert_eq!(calls.cancel.get(), 0);
        assert_eq!(cell.check(DispatchKind::Script), STOPPED);
        assert_eq!(cell.end(checkpoint), None);
        assert_eq!(run(&cell, DispatchKind::Script), STOPPED);
    }

    #[test]
    fn a_script_inside_a_script_is_covered_by_the_outer_timeout() {
        let (cell, _) = cell();
        let id = cell.begin(DispatchKind::Script).expect("nothing armed");
        assert_eq!(cell.time_nested(), None);
        assert_eq!(cell.end(id), None);
        assert_eq!(cell.time_nested(), None);
    }

    #[test]
    fn a_child_takes_over_a_stop_and_a_memory_cause_but_no_timeout() {
        let (idle, _) = cell();
        idle.request(Interrupt::Stop);
        idle.request(Interrupt::MaxMemory);
        let inherited = idle.inherited().expect("unlocked");
        assert!(!inherited.disposing);
        let (child, calls) = cell();
        child.adopt(inherited);
        assert!(child.refuses(DispatchKind::Script));
        assert_eq!(
            run(&child, DispatchKind::Script),
            Some(Halt::Terminated(TerminationCause::MaxMemory))
        );
        assert_eq!(calls.terminate.get(), 1);
        assert_eq!(run(&child, DispatchKind::Script), None);

        let (timed, _) = cell();
        let id = timed.begin(DispatchKind::Script).expect("nothing armed");
        timed.request(Interrupt::Timeout(id));
        let (child, _) = cell();
        child.adopt(timed.inherited().expect("unlocked"));
        assert_eq!(run(&child, DispatchKind::Script), None);
    }

    #[test]
    fn a_child_spends_an_inherited_stop_when_it_locks_its_isolate() {
        let (parent, _) = cell();
        parent.request(Interrupt::Stop);
        let (child, calls) = cell();
        child.adopt(parent.inherited().expect("unlocked"));
        child.relocked();
        assert!(!child.refuses(DispatchKind::Script));
        assert_eq!(run(&child, DispatchKind::Script), STOPPED);
        assert_eq!(calls.terminate.get(), 0);
    }

    #[test]
    fn a_dispose_or_an_exhausted_heap_is_never_lifted() {
        let (exhausted, calls) = cell();
        exhausted.request(Interrupt::Stop);
        exhausted.request(Interrupt::HeapExhausted);
        let outer = exhausted
            .begin(DispatchKind::Script)
            .expect("nothing armed");
        assert!(!exhausted.spend(exhausted.mark(), true));
        assert!(exhausted.refuses(DispatchKind::Script));
        assert_eq!(calls.cancel.get(), 0);
        assert!(exhausted.end(outer).is_some());

        let (disposed, calls) = cell();
        let outer = disposed.begin(DispatchKind::Script).expect("nothing armed");
        assert!(disposed.dispose());
        assert!(!disposed.spend(disposed.mark(), true));
        assert_eq!(calls.cancel.get(), 0);
        assert_eq!(disposed.end(outer), Some(Halt::Disposed));
    }

    // mini_racer's attach uses up the one-shot interrupt a stop left, and its
    // request flag makes the next script report the stop when it ends.
    #[test]
    fn a_spent_stop_lets_the_next_script_run_and_report_it() {
        let (cell, calls) = cell();
        cell.request(Interrupt::Stop);
        let since = cell.mark();
        let attach = cell
            .begin(DispatchKind::Maintenance)
            .expect("nothing armed");
        assert_eq!(calls.terminate.get(), 1);
        assert!(!cell.spend(since, false));
        assert_eq!(cell.end(attach), None);

        assert!(!cell.refuses(DispatchKind::Maintenance));
        assert_eq!(run(&cell, DispatchKind::Maintenance), None);
        assert!(!cell.refuses(DispatchKind::Script));
        let script = cell.begin(DispatchKind::Script).expect("nothing armed");
        assert_eq!(calls.terminate.get(), 1);
        assert_eq!(cell.settle(script), STOPPED);
        assert_eq!(cell.finish(script), None);
        assert_eq!(run(&cell, DispatchKind::Script), None);
    }

    #[test]
    fn a_stop_after_a_spent_one_terminates_the_next_dispatch() {
        let (cell, calls) = cell();
        cell.request(Interrupt::Stop);
        cell.spend(cell.mark(), false);
        cell.request(Interrupt::Stop);
        assert!(cell.refuses(DispatchKind::Script));
        assert_eq!(run(&cell, DispatchKind::Script), STOPPED);
        assert_eq!(calls.terminate.get(), 1);
    }

    // A request that lands while the dispatch runs keeps its own termination,
    // as upstream's later TerminateExecution stays pending.
    #[test]
    fn a_request_since_the_mark_is_neither_spent_nor_lifted() {
        let (cell, calls) = cell();
        let outer = cell.begin(DispatchKind::Script).expect("nothing armed");
        let since = cell.mark();
        cell.request(Interrupt::Stop);
        assert!(!cell.spend(since, true));
        assert!(cell.refuses(DispatchKind::Script));
        assert_eq!(calls.cancel.get(), 0);
        assert_eq!(cell.end(outer), STOPPED);
    }

    // Inside a callback the termination a nested dispatch used up is lifted,
    // so the script around it runs on and reports the cause when it ends.
    #[test]
    fn a_nested_spend_lifts_the_termination_for_the_script_around() {
        let (cell, calls) = cell();
        let outer = cell.begin(DispatchKind::Script).expect("nothing armed");
        cell.request(Interrupt::Stop);
        assert!(cell.spend(cell.mark(), true));
        assert_eq!(calls.cancel.get(), 1);
        assert!(!cell.refuses(DispatchKind::Script));
        assert_eq!(cell.check(DispatchKind::Script), STOPPED);
        assert_eq!(cell.end(outer), STOPPED);
        assert_eq!(calls.cancel.get(), 1);
    }

    #[test]
    fn a_settled_dispatch_with_a_timeout_is_never_lifted() {
        let (cell, calls) = cell();
        let outer = cell.begin(DispatchKind::Script).expect("nothing armed");
        cell.request(Interrupt::Timeout(outer));
        assert_eq!(
            cell.settle(outer),
            Some(Halt::Terminated(TerminationCause::Timeout))
        );
        assert!(cell.is_settled());
        cell.request(Interrupt::Timeout(outer));
        let cancels = calls.cancel.get();
        assert!(!cell.spend(cell.mark(), true));
        assert_eq!(calls.cancel.get(), cancels);
        assert_eq!(cell.finish(outer), None);
        assert!(!cell.is_settled());
    }

    // A getter running while a finished script's result serializes, or a
    // leftover microtask, runs on in mini_racer once a nested attach uses up
    // a stop, and the next script reports it.
    #[test]
    fn a_settled_dispatch_lifts_a_spent_stop() {
        let (cell, calls) = cell();
        let outer = cell.begin(DispatchKind::Script).expect("nothing armed");
        assert_eq!(cell.settle(outer), None);
        cell.request(Interrupt::Stop);
        assert!(cell.spend(cell.mark(), true));
        assert_eq!(calls.cancel.get(), 1);
        assert_eq!(cell.finish(outer), None);
        assert!(!cell.refuses(DispatchKind::Script));
        assert_eq!(run(&cell, DispatchKind::Script), STOPPED);
    }

    #[test]
    fn a_drained_script_carries_a_spent_stop_and_reports_a_live_one() {
        let (cell, _) = cell();
        let outer = cell.begin(DispatchKind::Script).expect("nothing armed");
        cell.request(Interrupt::Stop);
        assert_eq!(cell.settle(outer), STOPPED);
        cell.request(Interrupt::Stop);
        cell.spend(cell.mark(), true);
        assert_eq!(cell.end_drained(outer), None);
        assert_eq!(run(&cell, DispatchKind::Script), STOPPED);

        let outer = cell.begin(DispatchKind::Script).expect("nothing armed");
        cell.request(Interrupt::Stop);
        assert_eq!(cell.settle(outer), STOPPED);
        cell.request(Interrupt::Stop);
        assert_eq!(cell.end_drained(outer), STOPPED);
        assert_eq!(run(&cell, DispatchKind::Script), None);
    }

    #[test]
    fn a_nested_spend_lifts_a_timeout_and_keeps_reporting_it() {
        let (cell, calls) = cell();
        let outer = cell.begin(DispatchKind::Script).expect("nothing armed");
        cell.request(Interrupt::Timeout(outer));
        assert!(cell.spend(cell.mark(), true));
        assert_eq!(calls.cancel.get(), 1);
        assert_eq!(
            cell.end(outer),
            Some(Halt::Terminated(TerminationCause::Timeout))
        );
    }

    // V8 drops every termination it had not acted on when a single-threaded
    // runner locks its isolate again, however the cause arrived.
    #[test]
    fn relocking_spends_every_carried_stop_or_memory_cause() {
        let (cell, calls) = cell();
        cell.request(Interrupt::Stop);
        cell.relocked();
        assert!(!cell.refuses(DispatchKind::Script));
        assert_eq!(run(&cell, DispatchKind::Script), STOPPED);
        assert_eq!(calls.terminate.get(), 0);

        let heap = cell
            .begin(DispatchKind::Maintenance)
            .expect("nothing armed");
        cell.request(Interrupt::MaxMemory);
        assert_eq!(calls.terminate.get(), 1);
        assert_eq!(cell.end(heap), None);
        cell.relocked();
        assert!(!cell.refuses(DispatchKind::Script));
        assert_eq!(
            run(&cell, DispatchKind::Script),
            Some(Halt::Terminated(TerminationCause::MaxMemory))
        );

        cell.request(Interrupt::HeapExhausted);
        cell.relocked();
        assert!(cell.refuses(DispatchKind::Script));
    }

    // mini_racer's restore_termination: a nested script that reports a stop
    // terminates the script around it again.
    #[test]
    fn renew_hands_a_spent_stop_back_to_the_dispatch_around() {
        let (cell, calls) = cell();
        cell.request(Interrupt::Stop);
        cell.spend(cell.mark(), false);
        let outer = cell.begin(DispatchKind::Script).expect("nothing armed");
        assert_eq!(calls.terminate.get(), 0);
        assert_eq!(cell.check(DispatchKind::Script), STOPPED);
        cell.renew();
        assert_eq!(calls.terminate.get(), 1);
        assert!(cell.refuses(DispatchKind::Script));
        assert_eq!(cell.end(outer), STOPPED);
        assert_eq!(calls.cancel.get(), 1);
    }

    #[test]
    fn without_a_handle_causes_are_still_recorded() {
        let (cell, calls) = cell();
        cell.uninstall();
        let id = cell.begin(DispatchKind::Script).expect("nothing armed");
        cell.request(Interrupt::Stop);

        assert_eq!(calls.terminate.get(), 0);
        assert_eq!(cell.end(id), STOPPED);
    }
}
