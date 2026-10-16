use magnus::value::ReprValue;
use magnus::{Class, Error, ExceptionClass, Module, Object, RModule, RObject, Ruby, Value};
use mini_racer_engine::{EngineError, FlagError, JsErrorKind, TerminationCause};

use crate::export;
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
            SCRIPT.get(ruby).define_method("cause", export!(method cause, 0))?;
            Ok(())
        }
    };
}

roots! {
    PARSE = "MiniRacer"::"ParseError";
    RUNTIME = "MiniRacer"::"RuntimeError";
    SCRIPT = "MiniRacer"::"ScriptError";
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
        EngineError::Disposed => raised(ruby, DISPOSED.get(ruby), "disposed context"),
        EngineError::Orphaned | EngineError::DisposeBusy => {
            raised(ruby, RUNTIME.get(ruby), &error.to_string())
        }
        EngineError::Busy => raised(ruby, INTERNAL.get(ruby), &error.to_string()),
        EngineError::Internal(message) => raised(ruby, INTERNAL.get(ruby), &message),
    }
}

pub fn flag(ruby: &Ruby, error: FlagError) -> Error {
    match error {
        FlagError::Nul | FlagError::TooLong => {
            raised(ruby, ruby.exception_arg_error(), &error.to_string())
        }
        FlagError::AlreadyInitialized => {
            raised(ruby, PLATFORM_INITIALIZED.get(ruby), &error.to_string())
        }
    }
}

// A JavaScript error object decoded into Ruby keeps its cause here, as in
// mini_racer, rather than in the cause Ruby gives a raised exception.
fn cause(_ruby: &Ruby, rb_self: RObject) -> Result<Value, Error> {
    rb_self.ivar_get("@cause")
}

pub fn script_error(ruby: &Ruby) -> ExceptionClass {
    SCRIPT.get(ruby)
}

// mini_racer raises these with rb_raise from C strings, so their messages are
// binary strings; only messages that come from JavaScript are UTF-8.
pub fn raised(ruby: &Ruby, class: ExceptionClass, message: &str) -> Error {
    class
        .new_instance((ruby.str_from_slice(message.as_bytes()),))
        .map_or_else(|error| error, Error::from)
}

// mini_racer's V8 refuses an odd-length UTF-16LE string in a request with this
// JavaScript error.
pub fn undeserializable(ruby: &Ruby) -> Error {
    Error::new(
        INTERNAL.get(ruby),
        "Error: Unable to deserialize cloned data.",
    )
}

pub fn runtime(ruby: &Ruby, message: &str) -> Error {
    raised(ruby, RUNTIME.get(ruby), message)
}

pub fn internal(ruby: &Ruby, message: &str) -> Error {
    raised(ruby, INTERNAL.get(ruby), message)
}
