//! Policy configuration and evaluation logic.

use crate::protocol::CommandUpdate;
use crate::Error;
use gix_hash::ObjectId;

/// Policy configuration for receive-pack operations.
///
/// This struct holds the configuration for various Git receive policies
/// and provides evaluation methods to determine if commands should be allowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicySet {
    /// Forbid deletion of any reference
    deny_deletes: bool,
    /// Forbid non-fast-forward updates
    deny_non_fast_forwards: bool,
    /// Policy for updates to the current branch
    current_branch_policy: Policy,
    /// Policy for deletion of the current branch
    delete_current_policy: Policy,
    /// Enable worktree updates for current branch
    update_instead: bool,
}

/// Policy enforcement level for specific operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Allow the operation
    Allow,
    /// Deny the operation
    Deny,
    /// Warn about the operation but allow it
    Warn,
}

/// Reason codes for policy decisions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReasonCode {
    /// Operation is allowed
    Allowed,
    /// Denied due to deny_deletes policy
    DenyDeletes,
    /// Denied due to non-fast-forward update
    NonFastForward,
    /// Denied due to deny_current_branch policy
    DenyCurrent,
    /// Denied due to deny_delete_current policy
    DenyDeleteCurrent,
    /// Allowed but delegated to worktree updater
    UpdateInstead,
    /// Denied by hook execution
    HookRejected,
    /// Denied by proc-receive helper
    ProcReceiveRejected,
}

/// Action to be taken when updateInstead is triggered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInstead {
    /// The reference name being updated
    pub refname: String,
    /// The old object ID
    pub old_oid: ObjectId,
    /// The new object ID
    pub new_oid: ObjectId,
}

/// Decision result from policy evaluation.
///
/// This is an internal model used by M6 (transactions) and M7 (reporting)
/// to understand the outcome of policy evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyDecision {
    /// Whether the operation is allowed
    pub allowed: bool,
    /// The reason code for the decision
    pub reason_code: ReasonCode,
    /// Human-readable message explaining the decision
    pub message: String,
    /// Optional delegated action (e.g., for updateInstead)
    pub delegated_action: Option<UpdateInstead>,
}

/// Resolve the current branch from HEAD symref.
///
/// This function follows the HEAD symref chain to determine the current branch.
/// It validates alias/symref chains and returns an error if the chain is invalid.
///
/// Returns:
/// - `Ok(Some(refname))` if HEAD points to a valid branch reference
/// - `Ok(None)` if HEAD is detached (points directly to an object)
/// - `Err(Error::Validation)` if the symref chain is invalid
pub fn resolve_current_branch(ref_store: &gix_ref::file::Store) -> Result<Option<String>, Error> {
    // Try to find the HEAD reference
    let head_ref = match ref_store.try_find("HEAD") {
        Ok(Some(head)) => head,
        Ok(None) => {
            // HEAD doesn't exist - this is unusual but not necessarily an error
            return Ok(None);
        }
        Err(e) => {
            return Err(Error::environment_setup(&format!("failed to read HEAD: {}", e)));
        }
    };

    // Check if HEAD is a symbolic reference
    match &head_ref.target {
        gix_ref::Target::Symbolic(target_name) => {
            // Validate the symref chain by attempting to resolve it
            match validate_symref_chain(ref_store, target_name.as_bstr()) {
                Ok(resolved_name) => Ok(Some(resolved_name)),
                Err(e) => Err(Error::policy_violation("invalid_ref_alias", &e)),
            }
        }
        gix_ref::Target::Object(_) => {
            // HEAD points directly to an object (detached HEAD)
            Ok(None)
        }
    }
}

/// Validate a symref chain and return the final resolved reference name.
///
/// This function follows the symref chain to ensure it's valid and doesn't contain cycles.
/// It returns the final reference name that the chain resolves to.
fn validate_symref_chain(ref_store: &gix_ref::file::Store, start_ref: &gix_object::bstr::BStr) -> Result<String, String> {
    const MAX_SYMREF_DEPTH: usize = 5; // Prevent infinite loops
    let mut visited = std::collections::HashSet::new();
    let mut current_ref = start_ref.to_string();
    
    for _ in 0..MAX_SYMREF_DEPTH {
        // Check for cycles
        if visited.contains(&current_ref) {
            return Err(format!("symref cycle detected involving '{}'", current_ref));
        }
        visited.insert(current_ref.clone());
        
        // Try to resolve the current reference
        match ref_store.try_find(&current_ref) {
            Ok(Some(reference)) => {
                match &reference.target {
                    gix_ref::Target::Symbolic(target_name) => {
                        // Continue following the chain
                        current_ref = target_name.to_string();
                    }
                    gix_ref::Target::Object(_) => {
                        // Found the final target - this is a valid reference
                        return Ok(current_ref);
                    }
                }
            }
            Ok(None) => {
                return Err(format!("reference '{}' does not exist", current_ref));
            }
            Err(e) => {
                return Err(format!("failed to resolve reference '{}': {}", current_ref, e));
            }
        }
    }
    
    Err(format!("symref chain too deep (max {} levels)", MAX_SYMREF_DEPTH))
}

impl PolicySet {
    /// Create a new PolicySet with default (permissive) settings.
    pub fn new() -> Self {
        Self {
            deny_deletes: false,
            deny_non_fast_forwards: false,
            current_branch_policy: Policy::Allow,
            delete_current_policy: Policy::Allow,
            update_instead: false,
        }
    }

    /// Get the deny_deletes setting.
    pub fn deny_deletes(&self) -> bool {
        self.deny_deletes
    }

    /// Set the deny_deletes policy.
    pub fn with_deny_deletes(mut self, deny: bool) -> Self {
        self.deny_deletes = deny;
        self
    }

    /// Get the deny_non_fast_forwards setting.
    pub fn deny_non_fast_forwards(&self) -> bool {
        self.deny_non_fast_forwards
    }

    /// Set the deny_non_fast_forwards policy.
    pub fn with_deny_non_fast_forwards(mut self, deny: bool) -> Self {
        self.deny_non_fast_forwards = deny;
        self
    }

    /// Get the current branch policy.
    pub fn current_branch(&self) -> Policy {
        self.current_branch_policy
    }

    /// Set the current branch policy.
    pub fn with_current_branch(mut self, policy: Policy) -> Self {
        self.current_branch_policy = policy;
        self
    }

    /// Get the delete current branch policy.
    pub fn delete_current(&self) -> Policy {
        self.delete_current_policy
    }

    /// Set the delete current branch policy.
    pub fn with_delete_current(mut self, policy: Policy) -> Self {
        self.delete_current_policy = policy;
        self
    }

    /// Get the update_instead setting.
    pub fn update_instead(&self) -> bool {
        self.update_instead
    }

    /// Set the update_instead policy.
    pub fn with_update_instead(mut self, enable: bool) -> Self {
        self.update_instead = enable;
        self
    }

    /// Evaluate a command against the configured policies.
    ///
    /// This method implements the policy precedence order:
    /// 1. deny_delete_current
    /// 2. deny_current_branch  
    /// 3. deny_deletes
    /// 4. deny_non_fast_forwards
    /// 5. updateInstead (transform-only)
    ///
    /// Returns Ok(()) if the command is allowed, or Err with appropriate error type if denied.
    /// For internal use, also returns a PolicyDecision with detailed reasoning.
    pub fn evaluate(&self, command: &CommandUpdate, ref_store: &gix_ref::file::Store, main_odb: &gix_odb::Handle) -> Result<(), Error> {
        let current_branch = resolve_current_branch(ref_store)?;
        let decision = self.evaluate_internal(command, current_branch.as_deref(), main_odb)?;
        
        if decision.allowed {
            Ok(())
        } else {
            // Map policy decision to appropriate error with refname and OID context
            let refname = command.name();
            match decision.reason_code {
                ReasonCode::DenyDeletes => {
                    if let CommandUpdate::Delete { old, .. } = command {
                        Err(Error::policy_violation_with_oids("deny_deletes", refname, Some(*old), None))
                    } else {
                        Err(Error::policy_violation("deny_deletes", refname))
                    }
                }
                ReasonCode::NonFastForward => {
                    if let CommandUpdate::Update { old, new, .. } = command {
                        Err(Error::policy_violation_with_oids("deny_non_fast_forwards", refname, Some(*old), Some(*new)))
                    } else {
                        Err(Error::policy_violation("deny_non_fast_forwards", refname))
                    }
                }
                ReasonCode::DenyCurrent => {
                    match command {
                        CommandUpdate::Create { new, .. } => {
                            Err(Error::policy_violation_with_oids("deny_current_branch", refname, None, Some(*new)))
                        }
                        CommandUpdate::Update { old, new, .. } => {
                            Err(Error::policy_violation_with_oids("deny_current_branch", refname, Some(*old), Some(*new)))
                        }
                        CommandUpdate::Delete { old, .. } => {
                            Err(Error::policy_violation_with_oids("deny_current_branch", refname, Some(*old), None))
                        }
                    }
                }
                ReasonCode::DenyDeleteCurrent => {
                    if let CommandUpdate::Delete { old, .. } = command {
                        Err(Error::policy_violation_with_oids("deny_delete_current", refname, Some(*old), None))
                    } else {
                        Err(Error::policy_violation("deny_delete_current", refname))
                    }
                }
                _ => {
                    // Fallback to generic validation error
                    Err(Error::Validation(decision.message))
                }
            }
        }
    }

    /// Internal evaluation method that returns detailed PolicyDecision.
    ///
    /// This is used by M6/M7 for detailed decision processing.
    pub(crate) fn evaluate_internal(&self, command: &CommandUpdate, current_branch: Option<&str>, main_odb: &gix_odb::Handle) -> Result<PolicyDecision, Error> {
        let refname = command.name();
        let is_current_branch = current_branch.map_or(false, |cb| cb == refname);

        // Precedence 1: deny_delete_current
        if let CommandUpdate::Delete { .. } = command {
            if is_current_branch && self.delete_current_policy == Policy::Deny {
                return Ok(PolicyDecision {
                    allowed: false,
                    reason_code: ReasonCode::DenyDeleteCurrent,
                    message: format!("deletion of the current branch '{}' is denied", refname),
                    delegated_action: None,
                });
            }
        }

        // Precedence 2: deny_current_branch
        if is_current_branch && self.current_branch_policy == Policy::Deny {
            match command {
                CommandUpdate::Create { .. } | CommandUpdate::Update { .. } => {
                    // Check if updateInstead should apply (precedence 5)
                    if self.update_instead {
                        if let CommandUpdate::Update { old, new, .. } = command {
                            return Ok(PolicyDecision {
                                allowed: true,
                                reason_code: ReasonCode::UpdateInstead,
                                message: format!("update to current branch '{}' delegated to worktree updater", refname),
                                delegated_action: Some(UpdateInstead {
                                    refname: refname.to_string(),
                                    old_oid: *old,
                                    new_oid: *new,
                                }),
                            });
                        }
                    }
                    
                    return Ok(PolicyDecision {
                        allowed: false,
                        reason_code: ReasonCode::DenyCurrent,
                        message: format!("updates to the current branch '{}' are denied", refname),
                        delegated_action: None,
                    });
                }
                CommandUpdate::Delete { .. } => {
                    // Delete is handled by precedence 1, but if we get here, it's allowed by delete_current policy
                }
            }
        }

        // Precedence 3: deny_deletes
        if let CommandUpdate::Delete { .. } = command {
            if self.deny_deletes {
                return Ok(PolicyDecision {
                    allowed: false,
                    reason_code: ReasonCode::DenyDeletes,
                    message: format!("deletion of reference '{}' is denied", refname),
                    delegated_action: None,
                });
            }
        }

        // Precedence 4: deny_non_fast_forwards
        if let CommandUpdate::Update { old, new, .. } = command {
            if self.deny_non_fast_forwards {
                // TODO: Replace with M4 ConnectivityChecker once available
                // For now, use minimal fast-forward detection
                match super::ff::is_fast_forward(*old, *new, main_odb) {
                    Ok(is_ff) => {
                        if !is_ff {
                            return Ok(PolicyDecision {
                                allowed: false,
                                reason_code: ReasonCode::NonFastForward,
                                message: format!("non-fast-forward update to '{}' is denied", refname),
                                delegated_action: None,
                            });
                        }
                    }
                    Err(e) => {
                        return Err(Error::environment_setup(&format!("failed to check fast-forward status for '{}': {}", refname, e)));
                    }
                }
            }
        }

        // If we reach here, the command is allowed
        Ok(PolicyDecision {
            allowed: true,
            reason_code: ReasonCode::Allowed,
            message: format!("operation on '{}' is allowed", refname),
            delegated_action: None,
        })
    }
}

impl Default for PolicySet {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gix_hash::ObjectId;

    fn test_oid(suffix: u8) -> ObjectId {
        let mut bytes = [0u8; 20];
        bytes[19] = suffix;
        ObjectId::from_bytes_or_panic(&bytes)
    }

    fn test_odb() -> gix_odb::Handle {
        let temp_dir = tempfile::tempdir().unwrap();
        let objects_dir = temp_dir.path().join("objects");
        std::fs::create_dir_all(&objects_dir).unwrap();
        gix_odb::at(objects_dir).unwrap()
    }

    fn test_ref_store() -> (tempfile::TempDir, gix_ref::file::Store) {
        let temp_dir = tempfile::tempdir().unwrap();
        let git_dir = temp_dir.path().join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();
        let store = gix_ref::file::Store::at(
            git_dir,
            gix_ref::store::init::Options {
                write_reflog: gix_ref::store::WriteReflog::Disable,
                object_hash: gix_hash::Kind::Sha1,
                precompose_unicode: false,
                prohibit_windows_device_names: false,
            },
        );
        (temp_dir, store)
    }

    #[test]
    fn test_policy_precedence_deny_delete_current() {
        let policy = PolicySet::new()
            .with_delete_current(Policy::Deny)
            .with_deny_deletes(false); // This should not matter due to precedence

        let cmd = CommandUpdate::Delete {
            old: test_oid(1),
            name: "refs/heads/main".to_string(),
        };

        let odb = test_odb();
        let decision = policy.evaluate_internal(&cmd, Some("refs/heads/main"), &odb).unwrap();
        assert!(!decision.allowed);
        assert_eq!(decision.reason_code, ReasonCode::DenyDeleteCurrent);
    }

    #[test]
    fn test_policy_precedence_deny_current_branch() {
        let policy = PolicySet::new()
            .with_current_branch(Policy::Deny)
            .with_deny_deletes(false); // This should not matter due to precedence

        let cmd = CommandUpdate::Update {
            old: test_oid(1),
            new: test_oid(2),
            name: "refs/heads/main".to_string(),
        };

        let odb = test_odb();
        let decision = policy.evaluate_internal(&cmd, Some("refs/heads/main"), &odb).unwrap();
        assert!(!decision.allowed);
        assert_eq!(decision.reason_code, ReasonCode::DenyCurrent);
    }

    #[test]
    fn test_policy_precedence_deny_deletes() {
        let policy = PolicySet::new()
            .with_deny_deletes(true);

        let cmd = CommandUpdate::Delete {
            old: test_oid(1),
            name: "refs/heads/feature".to_string(),
        };

        let odb = test_odb();
        let decision = policy.evaluate_internal(&cmd, Some("refs/heads/main"), &odb).unwrap();
        assert!(!decision.allowed);
        assert_eq!(decision.reason_code, ReasonCode::DenyDeletes);
    }

    #[test]
    fn test_policy_precedence_deny_non_fast_forwards() {
        let policy = PolicySet::new()
            .with_deny_non_fast_forwards(true);

        let cmd = CommandUpdate::Update {
            old: test_oid(1),
            new: test_oid(2),
            name: "refs/heads/feature".to_string(),
        };

        let odb = test_odb();
        // Since we're using an empty ODB, the fast-forward check will fail
        // because the objects don't exist, which should result in an error
        let result = policy.evaluate_internal(&cmd, Some("refs/heads/main"), &odb);
        assert!(result.is_err());
    }

    #[test]
    fn test_update_instead_precedence() {
        let policy = PolicySet::new()
            .with_current_branch(Policy::Deny)
            .with_update_instead(true);

        let cmd = CommandUpdate::Update {
            old: test_oid(1),
            new: test_oid(2),
            name: "refs/heads/main".to_string(),
        };

        let odb = test_odb();
        let decision = policy.evaluate_internal(&cmd, Some("refs/heads/main"), &odb).unwrap();
        assert!(decision.allowed);
        assert_eq!(decision.reason_code, ReasonCode::UpdateInstead);
        assert!(decision.delegated_action.is_some());
    }

    #[test]
    fn test_allowed_operation() {
        let policy = PolicySet::new(); // All permissive

        let cmd = CommandUpdate::Create {
            new: test_oid(1),
            name: "refs/heads/feature".to_string(),
        };

        let odb = test_odb();
        let decision = policy.evaluate_internal(&cmd, Some("refs/heads/main"), &odb).unwrap();
        assert!(decision.allowed);
        assert_eq!(decision.reason_code, ReasonCode::Allowed);
    }

    #[test]
    fn test_non_current_branch_operations() {
        let policy = PolicySet::new()
            .with_current_branch(Policy::Deny)
            .with_delete_current(Policy::Deny);

        // Operations on non-current branches should be allowed
        let cmd = CommandUpdate::Update {
            old: test_oid(1),
            new: test_oid(2),
            name: "refs/heads/feature".to_string(),
        };

        let odb = test_odb();
        let decision = policy.evaluate_internal(&cmd, Some("refs/heads/main"), &odb).unwrap();
        assert!(decision.allowed);
    }

    #[test]
    fn test_resolve_current_branch_no_head() {
        let (_temp_dir, ref_store) = test_ref_store();
        let result = resolve_current_branch(&ref_store);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn test_resolve_current_branch_detached_head() {
        let (_temp_dir, ref_store) = test_ref_store();
        
        // Create a detached HEAD pointing directly to an object
        let head_path = ref_store.git_dir().join("HEAD");
        std::fs::write(&head_path, "1111111111111111111111111111111111111111\n").unwrap();
        
        let result = resolve_current_branch(&ref_store);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn test_resolve_current_branch_symbolic() {
        let (_temp_dir, ref_store) = test_ref_store();
        
        // Create a symbolic HEAD pointing to refs/heads/main
        let head_path = ref_store.git_dir().join("HEAD");
        std::fs::write(&head_path, "ref: refs/heads/main\n").unwrap();
        
        // Create the target reference
        let refs_heads_dir = ref_store.git_dir().join("refs").join("heads");
        std::fs::create_dir_all(&refs_heads_dir).unwrap();
        let main_path = refs_heads_dir.join("main");
        std::fs::write(&main_path, "1111111111111111111111111111111111111111\n").unwrap();
        
        let result = resolve_current_branch(&ref_store);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Some("refs/heads/main".to_string()));
    }

    #[test]
    fn test_resolve_current_branch_symref_chain() {
        let (_temp_dir, ref_store) = test_ref_store();
        
        // Create a chain: HEAD -> refs/heads/alias -> refs/heads/main
        let head_path = ref_store.git_dir().join("HEAD");
        std::fs::write(&head_path, "ref: refs/heads/alias\n").unwrap();
        
        let refs_heads_dir = ref_store.git_dir().join("refs").join("heads");
        std::fs::create_dir_all(&refs_heads_dir).unwrap();
        
        let alias_path = refs_heads_dir.join("alias");
        std::fs::write(&alias_path, "ref: refs/heads/main\n").unwrap();
        
        let main_path = refs_heads_dir.join("main");
        std::fs::write(&main_path, "1111111111111111111111111111111111111111\n").unwrap();
        
        let result = resolve_current_branch(&ref_store);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Some("refs/heads/main".to_string()));
    }

    #[test]
    fn test_resolve_current_branch_invalid_symref_chain() {
        let (_temp_dir, ref_store) = test_ref_store();
        
        // Create HEAD pointing to a non-existent reference
        let head_path = ref_store.git_dir().join("HEAD");
        std::fs::write(&head_path, "ref: refs/heads/nonexistent\n").unwrap();
        
        let result = resolve_current_branch(&ref_store);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid_ref_alias"));
    }

    #[test]
    fn test_resolve_current_branch_symref_cycle() {
        let (_temp_dir, ref_store) = test_ref_store();
        
        // Create a cycle: HEAD -> refs/heads/a -> refs/heads/b -> refs/heads/a
        let head_path = ref_store.git_dir().join("HEAD");
        std::fs::write(&head_path, "ref: refs/heads/a\n").unwrap();
        
        let refs_heads_dir = ref_store.git_dir().join("refs").join("heads");
        std::fs::create_dir_all(&refs_heads_dir).unwrap();
        
        let a_path = refs_heads_dir.join("a");
        std::fs::write(&a_path, "ref: refs/heads/b\n").unwrap();
        
        let b_path = refs_heads_dir.join("b");
        std::fs::write(&b_path, "ref: refs/heads/a\n").unwrap();
        
        let result = resolve_current_branch(&ref_store);
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("invalid_ref_alias"));
        assert!(error_msg.contains("cycle"));
    }
}