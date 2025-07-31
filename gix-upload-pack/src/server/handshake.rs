//! Handshake handling for upload-pack sessions
//!
//! This module handles the initial handshake phase of the upload-pack protocol,
//! including capability advertisement and protocol version negotiation.

use crate::{
    config::ServerOptions,
    error::{Error, Result},
    types::*,
};
use bstr::{BStr, ByteSlice};
use gix::Repository;
use gix_packetline::PacketLineRef;
use std::io::Write;

/// Handshake manager for protocol sessions
pub struct HandshakeManager<'a> {
    repository: &'a Repository,
    options: &'a ServerOptions,
}

impl<'a> HandshakeManager<'a> {
    /// Create a new handshake manager
    pub fn new(repository: &'a Repository, options: &'a ServerOptions) -> Self {
        Self { repository, options }
    }
    
    /// Perform initial handshake for protocol V1
    pub fn handshake_v1<W: Write>(
        &self,
        writer: &mut W,
        session: &mut SessionContext,
    ) -> Result<()> {
        // Get server capabilities
        let capabilities = &self.options.capabilities;
        
        // Collect references to advertise
        let refs = self.collect_advertised_refs()?;
        
        if refs.is_empty() {
            // No refs case - send capabilities only
            self.write_capabilities_only_line(writer, capabilities)?;
        } else {
            // Send first ref with capabilities
            let first_ref = &refs[0];
            self.write_ref_with_capabilities(writer, first_ref, capabilities)?;
            
            // Send remaining refs without capabilities
            for reference in refs.iter().skip(1) {
                self.write_ref_line(writer, reference)?;
            }
            
            // Send peeled refs for tags
            for reference in &refs {
                if let Some(peeled) = &reference.peeled {
                    self.write_peeled_ref(writer, reference, peeled)?;
                }
            }
        }
        
        // End advertisement with flush packet
        PacketLineRef::Flush.write_to(writer)?;
        
        // Update session with advertised capabilities
        session.server_capabilities = Some(capabilities.clone());
        
        Ok(())
    }
    
    /// Perform initial handshake for protocol V2
    pub fn handshake_v2<W: Write>(
        &self,
        writer: &mut W,
        session: &mut SessionContext,
    ) -> Result<()> {
        let capabilities = &self.options.capabilities;
        
        // Send capability advertisement
        self.write_v2_capabilities(writer, capabilities)?;
        
        // End with flush
        PacketLineRef::Flush.write_to(writer)?;
        
        // Update session
        session.server_capabilities = Some(capabilities.clone());
        session.protocol_version = ProtocolVersion::V2;
        
        Ok(())
    }
    
    /// Collect references that should be advertised
    fn collect_advertised_refs(&self) -> Result<Vec<Reference>> {
        let mut refs = Vec::new();
        
        // Iterate through all references
        let binding = self.repository.references()
            .map_err(|e| Error::Reference(format!("Failed to get references iterator: {}", e)))?;
        let references_iter = binding
            .all()
            .map_err(|e| Error::Reference(format!("Failed to iterate all references: {}", e)))?;
            
        for reference in references_iter {
            let reference = reference
                .map_err(|e| Error::Reference(format!("Failed to read reference: {}", e)))?;
            let name = reference.name().as_bstr().to_owned();
            
            // Skip hidden references
            if self.is_ref_hidden(name.as_ref()) {
                continue;
            }
            
            match reference.target() {
                gix_ref::TargetRef::Symbolic(_) => {
                    // Skip symbolic refs in advertisement (except HEAD)
                    if name != "HEAD" {
                        continue;
                    }
                    
                    // For HEAD, resolve to target
                    if let Some(Ok(resolved)) = reference.follow() {
                        if let gix_ref::TargetRef::Object(oid) = resolved.target() {
                            refs.push(Reference {
                                name,
                                target: oid.to_owned(),
                                peeled: None,
                            });
                        }
                    }
                }
                gix_ref::TargetRef::Object(oid) => {
                    let target = oid.to_owned();
                    
                    // Check if this is an annotated tag and get peeled value
                    let peeled = if name.starts_with_str("refs/tags/") {
                        self.get_peeled_tag_target(target)?
                    } else {
                        None
                    };
                    
                    refs.push(Reference {
                        name,
                        target,
                        peeled,
                    });
                }
            }
        }
        
        // Sort refs by name for consistent output
        refs.sort_by(|a, b| a.name.cmp(&b.name));
        
        Ok(refs)
    }
    
    /// Check if a reference should be hidden
    fn is_ref_hidden(&self, ref_name: &BStr) -> bool {
        let ref_str = ref_name.to_str_lossy();
        
        for pattern in &self.options.hidden_refs {
            if self.matches_pattern(&ref_str, pattern.to_str_lossy().as_ref()) {
                return true;
            }
        }
        
        false
    }
    
    /// Check if a reference name matches a pattern
    fn matches_pattern(&self, ref_name: &str, pattern: &str) -> bool {
        // Simple glob pattern matching
        if pattern.ends_with("*") {
            let prefix = &pattern[..pattern.len() - 1];
            ref_name.starts_with(prefix)
        } else if pattern.starts_with("*") {
            let suffix = &pattern[1..];
            ref_name.ends_with(suffix)
        } else {
            ref_name == pattern
        }
    }
    
    /// Get the peeled target of an annotated tag
    fn get_peeled_tag_target(&self, tag_oid: gix_hash::ObjectId) -> Result<Option<gix_hash::ObjectId>> {
        if let Ok(obj) = self.repository.find_object(tag_oid) {
            if obj.kind == gix::object::Kind::Tag {
                if let Ok(tag) = obj.try_into_tag() {
                    if let Ok(target) = tag.target_id() {
                        return Ok(Some(target.detach()));
                    }
                }
            }
        }
        Ok(None)
    }
    
    /// Write a reference line with capabilities
    fn write_ref_with_capabilities<W: Write>(
        &self,
        writer: &mut W,
        reference: &Reference,
        capabilities: &ServerCapabilities,
    ) -> Result<()> {
        let caps_str = self.format_capabilities_v1(capabilities);
        let line = format!(
            "{} {}\0{}\n",
            reference.target.to_hex(),
            reference.name.to_str_lossy(),
            caps_str
        );
        gix_packetline::encode::data_to_write(line.as_bytes(), &mut *writer)?;
        Ok(())
    }
    
    /// Write a reference line without capabilities
    fn write_ref_line<W: Write>(&self, writer: &mut W, reference: &Reference) -> Result<()> {
        let line = format!(
            "{} {}\n",
            reference.target.to_hex(),
            reference.name.to_str_lossy()
        );
        gix_packetline::encode::data_to_write(line.as_bytes(), &mut *writer)?;
        Ok(())
    }
    
    /// Write a peeled reference line
    fn write_peeled_ref<W: Write>(
        &self,
        writer: &mut W,
        reference: &Reference,
        peeled: &gix_hash::ObjectId,
    ) -> Result<()> {
        let line = format!(
            "{} {}^{{}}\n",
            peeled.to_hex(),
            reference.name.to_str_lossy()
        );
        gix_packetline::encode::data_to_write(line.as_bytes(), &mut *writer)?;
        Ok(())
    }
    
    /// Write capabilities-only line for repositories with no refs
    fn write_capabilities_only_line<W: Write>(
        &self,
        writer: &mut W,
        capabilities: &ServerCapabilities,
    ) -> Result<()> {
        let caps_str = self.format_capabilities_v1(capabilities);
        let null_oid = gix_hash::ObjectId::null(self.repository.object_hash());
        let line = format!("{} capabilities^{{}}\0{}\n", null_oid.to_hex(), caps_str);
        gix_packetline::encode::data_to_write(line.as_bytes(), &mut *writer)?;
        Ok(())
    }
    
    /// Write V2 capability advertisement
    fn write_v2_capabilities<W: Write>(
        &self,
        writer: &mut W,
        capabilities: &ServerCapabilities,
    ) -> Result<()> {
        // Version line
        gix_packetline::encode::data_to_write(b"version 2\n", &mut *writer)?;
        
        // Agent capability
        let agent_line = format!("agent={}\n", capabilities.agent.to_str_lossy());
        gix_packetline::encode::data_to_write(agent_line.as_bytes(), &mut *writer)?;
        
        // Object format capabilities
        for format in &capabilities.object_format {
            let format_line = format!("object-format={}\n", format);
            gix_packetline::encode::data_to_write(format_line.as_bytes(), &mut *writer)?;
        }
        
        // ls-refs command
        gix_packetline::encode::data_to_write(b"ls-refs=unborn\n", &mut *writer)?;
        
        // fetch command with sub-capabilities
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
        
        let fetch_line = if fetch_caps.is_empty() {
            "fetch\n".to_string()
        } else {
            format!("fetch={}\n", fetch_caps.join(" "))
        };
        gix_packetline::encode::data_to_write(fetch_line.as_bytes(), &mut *writer)?;
        
        // server-info command
        gix_packetline::encode::data_to_write(b"server-info\n", &mut *writer)?;
        
        // object-info command if enabled
        if capabilities.object_info {
            gix_packetline::encode::data_to_write(b"object-info\n", &mut *writer)?;
        }
        
        // Session ID if available
        if let Some(ref session_id) = capabilities.session_id {
            let session_line = format!("session-id={}\n", session_id.to_str_lossy());
            gix_packetline::encode::data_to_write(session_line.as_bytes(), writer)?;
        }
        
        Ok(())
    }
    
    /// Format capabilities for V1 protocol
    fn format_capabilities_v1(&self, capabilities: &ServerCapabilities) -> String {
        let mut caps = Vec::new();
        
        // Multi-ack capability
        match capabilities.multi_ack {
            MultiAckMode::None => {}
            MultiAckMode::Basic => caps.push("multi_ack".to_string()),
            MultiAckMode::Detailed => caps.push("multi_ack_detailed".to_string()),
        }
        
        // Basic capabilities
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
