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

        if self.options.enable_object_info {
            // object-info command with its features
            let object_info_command = Command::ObjectInfo;
            let object_info_features = object_info_command.default_features(ProtocolVersion::V2, &default_caps);
            let mut object_info_cap_strings = vec!["size".to_string()];

            for (feature, _) in &object_info_features {
                if *feature != "object-info" && !object_info_cap_strings.contains(&feature.to_string()) {
                    object_info_cap_strings.push(feature.to_string());
                }
            }

            caps.push(format!("object-info={}", object_info_cap_strings.join(" ")));
        }
        
        caps.join("\n")
    }
    
    /// Build V2 capabilities string
    fn build_v2_capabilities_string(&self) -> String {
        let mut caps = Vec::new();
        
        // Agent capability
        caps.push(format!("agent=git/gitoxide-{}", crate::VERSION));
        
        // Object format
        caps.push("object-format=sha1".to_string());
        
        // Fetch command with basic capabilities
        let mut fetch_caps = vec!["shallow", "filter"];
        if self.options.allow_filter {
            // Keep filter capability
        } else {
            fetch_caps.retain(|&x| x != "filter");
        }
        
        caps.push(format!("fetch={}", fetch_caps.join(" ")));
        
        // ls-refs command
        caps.push("ls-refs=symrefs peel unborn".to_string());

        // object-info command
        let object_info_caps = vec!["size", "type"];
        
        caps.join("\n")
    }
    
    /// Get the default server capabilities based on repository and configuration
    pub fn default_server_capabilities(&self) -> ServerCapabilities {
        ServerCapabilities::default()
    }
    
    /// Parse client capabilities from wire protocol
    pub fn parse_client_capabilities(&self, caps_str: &str) -> Result<ClientCapabilities> {
        let mut client_caps = ClientCapabilities::default();
        
        for cap in caps_str.split_whitespace() {
            match cap {
                "multi_ack" => client_caps.multi_ack = MultiAckMode::Basic,
                "multi_ack_detailed" => client_caps.multi_ack = MultiAckMode::Detailed,
                "thin-pack" => client_caps.thin_pack = true,
                "side-band" => client_caps.side_band = SideBandMode::Basic,
                "side-band-64k" => client_caps.side_band = SideBandMode::SideBand64k,
                "ofs-delta" => client_caps.ofs_delta = true,
                "include-tag" => client_caps.include_tag = true,
                "no-progress" => client_caps.no_progress = true,
                "shallow" => client_caps.shallow = true,
                "deepen-relative" => client_caps.deepen_relative = true,
                cap if cap.starts_with("agent=") => {
                    client_caps.agent = Some(cap.strip_prefix("agent=").unwrap().into());
                }
                cap if cap.starts_with("filter=") => {
                    client_caps.filter = Some(cap.strip_prefix("filter=").unwrap().into());
                }
                cap if cap.starts_with("session-id=") => {
                    client_caps.session_id = Some(cap.strip_prefix("session-id=").unwrap().into());
                }
                _ => {
                    // Unknown capability - log and continue
                    eprintln!("Unknown client capability: {}", cap);
                }
            }
        }
        
        Ok(client_caps)
    }
    
    /// Convert server capabilities to wire format string for V1 protocol
    pub fn server_capabilities_to_v1_string(&self, caps: &ServerCapabilities) -> String {
        let mut cap_strings = Vec::new();
        
        // Basic capabilities
        match caps.multi_ack {
            MultiAckMode::None => {}
            MultiAckMode::Basic => cap_strings.push("multi_ack".to_string()),
            MultiAckMode::Detailed => cap_strings.push("multi_ack_detailed".to_string()),
        }
        
        if caps.thin_pack {
            cap_strings.push("thin-pack".to_string());
        }
        
        match caps.side_band {
            SideBandMode::None => {}
            SideBandMode::Basic => cap_strings.push("side-band".to_string()),
            SideBandMode::SideBand64k => cap_strings.push("side-band-64k".to_string()),
        }
        
        if caps.ofs_delta {
            cap_strings.push("ofs-delta".to_string());
        }
        
        if caps.include_tag {
            cap_strings.push("include-tag".to_string());
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
        
        if caps.filter {
            cap_strings.push("filter".to_string());
        }
        
        if caps.allow_tip_sha1_in_want {
            cap_strings.push("allow-tip-sha1-in-want".to_string());
        }
        
        if caps.allow_reachable_sha1_in_want {
            cap_strings.push("allow-reachable-sha1-in-want".to_string());
        }
        
        if caps.no_done {
            cap_strings.push("no-done".to_string());
        }
        
        // Agent
        cap_strings.push(format!("agent={}", caps.agent.to_str_lossy()));
        
        cap_strings.join(" ")
    }
    
    /// Validate V2 command arguments using gix-protocol
    /// Validate a V2 command with arguments using gix-protocol integration  
    pub fn validate_v2_command(
        &self, 
        command: crate::types::Command, 
        args: &[bstr::BString], 
        server_caps: &Capabilities
    ) -> Result<()> {        
        // Create features from server capabilities
        let features = command.default_features(ProtocolVersion::V2, server_caps);
        
        // Validate using gix-protocol's validation
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
}
