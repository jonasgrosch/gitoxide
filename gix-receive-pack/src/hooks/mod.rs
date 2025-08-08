//! Hook execution framework for receive-pack operations.
//!
//! This module provides a trait-based hook system that supports both external process
//! execution (when the "hooks-external" feature is enabled) and no-op implementations
//! for testing and minimal configurations.
//!
//! The hook system follows Git's receive-pack hook model:
//! - `pre_receive`: Runs once with all commands before policy evaluation
//! - `update`: Runs per-command after policy evaluation
//! - `post_receive`: Runs once after successful ref updates
//!
//! # Feature Gates
//!
//! - `hooks-external`: Enables external process execution via gix-command
//! - Without this feature, only NoopHooks is available
//!
//! # Examples
//!
//! ```rust
//! use gix_receive_pack::hooks::{Hooks, NoopHooks, HookDecision};
//! use gix_receive_pack::protocol::CommandUpdate;
//! use gix_hash::ObjectId;
//!
//! let hooks = NoopHooks::new();
//! let commands = vec![
//!     CommandUpdate::Create {
//!         new: ObjectId::null(gix_hash::Kind::Sha1),
//!         name: "refs/heads/main".to_string(),
//!     }
//! ];
//!
//! // Pre-receive hook (batch decision)
//! let decision = hooks.pre_receive(&commands).unwrap();
//! assert!(decision.allowed);
//!
//! // Update hook (per-command decision)
//! let decision = hooks.update(&commands[0]).unwrap();
//! assert!(decision.allowed);
//! ```

use crate::protocol::CommandUpdate;
use crate::Error;

pub mod noop;
#[cfg(feature = "hooks-external")]
pub mod external;
pub mod env;

pub use noop::NoopHooks;
#[cfg(feature = "hooks-external")]
pub use external::{ExternalHooks, SidebandWriter, ExternalHookConfig, HookResult};

/// Result of a hook execution indicating whether to allow or deny the operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookDecision {
    /// Whether the hook allows the operation to proceed.
    pub allowed: bool,
    /// Exit code from the hook process (if applicable).
    pub exit_code: Option<i32>,
    /// Standard output from the hook.
    pub stdout: Vec<u8>,
    /// Standard error from the hook.
    pub stderr: Vec<u8>,
    /// Human-readable message explaining the decision.
    pub message: String,
}

impl HookDecision {
    /// Create a decision that allows the operation.
    pub fn allow() -> Self {
        Self {
            allowed: true,
            exit_code: Some(0),
            stdout: Vec::new(),
            stderr: Vec::new(),
            message: String::new(),
        }
    }

    /// Create a decision that allows the operation with output.
    pub fn allow_with_output(stdout: Vec<u8>, stderr: Vec<u8>) -> Self {
        Self {
            allowed: true,
            exit_code: Some(0),
            stdout,
            stderr,
            message: String::new(),
        }
    }

    /// Create a decision that denies the operation.
    pub fn deny(message: impl Into<String>) -> Self {
        Self {
            allowed: false,
            exit_code: Some(1),
            stdout: Vec::new(),
            stderr: Vec::new(),
            message: message.into(),
        }
    }

    /// Create a decision that denies the operation with detailed output.
    pub fn deny_with_output(
        message: impl Into<String>,
        exit_code: i32,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    ) -> Self {
        Self {
            allowed: false,
            exit_code: Some(exit_code),
            stdout,
            stderr,
            message: message.into(),
        }
    }
}

/// Trait for hook execution during receive-pack operations.
///
/// This trait defines the interface for executing Git hooks during the receive-pack
/// process. Implementations can range from no-op (for testing) to full external
/// process execution.
///
/// The hook execution follows Git's standard receive-pack hook model:
/// - `pre_receive`: Executed once with all commands before policy evaluation
/// - `update`: Executed per-command after policy evaluation  
/// - `post_receive`: Executed once after successful ref updates (fire-and-forget)
pub trait Hooks {
    /// Execute the update hook for a single command.
    ///
    /// This hook is called for each command after policy evaluation but before
    /// the ref update is applied. It receives the old OID, new OID, and refname.
    ///
    /// # Arguments
    /// * `command` - The command update to validate
    ///
    /// # Returns
    /// A `HookDecision` indicating whether to allow or deny the update.
    fn update(&mut self, command: &CommandUpdate) -> Result<HookDecision, Error>;

    /// Execute the pre-receive hook with all commands.
    ///
    /// This hook is called once with the entire batch of commands before any
    /// policy evaluation or ref updates. It can reject the entire push.
    ///
    /// # Arguments
    /// * `commands` - All commands in the push operation
    ///
    /// # Returns
    /// A `HookDecision` indicating whether to allow or deny the entire push.
    fn pre_receive(&mut self, commands: &[CommandUpdate]) -> Result<HookDecision, Error>;

    /// Execute the post-receive hook after successful updates.
    ///
    /// This hook is called once after all ref updates have been successfully
    /// applied. It's fire-and-forget and cannot affect the outcome of the push.
    ///
    /// # Arguments
    /// * `commands` - All commands that were successfully applied
    ///
    /// # Returns
    /// Unit result - errors are logged but don't affect the push outcome.
    fn post_receive(&mut self, commands: &[CommandUpdate]) -> Result<(), Error>;
}