mod context;
mod control;
mod counters;
mod error;
mod fast_calls;
mod notify;
pub mod process;
mod request;
mod runner;
mod serialize;
mod snapshot;
mod spin;
#[allow(
    unsafe_code,
    reason = "the engine's only FFI fence: symbols the linker routes into V8"
)]
mod sys;
mod text;

pub use context::{ContextHandle, Host, ScriptOptions, Unparker, WaitHandle, WaitOutcome};
pub use control::{CallRequest, CallbackReply, ContextConfig, JsText};
pub use counters::{Counts, Terminations};
pub use error::{
    EngineError, FlagError, HeapLimitError, InvalidSnapshot, JsError, JsErrorKind, SourceMapError,
    TerminationCause,
};
pub use snapshot::{SnapshotBlob, WeakSnapshotBlob};
pub use text::{Decoded, Transcoder};
