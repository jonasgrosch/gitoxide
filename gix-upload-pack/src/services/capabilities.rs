//! Capability management for upload-pack
//!
//! This module handles the advertisement and negotiation of capabilities
//! between the client and server during the upload-pack protocol.

use crate::{
    config::ServerOptions,
    error::{Error, Result},
    types::*,
};
use bstr::ByteSlice;
use gix::Repository;
use gix_protocol::Command;
use gix_transport::client::Capabilities;

/// Capability manager for handling server and client capabilities
pub struct CapabilityManager<'a> {
    #[allow(dead_code)]
    repository: &'a Repository,
    options: &'a ServerOptions,
}

impl<'a> CapabilityManager<'a> {
    /// Create a new capability manager
    pub fn new(repository: &'a Repository, options: &'a ServerOptions) -> Self {
        Self { repository, options }
    }
    
    /// Build server capabilities using gix-protocol defaults
    pub fn build_server_capabilities(&self, protocol_version: ProtocolVersion) -> Result<Capabilities> {
        match protocol_version {
            ProtocolVersion::V0 | ProtocolVersion::V1 => {
                // For V1, build capabilities using gix-protocol defaults
                let fetch_command = Command::Fetch;
                let default_features = fetch_command.default_features(protocol_version, &Capabilities::default());
                
                // Build capability string from default features
                let caps_string = self.build_v1_capabilities_from_features(&default_features);
                let full_string = format!("\0{}", caps_string);
                
                Capabilities::from_bytes(full_string.as_bytes())
                    .map(|(caps, _)| caps)
                    .map_err(|e| Error::ProtocolParsing(format!("Failed to parse V1 capabilities: {}", e)))
            }
            ProtocolVersion::V2 => {
                // For V2, build capabilities using gix-protocol command features
                let capabilities_string = self.build_v2_capabilities_with_commands();
                let full_string = format!("version 2\n{}", capabilities_string);
                Capabilities::from_lines(full_string.into())
                    .map_err(|e| Error::ProtocolParsing(format!("Failed to parse V2 capabilities: {}", e)))
            }
        }
    }
    
    /// Build V1 capabilities from gix-protocol features
    fn build_v1_capabilities_from_features(&self, features: &[(&str, Option<std::borrow::Cow<'static, str>>)]) -> String {
        let mut cap_strings = Vec::new();
        
        // Add protocol-defined features
        for (feature, value) in features {
            if let Some(val) = value {
                cap_strings.push(format!("{}={}", feature, val));
            } else {
                cap_strings.push(feature.to_string());
            }
        }
        
        // Add server-specific capabilities
        if self.options.allow_filter {
            cap_strings.push("filter".to_string());
        }
        if self.options.allow_tip_sha1_in_want {
            cap_strings.push("allow-tip-sha1-in-want".to_string());
        }
        if self.options.allow_reachable_sha1_in_want {
            cap_strings.push("allow-reachable-sha1-in-want".to_string());
        }
        
        // Agent string
        cap_strings.push(format!("agent=git/gitoxide-{}", crate::VERSION));
        
        cap_strings.join(" ")
    }
    
    /// Build V2 capabilities with proper command integration
    fn build_v2_capabilities_with_commands(&self) -> String {
        let mut caps = Vec::new();
        
        // Agent capability
        caps.push(format!("agent=git/gitoxide-{}", crate::VERSION));
        
        // Object format
        caps.push("object-format=sha1".to_string());
        
        // Get default capabilities for V2
        let default_caps = Capabilities::default();
        
        // Fetch command with its features
        let fetch_command = Command::Fetch;
        let fetch_features = fetch_command.default_features(ProtocolVersion::V2, &default_caps);
        let mut fetch_cap_strings = Vec::new();
        
        for (feature, _) in &fetch_features {
            if *feature != "fetch" { // Don't include the command name itself
                fetch_cap_strings.push(feature.to_string());
            }
        }
        
        // Add server-specific fetch capabilities
        if self.options.allow_filter {
            if !fetch_cap_strings.contains(&"filter".to_string()) {
                fetch_cap_strings.push("filter".to_string());
            }
        }
        
        caps.push(format!("fetch={}", fetch_cap_strings.join(" ")));
        
        // ls-refs command with its features
        let ls_refs_command = Command::LsRefs;
        let ls_refs_features = ls_refs_command.default_features(ProtocolVersion::V2, &default_caps);
        let mut ls_refs_cap_strings = vec!["symrefs".to_string(), "peel".to_string(), "unborn".to_string()];
        
        for (feature, _) in &ls_refs_features {
            if *feature != "ls-refs" && !ls_refs_cap_strings.contains(&feature.to_string()) {
                ls_refs_cap_strings.push(feature.to_string());
            }
        }

        caps.push(format!("ls-refs={}", ls_refs_cap_strings.join(" ")));
        
        caps.join("\n")
    }
    

    
    /// Get the default server capabilities based on repository and configuration
    pub fn default_server_capabilities(&self) -> ServerCapabilities {
        ServerCapabilities::default()
    }
    
    /// Parse client capabilities from capability string (centralized from v1)
    /// This replaces the duplicate parsing logic in v1 protocol
    pub fn parse_client_capabilities(&self, caps_str: &str) -> Result<ClientCapabilities> {
        let mut capabilities = ClientCapabilities::default();
        
        for cap in caps_str.split_whitespace() {
            match cap {
                "multi_ack" => capabilities.multi_ack = MultiAckMode::Basic,
                "multi_ack_detailed" => capabilities.multi_ack = MultiAckMode::Detailed,
                "thin-pack" => capabilities.thin_pack = true,
                cap if SideBandMode::from_capability_string(cap).is_some() => {
                    capabilities.side_band = SideBandMode::from_capability_string(cap).unwrap();
                }
                "ofs-delta" => capabilities.ofs_delta = true,
                "include-tag" => capabilities.include_tag = true,
                "no-progress" => capabilities.no_progress = true,
                "allow-tip-sha1-in-want" => capabilities.allow_tip_sha1_in_want = true,
                "allow-reachable-sha1-in-want" => capabilities.allow_reachable_sha1_in_want = true,
                "deepen-relative" => capabilities.deepen_relative = true,
                "shallow" => capabilities.shallow = true,
                cap if cap.starts_with("filter=") => {
                    capabilities.filter = Some(cap["filter=".len()..].into());
                }
                cap if cap.starts_with("agent=") => {
                    capabilities.agent = Some(cap["agent=".len()..].into());
                }
                cap if cap.starts_with("session-id=") => {
                    capabilities.session_id = Some(cap["session-id=".len()..].into());
                }
                cap if cap.starts_with("object-format=") => {
                    let format_name = &cap["object-format=".len()..];
                    match format_name {
                        "sha1" => capabilities.object_format = Some(gix_hash::Kind::Sha1),
                        "sha256" => capabilities.object_format = Some(gix_hash::Kind::Sha1), // Use Sha1 as fallback since Sha256 variant doesn't exist
                        _ => {
                            return Err(Error::UnsupportedCapability {
                                capability: cap.to_string(),
                            })
                        }
                    }
                }
                _ => {
                    // Unknown capabilities are ignored for forward compatibility
                }
            }
        }
        
        Ok(capabilities)
    }
    
    /// Get V1 capability strings (without writing to any writer)
    pub fn get_v1_capability_strings(&self, caps: &ServerCapabilities) -> Vec<String> {
        let mut cap_strings = Vec::new();
        
        // Multi-ack capabilities - Git advertises both when detailed is supported
        match caps.multi_ack {
            MultiAckMode::None => {}
            MultiAckMode::Basic => cap_strings.push("multi_ack".to_string()),
            MultiAckMode::Detailed => {
                cap_strings.push("multi_ack".to_string());
                // multi_ack_detailed will be added later in the correct position
            }
        }
        
        if caps.thin_pack {
            cap_strings.push("thin-pack".to_string());
        }
        
        // Side-band capabilities
        cap_strings.extend(caps.side_band.to_capability_strings().iter().map(|s| s.to_string()));
        
        if caps.ofs_delta {
            cap_strings.push("ofs-delta".to_string());
        }
        
        if caps.shallow {
            cap_strings.push("shallow".to_string());
        }
        
        if caps.deepen_since {
            cap_strings.push("deepen-since".to_string());
        }
        
        if caps.deepen_not {
            cap_strings.push("deepen-not".to_string());
        }
        
        if caps.deepen_relative {
            cap_strings.push("deepen-relative".to_string());
        }
        
        if caps.no_progress {
            cap_strings.push("no-progress".to_string());
        }
        
        if caps.include_tag {
            cap_strings.push("include-tag".to_string());
        }
        
        // Add multi_ack_detailed after no-progress and include-tag (Git's order)
        if caps.multi_ack == MultiAckMode::Detailed {
            cap_strings.push("multi_ack_detailed".to_string());
        }
        
        if caps.no_done {
            cap_strings.push("no-done".to_string());
        }
        
        if caps.filter {
            cap_strings.push("filter".to_string());
        }
        
        if caps.allow_tip_sha1_in_want {
            cap_strings.push("allow-tip-sha1-in-want".to_string());
        }
        
        if caps.allow_reachable_sha1_in_want {
            cap_strings.push("allow-reachable-sha1-in-want".to_string());
        }
        
        // Add symref capability for HEAD
        if let Ok(head) = self.repository.head() {
            if let gix::head::Kind::Symbolic(target_ref) = head.kind {
                cap_strings.push(format!("symref=HEAD:{}", target_ref.name.as_bstr().to_str_lossy()));
            }
        }

        // Object format - native git uses lowercase
        if !caps.object_format.is_empty() {
            cap_strings.push("object-format=sha1".to_string());
        }
        
        // Agent
        cap_strings.push(format!("agent={}", caps.agent.to_str_lossy()));
        
        // Session ID if available
        if let Some(session_id) = &caps.session_id {
            cap_strings.push(format!("session-id={}", session_id.to_str_lossy()));
        }
        
        cap_strings
    }

    /// Convert server capabilities to wire format string for V1 protocol (convenience method)
    pub fn server_capabilities_to_v1_string(&self, caps: &ServerCapabilities) -> String {
        self.get_v1_capability_strings(caps).join(" ")
    }
    
    /// Validate V2 command arguments using gix-protocol
    /// Now that we've fixed gix-protocol to accept standard git capabilities
    pub fn validate_v2_command(
        &self, 
        command: crate::types::Command, 
        args: &[bstr::BString], 
        server_caps: &Capabilities
    ) -> Result<()> {        
        // Create features from server capabilities
        let features = command.default_features(ProtocolVersion::V2, server_caps);
        
        // Validate using gix-protocol's validation (now fixed to accept standard capabilities)
        command.validate_argument_prefixes(ProtocolVersion::V2, server_caps, args, &features)
            .map_err(|e| Error::ProtocolParsing(format!("Command validation failed: {}", e)))
    }
    
    /// Get initial arguments for a V2 command
    pub fn get_initial_v2_arguments(&self, command: crate::types::Command, server_caps: &Capabilities) -> Vec<bstr::BString> {
        let features = command.default_features(ProtocolVersion::V2, server_caps);
        command.initial_v2_arguments(&features)
    }
    
    /// Check if a capability is supported by the server
    pub fn supports_capability(&self, caps: &Capabilities, capability: &str) -> bool {
        caps.contains(capability)
    }

    /// Get V2 capability lines (without writing to any writer)
    pub fn get_v2_capability_lines(&self, capabilities: &ServerCapabilities) -> Vec<String> {
        let mut lines = Vec::new();

        // Version line
        lines.push("version 2".to_string());

        // Agent capability
        lines.push(format!("agent={}", capabilities.agent.to_str_lossy()));

        // Object format capabilities
        for format in &capabilities.object_format {
            lines.push(format!("object-format={}", format));
        }

        // ls-refs command
        lines.push("ls-refs=unborn".to_string());

        // fetch command with sub-capabilities
        let mut fetch_caps = Vec::new();

        if capabilities.shallow {
            fetch_caps.push("shallow");
        }

        if capabilities.filter {
            fetch_caps.push("filter");
        }

        // Add v2-specific sideband capabilities
        for cap in capabilities.side_band.to_v2_capability_strings() {
            fetch_caps.push(cap);
        }

        if capabilities.packfile_uris {
            fetch_caps.push("packfile-uris");
        }

        if capabilities.wait_for_done {
            fetch_caps.push("wait-for-done");
        }

        let fetch_line = if fetch_caps.is_empty() {
            "fetch".to_string()
        } else {
            format!("fetch={}", fetch_caps.join(" "))
        };
        lines.push(fetch_line);

        // server-info command
        lines.push("server-info".to_string());

        // object-info command if enabled
        if capabilities.object_info {
            lines.push("object-info".to_string());
        }

        // Session ID if available
        if let Some(ref session_id) = capabilities.session_id {
            lines.push(format!("session-id={}", session_id.to_str_lossy()));
        }

        lines
    }

    /// Validate client capabilities against server capabilities
    pub fn validate_client_capabilities(
        &self,
        client_caps: &ClientCapabilities,
        server_caps: &ServerCapabilities,
    ) -> Result<()> {
        // Check object format compatibility
        if let Some(client_format) = client_caps.object_format {
            if !server_caps.object_format.contains(&client_format) {
                return Err(Error::UnsupportedObjectFormat {
                    format: client_format.to_string(),
                });
            }
        }

        // Check filter compatibility
        if let Some(ref filter) = client_caps.filter {
            if !server_caps.filter {
                return Err(Error::UnsupportedCapability {
                    capability: format!("filter={}", filter.to_str_lossy()),
                });
            }
        }

        // Check shallow capabilities
        if client_caps.shallow && !server_caps.shallow {
            return Err(Error::UnsupportedCapability {
                capability: "shallow".to_string(),
            });
        }

        if client_caps.deepen_relative && !server_caps.deepen_relative {
            return Err(Error::UnsupportedCapability {
                capability: "deepen-relative".to_string(),
            });
        }

        Ok(())
    }
}
