//! Configuration parsing for receive-pack policies, hooks, and proc-receive.
//!
//! This module provides configuration loaders that parse Git configuration
//! values into structured types used by the policy, hooks, and proc-receive
//! systems.
//!
//! # Configuration Keys
//!
//! ## Policy Configuration
//! - `receive.denyDeletes`: Forbid deletion of references
//! - `receive.denyNonFastForwards`: Forbid non-fast-forward updates  
//! - `receive.denyCurrentBranch`: Policy for updates to current branch
//! - `receive.denyDeleteCurrent`: Policy for deletion of current branch
//! - `receive.updateInstead`: Enable worktree updates for current branch
//!
//! ## Hook Configuration
//! - `hooks.timeout`: Timeout in milliseconds for hook execution
//! - `hooks.maxOutputSize`: Maximum output size in bytes
//! - `hooks.sidebandRelay`: Enable sideband relay for hook output
//! - `hooks.environment.*`: Additional environment variables
//!
//! ## Proc-Receive Configuration
//! - `procReceive.enabled`: Enable proc-receive protocol
//! - `procReceive.helperPath`: Path to proc-receive helper
//! - `procReceive.version`: Protocol version (default 1)
//! - `procReceive.timeout`: Timeout for helper operations

pub mod policy;
pub mod hooks;
pub mod proc_receive;

pub use policy::PolicyConfig;
pub use hooks::HookConfig;
pub use proc_receive::ProcReceiveConfig;

use crate::Error;

/// Result type for configuration parsing operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Load all receive-pack configuration from a Git config snapshot.
///
/// This is a convenience function that loads policy, hook, and proc-receive
/// configuration in one call.
///
/// # Arguments
/// * `config` - Git configuration snapshot
///
/// # Returns
/// A tuple of (PolicyConfig, HookConfig, ProcReceiveConfig)
pub fn load_all_config(
    config: &gix_config::File<'static>,
) -> Result<(PolicyConfig, HookConfig, ProcReceiveConfig)> {
    let policy_config = PolicyConfig::from_config(config)?;
    let hook_config = HookConfig::from_config(config)?;
    let proc_receive_config = ProcReceiveConfig::from_config(config)?;
    
    Ok((policy_config, hook_config, proc_receive_config))
}