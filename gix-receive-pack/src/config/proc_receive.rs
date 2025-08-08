//! Proc-receive configuration parsing from Git config.

use crate::Error;
use gix_config::File;
use gix_object::bstr::BStr;
use std::path::PathBuf;
use std::time::Duration;

/// Configuration for proc-receive protocol handling.
///
/// This struct holds configuration values that control proc-receive
/// protocol negotiation and helper execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcReceiveConfig {
    /// Whether proc-receive protocol is enabled
    pub enabled: bool,
    /// Path to the proc-receive helper executable
    pub helper_path: Option<PathBuf>,
    /// Protocol version to negotiate (default: 1)
    pub version: u32,
    /// Timeout for helper operations in milliseconds
    pub timeout_ms: u64,
}

impl ProcReceiveConfig {
    /// Create a new ProcReceiveConfig with default settings.
    pub fn new() -> Self {
        Self {
            enabled: false,
            helper_path: None,
            version: 1,
            timeout_ms: 30_000, // 30 seconds default
        }
    }

    /// Load proc-receive configuration from a Git config file.
    ///
    /// This method parses the following configuration keys:
    /// - `procReceive.enabled`: Boolean, enable proc-receive protocol (default: false)
    /// - `procReceive.helperPath`: String, path to helper executable
    /// - `procReceive.version`: Integer, protocol version (default: 1)
    /// - `procReceive.timeout`: Integer, timeout in milliseconds (default: 30000)
    ///
    /// # Arguments
    /// * `config` - Git configuration file to parse
    ///
    /// # Returns
    /// A ProcReceiveConfig with parsed settings, or Error if parsing fails
    pub fn from_config(config: &File<'static>) -> Result<Self, Error> {
        let mut proc_config = Self::new();

        // Parse procReceive.enabled
        if let Some(result) = config.boolean("procReceive.enabled") {
            match result {
                Ok(value) => proc_config.enabled = value,
                Err(e) => return Err(Error::Validation(format!("invalid boolean value for 'procReceive.enabled': {}", e))),
            }
        }

        // Parse procReceive.helperPath
        if let Some(value) = config.string("procReceive.helperPath") {
            proc_config.helper_path = Some(parse_path_from_string(&value, "procReceive.helperPath")?);
        }

        // Parse procReceive.version
        if let Some(result) = config.integer("procReceive.version") {
            match result {
                Ok(value) => proc_config.version = parse_version_from_integer(value, "procReceive.version")?,
                Err(e) => return Err(Error::Validation(format!("invalid integer value for 'procReceive.version': {}", e))),
            }
        }

        // Parse procReceive.timeout
        if let Some(result) = config.integer("procReceive.timeout") {
            match result {
                Ok(value) => proc_config.timeout_ms = parse_timeout_from_integer(value, "procReceive.timeout")?,
                Err(e) => return Err(Error::Validation(format!("invalid integer value for 'procReceive.timeout': {}", e))),
            }
        }

        Ok(proc_config)
    }

    /// Check if proc-receive is enabled and has a helper path configured.
    pub fn is_available(&self) -> bool {
        self.enabled && self.helper_path.is_some()
    }

    /// Get the helper path if available.
    pub fn helper_path(&self) -> Option<&PathBuf> {
        self.helper_path.as_ref()
    }

    /// Get the timeout as a Duration.
    pub fn timeout(&self) -> Duration {
        Duration::from_millis(self.timeout_ms)
    }

    /// Get the protocol version.
    pub fn protocol_version(&self) -> u32 {
        self.version
    }

    /// Check if proc-receive is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
}

impl Default for ProcReceiveConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse a file path configuration value from a string.
fn parse_path_from_string(value: &std::borrow::Cow<'_, BStr>, key: &str) -> Result<PathBuf, Error> {
    let path_str = std::str::from_utf8(value.as_ref())
        .map_err(|e| Error::Validation(format!("invalid UTF-8 in '{}': {}", key, e)))?;
    
    if path_str.is_empty() {
        return Err(Error::Validation(format!(
            "empty path value for '{}'",
            key
        )));
    }
    
    Ok(PathBuf::from(path_str))
}

/// Parse a protocol version value from an i64.
fn parse_version_from_integer(value: i64, key: &str) -> Result<u32, Error> {
    if value < 1 {
        return Err(Error::Validation(format!(
            "protocol version for '{}' must be at least 1, got: {}",
            key, value
        )));
    }
    
    Ok(value as u32)
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
    fn test_default_proc_receive_config() {
        let config = ProcReceiveConfig::new();
        
        assert!(!config.enabled);
        assert!(config.helper_path.is_none());
        assert_eq!(config.version, 1);
        assert_eq!(config.timeout_ms, 30_000);
        assert!(!config.is_available());
    }

    #[test]
    fn test_parse_enabled() {
        let config = create_config_with_values(&[
            ("procReceive.enabled", "true"),
        ]);

        let proc_config = ProcReceiveConfig::from_config(&config).unwrap();
        assert!(proc_config.enabled);
        assert!(proc_config.is_enabled());
    }

    #[test]
    fn test_parse_helper_path() {
        let config = create_config_with_values(&[
            ("procReceive.helperPath", "/usr/local/bin/proc-receive-helper"),
        ]);

        let proc_config = ProcReceiveConfig::from_config(&config).unwrap();
        assert_eq!(
            proc_config.helper_path(),
            Some(&PathBuf::from("/usr/local/bin/proc-receive-helper"))
        );
    }

    #[test]
    fn test_parse_version() {
        let config = create_config_with_values(&[
            ("procReceive.version", "2"),
        ]);

        let proc_config = ProcReceiveConfig::from_config(&config).unwrap();
        assert_eq!(proc_config.version, 2);
        assert_eq!(proc_config.protocol_version(), 2);
    }

    #[test]
    fn test_parse_timeout() {
        let config = create_config_with_values(&[
            ("procReceive.timeout", "60000"),
        ]);

        let proc_config = ProcReceiveConfig::from_config(&config).unwrap();
        assert_eq!(proc_config.timeout_ms, 60_000);
        assert_eq!(proc_config.timeout(), Duration::from_millis(60_000));
    }

    #[test]
    fn test_is_available_with_enabled_and_path() {
        let config = create_config_with_values(&[
            ("procReceive.enabled", "true"),
            ("procReceive.helperPath", "/usr/bin/helper"),
        ]);

        let proc_config = ProcReceiveConfig::from_config(&config).unwrap();
        assert!(proc_config.is_available());
    }

    #[test]
    fn test_is_available_enabled_without_path() {
        let config = create_config_with_values(&[
            ("procReceive.enabled", "true"),
        ]);

        let proc_config = ProcReceiveConfig::from_config(&config).unwrap();
        assert!(!proc_config.is_available()); // No helper path
    }

    #[test]
    fn test_is_available_path_without_enabled() {
        let config = create_config_with_values(&[
            ("procReceive.helperPath", "/usr/bin/helper"),
        ]);

        let proc_config = ProcReceiveConfig::from_config(&config).unwrap();
        assert!(!proc_config.is_available()); // Not enabled
    }

    #[test]
    fn test_invalid_boolean() {
        let config = create_config_with_values(&[
            ("procReceive.enabled", "maybe"),
        ]);

        let result = ProcReceiveConfig::from_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid boolean value"));
    }

    #[test]
    fn test_empty_helper_path() {
        let config = create_config_with_values(&[
            ("procReceive.helperPath", ""),
        ]);

        let result = ProcReceiveConfig::from_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty path value"));
    }

    #[test]
    fn test_invalid_version() {
        let config = create_config_with_values(&[
            ("procReceive.version", "0"),
        ]);

        let result = ProcReceiveConfig::from_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must be at least 1"));
    }

    #[test]
    fn test_negative_version() {
        let config = create_config_with_values(&[
            ("procReceive.version", "-1"),
        ]);

        let result = ProcReceiveConfig::from_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must be at least 1"));
    }

    #[test]
    fn test_invalid_timeout() {
        let config = create_config_with_values(&[
            ("procReceive.timeout", "-1000"),
        ]);

        let result = ProcReceiveConfig::from_config(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must be non-negative"));
    }

    #[test]
    fn test_empty_config() {
        let config = File::new(gix_config::file::Metadata::api());
        let proc_config = ProcReceiveConfig::from_config(&config).unwrap();

        // Should use defaults
        assert!(!proc_config.enabled);
        assert!(proc_config.helper_path.is_none());
        assert_eq!(proc_config.version, 1);
        assert_eq!(proc_config.timeout_ms, 30_000);
        assert!(!proc_config.is_available());
    }

    #[test]
    fn test_mixed_configuration() {
        let config = create_config_with_values(&[
            ("procReceive.enabled", "true"),
            ("procReceive.helperPath", "/opt/git-hooks/proc-receive"),
            ("procReceive.version", "2"),
            ("procReceive.timeout", "45000"),
        ]);

        let proc_config = ProcReceiveConfig::from_config(&config).unwrap();
        
        assert!(proc_config.enabled);
        assert_eq!(
            proc_config.helper_path(),
            Some(&PathBuf::from("/opt/git-hooks/proc-receive"))
        );
        assert_eq!(proc_config.version, 2);
        assert_eq!(proc_config.timeout_ms, 45_000);
        assert!(proc_config.is_available());
    }

    #[test]
    fn test_zero_timeout() {
        let config = create_config_with_values(&[
            ("procReceive.timeout", "0"),
        ]);

        let proc_config = ProcReceiveConfig::from_config(&config).unwrap();
        assert_eq!(proc_config.timeout_ms, 0);
        assert_eq!(proc_config.timeout(), Duration::from_millis(0));
    }
}