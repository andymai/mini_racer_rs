pub use mini_racer_sync::TerminationCause;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FlagError {
    #[error("flag contains NUL byte")]
    Nul,
    #[error("flag too long")]
    TooLong,
    #[error("platform already initialized")]
    AlreadyInitialized,
    /// The flag sets a heap size V8 would use in place of the heap limit.
    #[error("--{0} would override the heap limit")]
    OverridesHeapLimit(&'static str),
}

/// Why `platform::set_heap_limit` refused a limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HeapLimitError {
    #[error(
        "heap limit must be from {} to {} bytes",
        crate::process::platform::MIN_HEAP_LIMIT,
        crate::process::platform::MAX_HEAP_LIMIT
    )]
    Range,
    /// A flag already set sizes the heap in its place.
    #[error("the heap limit would be overridden by --{0}")]
    Overridden(&'static str),
}

/// Why a startup snapshot's blob is refused before V8 reads it, in
/// `mini_racer`'s words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidSnapshot {
    #[error("snapshot is too large")]
    TooLarge,
    #[error("snapshot is too short")]
    TooShort,
    #[error("snapshot version mismatch")]
    Version,
    #[error("snapshot header is invalid")]
    Header,
    #[error("snapshot has no contexts")]
    NoContexts,
    #[error("snapshot context table is invalid")]
    ContextTable,
    /// A startup section V8 aborts on, which `mini_racer` hands V8.
    #[error("snapshot startup section is invalid")]
    Startup,
    #[error("snapshot read-only section is invalid")]
    ReadOnly,
    #[error("snapshot shared-heap section is invalid")]
    SharedHeap,
    #[error("snapshot context section is invalid")]
    Context,
    #[error("snapshot checksum mismatch")]
    Checksum,
}

/// Why `SourceMaps::register` refused a map.
#[derive(Debug, thiserror::Error)]
pub enum SourceMapError {
    #[error(transparent)]
    Map(#[from] mini_racer_sourcemap::MapError),
    #[error("at most {0} bundles can have a source map")]
    Full(usize),
    #[error("the bundle is shorter than {0} bytes")]
    Short(usize),
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

    /// A script that reached the context's heap limit of `limit` bytes.
    pub(crate) fn heap_limit_reached(limit: usize) -> Self {
        Self {
            kind: JsErrorKind::Terminated(TerminationCause::HeapExhausted),
            message: format!("heap limit of {limit} bytes reached"),
        }
    }

    /// Any later script in a context whose heap a collection after a hit left
    /// over its limit of `limit` bytes.
    pub(crate) fn heap_limit_exceeded(limit: usize) -> Self {
        Self {
            kind: JsErrorKind::Terminated(TerminationCause::HeapExhausted),
            message: format!(
                "heap still over its limit of {limit} bytes after a collection: \
                 this context runs no more JavaScript"
            ),
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
    #[error(transparent)]
    InvalidSnapshot(#[from] InvalidSnapshot),
    #[error("internal error: {0}")]
    Internal(String),
    /// V8's flags sized a context's heap past its heap limit.
    #[error("the heap limit is overridden by a V8 flag")]
    HeapLimitOverridden,
}

fn ebusy() -> String {
    let message = std::io::Error::from_raw_os_error(libc::EBUSY).to_string();
    match message.rsplit_once(" (os error ") {
        Some((text, _)) => text.to_owned(),
        None => message,
    }
}
