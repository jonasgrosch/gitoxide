//! Hook configuration parsing from Git config.

use crate::Error;
use gix_config::File;

use std::collections::HashMap;
use std::time::Duration;

/// Configuration for hook execution.
///
/// This struct holds configuration values that control how hooks are executed,
/// including timeouts, output limits, and sideband relay settings.
///
/// Note: Environment variables (GIT_DIR, GIT_WORK_TREE, GIT_PUSH_OPTION_*, etc.)
/// are automatically set by the Git receive-pack process and don't need to be
/// configured here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookConfig {
    /// Timeout for hook execution in milliseconds
    pub timeout_ms: u64,
    /// Maximum output size in bytes (stdout + stderr combined)
    pub max_output_size: usize,
    /// Whether to enable sideband relay for hook output
    pub sideband_relay: bool,
    /// Reserved for future use - environment variables are handled automatically
    pub environment: HashMap<String, String>,
}

impl HookConfig {
    /// Create a new HookConfig with default settings.
    pub fn new() -> Self {
        Self {
            timeout_ms: 30_000,           // 30 seconds default
            max_output_size: 1024 * 1024, // 1MB default
            sideband_relay: true,
            environment: HashMap::new(),
        }
    }

    /// Load hook configuration from a Git config file.
    ///
    /// This method parses the following configuration keys:
    /// - `hooks.timeout`: Timeout in milliseconds (default: 30000)
    /// - `hooks.maxOutputSize`: Maximum output size in bytes (default: 1048576)
    /// - `hooks.sidebandRelay`: Enable sideband relay (default: true)
    ///
    /// Note: Environment variables are automatically set by Git's receive-pack
    /// process and don't need configuration (GIT_DIR, GIT_WORK_TREE, etc.).
    ///
    /// # Arguments
    /// * `config` - Git configuration file to parse
    ///
    /// # Returns
    /// A HookConfig with parsed settings, or Error if parsing fails
    pub fn from_config(config: &File<'static>) -> Result<Self, Error> {
        let mut hook_config = Self::new();

        // Parse hooks.timeout
        if let Some(result) = config.integer("hooks.timeout") {
            match result {
                Ok(value) => hook_config.timeout_ms = parse_timeout_from_integer(value, "hooks.timeout")?,
                Err(e) => {
                    return Err(Error::Validation(format!(
                        "invalid integer value for 'hooks.timeout': {}",
                        e
                    )))
                }
            }
        }

        // Parse hooks.maxOutputSize
        if let Some(result) = config.integer("hooks.maxOutputSize") {
            match result {
                Ok(value) => hook_config.max_output_size = parse_size_from_integer(value, "hooks.maxOutputSize")?,
                Err(e) => {
                    return Err(Error::Validation(format!(
                        "invalid integer value for 'hooks.maxOutputSize': {}",
                        e
                    )))
                }
            }
        }

        // Parse hooks.sidebandRelay
        if let Some(result) = config.boolean("hooks.sidebandRelay") {
            match result {
                Ok(value) => hook_config.sideband_relay = value,
                Err(e) => {
                    return Err(Error::Validation(format!(
                        "invalid boolean value for 'hooks.sidebandRelay': {}",
                        e
                    )))
                }
            }
        }

        Ok(hook_config)
    }

    /// Get the timeout as a Duration.
    pub fn timeout(&self) -> Duration {
        Duration::from_millis(self.timeout_ms)
    }

    /// Check if sideband relay is enabled.
    pub fn is_sideband_relay_enabled(&self) -> bool {
        self.sideband_relay
    }

    /// Get additional environment variables.
    pub fn environment_vars(&self) -> &HashMap<String, String> {
        &self.environment
    }

    /// Get the maximum output size in bytes.
    pub fn max_output_size(&self) -> usize {
        self.max_output_size
    }
}

impl Default for HookConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse a timeout value in milliseconds from an i64.
fn parse_timeout_from_integer(value: i64, key: &str) -> Result<u64, Error> {
    if value < 0 {
        return Err(Error::Validation(format!(
            "timeout value for '{}' must be non-negative, got: {}",
            key, value
        )));
    }

    Ok(value as u64)
}

/// Parse a size value in bytes from an i64.
fn parse_size_from_integer(value: i64, key: &str) -> Result<usize, Error> {
    if value < 0 {
        return Err(Error::Validation(format!(
            "size value for '{}' must be non-negative, got: {}",
            key, value
        )));
    }

    Ok(value as usize)
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
            match parts.len() {
                2 => {
                    let section = parts[0].to_string();
                    let key_name = parts[1].to_string();
                    sections
                        .entry(section)
                        .or_insert_with(Vec::new)
                        .push((key_name, value.to_string()));
                }
                3 => {
                    // For 3-part keys, just use the first part as section and combine the rest
                    // This is mainly for testing - in practice, hooks don't need complex config keys
                    let section = parts[0].to_string();
                    let key_name = format!("{}.{}", parts[1], parts[2]);
                    sections
                        .entry(section)
                        .or_insert_with(Vec::new)
                        .push((key_name, value.to_string()));
                }
                _ => {
                    // For now, just treat as a regular key-value in the first part as section
                    let section = parts[0].to_string();
                    let key_name = parts[1..].join(".");
                    sections
                        .entry(section)
                        .or_insert_with(Vec::new)
                        .push((key_name, value.to_string()));
                }
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
    fn test_default_hook_config() {
        let config = HookConfig::new();

        assert_eq!(config.timeout_ms, 30_000);
        assert_eq!(config.max_output_size, 1024 * 1024);
        assert!(config.sideband_relay);
        assert!(config.environment.is_empty());
    }

    #[test]
    fn test_parse_timeout() {
        let config = create_config_with_values(&[("hooks.timeout", "60000")]);

        let hook_config = HookConfig::from_config(&config).unwrap();
        assert_eq!(hook_config.timeout_ms, 60_000);
        assert_eq!(hook_config.timeout(), Duration::from_millis(60_000));
    }

    #[test]
    fn test_parse_max_output_size() {
        let config = create_config_with_values(&[
            ("hooks.maxOutputSize", "2097152"), // 2MB
        ]);

        let hook_config = HookConfig::from_config(&config).unwrap();
        assert_eq!(hook_config.max_output_size, 2 * 1024 * 1024);
    }

    #[test]
    fn test_parse_sideband_relay() {
        let config = create_config_with_values(&[("hooks.sidebandRelay", "false")]);

        let hook_config = HookConfig::from_config(&config).unwrap();
        assert!(!hook_config.sideband_relay);
        assert!(!hook_config.is_sideband_relay_enabled());
    }

    #[test]
    fn test_invalid_timeout() {
        let config = create_config_with_values(&[("hooks.timeout", "-1000")]);

        let result = HookConfig::from_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must be non-negative"));
    }

    #[test]
    fn test_invalid_size() {
        let config = create_config_with_values(&[("hooks.maxOutputSize", "-1")]);

        let result = HookConfig::from_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must be non-negative"));
    }

    #[test]
    fn test_invalid_boolean() {
        let config = create_config_with_values(&[("hooks.sidebandRelay", "maybe")]);

        let result = HookConfig::from_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid boolean value"));
    }

    #[test]
    fn test_empty_config() {
        let config = File::new(gix_config::file::Metadata::api());
        let hook_config = HookConfig::from_config(&config).unwrap();

        // Should use defaults
        assert_eq!(hook_config.timeout_ms, 30_000);
        assert_eq!(hook_config.max_output_size, 1024 * 1024);
        assert!(hook_config.sideband_relay);
        assert!(hook_config.environment.is_empty());
    }

    #[test]
    fn test_mixed_configuration() {
        let config = create_config_with_values(&[
            ("hooks.timeout", "45000"),
            ("hooks.maxOutputSize", "512000"),
            ("hooks.sidebandRelay", "true"),
        ]);

        let hook_config = HookConfig::from_config(&config).unwrap();

        assert_eq!(hook_config.timeout_ms, 45_000);
        assert_eq!(hook_config.max_output_size, 512_000);
        assert!(hook_config.sideband_relay);

        // Environment variables are handled automatically by Git
        let env_vars = hook_config.environment_vars();
        assert!(env_vars.is_empty());
    }

    #[test]
    fn test_zero_timeout() {
        let config = create_config_with_values(&[("hooks.timeout", "0")]);

        let hook_config = HookConfig::from_config(&config).unwrap();
        assert_eq!(hook_config.timeout_ms, 0);
        assert_eq!(hook_config.timeout(), Duration::from_millis(0));
    }

    #[test]
    fn test_zero_max_output_size() {
        let config = create_config_with_values(&[("hooks.maxOutputSize", "0")]);

        let hook_config = HookConfig::from_config(&config).unwrap();
        assert_eq!(hook_config.max_output_size, 0);
    }
}
