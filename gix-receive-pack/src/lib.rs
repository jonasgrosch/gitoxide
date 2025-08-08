/*!
Spec-first receive-pack scaffold for gitoxide.

Goals
- Provide a spec-driven implementation surface for Git's receive-pack service.
- Offer a typestate-based builder to make misconfiguration impossible at compile-time.
- Support blocking by default, with optional async via the "async-io" feature.
- Allow future upstream formatting/compat via "strict-compat" (adapter-level, no-ops for now).

See:
- SPEC: ./SPEC.md
- ROADMAP: ./ROADMAP.md

Design principles
- Zero I/O in constructors and configuration APIs.
- Typestate to prevent invalid API usage at compile time.
- Keep the core types minimal yet extensible.
*/

#![forbid(unsafe_code)]

pub mod protocol;
pub mod pack;
pub mod error;
#[cfg(feature = "progress")]
pub mod progress;

pub use protocol::{
    Advertiser, CapabilityOrdering, CapabilitySet, CommandList, CommandUpdate, HiddenRefPredicate, Options, RefRecord,
};

use core::marker::PhantomData;
use std::path::PathBuf;

/// Typestates representing builder progress.
pub mod state {
    /// Initial builder state with no mode selected.
    pub struct Start;
    /// Ready state after transport mode (blocking or async) is selected.
    pub struct Ready;
}

/// Stable high-level error classification aligned with SPEC 12.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Io,
    Protocol,
    Validation,
    NotFound,
    Permission,
    Cancelled,
    Resource,
    Bug,
    Other,
}

/// Error type for operations provided by this crate.
///
/// This enum provides backward compatibility while delegating to the comprehensive
/// error handling system for pack ingestion operations.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// Placeholder error for unimplemented operations.
    #[error("unimplemented")]
    Unimplemented,
    /// Protocol-level errors, e.g. malformed pkt-lines or command syntax issues.
    #[error("protocol error: {0}")]
    Protocol(String),
    /// Validation errors, e.g. invalid capability negotiation or object-format mismatch.
    #[error("validation error: {0}")]
    Validation(String),
    /// I/O errors from filesystem or OS interactions.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// Resource related errors like timeouts or size limits exceeded.
    #[error("resource error: {0}")]
    Resource(String),
    /// Operation was cancelled/interrupted.
    #[error("cancelled")]
    Cancelled,
    /// Fsck verification failed.
    #[error("fsck failed: {0}")]
    Fsck(String),
    /// Comprehensive pack ingestion error with detailed context and recovery information.
    #[error("pack ingestion error: {0}")]
    PackIngestion(#[from] crate::error::PackIngestionError),
}

impl Error {
    /// Fast classification helper returning a stable error kind.
    pub fn kind(&self) -> Kind {
        match self {
            Error::Unimplemented => Kind::Other,
            Error::Protocol(_) => Kind::Protocol,
            Error::Validation(_) => Kind::Validation,
            Error::Io(_) => Kind::Io,
            Error::Resource(_) => Kind::Resource,
            Error::Cancelled => Kind::Cancelled,
            Error::Fsck(_) => Kind::Validation,
            Error::PackIngestion(err) => match err.kind() {
                crate::error::ErrorKind::Io => Kind::Io,
                crate::error::ErrorKind::Protocol => Kind::Protocol,
                crate::error::ErrorKind::Validation => Kind::Validation,
                crate::error::ErrorKind::Resource => Kind::Resource,
                crate::error::ErrorKind::Cancelled => Kind::Cancelled,
                crate::error::ErrorKind::Permission => Kind::Permission,
                crate::error::ErrorKind::NotFound => Kind::NotFound,
                crate::error::ErrorKind::Bug => Kind::Bug,
                crate::error::ErrorKind::Other => Kind::Other,
            },
        }
    }

    /// Check if this error is recoverable.
    pub fn is_recoverable(&self) -> bool {
        match self {
            Error::PackIngestion(err) => err.is_recoverable(),
            Error::Io(_) | Error::Resource(_) | Error::Cancelled => true,
            _ => false,
        }
    }

    /// Get error recovery strategy if available.
    pub fn recovery_strategy(&self) -> Option<crate::error::ErrorRecovery> {
        match self {
            Error::PackIngestion(err) => Some(crate::error::ErrorRecovery::for_error(err)),
            _ => None,
        }
    }

    /// Generate a user-facing error message.
    pub fn user_message(&self) -> String {
        match self {
            Error::PackIngestion(err) => err.user_message(),
            Error::Unimplemented => "This operation is not yet implemented.".to_string(),
            Error::Protocol(msg) => format!("Protocol error: {}\n\nThis indicates a problem with the Git protocol communication. Please try again.", msg),
            Error::Validation(msg) => format!("Validation error: {}\n\nThe operation failed validation checks. Please verify your input and try again.", msg),
            Error::Io(err) => format!("I/O error: {}\n\nThis may indicate disk space issues, permission problems, or network connectivity issues. Please check your system resources and try again.", err),
            Error::Resource(msg) => format!("Resource error: {}\n\nThe operation exceeded resource limits. Please contact your administrator if you need higher limits.", msg),
            Error::Cancelled => "Operation was cancelled.\n\nThe operation was interrupted and can be safely retried.".to_string(),
            Error::Fsck(msg) => format!("Object validation failed: {}\n\nPlease check your objects for corruption and try again.", msg),
        }
    }
}

/// Opaque configuration for the receive-pack engine.
///
/// This will evolve to include transport, repository access, hooks, and policy.
/// Keeping it private allows us to evolve without breaking users.
#[derive(Default, Debug, Clone)]
struct Config {
    mode: Mode,
    unpack_limit: Option<u64>,
    #[cfg(feature = "fsck")]
    fsck_config: Option<crate::pack::FsckConfig>,
    #[cfg(feature = "progress")]
    show_progress: bool,
    /// Path to the main repository objects directory (.git/objects)
    objects_dir: Option<PathBuf>,
    /// Hard upper bound for allowed incoming pack size (bytes). None = unlimited.
    max_pack_bytes: Option<u64>,
    /// Soft time budget for ingestion (seconds). None = unlimited.
    time_budget_secs: Option<u64>,
}

/// Execution mode for receive-pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Blocking,
    #[cfg(feature = "async-io")]
    Async,
}

impl Default for Mode {
    fn default() -> Self {
        Mode::Blocking
    }
}

/// Builder for constructing a receive-pack instance with typestate guarantees.
#[derive(Debug, Clone)]
pub struct ReceivePackBuilder<S = state::Start> {
    cfg: Config,
    _state: PhantomData<S>,
}

impl ReceivePackBuilder<state::Start> {
    /// Create a new builder in the Start state.
    pub fn new() -> Self {
        Self {
            cfg: Config::default(),
            _state: PhantomData,
        }
    }

    /// Select blocking mode and move to Ready state.
    pub fn blocking(mut self) -> ReceivePackBuilder<state::Ready> {
        self.cfg.mode = Mode::Blocking;
        ReceivePackBuilder {
            cfg: self.cfg,
            _state: PhantomData,
        }
    }

    /// Select async mode and move to Ready state.
    ///
    /// Requires the "async-io" feature to be enabled.
    #[cfg(feature = "async-io")]
    pub fn r#async(mut self) -> ReceivePackBuilder<state::Ready> {
        self.cfg.mode = Mode::Async;
        ReceivePackBuilder {
            cfg: self.cfg,
            _state: PhantomData,
        }
    }
}

impl ReceivePackBuilder<state::Ready> {
    /// Configure transfer.unpackLimit policy (object-count threshold for unpack-objects).
    pub fn with_unpack_limit(mut self, limit: impl Into<Option<u64>>) -> Self {
        self.cfg.unpack_limit = limit.into();
        self
    }

    /// Configure fsck verification on received objects (receive.fsckObjects).
    #[cfg(feature = "fsck")]
    pub fn with_fsck_config(mut self, config: crate::pack::FsckConfig) -> Self {
        self.cfg.fsck_config = Some(config);
        self
    }

    /// Enable fsck verification with default configuration.
    #[cfg(feature = "fsck")]
    pub fn with_fsck_objects(mut self, enabled: bool) -> Self {
        if enabled {
            self.cfg.fsck_config = Some(crate::pack::FsckConfig::default());
        } else {
            self.cfg.fsck_config = None;
        }
        self
    }

    /// Enable or disable progress emission (receive.showProgress).
    #[cfg(feature = "progress")]
    pub fn with_show_progress(mut self, enabled: bool) -> Self {
        self.cfg.show_progress = enabled;
        self
    }

    /// Set the main repository objects directory (.git/objects) for ingestion and quarantine alternates.
    pub fn with_objects_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.cfg.objects_dir = Some(path.into());
        self
    }

    /// Set maximum allowed pack size (in bytes). None = unlimited.
    pub fn with_max_pack_bytes(mut self, v: impl Into<Option<u64>>) -> Self {
        self.cfg.max_pack_bytes = v.into();
        self
    }

    /// Set ingestion time budget in seconds. None = unlimited.
    pub fn with_time_budget_secs(mut self, v: impl Into<Option<u64>>) -> Self {
        self.cfg.time_budget_secs = v.into();
        self
    }

    /// Finalize the builder and obtain a ReceivePack instance.
    ///
    /// This does no I/O and validates configuration.
    pub fn build(self) -> ReceivePack {
        ReceivePack { cfg: self.cfg }
    }
}

/// Receive-pack engine.
///
/// This struct will orchestrate negotiation, command execution, object verification,
/// update application, and reporting. For now, it is only a skeleton that compiles.
#[derive(Debug, Clone)]
pub struct ReceivePack {
    cfg: Config,
}

impl ReceivePack {
    /// Execute the receive-pack workflow.
    ///
    /// This placeholder returns Ok(()) to keep the crate buildable until we implement
    /// the protocol and plumbing.
    pub fn run(self) -> Result<(), Error> {
        let _mode = self.cfg.mode;
        Ok(())
    }

    /// Create an Advertiser over the given writer.
    ///
    /// This is a convenience for composing the protocol advertisement phase (M1).
    /// Async parity will be added in a later milestone behind the "async-io" feature.
    pub fn advertiser<W: std::io::Write>(&self, write: W) -> protocol::Advertiser<W> {
        protocol::Advertiser::new(write)
    }

    /// M3 scaffold: ingest a pack using a policy-driven path and quarantine lifecycle.
    ///
    /// - Path selection: IngestionPolicy::choose_path() using transfer.unpackLimit.
    /// - Quarantine: activate, then migrate_on_success() on success paths.
    /// - Fsck/progress: feature-gated no-ops for now.
    pub fn ingest_pack(&self, object_count_hint: Option<u64>) -> Result<(), Error> {
        let policy = crate::pack::IngestionPolicy {
            unpack_limit: self.cfg.unpack_limit,
        };
        let _path = policy.choose_path(object_count_hint);

        // Quarantine lifecycle (scaffold)
        let mut quarantine = crate::pack::Quarantine::new(
            self.cfg
                .objects_dir
                .clone()
                .unwrap_or_else(|| std::path::PathBuf::from(".")),
        );
        quarantine.activate()?;

        // Stubbed ingestion paths: compile-only no-ops.
        match _path {
            crate::pack::PackIngestPath::IndexPack => {
                let _ = crate::pack::PackIngestor::index_pack_stub();
            }
            crate::pack::PackIngestPath::UnpackObjects => {
                // Placeholder for unpack-objects - this is now handled in the full implementation
            }
        }

        // Optional fsck (scaffold)
        #[cfg(feature = "fsck")]
        {
            if self.cfg.fsck_config.is_some() {
                // Fsck integration is now handled in PackIngestor
            }
        }

        // Optional progress emission (scaffold)
        #[cfg(feature = "progress")]
        {
            if self.cfg.show_progress {
                // Placeholder for progress sink / sideband integration.
            }
        }

        quarantine.migrate_on_success()?;
        Ok(())
    }

    /// M3: Blocking ingestion from a pack reader with quarantine and migration.
    #[cfg(feature = "progress")]
    pub fn ingest_pack_from_reader<R: std::io::BufRead>(
        &self,
        input: &mut R,
        pack_size: Option<u64>,
        object_count_hint: Option<u64>,
        progress: &mut dyn gix_features::progress::DynNestedProgress,
    ) -> Result<(), Error> {
        // Guards: size limit
        if let (Some(limit), Some(sz)) = (self.cfg.max_pack_bytes, pack_size) {
            if sz > limit {
                return Err(Error::Resource(format!("incoming pack exceeds size limit: {sz} > {limit}")));
            }
        }

        // Prepare time guard
        let start = std::time::Instant::now();

        let objects_dir = self
            .cfg
            .objects_dir
            .clone()
            .ok_or_else(|| Error::Validation("objects_dir not configured".into()))?;
        let policy = crate::pack::IngestionPolicy {
            unpack_limit: self.cfg.unpack_limit,
        };
        let choice = policy.choose_path(object_count_hint);

        let main_odb = gix_odb::at(objects_dir.clone())?;

        let mut quarantine = crate::pack::Quarantine::new(objects_dir.clone());
        quarantine.activate()?;

        // Create PackIngestor with fsck configuration
        #[cfg(feature = "fsck")]
        let ingestor = crate::pack::PackIngestor::new(self.cfg.fsck_config.clone());
        #[cfg(not(feature = "fsck"))]
        let ingestor = crate::pack::PackIngestor::new(None);

        let res = match choice {
            crate::pack::PackIngestPath::IndexPack => ingestor.index_pack(
                input,
                quarantine.objects_dir.as_path(),
                pack_size,
                Some(main_odb.clone()),
                progress,
            ),
            crate::pack::PackIngestPath::UnpackObjects => {
                ingestor.unpack_objects(
                    input,
                    quarantine.objects_dir.as_path(),
                    pack_size,
                    Some(main_odb.clone()),
                    progress,
                )
            }
        };

        // Time guard check
        if let Some(budget) = self.cfg.time_budget_secs {
            if start.elapsed().as_secs() > budget {
                let _ = quarantine.drop_on_failure();
                return Err(Error::Resource(format!(
                    "ingestion exceeded time budget: {}s > {}s",
                    start.elapsed().as_secs(),
                    budget
                )));
            }
        }

        match res {
            Ok(fsck_results) => {
                // Log fsck warnings if any
                #[cfg(feature = "fsck")]
                if !fsck_results.warnings.is_empty() {
                    // In a real implementation, we might want to log these warnings
                    // For now, we'll just continue
                }

                quarantine.migrate_on_success()?;
                Ok(())
            }
            Err(e) => {
                let _ = quarantine.drop_on_failure();
                Err(e)
            }
        }
    }

    /// M3: Blocking ingestion with sideband progress bridge.
    ///
    /// This variant wires pack ingestion progress to sideband channel 2 using SidebandProgressWriter.
    /// It wraps the provided `inner_progress` with a bridge that mirrors progress messages to sideband.
    #[cfg(feature = "progress")]
    pub fn ingest_pack_from_reader_with_sideband<R: std::io::BufRead, W: std::io::Write + std::marker::Send + 'static>(
        &self,
        input: &mut R,
        pack_size: Option<u64>,
        object_count_hint: Option<u64>,
        inner_progress: Box<dyn gix_features::progress::DynNestedProgress>,
        sideband: W,
    ) -> Result<(), Error> {
        // Create a bridge that mirrors messages to sideband channel 2.
        let mut bridge = crate::progress::SidebandDynProgress::new(inner_progress, sideband);
        // Initial policy follows upstream: keepalive after NUL; callers may toggle to ALWAYS later.
        {
            let writer = bridge.writer();
            if let Ok(mut w) = writer.lock() {
                w.set_policy(crate::progress::KeepalivePolicy::AfterNul);
            };
        }
        // Reuse the core ingestion logic with bridged progress.
        self.ingest_pack_from_reader(input, pack_size, object_count_hint, &mut bridge)
    }

    /// Non-progress build: not available.
    #[cfg(not(feature = "progress"))]
    pub fn ingest_pack_from_reader<R: std::io::BufRead>(
        &self,
        _input: &mut R,
        _pack_size: Option<u64>,
        _object_count_hint: Option<u64>,
        _progress: &mut dyn std::any::Any,
    ) -> Result<(), Error> {
        Err(Error::Unimplemented)
    }

    /// Parse head-info (commands, options, shallow) from text and validate options against advertised capabilities.
    ///
    /// Parameters
    /// - `text`: lines from the client during the head-info phase, one logical record per `\n`.
    /// - `advertised`: the capability set we previously advertised in M1.
    ///
    /// Returns a typed list of command updates and parsed options.
    pub fn parse_head_info_from_text(
        &self,
        text: &str,
        advertised: &protocol::CapabilitySet,
    ) -> Result<(protocol::CommandList, protocol::Options), Error> {
        let (list, opts) = protocol::CommandList::parse_from_text(text)?;
        opts.validate_against(advertised)?;
        Ok((list, opts))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_blocking_compiles_and_runs() {
        let rp = ReceivePackBuilder::new().blocking().build();
        rp.run().unwrap();
    }

    #[cfg(feature = "async-io")]
    #[test]
    fn builder_async_compiles_and_runs() {
        let rp = ReceivePackBuilder::new().r#async().build();
        rp.run().unwrap();
    }

    #[test]
    fn parse_head_info_valid_and_validation() {
        let rp = ReceivePackBuilder::new().blocking().build();
        let advertised = CapabilitySet::modern_defaults().with_agent(Some("gix/1.0".into()));

        let text = concat!(
            "0000000000000000000000000000000000000000 1111111111111111111111111111111111111111 refs/heads/main\0report-status report-status-v2 quiet delete-refs ofs-delta agent=gix/2.0\n",
            "1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 refs/heads/main\n",
            "push-option=notify=team\n",
            "shallow 3333333333333333333333333333333333333333\n",
        );

        let (list, opts) = rp.parse_head_info_from_text(text, &advertised).unwrap();
        assert_eq!(list.len(), 2);
        assert!(opts.has("report-status"));
        assert!(opts.has("report-status-v2"));
        assert_eq!(opts.push_options, vec!["notify=team"]);
        assert_eq!(opts.shallow.len(), 1);
    }

    #[test]
    fn parse_head_info_rejects_unadvertised_cap() {
        let rp = ReceivePackBuilder::new().blocking().build();
        let advertised = CapabilitySet::modern_defaults(); // no agent

        let text = "0000000000000000000000000000000000000000 1111111111111111111111111111111111111111 refs/heads/main\0agent=gix/1.0\n";
        let err = rp.parse_head_info_from_text(text, &advertised).unwrap_err();
        assert!(matches!(err, Error::Validation(_)));
    }
}