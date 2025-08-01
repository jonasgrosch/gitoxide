//! Error types for upload-pack operations

use std::path::PathBuf;

/// Result type alias for upload-pack operations
pub type Result<T> = std::result::Result<T, Error>;

/// Comprehensive error type for upload-pack operations
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Repository access error
    #[error("Repository error: {0}")]
    Repository(#[from] gix::open::Error),

    /// Object database error
    #[error("Object database error: {0}")]
    Odb(String),

    /// Reference error
    #[error("Reference error: {0}")]
    Reference(String),

    /// Pack generation error
    #[error("Pack generation error: {0}")]
    Pack(String),

    /// Protocol error
    #[error("Protocol error: {0}")]
    Protocol(#[from] gix_protocol::handshake::Error),

    /// Protocol parsing error
    #[error("Protocol parsing error: {0}")]
    ProtocolParsing(String),

    /// Transport error
    #[error("Transport error: {0}")]
    Transport(#[from] gix_transport::client::Error),

    /// Packetline error
    #[error("Packetline error: {0}")]
    Packetline(#[from] gix_packetline::encode::Error),

    /// Packetline decode error
    #[error("Packetline decode error: {0}")]
    PacketlineDecode(#[from] gix_packetline::decode::Error),

    /// I/O error
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Reference packed buffer error
    #[error("Reference packed buffer error: {0}")]
    RefPackedBuffer(#[from] gix_ref::packed::buffer::open::Error),

    /// Reference iterator error
    #[error("Reference iterator error: {0}")]
    RefIterInit(#[from] gix::reference::iter::init::Error),

    /// Generic boxed error
    #[error("Generic error: {0}")]
    Boxed(#[from] Box<dyn std::error::Error + Send + Sync>),

    /// Object commit error
    #[error("Object commit error: {0}")]
    ObjectCommit(#[from] gix::object::commit::Error),

    /// Object decode error
    #[error("Object decode error: {0}")]
    ObjectDecode(#[from] gix_object::decode::Error),

    /// Revision walk error
    #[error("Revision walk error: {0}")]
    RevisionWalk(#[from] gix::revision::walk::Error),

    /// Invalid object ID
    #[error("Invalid object ID: {oid}")]
    InvalidObjectId { oid: String },

    /// Object not found
    #[error("Object not found: {oid}")]
    ObjectNotFound { oid: gix_hash::ObjectId },

    /// Invalid reference
    #[error("Invalid reference: {name}")]
    InvalidReference { name: String },

    /// Reference not found
    #[error("Reference not found: {name}")]
    ReferenceNotFound { name: String },

    /// Capability not supported
    #[error("Capability not supported: {capability}")]
    UnsupportedCapability { capability: String },

    /// Unsupported command
    #[error("Unsupported command: {command}")]
    UnsupportedCommand { command: String },

    /// Unsupported object format
    #[error("Unsupported object format: {format}")]
    UnsupportedObjectFormat { format: String },

    /// Capability mismatch between client and server
    #[error("Capability mismatch: {message}")]
    CapabilityMismatch { message: String },

    /// Invalid filter specification
    #[error("Invalid filter: {message}")]
    InvalidFilter { message: String },

    /// Invalid protocol version
    #[error("Invalid protocol version: {version}")]
    InvalidProtocolVersion { version: u8 },

    /// Shallow operation error
    #[error("Shallow operation error: {message}")]
    Shallow { message: String },

    /// Filter operation error
    #[error("Filter operation error: {message}")]
    Filter { message: String },

    /// Configuration error
    #[error("Configuration error: {message}")]
    Config { message: String },

    /// Hook execution error
    #[error("Hook execution failed: {hook} at {path}")]
    Hook { hook: String, path: PathBuf },

    /// Permission denied
    #[error("Permission denied: {message}")]
    PermissionDenied { message: String },

    /// Repository format not supported
    #[error("Repository format version {version} not supported")]
    UnsupportedRepositoryFormat { version: u32 },

    /// Custom error for extensibility
    #[error("Custom error: {message}")]
    Custom { message: String },

    /// Path error
    #[error("Path error: {0}")]
    Path(#[from] gix::path::relative_path::Error),
}

impl Error {
    /// Create a custom error with a message
    pub fn custom(message: impl Into<String>) -> Self {
        Self::Custom {
            message: message.into(),
        }
    }

    /// Check if this error indicates the client should retry
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Io(_) | Self::Transport(_) | Self::Packetline(_)
        )
    }

    /// Check if this error should be reported to the client
    pub fn is_client_error(&self) -> bool {
        matches!(
            self,
            Self::InvalidObjectId { .. }
                | Self::ObjectNotFound { .. }
                | Self::InvalidReference { .. }
                | Self::ReferenceNotFound { .. }
                | Self::UnsupportedCapability { .. }
                | Self::InvalidProtocolVersion { .. }
                | Self::Shallow { .. }
                | Self::Filter { .. }
        )
    }
}
