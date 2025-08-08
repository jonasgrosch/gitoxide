//! Comprehensive error handling for pack ingestion operations.
//!
//! This module provides detailed error classification, context preservation,
//! user-facing error mapping, and recovery strategies for pack ingestion failures.

use std::collections::HashMap;
use std::time::Duration;

use gix_hash::ObjectId;

/// Stable high-level error classification for programmatic handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// I/O errors from filesystem or network operations
    Io,
    /// Protocol-level errors (malformed packets, invalid commands)
    Protocol,
    /// Validation errors (fsck failures, invalid objects)
    Validation,
    /// Resource limit exceeded (size, time, memory)
    Resource,
    /// Operation was cancelled or interrupted
    Cancelled,
    /// Permission denied or access control failure
    Permission,
    /// Requested resource not found
    NotFound,
    /// Internal bug or unexpected condition
    Bug,
    /// Other unclassified errors
    Other,
}

impl ErrorKind {
    /// Returns true if this error kind typically allows for recovery attempts.
    pub fn is_recoverable(self) -> bool {
        match self {
            ErrorKind::Io | ErrorKind::Resource | ErrorKind::Cancelled => true,
            ErrorKind::Protocol
            | ErrorKind::Validation
            | ErrorKind::Permission
            | ErrorKind::NotFound
            | ErrorKind::Bug
            | ErrorKind::Other => false,
        }
    }

    /// Returns true if this error kind indicates a temporary condition.
    pub fn is_temporary(self) -> bool {
        match self {
            ErrorKind::Io | ErrorKind::Resource | ErrorKind::Cancelled => true,
            ErrorKind::Protocol
            | ErrorKind::Validation
            | ErrorKind::Permission
            | ErrorKind::NotFound
            | ErrorKind::Bug
            | ErrorKind::Other => false,
        }
    }

    /// Returns the suggested retry strategy for this error kind.
    pub fn retry_strategy(self) -> RetryStrategy {
        match self {
            ErrorKind::Io => RetryStrategy::ExponentialBackoff { max_attempts: 3 },
            ErrorKind::Resource => RetryStrategy::LinearBackoff { max_attempts: 2 },
            ErrorKind::Cancelled => RetryStrategy::Immediate { max_attempts: 1 },
            _ => RetryStrategy::None,
        }
    }
}

/// Retry strategy for error recovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryStrategy {
    /// No retry should be attempted
    None,
    /// Retry immediately up to max_attempts times
    Immediate { max_attempts: u32 },
    /// Retry with linear backoff (1s, 2s, 3s, ...)
    LinearBackoff { max_attempts: u32 },
    /// Retry with exponential backoff (1s, 2s, 4s, 8s, ...)
    ExponentialBackoff { max_attempts: u32 },
}

/// Context information for error reporting and debugging.
#[derive(Debug, Clone)]
pub struct ErrorContext {
    /// Operation that was being performed when the error occurred
    pub operation: String,
    /// Additional context key-value pairs
    pub context: HashMap<String, String>,
    /// Object ID related to the error, if applicable
    pub object_id: Option<ObjectId>,
    /// Pack size being processed, if applicable
    pub pack_size: Option<u64>,
    /// Time elapsed when error occurred
    pub elapsed: Option<Duration>,
}

impl ErrorContext {
    /// Create a new error context for the given operation.
    pub fn new(operation: impl Into<String>) -> Self {
        Self {
            operation: operation.into(),
            context: HashMap::new(),
            object_id: None,
            pack_size: None,
            elapsed: None,
        }
    }

    /// Add a context key-value pair.
    pub fn with_context(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.context.insert(key.into(), value.into());
        self
    }

    /// Set the object ID related to this error.
    pub fn with_object_id(mut self, object_id: ObjectId) -> Self {
        self.object_id = Some(object_id);
        self
    }

    /// Set the pack size being processed.
    pub fn with_pack_size(mut self, pack_size: u64) -> Self {
        self.pack_size = Some(pack_size);
        self
    }

    /// Set the elapsed time when the error occurred.
    pub fn with_elapsed(mut self, elapsed: Duration) -> Self {
        self.elapsed = Some(elapsed);
        self
    }
}

/// Comprehensive error type for pack ingestion operations.
#[derive(Debug, thiserror::Error)]
pub enum PackIngestionError {
    /// Pack parsing errors from gix-pack
    #[error("pack parsing failed: {message}")]
    PackParsing {
        message: String,
        context: ErrorContext,
        source_message: Option<String>,
    },

    /// Quarantine setup or management errors
    #[error("quarantine operation failed: {message}")]
    QuarantineOperation {
        message: String,
        context: ErrorContext,
        source_message: Option<String>,
    },

    /// Object validation errors from fsck
    #[error("object validation failed: {message}")]
    ObjectValidation {
        message: String,
        context: ErrorContext,
        object_id: Option<ObjectId>,
        validation_errors: Vec<String>,
        source_message: Option<String>,
    },

    /// Resource limit exceeded errors
    #[error("resource limit exceeded: {limit_type} ({current} > {limit})")]
    ResourceLimitExceeded {
        limit_type: String,
        current: u64,
        limit: u64,
        context: ErrorContext,
    },

    /// Thin pack resolution errors
    #[error("thin pack resolution failed: missing base object {object_id}")]
    ThinPackResolution {
        object_id: ObjectId,
        context: ErrorContext,
        missing_objects: Vec<ObjectId>,
    },

    /// Index pack operation errors
    #[error("index-pack operation failed: {message}")]
    IndexPackOperation {
        message: String,
        context: ErrorContext,
        source_message: Option<String>,
    },

    /// Unpack objects operation errors
    #[error("unpack-objects operation failed: {message}")]
    UnpackObjectsOperation {
        message: String,
        context: ErrorContext,
        source_message: Option<String>,
    },

    /// Object database errors
    #[error("object database error: {message}")]
    ObjectDatabase {
        message: String,
        context: ErrorContext,
        source_message: Option<String>,
    },

    /// Progress reporting errors
    #[error("progress reporting failed: {message}")]
    ProgressReporting {
        message: String,
        context: ErrorContext,
        source_message: Option<String>,
    },

    /// Sideband communication errors
    #[error("sideband communication failed: {message}")]
    SidebandCommunication {
        message: String,
        context: ErrorContext,
        source_message: Option<String>,
    },

    /// Configuration errors
    #[error("configuration error: {message}")]
    Configuration { message: String, context: ErrorContext },

    /// I/O errors
    #[error("I/O error: {message}")]
    Io {
        message: String,
        context: ErrorContext,
        source_message: String,
    },

    /// Operation cancelled or interrupted
    #[error("operation cancelled: {message}")]
    Cancelled { message: String, context: ErrorContext },

    /// Multiple errors occurred during batch operations
    #[error("multiple errors occurred: {}", .errors.len())]
    Multiple {
        errors: Vec<PackIngestionError>,
        context: ErrorContext,
    },
}

impl PackIngestionError {
    /// Get the stable error kind for programmatic handling.
    pub fn kind(&self) -> ErrorKind {
        match self {
            PackIngestionError::PackParsing { .. } => ErrorKind::Protocol,
            PackIngestionError::QuarantineOperation { .. } => ErrorKind::Io,
            PackIngestionError::ObjectValidation { .. } => ErrorKind::Validation,
            PackIngestionError::ResourceLimitExceeded { .. } => ErrorKind::Resource,
            PackIngestionError::ThinPackResolution { .. } => ErrorKind::Validation,
            PackIngestionError::IndexPackOperation { .. } => ErrorKind::Protocol,
            PackIngestionError::UnpackObjectsOperation { .. } => ErrorKind::Protocol,
            PackIngestionError::ObjectDatabase { .. } => ErrorKind::Io,
            PackIngestionError::ProgressReporting { .. } => ErrorKind::Other,
            PackIngestionError::SidebandCommunication { .. } => ErrorKind::Io,
            PackIngestionError::Configuration { .. } => ErrorKind::Other,
            PackIngestionError::Io { .. } => ErrorKind::Io,
            PackIngestionError::Cancelled { .. } => ErrorKind::Cancelled,
            PackIngestionError::Multiple { errors, .. } => {
                // Return the most severe error kind from the collection
                errors
                    .iter()
                    .map(|e| e.kind())
                    .max_by_key(|k| match k {
                        ErrorKind::Bug => 7,
                        ErrorKind::Validation => 6,
                        ErrorKind::Protocol => 5,
                        ErrorKind::Permission => 4,
                        ErrorKind::Resource => 3,
                        ErrorKind::Io => 2,
                        ErrorKind::Cancelled => 1,
                        _ => 0,
                    })
                    .unwrap_or(ErrorKind::Other)
            }
        }
    }

    /// Check if this error is recoverable.
    pub fn is_recoverable(&self) -> bool {
        self.kind().is_recoverable()
    }

    /// Check if this error indicates a temporary condition.
    pub fn is_temporary(&self) -> bool {
        self.kind().is_temporary()
    }

    /// Get the suggested retry strategy for this error.
    pub fn retry_strategy(&self) -> RetryStrategy {
        self.kind().retry_strategy()
    }

    /// Get the error context.
    pub fn context(&self) -> &ErrorContext {
        match self {
            PackIngestionError::PackParsing { context, .. } => context,
            PackIngestionError::QuarantineOperation { context, .. } => context,
            PackIngestionError::ObjectValidation { context, .. } => context,
            PackIngestionError::ResourceLimitExceeded { context, .. } => context,
            PackIngestionError::ThinPackResolution { context, .. } => context,
            PackIngestionError::IndexPackOperation { context, .. } => context,
            PackIngestionError::UnpackObjectsOperation { context, .. } => context,
            PackIngestionError::ObjectDatabase { context, .. } => context,
            PackIngestionError::ProgressReporting { context, .. } => context,
            PackIngestionError::SidebandCommunication { context, .. } => context,
            PackIngestionError::Configuration { context, .. } => context,
            PackIngestionError::Io { context, .. } => context,
            PackIngestionError::Cancelled { context, .. } => context,
            PackIngestionError::Multiple { context, .. } => context,
        }
    }

    /// Generate a user-facing error message with actionable information.
    pub fn user_message(&self) -> String {
        match self {
            PackIngestionError::PackParsing { message, context, .. } => {
                let mut msg = format!("Failed to parse incoming pack data: {}", message);
                if let Some(pack_size) = context.pack_size {
                    msg.push_str(&format!(" (pack size: {} bytes)", pack_size));
                }
                msg.push_str("\n\nThis usually indicates corrupted or malformed pack data. Please try pushing again.");
                msg
            }
            PackIngestionError::QuarantineOperation { message, .. } => {
                format!("Failed to set up quarantine environment: {}\n\nThis may indicate insufficient disk space or permissions. Please check your repository's disk usage and permissions.", message)
            }
            PackIngestionError::ObjectValidation {
                message,
                object_id,
                validation_errors,
                ..
            } => {
                let mut msg = format!("Object validation failed: {}", message);
                if let Some(oid) = object_id {
                    msg.push_str(&format!(" (object: {})", oid));
                }
                if !validation_errors.is_empty() {
                    msg.push_str("\n\nValidation errors:");
                    for error in validation_errors {
                        msg.push_str(&format!("\n  - {}", error));
                    }
                }
                msg.push_str("\n\nPlease check your objects for corruption and try again.");
                msg
            }
            PackIngestionError::ResourceLimitExceeded {
                limit_type,
                current,
                limit,
                ..
            } => {
                format!("Resource limit exceeded: {} ({} > {})\n\nThe operation was rejected because it would exceed configured limits. Please contact your administrator if you need higher limits.", limit_type, current, limit)
            }
            PackIngestionError::ThinPackResolution {
                object_id,
                missing_objects,
                ..
            } => {
                let mut msg = format!("Failed to resolve thin pack: missing base object {}", object_id);
                if missing_objects.len() > 1 {
                    msg.push_str(&format!(" (and {} other objects)", missing_objects.len() - 1));
                }
                msg.push_str("\n\nThis usually happens when pushing changes that depend on objects not present in the repository. Try pushing with --no-thin or ensure all required objects are available.");
                msg
            }
            PackIngestionError::IndexPackOperation { message, .. } => {
                format!("Pack indexing failed: {}\n\nThis may indicate corrupted pack data or insufficient resources. Please try again.", message)
            }
            PackIngestionError::UnpackObjectsOperation { message, .. } => {
                format!("Object unpacking failed: {}\n\nThis may indicate corrupted pack data or insufficient disk space. Please check available disk space and try again.", message)
            }
            PackIngestionError::ObjectDatabase { message, .. } => {
                format!("Object database error: {}\n\nThis may indicate repository corruption or I/O issues. Please check your repository integrity.", message)
            }
            PackIngestionError::ProgressReporting { message, .. } => {
                format!(
                    "Progress reporting failed: {}\n\nThe operation may have succeeded despite this error.",
                    message
                )
            }
            PackIngestionError::SidebandCommunication { message, .. } => {
                format!("Communication error: {}\n\nThere was a problem sending progress updates. The operation may have succeeded.", message)
            }
            PackIngestionError::Configuration { message, .. } => {
                format!(
                    "Configuration error: {}\n\nPlease check your repository configuration and try again.",
                    message
                )
            }
            PackIngestionError::Io { message, .. } => {
                format!("I/O error: {}\n\nThis may indicate disk space issues, permission problems, or network connectivity issues. Please check your system resources and try again.", message)
            }
            PackIngestionError::Cancelled { message, .. } => {
                format!(
                    "Operation cancelled: {}\n\nThe operation was interrupted and can be safely retried.",
                    message
                )
            }
            PackIngestionError::Multiple { errors, .. } => {
                let mut msg = format!("Multiple errors occurred ({} total):\n", errors.len());
                for (i, error) in errors.iter().take(3).enumerate() {
                    msg.push_str(&format!("{}. {}\n", i + 1, error));
                }
                if errors.len() > 3 {
                    msg.push_str(&format!("... and {} more errors\n", errors.len() - 3));
                }
                msg.push_str("\nPlease address these issues and try again.");
                msg
            }
        }
    }

    /// Generate a technical error message for logging and debugging.
    pub fn technical_message(&self) -> String {
        let mut msg = format!("Error in {}: {}", self.context().operation, self);

        // Add context information
        let ctx = self.context();
        if !ctx.context.is_empty() {
            msg.push_str("\nContext:");
            for (key, value) in &ctx.context {
                msg.push_str(&format!("\n  {}: {}", key, value));
            }
        }

        if let Some(object_id) = ctx.object_id {
            msg.push_str(&format!("\nObject ID: {}", object_id));
        }

        if let Some(pack_size) = ctx.pack_size {
            msg.push_str(&format!("\nPack size: {} bytes", pack_size));
        }

        if let Some(elapsed) = ctx.elapsed {
            msg.push_str(&format!("\nElapsed time: {:?}", elapsed));
        }

        // Add source information if available
        let source_msg = match self {
            PackIngestionError::PackParsing { source_message, .. } => source_message.as_ref(),
            PackIngestionError::QuarantineOperation { source_message, .. } => source_message.as_ref(),
            PackIngestionError::ObjectValidation { source_message, .. } => source_message.as_ref(),
            PackIngestionError::IndexPackOperation { source_message, .. } => source_message.as_ref(),
            PackIngestionError::UnpackObjectsOperation { source_message, .. } => source_message.as_ref(),
            PackIngestionError::ObjectDatabase { source_message, .. } => source_message.as_ref(),
            PackIngestionError::ProgressReporting { source_message, .. } => source_message.as_ref(),
            PackIngestionError::SidebandCommunication { source_message, .. } => source_message.as_ref(),
            PackIngestionError::Io { source_message, .. } => Some(source_message),
            _ => None,
        };

        if let Some(source) = source_msg {
            msg.push_str(&format!("\nCaused by: {}", source));
        }

        msg
    }

    /// Create a pack parsing error with context.
    pub fn pack_parsing(
        message: impl Into<String>,
        context: ErrorContext,
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        Self::PackParsing {
            message: message.into(),
            context,
            source_message: source.map(|e| e.to_string()),
        }
    }

    /// Create a quarantine operation error with context.
    pub fn quarantine_operation(
        message: impl Into<String>,
        context: ErrorContext,
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        Self::QuarantineOperation {
            message: message.into(),
            context,
            source_message: source.map(|e| e.to_string()),
        }
    }

    /// Create an object validation error with context.
    pub fn object_validation(
        message: impl Into<String>,
        context: ErrorContext,
        object_id: Option<ObjectId>,
        validation_errors: Vec<String>,
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        Self::ObjectValidation {
            message: message.into(),
            context,
            object_id,
            validation_errors,
            source_message: source.map(|e| e.to_string()),
        }
    }

    /// Create a resource limit exceeded error.
    pub fn resource_limit_exceeded(
        limit_type: impl Into<String>,
        current: u64,
        limit: u64,
        context: ErrorContext,
    ) -> Self {
        Self::ResourceLimitExceeded {
            limit_type: limit_type.into(),
            current,
            limit,
            context,
        }
    }

    /// Create a thin pack resolution error.
    pub fn thin_pack_resolution(object_id: ObjectId, context: ErrorContext, missing_objects: Vec<ObjectId>) -> Self {
        Self::ThinPackResolution {
            object_id,
            context,
            missing_objects,
        }
    }

    /// Create an I/O error with context.
    pub fn io(message: impl Into<String>, context: ErrorContext, source: std::io::Error) -> Self {
        Self::Io {
            message: message.into(),
            context,
            source_message: source.to_string(),
        }
    }

    /// Create a cancelled operation error.
    pub fn cancelled(message: impl Into<String>, context: ErrorContext) -> Self {
        Self::Cancelled {
            message: message.into(),
            context,
        }
    }

    /// Create an index pack operation error.
    pub fn index_pack_operation(
        message: impl Into<String>,
        context: ErrorContext,
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        Self::IndexPackOperation {
            message: message.into(),
            context,
            source_message: source.map(|e| e.to_string()),
        }
    }

    /// Create an unpack objects operation error.
    pub fn unpack_objects_operation(
        message: impl Into<String>,
        context: ErrorContext,
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        Self::UnpackObjectsOperation {
            message: message.into(),
            context,
            source_message: source.map(|e| e.to_string()),
        }
    }

    /// Create an object database error.
    pub fn object_database(
        message: impl Into<String>,
        context: ErrorContext,
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        Self::ObjectDatabase {
            message: message.into(),
            context,
            source_message: source.map(|e| e.to_string()),
        }
    }

    /// Create a progress reporting error.
    pub fn progress_reporting(
        message: impl Into<String>,
        context: ErrorContext,
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        Self::ProgressReporting {
            message: message.into(),
            context,
            source_message: source.map(|e| e.to_string()),
        }
    }

    /// Create a sideband communication error.
    pub fn sideband_communication(
        message: impl Into<String>,
        context: ErrorContext,
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        Self::SidebandCommunication {
            message: message.into(),
            context,
            source_message: source.map(|e| e.to_string()),
        }
    }

    /// Create a configuration error.
    pub fn configuration(message: impl Into<String>, context: ErrorContext) -> Self {
        Self::Configuration {
            message: message.into(),
            context,
        }
    }
}

/// Error recovery strategies for different failure modes.
#[derive(Debug, Clone)]
pub struct ErrorRecovery {
    /// The error kind that occurred
    pub error_kind: ErrorKind,
    /// Suggested recovery actions
    pub recovery_actions: Vec<RecoveryAction>,
    /// Whether automatic recovery should be attempted
    pub auto_recovery: bool,
}

/// Specific recovery actions that can be taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryAction {
    /// Retry the operation with the same parameters
    Retry,
    /// Retry with different strategy (e.g., switch from index-pack to unpack-objects)
    RetryWithFallback,
    /// Clean up quarantine and retry
    CleanupAndRetry,
    /// Reduce resource limits and retry
    ReduceLimitsAndRetry,
    /// Skip validation and retry
    SkipValidationAndRetry,
    /// Manual intervention required
    ManualIntervention,
}

impl ErrorRecovery {
    /// Create a recovery strategy for the given error.
    pub fn for_error(error: &PackIngestionError) -> Self {
        let error_kind = error.kind();
        let recovery_actions = match error_kind {
            ErrorKind::Io => vec![RecoveryAction::Retry, RecoveryAction::CleanupAndRetry],
            ErrorKind::Resource => vec![RecoveryAction::ReduceLimitsAndRetry, RecoveryAction::RetryWithFallback],
            ErrorKind::Cancelled => vec![RecoveryAction::Retry],
            ErrorKind::Protocol => vec![RecoveryAction::RetryWithFallback],
            ErrorKind::Validation => vec![
                RecoveryAction::SkipValidationAndRetry,
                RecoveryAction::ManualIntervention,
            ],
            _ => vec![RecoveryAction::ManualIntervention],
        };

        let auto_recovery = error_kind.is_recoverable()
            && !matches!(recovery_actions.first(), Some(RecoveryAction::ManualIntervention));

        Self {
            error_kind,
            recovery_actions,
            auto_recovery,
        }
    }

    /// Check if automatic recovery should be attempted.
    pub fn should_auto_recover(&self) -> bool {
        self.auto_recovery && !self.recovery_actions.is_empty()
    }

    /// Get the next recovery action to try.
    pub fn next_action(&self) -> Option<&RecoveryAction> {
        self.recovery_actions.first()
    }
}

/// Result type alias for pack ingestion operations.
pub type Result<T> = std::result::Result<T, PackIngestionError>;
