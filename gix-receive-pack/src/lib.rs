/*!
Spec-first receive-pack scaffold for gitoxide.

Goals
- Provide a spec-driven implementation surface for Git's receive-pack service.
- Offer a typestate-based builder to make misconfiguration impossible at compile-time.
- Support blocking by default, with optional async via the "async" feature.
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

use core::marker::PhantomData;

/// Typestates representing builder progress.
pub mod state {
    /// Initial builder state with no mode selected.
    pub struct Start;
    /// Ready state after transport mode (blocking or async) is selected.
    pub struct Ready;
}

/// Error type for operations provided by this crate.
///
/// This is intentionally minimal to keep the skeleton buildable while we iterate.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// Placeholder error for unimplemented operations.
    #[error("unimplemented")]
    Unimplemented,
}

/// Opaque configuration for the receive-pack engine.
///
/// This will evolve to include transport, repository access, hooks, and policy.
/// Keeping it private allows us to evolve without breaking users.
#[derive(Default, Debug, Clone)]
struct Config {
    mode: Mode,
}

/// Execution mode for receive-pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Blocking,
    #[cfg(feature = "async")]
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
    /// Requires the "async" feature to be enabled.
    #[cfg(feature = "async")]
    pub fn r#async(mut self) -> ReceivePackBuilder<state::Ready> {
        self.cfg.mode = Mode::Async;
        ReceivePackBuilder {
            cfg: self.cfg,
            _state: PhantomData,
        }
    }
}

impl ReceivePackBuilder<state::Ready> {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_blocking_compiles_and_runs() {
        let rp = ReceivePackBuilder::new().blocking().build();
        rp.run().unwrap();
    }

    #[cfg(feature = "async")]
    #[test]
    fn builder_async_compiles_and_runs() {
        let rp = ReceivePackBuilder::new().r#async().build();
        rp.run().unwrap();
    }
}