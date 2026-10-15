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
    #[error("internal error: {0}")]
    Internal(String),
}
