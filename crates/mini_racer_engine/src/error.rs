pub use mini_racer_sync::TerminationCause;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FlagError {
    #[error("flag contains NUL byte")]
    Nul,
    #[error("flag too long")]
    TooLong,
    #[error("platform already initialized")]
    AlreadyInitialized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsErrorKind {
    Parse,
    Runtime,
    /// V8 could not read the request itself.
    Internal,
    Terminated(TerminationCause),
    /// The exception escaping the dispatch is the one V8 threw for a raising
    /// callback, so the caller raises the Ruby exception it kept instead.
    HostException,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct JsError {
    pub kind: JsErrorKind,
    pub message: String,
}

impl JsError {
    pub(crate) fn terminated(cause: TerminationCause) -> Self {
        let message = match cause {
            TerminationCause::MaxMemory | TerminationCause::HeapExhausted => "out of memory",
            TerminationCause::Stop | TerminationCause::Timeout | TerminationCause::Cancelled => {
                "terminated"
            }
        };
        Self {
            kind: JsErrorKind::Terminated(cause),
            message: message.to_owned(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(transparent)]
    Js(#[from] JsError),
    #[error("context disposed")]
    Disposed,
    #[error("single-threaded runner: {}", ebusy())]
    Orphaned,
    #[error("context busy: another thread is using it")]
    Busy,
    #[error("context dispose: {}", ebusy())]
    DisposeBusy,
    /// `mini_racer`'s text for a snapshot that could not be built.
    #[error("{0}")]
    Snapshot(String),
    #[error("internal error: {0}")]
    Internal(String),
}

fn ebusy() -> String {
    let message = std::io::Error::from_raw_os_error(libc::EBUSY).to_string();
    match message.rsplit_once(" (os error ") {
        Some((text, _)) => text.to_owned(),
        None => message,
    }
}
