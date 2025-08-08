//! Policy configuration parsing from Git config.

use crate::policy::{PolicySet, set::Policy};
use crate::Error;
use gix_config::File;
use gix_config_value::Boolean;
use gix_object::bstr::BStr;

/// Configuration loader for receive-pack policies.
///
/// This struct provides methods to parse Git configuration values
/// into a PolicySet for use in policy evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyConfig {
    /// Parsed policy set
    policy_set: PolicySet,
}

impl PolicyConfig {
    /// Create a new PolicyConfig with default settings.
    pub fn new() -> Self {
        Self {
            policy_set: PolicySet::new(),
        }
    }

    /// Load policy configuration from a Git config file.
    ///
    /// This method parses the following configuration keys:
    /// - `receive.denyDeletes`: Boolean, forbid deletion of references
    /// - `receive.denyNonFastForwards`: Boolean, forbid non-fast-forward updates
    /// - `receive.denyCurrentBranch`: String, policy for current branch updates
    /// - `receive.denyDeleteCurrent`: String, policy for current branch deletion
    /// - `receive.updateInstead`: Boolean, enable worktree updates
    ///
    /// # Arguments
    /// * `config` - Git configuration file to parse
    ///
    /// # Returns
    /// A PolicyConfig with parsed settings, or Error if parsing fails
    pub fn from_config(config: &File<'static>) -> Result<Self, Error> {
        let mut policy_set = PolicySet::new();

        // Parse receive.denyDeletes
        if let Some(result) = config.boolean("receive.denyDeletes") {
            match result {
                Ok(value) => policy_set = policy_set.with_deny_deletes(value),
                Err(e) => return Err(Error::Validation(format!("invalid boolean value for 'receive.denyDeletes': {}", e))),
            }
        }

        // Parse receive.denyNonFastForwards
        if let Some(result) = config.boolean("receive.denyNonFastForwards") {
            match result {
                Ok(value) => policy_set = policy_set.with_deny_non_fast_forwards(value),
                Err(e) => return Err(Error::Validation(format!("invalid boolean value for 'receive.denyNonFastForwards': {}", e))),
            }
        }

        // Parse receive.denyCurrentBranch
        if let Some(value) = config.string("receive.denyCurrentBranch") {
            let policy = parse_policy_string(&value, "receive.denyCurrentBranch")?;
            policy_set = policy_set.with_current_branch(policy);
        }

        // Parse receive.denyDeleteCurrent
        if let Some(value) = config.string("receive.denyDeleteCurrent") {
            let policy = parse_policy_string(&value, "receive.denyDeleteCurrent")?;
            policy_set = policy_set.with_delete_current(policy);
        }

        // Parse receive.updateInstead
        if let Some(result) = config.boolean("receive.updateInstead") {
            match result {
                Ok(value) => policy_set = policy_set.with_update_instead(value),
                Err(e) => return Err(Error::Validation(format!("invalid boolean value for 'receive.updateInstead': {}", e))),
            }
        }

        Ok(Self { policy_set })
    }

    /// Get the parsed PolicySet.
    pub fn policy_set(&self) -> &PolicySet {
        &self.policy_set
    }

    /// Convert into the PolicySet.
    pub fn into_policy_set(self) -> PolicySet {
        self.policy_set
    }
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse a policy configuration value from a string.
///
/// Valid values are:
/// - "refuse" or "deny" -> Policy::Deny
/// - "warn" -> Policy::Warn  
/// - "allow" or "ignore" -> Policy::Allow
/// - Boolean true -> Policy::Deny
/// - Boolean false -> Policy::Allow
fn parse_policy_string(value: &std::borrow::Cow<'_, BStr>, key: &str) -> Result<Policy, Error> {
    // First try to parse as boolean
    if let Ok(boolean) = Boolean::try_from(value.as_ref()) {
        return Ok(if boolean.0 { Policy::Deny } else { Policy::Allow });
    }

    // Parse as string value
    let value_str = std::str::from_utf8(value.as_ref())
        .map_err(|e| Error::Validation(format!("invalid UTF-8 in '{}': {}", key, e)))?;

    match value_str.to_lowercase().as_str() {
        "refuse" | "deny" => Ok(Policy::Deny),
        "warn" => Ok(Policy::Warn),
        "allow" | "ignore" => Ok(Policy::Allow),
        _ => Err(Error::Validation(format!(
            "invalid policy value for '{}': '{}'. Valid values are: refuse, deny, warn, allow, ignore, true, false",
            key, value_str
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gix_config::File;

    fn create_config_with_values(values: &[(&str, &str)]) -> File<'static> {
        let mut config_text = String::new();
        let mut sections: std::collections::HashMap<String, Vec<(String, String)>> = std::collections::HashMap::new();
        
        // Group values by section
        for (key, value) in values {
            let parts: Vec<&str> = key.split('.').collect();
            if parts.len() == 2 {
                let section = parts[0].to_string();
                let key_name = parts[1].to_string();
                sections.entry(section).or_insert_with(Vec::new).push((key_name, value.to_string()));
            }
        }
        
        // Build config text
        for (section, keys) in sections {
            config_text.push_str(&format!("[{}]\n", section));
            for (key, value) in keys {
                config_text.push_str(&format!("    {} = {}\n", key, value));
            }
            config_text.push('\n');
        }
        
        // Create a static string to avoid lifetime issues
        let config_string: &'static str = Box::leak(config_text.into_boxed_str());
        File::try_from(config_string).unwrap()
    }

    #[test]
    fn test_default_policy_config() {
        let config = PolicyConfig::new();
        let policy_set = config.policy_set();
        
        assert!(!policy_set.deny_deletes());
        assert!(!policy_set.deny_non_fast_forwards());
        assert_eq!(policy_set.current_branch(), Policy::Allow);
        assert_eq!(policy_set.delete_current(), Policy::Allow);
        assert!(!policy_set.update_instead());
    }

    #[test]
    fn test_parse_boolean_policies() {
        let config = create_config_with_values(&[
            ("receive.denyDeletes", "true"),
            ("receive.denyNonFastForwards", "false"),
            ("receive.updateInstead", "yes"),
        ]);

        let policy_config = PolicyConfig::from_config(&config).unwrap();
        let policy_set = policy_config.policy_set();

        assert!(policy_set.deny_deletes());
        assert!(!policy_set.deny_non_fast_forwards());
        assert!(policy_set.update_instead());
    }

    #[test]
    fn test_parse_string_policies() {
        let config = create_config_with_values(&[
            ("receive.denyCurrentBranch", "refuse"),
            ("receive.denyDeleteCurrent", "warn"),
        ]);

        let policy_config = PolicyConfig::from_config(&config).unwrap();
        let policy_set = policy_config.policy_set();

        assert_eq!(policy_set.current_branch(), Policy::Deny);
        assert_eq!(policy_set.delete_current(), Policy::Warn);
    }

    #[test]
    fn test_parse_boolean_as_policy() {
        let config = create_config_with_values(&[
            ("receive.denyCurrentBranch", "true"),
            ("receive.denyDeleteCurrent", "false"),
        ]);

        let policy_config = PolicyConfig::from_config(&config).unwrap();
        let policy_set = policy_config.policy_set();

        assert_eq!(policy_set.current_branch(), Policy::Deny);
        assert_eq!(policy_set.delete_current(), Policy::Allow);
    }

    #[test]
    fn test_parse_case_insensitive_policies() {
        let config = create_config_with_values(&[
            ("receive.denyCurrentBranch", "REFUSE"),
            ("receive.denyDeleteCurrent", "Allow"),
        ]);

        let policy_config = PolicyConfig::from_config(&config).unwrap();
        let policy_set = policy_config.policy_set();

        assert_eq!(policy_set.current_branch(), Policy::Deny);
        assert_eq!(policy_set.delete_current(), Policy::Allow);
    }

    #[test]
    fn test_invalid_boolean_value() {
        let config = create_config_with_values(&[
            ("receive.denyDeletes", "maybe"),
        ]);

        let result = PolicyConfig::from_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid boolean value"));
    }

    #[test]
    fn test_invalid_policy_value() {
        let config = create_config_with_values(&[
            ("receive.denyCurrentBranch", "invalid"),
        ]);

        let result = PolicyConfig::from_config(&config);
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("invalid policy value"));
        assert!(error_msg.contains("refuse, deny, warn, allow, ignore, true, false"));
    }

    #[test]
    fn test_empty_config() {
        let config = File::new(gix_config::file::Metadata::api());
        let policy_config = PolicyConfig::from_config(&config).unwrap();
        let policy_set = policy_config.policy_set();

        // Should use defaults
        assert!(!policy_set.deny_deletes());
        assert!(!policy_set.deny_non_fast_forwards());
        assert_eq!(policy_set.current_branch(), Policy::Allow);
        assert_eq!(policy_set.delete_current(), Policy::Allow);
        assert!(!policy_set.update_instead());
    }

    #[test]
    fn test_mixed_configuration() {
        let config = create_config_with_values(&[
            ("receive.denyDeletes", "true"),
            ("receive.denyNonFastForwards", "false"),
            ("receive.denyCurrentBranch", "warn"),
            ("receive.denyDeleteCurrent", "true"),
            ("receive.updateInstead", "yes"),
        ]);

        let policy_config = PolicyConfig::from_config(&config).unwrap();
        let policy_set = policy_config.policy_set();

        assert!(policy_set.deny_deletes());
        assert!(!policy_set.deny_non_fast_forwards());
        assert_eq!(policy_set.current_branch(), Policy::Warn);
        assert_eq!(policy_set.delete_current(), Policy::Deny);
        assert!(policy_set.update_instead());
    }
}