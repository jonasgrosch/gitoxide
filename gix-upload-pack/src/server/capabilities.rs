//! Capability management for upload-pack
//!
//! This module handles the advertisement and negotiation of capabilities
//! between the client and server during the upload-pack protocol.

use crate::{
    config::ServerOptions,
    error::{Error, Result},
    types::*,
};
use bstr::{BStr, BString, ByteSlice};
use gix::Repository;
use std::collections::HashMap;

/// Capability manager for handling server and client capabilities
pub struct CapabilityManager<'a> {
    repository: &'a Repository,
    options: &'a ServerOptions,
}

impl<'a> CapabilityManager<'a> {
    /// Create a new capability manager
    pub fn new(repository: &'a Repository, options: &'a ServerOptions) -> Self {
        Self { repository, options }
    }
    
    /// Get the default server capabilities based on repository and configuration
    pub fn default_server_capabilities(&self) -> ServerCapabilities {
        let mut caps = ServerCapabilities::default();
        
        // Multi-ack capability - always support detailed for better performance
        caps.multi_ack = MultiAckMode::Detailed;
        
        // Thin pack support
        caps.thin_pack = true;
        
        // Side-band support for progress and error reporting
        caps.side_band = SideBandMode::SideBand64k;
        
        // OFS delta support for more efficient packs
        caps.ofs_delta = true;
        
        // Shallow support
        caps.shallow = true;
        caps.deepen_since = true;
        caps.deepen_not = true;
        caps.deepen_relative = self.options.allow_deepen_relative;
        
        // Progress reporting (can be disabled by client)
        caps.no_progress = false;
        
        // Tag inclusion
        caps.include_tag = true;
        
        // Object filtering
        caps.filter = self.options.allow_filter;
        
        // SHA1-in-want capabilities
        caps.allow_tip_sha1_in_want = self.options.allow_tip_sha1_in_want;
        caps.allow_reachable_sha1_in_want = self.options.allow_reachable_sha1_in_want;
        caps.allow_any_sha1_in_want = self.options.allow_any_sha1_in_want;
        
        // No-done capability for faster negotiation
        caps.no_done = true;
        
        // Agent string
        caps.agent = self.get_agent_string();
        
        // Object format support
        caps.object_format = self.get_supported_object_formats().into();
        
        // Session ID for request correlation
        caps.session_id = self.generate_session_id();
        
        // Protocol V2 specific capabilities
        caps.packfile_uris = self.options.allow_packfile_uris;
        caps.wait_for_done = true;
        caps.object_info = self.options.enable_object_info;
        
        caps
    }
    
    /// Negotiate capabilities with client
    pub fn negotiate_capabilities(
        &self,
        client_caps: &ClientCapabilities,
    ) -> Result<(ServerCapabilities, ClientCapabilities)> {
        let mut server_caps = self.default_server_capabilities();
        let mut negotiated_client_caps = client_caps.clone();
        
        // Negotiate multi-ack mode
        server_caps.multi_ack = match (server_caps.multi_ack, client_caps.multi_ack) {
            (_, MultiAckMode::Detailed) => MultiAckMode::Detailed,
            (_, MultiAckMode::Basic) => MultiAckMode::Basic,
            _ => MultiAckMode::None,
        };
        
        // Negotiate side-band mode
        server_caps.side_band = match (server_caps.side_band, client_caps.side_band) {
            (SideBandMode::SideBand64k, SideBandMode::SideBand64k) => SideBandMode::SideBand64k,
            (_, SideBandMode::Basic) | (SideBandMode::Basic, _) => SideBandMode::Basic,
            _ => SideBandMode::None,
        };
        
        // Capability intersections
        server_caps.thin_pack = server_caps.thin_pack && client_caps.thin_pack;
        server_caps.ofs_delta = server_caps.ofs_delta && client_caps.ofs_delta;
        server_caps.include_tag = server_caps.include_tag && client_caps.include_tag;
        server_caps.no_progress = server_caps.no_progress || client_caps.no_progress;
        
        // Shallow capabilities
        if !client_caps.shallow {
            server_caps.shallow = false;
            server_caps.deepen_since = false;
            server_caps.deepen_not = false;
            server_caps.deepen_relative = false;
        }
        
        // Filter support
        if !server_caps.filter {
            negotiated_client_caps.filter = None;
        }
        
        // SHA1-in-want capabilities
        if !server_caps.allow_tip_sha1_in_want {
            negotiated_client_caps.allow_tip_sha1_in_want = false;
        }
        if !server_caps.allow_reachable_sha1_in_want {
            negotiated_client_caps.allow_reachable_sha1_in_want = false;
        }
        
        // Object format negotiation
        if let Some(client_format) = client_caps.object_format {
            if !server_caps.object_format.contains(&client_format) {
                return Err(Error::UnsupportedObjectFormat { 
                    format: client_format.to_string() 
                });
            }
            // Use client's preferred format if supported
            server_caps.object_format = smallvec::smallvec![client_format];
        }
        
        // Validate negotiated capabilities
        self.validate_negotiated_capabilities(&server_caps, &negotiated_client_caps)?;
        
        Ok((server_caps, negotiated_client_caps))
    }
    
    /// Validate that negotiated capabilities are consistent and safe
    fn validate_negotiated_capabilities(
        &self,
        server_caps: &ServerCapabilities,
        client_caps: &ClientCapabilities,
    ) -> Result<()> {
        // Check that object format is consistent
        if let Some(client_format) = client_caps.object_format {
            if !server_caps.object_format.contains(&client_format) {
                return Err(Error::CapabilityMismatch {
                    message: "Client and server object formats don't match".to_string(),
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
            
            // Validate filter specification
            self.validate_filter_spec(filter.as_ref())?;
        }
        
        // Check shallow capability consistency
        if client_caps.deepen_relative && !server_caps.deepen_relative {
            return Err(Error::UnsupportedCapability {
                capability: "deepen-relative".to_string(),
            });
        }
        
        Ok(())
    }
    
    /// Validate filter specification
    fn validate_filter_spec(&self, filter: &BStr) -> Result<()> {
        let filter_str = filter.to_str_lossy();
        
        if filter_str == "blob:none" {
            // Always valid
            Ok(())
        } else if filter_str.starts_with("blob:limit=") {
            // Validate size limit
            let limit_str = &filter_str["blob:limit=".len()..];
            limit_str.parse::<u64>()
                .map_err(|_| Error::InvalidFilter { 
                    message: format!("Invalid size limit in filter '{}'", filter_str),
                })?;
            Ok(())
        } else if filter_str.starts_with("tree:") {
            // Validate tree depth
            let depth_str = &filter_str["tree:".len()..];
            depth_str.parse::<u32>()
                .map_err(|_| Error::InvalidFilter {
                    message: format!("Invalid tree depth in filter '{}'", filter_str),
                })?;
            Ok(())
        } else if filter_str.starts_with("sparse:") {
            // Validate sparse checkout spec (simplified validation)
            if filter_str.len() <= "sparse:".len() {
                return Err(Error::InvalidFilter {
                    message: format!("Empty sparse specification in filter '{}'", filter_str),
                });
            }
            Ok(())
        } else {
            Err(Error::InvalidFilter {
                message: format!("Unknown filter type in '{}'", filter_str),
            })
        }
    }
    
    /// Get agent string for capability advertisement
    fn get_agent_string(&self) -> BString {
        format!("gitoxide/{}", env!("CARGO_PKG_VERSION")).into()
    }
    
    /// Get supported object formats
    fn get_supported_object_formats(&self) -> Vec<gix_hash::Kind> {
        // Return the repository's hash kind (currently only SHA-1 is supported by gitoxide)
        vec![self.repository.object_hash()]
    }
    
    /// Generate a session ID for request correlation
    fn generate_session_id(&self) -> Option<BString> {
        if self.options.enable_session_id {
            use std::time::{SystemTime, UNIX_EPOCH};
            
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            
            // Simple session ID: timestamp + process ID as unique component
            let session_id = format!("gitoxide-{}-{:x}", 
                timestamp, 
                std::process::id()
            );
            
            Some(session_id.into())
        } else {
            None
        }
    }
    
    /// Check if a capability is supported by the server
    pub fn supports_capability(&self, capability: &str) -> bool {
        let caps = self.default_server_capabilities();
        
        match capability {
            "multi_ack" => caps.multi_ack != MultiAckMode::None,
            "multi_ack_detailed" => caps.multi_ack == MultiAckMode::Detailed,
            "thin-pack" => caps.thin_pack,
            "side-band" => caps.side_band != SideBandMode::None,
            "side-band-64k" => caps.side_band == SideBandMode::SideBand64k,
            "ofs-delta" => caps.ofs_delta,
            "shallow" => caps.shallow,
            "deepen-since" => caps.deepen_since,
            "deepen-not" => caps.deepen_not,
            "deepen-relative" => caps.deepen_relative,
            "no-progress" => true, // Always can support no progress
            "include-tag" => caps.include_tag,
            "filter" => caps.filter,
            "allow-tip-sha1-in-want" => caps.allow_tip_sha1_in_want,
            "allow-reachable-sha1-in-want" => caps.allow_reachable_sha1_in_want,
            "allow-any-sha1-in-want" => caps.allow_any_sha1_in_want,
            "no-done" => caps.no_done,
            "packfile-uris" => caps.packfile_uris,
            "wait-for-done" => caps.wait_for_done,
            "object-info" => caps.object_info,
            _ => false,
        }
    }
    
    /// Get capability-specific configuration
    pub fn get_capability_config(&self, capability: &str) -> Option<HashMap<String, String>> {
        match capability {
            "filter" => {
                let mut config = HashMap::new();
                
                // Add supported filter types
                let mut filter_types = Vec::new();
                if self.options.allow_blob_filter {
                    filter_types.push("blob:none");
                    filter_types.push("blob:limit");
                }
                if self.options.allow_tree_filter {
                    filter_types.push("tree");
                }
                if self.options.allow_sparse_filter {
                    filter_types.push("sparse");
                }
                
                if !filter_types.is_empty() {
                    config.insert("types".to_string(), filter_types.join(","));
                }
                
                Some(config)
            }
            "shallow" => {
                let mut config = HashMap::new();
                
                if let Some(max_depth) = self.options.max_shallow_depth {
                    config.insert("max-depth".to_string(), max_depth.to_string());
                }
                
                Some(config)
            }
            "object-format" => {
                let mut config = HashMap::new();
                let formats: Vec<String> = self.get_supported_object_formats()
                    .iter()
                    .map(|f| f.to_string())
                    .collect();
                
                config.insert("formats".to_string(), formats.join(","));
                Some(config)
            }
            _ => None,
        }
    }
    
    /// Format capabilities for protocol advertisement
    pub fn format_capabilities_v1(&self, capabilities: &ServerCapabilities) -> String {
        let mut caps = Vec::new();
        
        // Multi-ack capability
        match capabilities.multi_ack {
            MultiAckMode::None => {}
            MultiAckMode::Basic => caps.push("multi_ack".to_string()),
            MultiAckMode::Detailed => caps.push("multi_ack_detailed".to_string()),
        }
        
        // Other capabilities
        if capabilities.thin_pack {
            caps.push("thin-pack".to_string());
        }
        
        match capabilities.side_band {
            SideBandMode::None => {}
            SideBandMode::Basic => caps.push("side-band".to_string()),
            SideBandMode::SideBand64k => caps.push("side-band-64k".to_string()),
        }
        
        if capabilities.ofs_delta {
            caps.push("ofs-delta".to_string());
        }
        
        if capabilities.shallow {
            caps.push("shallow".to_string());
        }
        
        if capabilities.deepen_since {
            caps.push("deepen-since".to_string());
        }
        
        if capabilities.deepen_not {
            caps.push("deepen-not".to_string());
        }
        
        if capabilities.deepen_relative {
            caps.push("deepen-relative".to_string());
        }
        
        if capabilities.no_progress {
            caps.push("no-progress".to_string());
        }
        
        if capabilities.include_tag {
            caps.push("include-tag".to_string());
        }
        
        if capabilities.filter {
            caps.push("filter".to_string());
        }
        
        if capabilities.allow_tip_sha1_in_want {
            caps.push("allow-tip-sha1-in-want".to_string());
        }
        
        if capabilities.allow_reachable_sha1_in_want {
            caps.push("allow-reachable-sha1-in-want".to_string());
        }
        
        if capabilities.allow_any_sha1_in_want {
            caps.push("allow-any-sha1-in-want".to_string());
        }
        
        if capabilities.no_done {
            caps.push("no-done".to_string());
        }
        
        // Agent string
        caps.push(format!("agent={}", capabilities.agent.to_str_lossy()));
        
        // Object format
        for format in &capabilities.object_format {
            caps.push(format!("object-format={}", format));
        }
        
        // Session ID
        if let Some(ref session_id) = capabilities.session_id {
            caps.push(format!("session-id={}", session_id.to_str_lossy()));
        }
        
        caps.join(" ")
    }
    
    /// Format capabilities for protocol V2 advertisement
    pub fn format_capabilities_v2(&self, capabilities: &ServerCapabilities) -> Vec<String> {
        let mut caps = Vec::new();
        
        // Version
        caps.push("version 2".to_string());
        
        // Agent
        caps.push(format!("agent={}", capabilities.agent.to_str_lossy()));
        
        // Object formats
        for format in &capabilities.object_format {
            caps.push(format!("object-format={}", format));
        }
        
        // Commands with their capabilities
        caps.push("ls-refs=unborn".to_string());
        
        // Fetch command capabilities
        let mut fetch_caps = Vec::new();
        if capabilities.shallow {
            fetch_caps.push("shallow");
        }
        if capabilities.filter {
            fetch_caps.push("filter");
        }
        match capabilities.side_band {
            SideBandMode::None => {}
            SideBandMode::Basic => fetch_caps.push("sideband"),
            SideBandMode::SideBand64k => fetch_caps.push("sideband-all"),
        }
        if capabilities.packfile_uris {
            fetch_caps.push("packfile-uris");
        }
        if capabilities.wait_for_done {
            fetch_caps.push("wait-for-done");
        }
        
        if fetch_caps.is_empty() {
            caps.push("fetch".to_string());
        } else {
            caps.push(format!("fetch={}", fetch_caps.join(" ")));
        }
        
        // Server info command
        caps.push("server-info".to_string());
        
        // Object info command
        if capabilities.object_info {
            caps.push("object-info".to_string());
        }
        
        // Session ID
        if let Some(ref session_id) = capabilities.session_id {
            caps.push(format!("session-id={}", session_id.to_str_lossy()));
        }
        
        caps
    }
}
