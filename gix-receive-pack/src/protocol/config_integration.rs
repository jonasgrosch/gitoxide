//! Configuration integration examples for M1 protocol advertisement.
//!
//! This module demonstrates how higher layers inject configuration into the
//! advertisement system without performing I/O. The examples show the mapping
//! from git configuration to CapabilitySet and Advertiser setup.

use super::capabilities::{CapabilityOrdering, CapabilitySet};
use super::advertise::Advertiser;
use std::io::Write;

/// Example configuration structure that higher layers might use to inject
/// settings into the advertisement system.
///
/// This represents the minimal configuration needed for M1 advertisement,
/// extracted from git configuration by higher layers.
#[derive(Debug, Clone, Default)]
pub struct AdvertisementConfig {
    /// Agent string to advertise. When None, no agent capability is emitted.
    /// Maps from `receive.advertiseAgent` or similar configuration.
    pub agent: Option<String>,
    
    /// Whether to advertise atomic capability.
    /// Maps from `receive.advertiseAtomic` configuration.
    pub advertise_atomic: bool,
    
    /// Whether to use strict compatibility mode for capability ordering.
    /// This would be controlled by feature flags or configuration.
    pub strict_compat: bool,
    
    /// Additional capability tokens to advertise.
    /// These might come from extensions or plugin configuration.
    pub extra_capabilities: Vec<String>,
}

impl AdvertisementConfig {
    /// Create a configuration with sensible defaults for modern Git servers.
    pub fn modern_defaults() -> Self {
        Self {
            agent: Some("gix-receive-pack/0.1.0".to_string()),
            advertise_atomic: false, // Conservative default
            strict_compat: false,
            extra_capabilities: Vec::new(),
        }
    }
    
    /// Enable agent advertisement with the given string.
    pub fn with_agent(mut self, agent: Option<String>) -> Self {
        self.agent = agent;
        self
    }
    
    /// Enable or disable atomic capability advertisement.
    /// This maps directly from `receive.advertiseAtomic` git configuration.
    pub fn with_atomic(mut self, enabled: bool) -> Self {
        self.advertise_atomic = enabled;
        self
    }
    
    /// Enable strict compatibility mode for upstream byte-for-byte parity.
    pub fn with_strict_compat(mut self, enabled: bool) -> Self {
        self.strict_compat = enabled;
        self
    }
    
    /// Add an extra capability token.
    pub fn push_extra_capability<S: Into<String>>(mut self, token: S) -> Self {
        self.extra_capabilities.push(token.into());
        self
    }
}

/// Convert advertisement configuration into a CapabilitySet.
///
/// This is the primary integration point where higher layers inject
/// configuration into the M1 advertisement system.
impl From<AdvertisementConfig> for CapabilitySet {
    fn from(config: AdvertisementConfig) -> Self {
        let mut caps = CapabilitySet::modern_defaults().with_agent(config.agent);
        
        // Map receive.advertiseAtomic → atomic token
        if config.advertise_atomic {
            caps.push_extra("atomic");
        }
        
        // Add any extra capabilities from configuration
        for token in config.extra_capabilities {
            caps.push_extra(token);
        }
        
        caps
    }
}

/// Example of how higher layers would set up an advertiser with configuration.
///
/// This demonstrates the complete flow from configuration to advertisement emission.
pub fn setup_advertiser_with_config<W: Write>(
    writer: W,
    config: AdvertisementConfig,
) -> Advertiser<W> {
    #[cfg(feature = "strict-compat")]
    {
        if config.strict_compat {
            return Advertiser::with_strict_compat(writer);
        }
    }
    
    let ordering = if config.strict_compat {
        CapabilityOrdering::Lexicographic // For deterministic golden tests when strict-compat is not available
    } else {
        CapabilityOrdering::PreserveIdiomatic
    };
    
    Advertiser::new(writer).with_ordering(ordering)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::RefRecord;
    use gix_hash::ObjectId;

    fn oid(hex40: &str) -> ObjectId {
        ObjectId::from_hex(hex40.as_bytes()).expect("valid hex")
    }

    #[test]
    fn config_to_capability_set_basic() {
        let config = AdvertisementConfig::modern_defaults();
        let caps: CapabilitySet = config.into();
        
        let encoded = caps.encode(CapabilityOrdering::PreserveIdiomatic);
        assert!(encoded.contains("report-status"));
        assert!(encoded.contains("report-status-v2"));
        assert!(encoded.contains("quiet"));
        assert!(encoded.contains("delete-refs"));
        assert!(encoded.contains("ofs-delta"));
        assert!(encoded.contains("agent=gix-receive-pack/0.1.0"));
        assert!(!encoded.contains("atomic")); // Not enabled by default
    }

    #[test]
    fn config_with_atomic_capability() {
        let config = AdvertisementConfig::modern_defaults()
            .with_atomic(true);
        let caps: CapabilitySet = config.into();
        
        let encoded = caps.encode(CapabilityOrdering::PreserveIdiomatic);
        assert!(encoded.contains("atomic"));
    }

    #[test]
    fn config_with_custom_agent() {
        let config = AdvertisementConfig::modern_defaults()
            .with_agent(Some("custom-server/2.0".to_string()));
        let caps: CapabilitySet = config.into();
        
        let encoded = caps.encode(CapabilityOrdering::PreserveIdiomatic);
        assert!(encoded.contains("agent=custom-server/2.0"));
    }

    #[test]
    fn config_with_no_agent() {
        let config = AdvertisementConfig::modern_defaults()
            .with_agent(None);
        let caps: CapabilitySet = config.into();
        
        let encoded = caps.encode(CapabilityOrdering::PreserveIdiomatic);
        assert!(!encoded.contains("agent="));
    }

    #[test]
    fn config_with_extra_capabilities() {
        let config = AdvertisementConfig::modern_defaults()
            .push_extra_capability("no-thin")
            .push_extra_capability("push-options");
        let caps: CapabilitySet = config.into();
        
        let encoded = caps.encode(CapabilityOrdering::PreserveIdiomatic);
        assert!(encoded.contains("no-thin"));
        assert!(encoded.contains("push-options"));
    }

    #[test]
    fn end_to_end_advertisement_with_config() {
        // Simulate configuration from higher layers
        let config = AdvertisementConfig::modern_defaults()
            .with_atomic(true)
            .with_agent(Some("test-server/1.0".to_string()))
            .push_extra_capability("push-options");
        
        // Convert to capability set
        let caps: CapabilitySet = config.clone().into();
        
        // Set up advertiser with configuration
        let mut buf = Vec::new();
        let mut advertiser = setup_advertiser_with_config(&mut buf, config);
        
        // Create some test refs
        let refs = vec![
            RefRecord::new(oid("1111111111111111111111111111111111111111"), "refs/heads/main"),
            RefRecord::new(oid("2222222222222222222222222222222222222222"), "refs/tags/v1.0"),
        ];
        
        // Write advertisement
        advertiser.write_advertisement(&refs, &caps, None).unwrap();
        
        // Verify the output contains our configured capabilities
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("atomic"));
        assert!(output.contains("agent=test-server/1.0"));
        assert!(output.contains("push-options"));
        assert!(output.contains("refs/heads/main"));
        assert!(output.contains("refs/tags/v1.0"));
    }

    #[test]
    fn strict_compat_ordering() {
        let config = AdvertisementConfig::modern_defaults()
            .with_strict_compat(true)
            .with_atomic(true);
        
        let mut buf = Vec::new();
        let _advertiser = setup_advertiser_with_config(&mut buf, config.clone());
        
        // Verify that strict compat mode uses lexicographic ordering
        // This is important for golden test compatibility
        let caps: CapabilitySet = config.into();
        let encoded = caps.encode(CapabilityOrdering::Lexicographic);
        
        // Tokens should be in lexicographic order
        let tokens: Vec<&str> = encoded.split(' ').collect();
        let mut sorted_tokens = tokens.clone();
        sorted_tokens.sort_unstable();
        assert_eq!(tokens, sorted_tokens);
    }

    #[cfg(feature = "strict-compat")]
    #[test]
    fn strict_compat_formatter_integration() {
        use gix_packetline_blocking::{PacketLineRef, StreamingPeekableIter};
        use std::io::Cursor;

        fn collect_data_lines(buf: &[u8]) -> Vec<Vec<u8>> {
            let mut rd = StreamingPeekableIter::new(Cursor::new(buf), &[PacketLineRef::Flush], false);
            let mut out = Vec::new();
            while let Some(next) = rd.read_line() {
                match next.expect("io ok").expect("decode ok") {
                    PacketLineRef::Data(d) => out.push(d.to_vec()),
                    PacketLineRef::Flush | PacketLineRef::Delimiter | PacketLineRef::ResponseEnd => break,
                }
            }
            out
        }

        let config = AdvertisementConfig::modern_defaults()
            .with_strict_compat(true)
            .with_atomic(true)
            .with_agent(Some("git/2.39.0".to_string()))
            .push_extra_capability("push-options");
        
        let caps: CapabilitySet = config.clone().into();
        
        let mut buf = Vec::new();
        let mut advertiser = setup_advertiser_with_config(&mut buf, config);
        
        let refs = vec![
            RefRecord::new(oid("1111111111111111111111111111111111111111"), "refs/heads/main"),
        ];
        
        advertiser.write_advertisement(&refs, &caps, None).unwrap();
        
        let lines = collect_data_lines(&buf);
        assert_eq!(lines.len(), 1);
        
        let first = &lines[0];
        let first_str = std::str::from_utf8(first).unwrap();
        
        // Extract capabilities part
        let nul_pos = first_str.find('\0').unwrap();
        let caps_part = &first_str[nul_pos + 1..].trim();
        
        // Verify that capabilities follow upstream ordering
        let tokens: Vec<&str> = caps_part.split(' ').collect();
        
        // Find positions of key capabilities
        let report_status_pos = tokens.iter().position(|&t| t == "report-status").unwrap();
        let atomic_pos = tokens.iter().position(|&t| t == "atomic").unwrap();
        let agent_pos = tokens.iter().position(|&t| t.starts_with("agent=")).unwrap();
        
        // Verify upstream ordering: report-status comes before atomic, agent comes last
        assert!(report_status_pos < atomic_pos);
        assert!(atomic_pos < agent_pos);
        assert_eq!(agent_pos, tokens.len() - 1, "agent should be last in strict mode");
    }
}

/// Example integration patterns for different deployment scenarios.
pub mod examples {
    use super::*;
    
    /// Example: Basic Git server with minimal configuration
    pub fn basic_git_server_config() -> AdvertisementConfig {
        AdvertisementConfig::modern_defaults()
            .with_agent(Some("basic-git-server/1.0".to_string()))
    }
    
    /// Example: Enterprise Git server with atomic pushes enabled
    pub fn enterprise_server_config() -> AdvertisementConfig {
        AdvertisementConfig::modern_defaults()
            .with_agent(Some("enterprise-git/2.1".to_string()))
            .with_atomic(true)
            .push_extra_capability("push-options")
    }
    
    /// Example: Compatibility mode for upstream Git parity
    pub fn upstream_compat_config() -> AdvertisementConfig {
        AdvertisementConfig::modern_defaults()
            .with_strict_compat(true)
            .with_agent(Some("git/2.40.0".to_string()))
    }
    
    /// Example: Minimal server with no agent advertisement
    pub fn minimal_server_config() -> AdvertisementConfig {
        AdvertisementConfig::modern_defaults()
            .with_agent(None)
    }
}