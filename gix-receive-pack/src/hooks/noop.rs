//! No-operation hook implementation.
//!
//! This module provides a hook implementation that always allows operations
//! and produces no output. It's useful for testing, minimal configurations,
//! or when the "hooks-external" feature is disabled.

use super::{Hooks, HookDecision};
use crate::protocol::CommandUpdate;
use crate::Error;

/// A hook implementation that always allows operations and produces no output.
///
/// This is the default hook implementation when external hooks are not needed
/// or when the "hooks-external" feature is disabled. It provides a deterministic
/// "allow" response for all hook invocations.
///
/// # Examples
///
/// ```rust
/// use gix_receive_pack::hooks::{Hooks, NoopHooks, HookDecision};
/// use gix_receive_pack::protocol::CommandUpdate;
/// use gix_hash::ObjectId;
///
/// let mut hooks = NoopHooks::new();
/// let command = CommandUpdate::Create {
///     new: ObjectId::null(gix_hash::Kind::Sha1),
///     name: "refs/heads/main".to_string(),
/// };
///
/// let decision = hooks.update(&command).unwrap();
/// assert!(decision.allowed);
/// assert_eq!(decision.exit_code, Some(0));
/// assert!(decision.stdout.is_empty());
/// assert!(decision.stderr.is_empty());
/// ```
#[derive(Debug, Clone, Default)]
pub struct NoopHooks {
    _private: (),
}

impl NoopHooks {
    /// Create a new NoopHooks instance.
    pub fn new() -> Self {
        Self { _private: () }
    }
}

impl Hooks for NoopHooks {
    fn update(&mut self, _command: &CommandUpdate) -> Result<HookDecision, Error> {
        Ok(HookDecision::allow())
    }

    fn pre_receive(&mut self, _commands: &[CommandUpdate]) -> Result<HookDecision, Error> {
        Ok(HookDecision::allow())
    }

    fn post_receive(&mut self, _commands: &[CommandUpdate]) -> Result<(), Error> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gix_hash::ObjectId;

    #[test]
    fn noop_hooks_always_allow() {
        let mut hooks = NoopHooks::new();
        
        let command = CommandUpdate::Create {
            new: ObjectId::null(gix_hash::Kind::Sha1),
            name: "refs/heads/main".to_string(),
        };

        // Test update hook
        let decision = hooks.update(&command).unwrap();
        assert!(decision.allowed);
        assert_eq!(decision.exit_code, Some(0));
        assert!(decision.stdout.is_empty());
        assert!(decision.stderr.is_empty());
        assert!(decision.message.is_empty());

        // Test pre_receive hook
        let commands = vec![command.clone()];
        let decision = hooks.pre_receive(&commands).unwrap();
        assert!(decision.allowed);
        assert_eq!(decision.exit_code, Some(0));
        assert!(decision.stdout.is_empty());
        assert!(decision.stderr.is_empty());

        // Test post_receive hook
        hooks.post_receive(&commands).unwrap();
    }

    #[test]
    fn noop_hooks_handles_multiple_commands() {
        let mut hooks = NoopHooks::new();
        
        let commands = vec![
            CommandUpdate::Create {
                new: ObjectId::null(gix_hash::Kind::Sha1),
                name: "refs/heads/main".to_string(),
            },
            CommandUpdate::Update {
                old: ObjectId::null(gix_hash::Kind::Sha1),
                new: ObjectId::null(gix_hash::Kind::Sha1),
                name: "refs/heads/develop".to_string(),
            },
            CommandUpdate::Delete {
                old: ObjectId::null(gix_hash::Kind::Sha1),
                name: "refs/heads/feature".to_string(),
            },
        ];

        // All commands should be allowed
        for command in &commands {
            let decision = hooks.update(command).unwrap();
            assert!(decision.allowed);
        }

        // Batch operations should be allowed
        let decision = hooks.pre_receive(&commands).unwrap();
        assert!(decision.allowed);

        hooks.post_receive(&commands).unwrap();
    }

    #[test]
    fn noop_hooks_default_construction() {
        let mut hooks1 = NoopHooks::new();
        let mut hooks2 = NoopHooks::default();
        
        // Both should behave identically
        let command = CommandUpdate::Create {
            new: ObjectId::null(gix_hash::Kind::Sha1),
            name: "refs/heads/test".to_string(),
        };

        let decision1 = hooks1.update(&command).unwrap();
        let decision2 = hooks2.update(&command).unwrap();
        
        assert_eq!(decision1.allowed, decision2.allowed);
        assert_eq!(decision1.exit_code, decision2.exit_code);
    }
}