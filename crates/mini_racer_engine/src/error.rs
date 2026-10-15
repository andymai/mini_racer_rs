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
    Terminated,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct JsError {
    pub kind: JsErrorKind,
    pub message: String,
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(transparent)]
    Js(#[from] JsError),
    #[error("context disposed")]
    Disposed,
    #[error("single-threaded runner: {}", ebusy())]
    Orphaned,
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
