//! Server-side protocol types and request envelope.

/// The kind of server-side service to handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceKind {
    /// Upload-pack: fetch/clone.
    UploadPack,
    /// Receive-pack: push.
    ReceivePack,
}

/// Supported Git protocol versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolVersion {
    /// v0 smart protocol
    V0,
    /// v1 smart protocol
    V1,
    /// v2 command-based protocol
    V2,
}

/// A server request encapsulating the context and I/O streams.
pub struct ServerRequest<'a, R, W> {
    /// Which service to invoke.
    pub kind: ServiceKind,
    /// Which protocol version is negotiated/selected.
    pub version: ProtocolVersion,
    /// Repository to operate on.
    pub repo: &'a gix::Repository,
    /// Input stream.
    pub input: R,
    /// Output stream.
    pub output: W,
    /// Whether the transport is stateless (HTTP) vs stateful (SSH/git-daemon).
    pub stateless: bool,
    /// Optional trace identifier for correlation.
    pub trace_id: Option<String>,
    /// Optional cancellation flag.
    pub cancellation: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}


