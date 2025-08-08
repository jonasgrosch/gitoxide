//! Hook environment management.
//!
//! This module provides utilities for building the environment variables
//! that should be passed to external hooks during execution.

use std::collections::HashMap;
use std::path::PathBuf;
use crate::Error;
use crate::pack::Quarantine;
use crate::protocol::options::Options;

/// Builder for constructing hook execution environments.
///
/// This builder assembles the environment variables that should be available
/// to hooks during execution, following Git's standard environment model.
#[derive(Debug, Clone)]
pub struct HookEnvironment {
    /// Path to the Git directory (.git)
    pub git_dir: Option<PathBuf>,
    /// Path to the quarantine directory (when active)
    pub git_quarantine_path: Option<PathBuf>,
    /// Push options from the client
    pub push_options: Vec<String>,
    /// Optional identity information
    pub identity: Option<Identity>,
    /// Additional environment variables
    pub additional_vars: HashMap<String, String>,
}

/// Identity information for the pusher.
#[derive(Debug, Clone)]
pub struct Identity {
    /// Pusher name
    pub name: Option<String>,
    /// Pusher email
    pub email: Option<String>,
}

impl HookEnvironment {
    /// Create a new environment builder.
    pub fn new() -> Self {
        Self {
            git_dir: None,
            git_quarantine_path: None,
            push_options: Vec::new(),
            identity: None,
            additional_vars: HashMap::new(),
        }
    }

    /// Set the Git directory path.
    pub fn with_git_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.git_dir = Some(path.into());
        self
    }

    /// Set the quarantine directory path.
    pub fn with_quarantine_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.git_quarantine_path = Some(path.into());
        self
    }

    /// Set the quarantine from a Quarantine instance.
    /// 
    /// This will set the quarantine path only if the quarantine is active.
    pub fn with_quarantine(mut self, quarantine: &Quarantine) -> Self {
        // Only set the quarantine path if it's active
        if quarantine.is_active() {
            self.git_quarantine_path = Some(quarantine.objects_dir.clone());
        }
        self
    }

    /// Set the push options.
    pub fn with_push_options(mut self, options: Vec<String>) -> Self {
        self.push_options = options;
        self
    }

    /// Set push options from a parsed Options instance.
    pub fn with_options(mut self, options: &Options) -> Self {
        self.push_options = options.push_options.clone();
        self
    }

    /// Set the identity information.
    pub fn with_identity(mut self, identity: Identity) -> Self {
        self.identity = Some(identity);
        self
    }

    /// Add an additional environment variable.
    pub fn with_var(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.additional_vars.insert(key.into(), value.into());
        self
    }

    /// Build the environment variables map.
    ///
    /// This validates that required variables can be constructed and returns
    /// a map suitable for process execution.
    pub fn build(self) -> Result<HashMap<String, String>, Error> {
        let mut env = HashMap::new();

        // GIT_DIR is required
        let git_dir = self.git_dir
            .ok_or_else(|| Error::environment_setup("GIT_DIR not configured for hook environment"))?;
        
        env.insert("GIT_DIR".to_string(), git_dir.to_string_lossy().to_string());

        // GIT_QUARANTINE_PATH (optional, when quarantine is active)
        if let Some(quarantine_path) = self.git_quarantine_path {
            env.insert(
                "GIT_QUARANTINE_PATH".to_string(),
                quarantine_path.to_string_lossy().to_string(),
            );
        }

        // Push options
        env.insert("GIT_PUSH_OPTION_COUNT".to_string(), self.push_options.len().to_string());
        for (i, option) in self.push_options.iter().enumerate() {
            env.insert(format!("GIT_PUSH_OPTION_{}", i), option.clone());
        }

        // Identity (optional)
        if let Some(identity) = self.identity {
            if let Some(name) = identity.name {
                env.insert("GIT_PUSHER_NAME".to_string(), name);
            }
            if let Some(email) = identity.email {
                env.insert("GIT_PUSHER_EMAIL".to_string(), email);
            }
        }

        // Additional variables
        env.extend(self.additional_vars);

        Ok(env)
    }
}

impl Default for HookEnvironment {
    fn default() -> Self {
        Self::new()
    }
}

impl Identity {
    /// Create a new identity.
    pub fn new() -> Self {
        Self {
            name: None,
            email: None,
        }
    }

    /// Set the name.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set the email.
    pub fn with_email(mut self, email: impl Into<String>) -> Self {
        self.email = Some(email.into());
        self
    }
}

impl Default for Identity {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;


    #[test]
    fn hook_environment_basic_build() {
        let env = HookEnvironment::new()
            .with_git_dir("/path/to/repo/.git")
            .build()
            .unwrap();

        assert_eq!(env.get("GIT_DIR"), Some(&"/path/to/repo/.git".to_string()));
        assert_eq!(env.get("GIT_PUSH_OPTION_COUNT"), Some(&"0".to_string()));
        assert!(!env.contains_key("GIT_QUARANTINE_PATH"));
    }

    #[test]
    fn hook_environment_with_quarantine() {
        let env = HookEnvironment::new()
            .with_git_dir("/path/to/repo/.git")
            .with_quarantine_path("/path/to/quarantine")
            .build()
            .unwrap();

        assert_eq!(env.get("GIT_DIR"), Some(&"/path/to/repo/.git".to_string()));
        assert_eq!(env.get("GIT_QUARANTINE_PATH"), Some(&"/path/to/quarantine".to_string()));
    }

    #[test]
    fn hook_environment_with_push_options() {
        let push_options = vec![
            "notify=team".to_string(),
            "deploy=staging".to_string(),
        ];

        let env = HookEnvironment::new()
            .with_git_dir("/path/to/repo/.git")
            .with_push_options(push_options)
            .build()
            .unwrap();

        assert_eq!(env.get("GIT_PUSH_OPTION_COUNT"), Some(&"2".to_string()));
        assert_eq!(env.get("GIT_PUSH_OPTION_0"), Some(&"notify=team".to_string()));
        assert_eq!(env.get("GIT_PUSH_OPTION_1"), Some(&"deploy=staging".to_string()));
    }

    #[test]
    fn hook_environment_with_identity() {
        let identity = Identity::new()
            .with_name("John Doe")
            .with_email("john@example.com");

        let env = HookEnvironment::new()
            .with_git_dir("/path/to/repo/.git")
            .with_identity(identity)
            .build()
            .unwrap();

        assert_eq!(env.get("GIT_PUSHER_NAME"), Some(&"John Doe".to_string()));
        assert_eq!(env.get("GIT_PUSHER_EMAIL"), Some(&"john@example.com".to_string()));
    }

    #[test]
    fn hook_environment_with_additional_vars() {
        let env = HookEnvironment::new()
            .with_git_dir("/path/to/repo/.git")
            .with_var("CUSTOM_VAR", "custom_value")
            .with_var("SESSION_ID", "abc123")
            .build()
            .unwrap();

        assert_eq!(env.get("CUSTOM_VAR"), Some(&"custom_value".to_string()));
        assert_eq!(env.get("SESSION_ID"), Some(&"abc123".to_string()));
    }

    #[test]
    fn hook_environment_missing_git_dir_fails() {
        let result = HookEnvironment::new().build();
        assert!(result.is_err());
        
        if let Err(Error::Validation(msg)) = result {
            assert!(msg.contains("GIT_DIR not configured"));
        } else {
            panic!("Expected validation error for missing GIT_DIR");
        }
    }

    #[test]
    fn hook_environment_empty_push_options() {
        let env = HookEnvironment::new()
            .with_git_dir("/path/to/repo/.git")
            .with_push_options(vec![])
            .build()
            .unwrap();

        assert_eq!(env.get("GIT_PUSH_OPTION_COUNT"), Some(&"0".to_string()));
        assert!(!env.contains_key("GIT_PUSH_OPTION_0"));
    }

    #[test]
    fn identity_builder_pattern() {
        let identity = Identity::new()
            .with_name("Jane Doe")
            .with_email("jane@example.com");

        assert_eq!(identity.name, Some("Jane Doe".to_string()));
        assert_eq!(identity.email, Some("jane@example.com".to_string()));
    }

    #[test]
    fn identity_partial_information() {
        let identity_name_only = Identity::new().with_name("John");
        assert_eq!(identity_name_only.name, Some("John".to_string()));
        assert_eq!(identity_name_only.email, None);

        let identity_email_only = Identity::new().with_email("john@example.com");
        assert_eq!(identity_email_only.name, None);
        assert_eq!(identity_email_only.email, Some("john@example.com".to_string()));
    }

    #[test]
    fn hook_environment_with_quarantine_instance() {
        // Test with inactive quarantine
        let quarantine = Quarantine::new("/path/to/repo/.git/objects");
        let env = HookEnvironment::new()
            .with_git_dir("/path/to/repo/.git")
            .with_quarantine(&quarantine)
            .build()
            .unwrap();

        assert_eq!(env.get("GIT_DIR"), Some(&"/path/to/repo/.git".to_string()));
        assert!(!env.contains_key("GIT_QUARANTINE_PATH")); // Should not be set for inactive quarantine

        // Test with active quarantine
        let active_quarantine = Quarantine::new("/path/to/repo/.git/objects");
        // We can't actually activate it in tests without filesystem operations,
        // but we can test the logic by manually setting the path
        let env = HookEnvironment::new()
            .with_git_dir("/path/to/repo/.git")
            .with_quarantine_path(&active_quarantine.objects_dir)
            .build()
            .unwrap();

        assert_eq!(env.get("GIT_DIR"), Some(&"/path/to/repo/.git".to_string()));
        assert!(env.contains_key("GIT_QUARANTINE_PATH"));
    }

    #[test]
    fn hook_environment_with_options_instance() {
        let mut options = Options::default();
        options.add_push_option("ci-skip=true");
        options.add_push_option("notify=team");

        let env = HookEnvironment::new()
            .with_git_dir("/path/to/repo/.git")
            .with_options(&options)
            .build()
            .unwrap();

        assert_eq!(env.get("GIT_PUSH_OPTION_COUNT"), Some(&"2".to_string()));
        assert_eq!(env.get("GIT_PUSH_OPTION_0"), Some(&"ci-skip=true".to_string()));
        assert_eq!(env.get("GIT_PUSH_OPTION_1"), Some(&"notify=team".to_string()));
    }
}