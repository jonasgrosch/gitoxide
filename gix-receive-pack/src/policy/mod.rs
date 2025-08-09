//! Policy evaluation and enforcement for receive-pack operations.
//!
//! This module implements Git's receive-pack policies including:
//! - deny_deletes: Forbid deletion of references
//! - deny_non_fast_forwards: Forbid non-fast-forward updates
//! - deny_current_branch: Forbid updates to the current branch
//! - deny_delete_current: Forbid deletion of the current branch
//! - update_instead: Allow worktree updates for current branch
//!
//! Policy evaluation follows a strict precedence order (first match wins):
//! 1. deny_delete_current
//! 2. deny_current_branch
//! 3. deny_deletes
//! 4. deny_non_fast_forwards
//! 5. updateInstead (transform-only, not a hard allow)

pub mod set;
pub mod ff;

pub use set::{PolicySet, PolicyDecision, ReasonCode, UpdateInstead};
pub use ff::is_fast_forward;