//! Service trait implemented by server-side protocol handlers.

use crate::protocol::ServerRequest;

/// The error type used by services in this crate.
///
/// Keep this minimal in core. Service crates can wrap/map their own error types
/// into this if needed by the orchestrator.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A generic I/O error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// Protocol-level error in server handling.
    #[error("protocol error: {0}")]
    Protocol(String),
    /// Input validation error.
    #[error("validation error: {0}")]
    Validation(String),
    /// An internal error - implementation detail.
    #[error("internal error: {0}")]
    Internal(String),
}

/// Server-side service handling a single request.
pub trait Service<R, W> {
    /// Handle a single request.
    fn handle(&mut self, req: ServerRequest<'_, R, W>) -> Result<(), Error>;
}


