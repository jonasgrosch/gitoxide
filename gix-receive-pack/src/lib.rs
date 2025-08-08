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
//
/// This is intentionally minimal to keep the skeleton buildable while we iterate.
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
}

impl Error {
    /// Fast classification helper returning a stable error kind.
    pub fn kind(&self) -> Kind {
        match self {
            Error::Unimplemented => Kind::Other,
            Error::Protocol(_) => Kind::Protocol,
            Error::Validation(_) => Kind::Validation,
            Error::Io(_) => Kind::Io,
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
    fsck_objects: bool,
    #[cfg(feature = "progress")]
    show_progress: bool,
    /// Path to the main repository objects directory (.git/objects)
    objects_dir: Option<PathBuf>,
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

    /// Enable or disable fsck verification on received objects (receive.fsckObjects).
    #[cfg(feature = "fsck")]
    pub fn with_fsck_objects(mut self, enabled: bool) -> Self {
        self.cfg.fsck_objects = enabled;
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
                let _ = crate::pack::PackIngestor::unpack_objects_stub();
            }
        }

        // Optional fsck (scaffold)
        #[cfg(feature = "fsck")]
        {
            if self.cfg.fsck_objects {
                // Placeholder for fsck integration.
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
        progress: &mut dyn gix_features::progress::prodash::DynNestedProgress,
    ) -> Result<(), Error> {
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

        let res = match choice {
            crate::pack::PackIngestPath::IndexPack => crate::pack::PackIngestor::index_pack(
                input,
                quarantine.objects_dir.as_path(),
                pack_size,
                Some(main_odb.clone()),
                progress,
            ),
            crate::pack::PackIngestPath::UnpackObjects => {
                // For now, route to index-pack as a safe default until unpack-objects is fully implemented.
                crate::pack::PackIngestor::index_pack(
                    input,
                    quarantine.objects_dir.as_path(),
                    pack_size,
                    Some(main_odb.clone()),
                    progress,
                )
            }
        };

        match res {
            Ok(_) => {
                quarantine.migrate_on_success()?;
                Ok(())
            }
            Err(e) => {
                let _ = quarantine.drop_on_failure();
                Err(e)
            }
        }
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