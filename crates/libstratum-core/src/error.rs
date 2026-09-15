use thiserror::Error;

/// Errors returned by plugin implementations.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SpiError {
    #[error("not implemented yet: {0}")]
    Unimplemented(&'static str),
    #[error("malformed input: {0}")]
    Malformed(String),
    #[error("I/O error: {0}")]
    Io(String),
    #[error("limit exceeded: {0}")]
    LimitExceeded(&'static str),
}

/// Errors from [`crate::Engine::open`]. Missing or mismatched debug info is
/// *not* an error; it becomes a diagnostic on the session.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum OpenError {
    #[error("no registered format recognizes this input")]
    UnknownFormat,
    #[error("cannot read input: {0}")]
    Read(#[source] SpiError),
    #[error("{format}: {source}")]
    Format {
        format: &'static str,
        #[source]
        source: SpiError,
    },
    #[error("internal error: {0}")]
    Internal(String),
}

/// Errors from queries. An empty answer is `Ok` with a reason, not an error.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum QueryError {
    #[error("cancelled")]
    Cancelled,
    #[error("limit exceeded: {0}")]
    LimitExceeded(&'static str),
    #[error("not implemented yet: {0}")]
    Unimplemented(&'static str),
    #[error("internal error: {0}")]
    Internal(String),
}
