use magnus::value::ReprValue;
use magnus::{Error, ExceptionClass, Module, RModule, Ruby};
use mini_racer_engine::{EngineError, FlagError, JsErrorKind, TerminationCause};

use crate::gc_root::StaticRoot;

// const_get on a module falls back to Object, so a missing MiniRacer::RuntimeError
// would silently resolve to ::RuntimeError; the inspect check rejects that.
fn class(ruby: &Ruby, module: &str, name: &str) -> Result<ExceptionClass, Error> {
    let found: ExceptionClass = ruby
        .class_object()
        .const_get::<_, RModule>(module)?
        .const_get(name)?;
    let expected = format!("{module}::{name}");
    if found.inspect() == expected {
        Ok(found)
    } else {
        Err(Error::new(
            ruby.exception_load_error(),
            format!("{expected} is not defined"),
        ))
    }
}

macro_rules! roots {
    ($($root:ident = $module:literal :: $name:literal;)*) => {
        $(
            static $root: StaticRoot<ExceptionClass> = StaticRoot::new(|ruby| {
                class(ruby, $module, $name).expect("errors::init resolved this class")
            });
        )*

        pub fn init(ruby: &Ruby) -> Result<(), Error> {
            $( class(ruby, $module, $name)?; )*
            Ok(())
        }
    };
}

roots! {
    PARSE = "MiniRacer"::"ParseError";
    RUNTIME = "MiniRacer"::"RuntimeError";
    TERMINATED = "MiniRacer"::"ScriptTerminatedError";
    OUT_OF_MEMORY = "MiniRacer"::"V8OutOfMemoryError";
    DISPOSED = "MiniRacer"::"ContextDisposedError";
    PLATFORM_INITIALIZED = "MiniRacer"::"PlatformAlreadyInitialized";
    INTERNAL = "MiniRacer"::"InternalError";
}

pub fn engine(ruby: &Ruby, error: EngineError) -> Error {
    match error {
        EngineError::Js(js) => {
            let root = match js.kind {
                JsErrorKind::Parse => &PARSE,
                JsErrorKind::Runtime => &RUNTIME,
                JsErrorKind::Terminated(
                    TerminationCause::Stop
                    | TerminationCause::Timeout
                    | TerminationCause::Cancelled,
                ) => &TERMINATED,
                JsErrorKind::Terminated(
                    TerminationCause::MaxMemory | TerminationCause::HeapExhausted,
                ) => &OUT_OF_MEMORY,
            };
            Error::new(root.get(ruby), js.message)
        }
        EngineError::Disposed => Error::new(DISPOSED.get(ruby), "disposed context"),
        EngineError::Orphaned => Error::new(RUNTIME.get(ruby), EngineError::Orphaned.to_string()),
        EngineError::Busy => Error::new(INTERNAL.get(ruby), EngineError::Busy.to_string()),
        EngineError::DisposeBusy => {
            Error::new(RUNTIME.get(ruby), EngineError::DisposeBusy.to_string())
        }
        EngineError::Internal(message) => Error::new(INTERNAL.get(ruby), message),
    }
}

pub fn flag(ruby: &Ruby, error: FlagError) -> Error {
    match error {
        FlagError::Nul | FlagError::TooLong => {
            Error::new(ruby.exception_arg_error(), error.to_string())
        }
        FlagError::AlreadyInitialized => {
            Error::new(PLATFORM_INITIALIZED.get(ruby), error.to_string())
        }
    }
}

pub fn runtime(ruby: &Ruby, message: String) -> Error {
    Error::new(RUNTIME.get(ruby), message)
}

pub fn internal(ruby: &Ruby, message: String) -> Error {
    Error::new(INTERNAL.get(ruby), message)
}
