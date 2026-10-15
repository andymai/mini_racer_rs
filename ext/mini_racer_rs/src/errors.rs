use magnus::{Error, ExceptionClass, Module, RModule, Ruby};
use mini_racer_engine::{EngineError, FlagError, JsErrorKind};

use crate::gc_root::StaticRoot;

fn class(ruby: &Ruby, module: &str, name: &str) -> Result<ExceptionClass, Error> {
    let module: RModule = ruby.class_object().const_get(module)?;
    module.const_get(name)
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
    DISPOSED = "MiniRacer"::"ContextDisposedError";
    PLATFORM_INITIALIZED = "MiniRacer"::"PlatformAlreadyInitialized";
    INTERNAL = "MiniRacerRs"::"InternalError";
}

pub fn engine(ruby: &Ruby, error: EngineError) -> Error {
    match error {
        EngineError::Js(js) => {
            let root = match js.kind {
                JsErrorKind::Parse => &PARSE,
                JsErrorKind::Runtime => &RUNTIME,
            };
            Error::new(root.get(ruby), js.message)
        }
        EngineError::Disposed => Error::new(DISPOSED.get(ruby), "disposed context"),
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
