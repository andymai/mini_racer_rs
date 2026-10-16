use std::os::fd::RawFd;
use std::sync::OnceLock;
use std::time::Duration;

use magnus::error::ErrorType;
use magnus::value::ReprValue;
use magnus::{Class, Error, Exception, Integer, RArray, RString, Ruby, Value, kwargs};
use mini_racer_engine::{
    CallbackReply, EngineError, Host, JsError, JsErrorKind, JsText, WaitHandle, WaitOutcome,
};

use crate::encode;
use crate::gc_root::OwnedRoot;
use crate::{context, decode, errors, rb_raw};

struct SpinBudget {
    gvl: Duration,
    nogvl: Duration,
}

// Unstable tuning knob: MINI_RACER_RS_SPIN_US=<with GVL>,<without GVL>.
fn budget() -> &'static SpinBudget {
    static BUDGET: OnceLock<SpinBudget> = OnceLock::new();
    BUDGET.get_or_init(|| {
        let setting = std::env::var("MINI_RACER_RS_SPIN_US").unwrap_or_default();
        let parts: Vec<Option<u64>> = setting
            .split(',')
            .map(|part| part.trim().parse().ok())
            .collect();
        let at = |i: usize, default: u64| {
            Duration::from_micros(parts.get(i).copied().flatten().unwrap_or(default))
        };
        SpinBudget {
            gvl: at(0, 20),
            nogvl: at(1, 50),
        }
    })
}

/// What a context's callbacks run: its attached callables by id, and where
/// the exception the latest callback raised is kept, as `mini_racer` keeps one
/// per context and clears it after every dispatch, nested ones included.
pub struct Callbacks<'r> {
    pub callables: RArray,
    pub raised: &'r OwnedRoot<Value>,
}

/// Waits for a runner from a Ruby thread: spins holding the GVL, then parks
/// the fiber under a fiber scheduler, or else releases the GVL, spins and
/// parks the thread. A Ruby interrupt wakes the park; traps run and the wait
/// goes on, while an exception (Timeout, Thread#raise, #kill) cancels the
/// dispatch and is raised once the engine has its response.
pub struct RubyHost<'r> {
    ruby: &'r Ruby,
    pending: Option<Error>,
    callbacks: Option<Callbacks<'r>>,
}

impl<'r> RubyHost<'r> {
    pub const fn new(ruby: &'r Ruby) -> Self {
        Self {
            ruby,
            pending: None,
            callbacks: None,
        }
    }

    pub const fn with_callbacks(ruby: &'r Ruby, callbacks: Callbacks<'r>) -> Self {
        Self {
            ruby,
            pending: None,
            callbacks: Some(callbacks),
        }
    }

    /// The Ruby exception that cancelled the dispatch outranks whatever the
    /// engine returned.
    pub fn finish<T>(self, result: Result<T, EngineError>) -> Result<T, Error> {
        // mini_racer raises an interrupt before it clears the kept exception,
        // so a later rethrow of that error still raises it.
        if let Some(pending) = self.pending {
            return Err(pending);
        }
        let raised = self
            .callbacks
            .and_then(|callbacks| callbacks.raised.take(self.ruby))
            .and_then(Exception::from_value);
        match (result, raised) {
            (
                Err(EngineError::Js(JsError {
                    kind: JsErrorKind::HostException,
                    ..
                })),
                Some(raised),
            ) => Err(Error::from(raised)),
            (result, _) => result.map_err(|error| errors::engine(self.ruby, error)),
        }
    }

    // Under a fiber scheduler only this fiber waits: the runner's answer makes
    // the context's notifier readable and the scheduler resumes the fiber,
    // while whatever the scheduler raises into it (a timeout, a stop) cancels
    // the dispatch. None means no notifier could be made.
    fn wait_in_fiber(&mut self, wait: &WaitHandle<'_>) -> Option<WaitOutcome> {
        let ruby = self.ruby;
        loop {
            let mut raised = None;
            if wait
                .park_on_fd(|fd| raised = wait_readable(ruby, fd).err())
                .is_err()
            {
                return None;
            }
            if wait.ready() {
                return Some(WaitOutcome::Ready);
            }
            if let Some(error) = raised {
                self.pending = Some(error);
                return Some(WaitOutcome::Cancel);
            }
        }
    }

    // A captured throw resumes through Ruby's error info, which any protected
    // call in between would clear, so nothing runs Ruby code until it does.
    fn jumping(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|pending| matches!(pending.error_type(), ErrorType::Jump(_)))
    }

    // mini_racer's order: the id comes off the end of the arguments, and a
    // reply it cannot send raises its internal error with the encoder's text.
    fn run(&self, request: &[u8]) -> Result<Vec<u8>, Error> {
        let ruby = self.ruby;
        let bad = || errors::runtime(ruby, "bad callback id");
        let args = RArray::from_value(decode::value(ruby, request)?).ok_or_else(bad)?;
        let id = Integer::from_value(args.pop::<Value>()?).ok_or_else(bad)?;
        let id = isize::try_from(id.to_i64()?).map_err(|_| bad())?;
        let callables = self
            .callbacks
            .as_ref()
            .map(|callbacks| callbacks.callables)
            .ok_or_else(bad)?;
        if id < 0 || id.cast_unsigned() >= callables.len() {
            return Err(bad());
        }
        let callable = callables.entry::<Value>(id)?;
        let result = rb_raw::call_with_array(ruby, callable, args)?;
        encode::encode(ruby, result, |message| errors::internal(ruby, message))
    }
}

// The engine owns the descriptor, so Ruby must never close it.
fn wait_readable(ruby: &Ruby, fd: RawFd) -> Result<(), Error> {
    let io: Value = ruby
        .class_io()
        .funcall("for_fd", (fd, kwargs!("autoclose" => false)))?;
    let _: Value = io.funcall("wait_readable", ())?;
    Ok(())
}

fn exception(ruby: &Ruby, error: &Error) -> Option<Exception> {
    match error.error_type() {
        ErrorType::Jump(_) => None,
        ErrorType::Exception(exception) => Some(*exception),
        ErrorType::Error(class, message) => class
            .new_instance((ruby.str_new(message),))
            .map_or_else(|error| exception(ruby, &error), Some),
    }
}

// What V8 throws for a raising callback: the exception's to_s, in its own
// encoding, or mini_racer's fallback text.
fn message(ruby: &Ruby, exception: Exception) -> JsText {
    exception
        .funcall::<_, _, RString>("to_s", ())
        .ok()
        .and_then(|text| context::js_text(ruby, text).ok())
        .unwrap_or_else(|| JsText::from("Ruby exception"))
}

impl Host for RubyHost<'_> {
    fn wait(&mut self, wait: &WaitHandle<'_>) -> WaitOutcome {
        let budget = budget();
        if wait.spin(budget.gvl) {
            return WaitOutcome::Ready;
        }
        if rb_raw::fiber_scheduler(self.ruby)
            && let Some(outcome) = self.wait_in_fiber(wait)
        {
            return outcome;
        }
        let unparker = wait.unparker();
        loop {
            let parked = rb_raw::nogvl(
                self.ruby,
                || {
                    if !wait.spin(budget.nogvl) {
                        wait.park();
                    }
                },
                &move || unparker.unpark(),
            );
            if wait.ready() {
                return WaitOutcome::Ready;
            }
            if parked.as_ref().is_some_and(Result::is_err) {
                self.pending = Some(errors::internal(self.ruby, "panic while waiting"));
                return WaitOutcome::Cancel;
            }
            if let Err(error) = rb_raw::check_ints(self.ruby) {
                self.pending = Some(error);
                return WaitOutcome::Cancel;
            }
        }
    }

    // Interrupts cannot end this wait, but they still run: a pending one would
    // otherwise make every rb_nogvl return at once and keep the GVL held. An
    // exception they raise is kept and raised once the dispatch is over.
    fn wait_blocking(&mut self, wait: &WaitHandle<'_>) {
        if self.jumping() {
            while !wait.ready() {
                wait.park();
            }
            return;
        }
        while !wait.ready() {
            let parked = rb_raw::nogvl(
                self.ruby,
                || {
                    while !wait.ready() {
                        wait.park();
                    }
                },
                &|| {},
            );
            if parked.is_none()
                && let Err(error) = rb_raw::check_ints(self.ruby)
            {
                self.pending.get_or_insert(error);
            }
        }
    }

    // A throw or Thread#kill out of the callable aborts the dispatch and
    // resumes once it is over.
    fn callback(&mut self, request: &[u8]) -> CallbackReply {
        let error = match self.run(request) {
            Ok(reply) => return CallbackReply::Value(reply),
            Err(error) => error,
        };
        let Some(exception) = exception(self.ruby, &error) else {
            self.pending.get_or_insert(error);
            return CallbackReply::Abort;
        };
        let message = message(self.ruby, exception);
        if let Some(callbacks) = &self.callbacks {
            callbacks.raised.set(self.ruby, exception.as_value());
        }
        CallbackReply::Raised(message)
    }
}
