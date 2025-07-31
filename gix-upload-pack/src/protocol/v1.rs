//! Protocol version 1 implementation
//!
//! This module implements the Git wire protocol version 1, which is the traditional
//! stateful protocol used by Git for communication between client and server.

use crate::{
    config::ServerOptions,
    error::{Error, Result},
    protocol::ProtocolHandler,
    server::pack_generation,
    types::*,
};
use bstr::ByteSlice;
use gix::Repository;
use gix_pack::Find;
use gix_packetline::{PacketLineRef, StreamingPeekableIter};
use std::io::{BufRead, BufReader, Read, Write};

/// Protocol V1 handler
pub struct Handler<'a> {
    repository: &'a Repository,
    options: &'a ServerOptions,
}

impl<'a> Handler<'a> {
    /// Create a new V1 protocol handler
    pub fn new(repository: &'a Repository, options: &'a ServerOptions) -> Self {
        Self { repository, options }
    }
    
    /// Advertise references and capabilities
    fn advertise_refs<W: Write>(&self, writer: &mut W) -> Result<()> {
        let capabilities = &self.options.capabilities;
        
        // Get all references
        let refs = self.collect_references()?;
        
        if refs.is_empty() {
            // Send null capabilities line when no refs
            self.write_capabilities_line(writer, &gix_hash::ObjectId::null(gix_hash::Kind::Sha1), "capabilities^{}", capabilities)?;
        } else {
            let mut first = true;
            for reference in refs {
                if first {
                    // First ref includes capabilities
                    self.write_ref_with_capabilities(writer, &reference, capabilities)?;
                    first = false;
                } else {
                    // Subsequent refs without capabilities
                    self.write_ref_line(writer, &reference)?;
                }
            }
        }
        
        // Send flush packet to end advertisement
        gix_packetline::PacketLineRef::Flush.write_to(writer)?;
        Ok(())
    }
    
    /// Collect all references that should be advertised
    fn collect_references(&self) -> Result<Vec<Reference>> {
        let mut refs = Vec::new();
        
        // Iterate through all references
        for reference in self.repository.references().map_err(|e| Error::RefPackedBuffer(e))?.all().map_err(|e| Error::RefIterInit(e))? {
            let reference = reference.map_err(|e| Error::Boxed(e))?;
            let name = reference.name().as_bstr().to_owned();
            
            // Check if reference should be hidden
            if self.options.is_ref_hidden(name.to_str_lossy().as_ref()) {
                continue;
            }
            
            match reference.target() {
                gix::refs::TargetRef::Symbolic(_) => {
                    // Skip symbolic refs in advertisement
                    continue;
                }
                gix::refs::TargetRef::Object(oid) => {
                    let target = oid.to_owned();
                    
                    // Check if this is an annotated tag and get peeled value
                    let peeled = if name.starts_with_str("refs/tags/") {
                        if let Ok(obj) = self.repository.find_object(target) {
                            if obj.kind == gix::object::Kind::Tag {
                                // Get the peeled target of the tag
                                obj.try_into_tag()
                                    .ok()
                                    .and_then(|tag| tag.target_id().ok())
                                    .map(|id| id.detach())
                            } else {
                                None
                            }
                        } else {
                            None
                        }
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
        
        // Sort refs for consistent output
        refs.sort_by(|a, b| a.name.cmp(&b.name));
        
        Ok(refs)
    }
    
    /// Write a reference line with capabilities
    fn write_ref_with_capabilities<W: Write>(
        &self,
        writer: &mut W,
        reference: &Reference,
        capabilities: &ServerCapabilities,
    ) -> Result<()> {
        let caps_str = self.format_capabilities(capabilities);
        let line = format!(
            "{} {}\0{}\n",
            reference.target.to_hex(),
            reference.name.to_str_lossy(),
            caps_str
        );
        gix_packetline::encode::data_to_write(line.as_bytes(), &mut *writer)?;
        
        // If there's a peeled value, send it too
        if let Some(peeled) = &reference.peeled {
            let peeled_line = format!("{} {}^{{}}\n", peeled.to_hex(), reference.name.to_str_lossy());
            gix_packetline::encode::data_to_write(peeled_line.as_bytes(), writer)?;
        }
        
        Ok(())
    }
    
    /// Write a reference line without capabilities
    fn write_ref_line<W: Write>(&self, writer: &mut W, reference: &Reference) -> Result<()> {
        let line = format!("{} {}\n", reference.target.to_hex(), reference.name.to_str_lossy());
        gix_packetline::encode::data_to_write(line.as_bytes(), &mut *writer)?;
        
        // If there's a peeled value, send it too
        if let Some(peeled) = &reference.peeled {
            let peeled_line = format!("{} {}^{{}}\n", peeled.to_hex(), reference.name.to_str_lossy());
            gix_packetline::encode::data_to_write(peeled_line.as_bytes(), writer)?;
        }
        
        Ok(())
    }
    
    /// Write capabilities-only line for empty repositories
    fn write_capabilities_line<W: Write>(
        &self,
        writer: &mut W,
        oid: &gix_hash::ObjectId,
        refname: &str,
        capabilities: &ServerCapabilities,
    ) -> Result<()> {
        let caps_str = self.format_capabilities(capabilities);
        let line = format!("{} {}\0{}\n", oid.to_hex(), refname, caps_str);
        gix_packetline::encode::data_to_write(line.as_bytes(), writer)?;
        Ok(())
    }
    
    /// Format capabilities as a string
    fn format_capabilities(&self, capabilities: &ServerCapabilities) -> String {
        let mut caps = Vec::new();
        
        // Multi-ack capability
        match capabilities.multi_ack {
            MultiAckMode::None => {}
            MultiAckMode::Basic => {
                caps.push("multi_ack".to_string());
            }
            MultiAckMode::Detailed => {
                caps.push("multi_ack_detailed".to_string());
            }
        }
        
        // Other capabilities
        if capabilities.thin_pack {
            caps.push("thin-pack".to_string());
        }
        
        match capabilities.side_band {
            SideBandMode::None => {}
            SideBandMode::Basic => {
                caps.push("side-band".to_string());
            }
            SideBandMode::SideBand64k => {
                caps.push("side-band-64k".to_string());
            }
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
        if !capabilities.object_format.is_empty() {
            let formats: Vec<String> = capabilities.object_format
                .iter()
                .map(|f| format!("object-format={}", f))
                .collect();
            caps.extend(formats);
        }
        
        // Session ID if available
        if let Some(session_id) = &capabilities.session_id {
            caps.push(format!("session-id={}", session_id.to_str_lossy()));
        }
        
        caps.join(" ")
    }
    
    /// Handle want/have negotiation phase
    fn handle_negotiation<R: BufRead, W: Write>(
        &self,
        reader: &mut R,
        writer: &mut W,
        session: &mut SessionContext,
    ) -> Result<()> {
        let mut line_reader = gix_packetline::StreamingPeekableIter::new(reader, &[PacketLineRef::Flush], false);
        
        // Phase 1: Collect wants and initial capabilities
        self.collect_wants(&mut line_reader, session)?;
        
        // Phase 2: Handle haves and send acks
        self.handle_haves(&mut line_reader, writer, session)?;
        
        Ok(())
    }
    
    /// Collect want lines from client
    fn collect_wants<R: BufRead>(
        &self,
        reader: &mut StreamingPeekableIter<R>,
        session: &mut SessionContext,
    ) -> Result<()> {
        while let Some(line_result) = reader.read_line() {
            let line = line_result??;
            if matches!(line, gix_packetline::PacketLineRef::Flush) {
                break;
            }
            
            if let Some(line_data) = line.as_slice() {
                if let Some(want_line) = line_data.strip_prefix(b"want ") {
                    self.parse_want_line(want_line, session)?;
                } else if let Some(shallow_line) = line_data.strip_prefix(b"shallow ") {
                    self.parse_shallow_line(shallow_line, session)?;
                } else if let Some(deepen_line) = line_data.strip_prefix(b"deepen ") {
                    self.parse_deepen_line(deepen_line, session)?;
                } else if let Some(deepen_since_line) = line_data.strip_prefix(b"deepen-since ") {
                    self.parse_deepen_since_line(deepen_since_line, session)?;
                } else if let Some(deepen_not_line) = line_data.strip_prefix(b"deepen-not ") {
                    self.parse_deepen_not_line(deepen_not_line, session)?;
                }
            }
        }
        
        Ok(())
    }
    
    /// Parse a want line and extract object ID and capabilities
    fn parse_want_line(&self, line: &[u8], session: &mut SessionContext) -> Result<()> {
        let line_str = std::str::from_utf8(line)
            .map_err(|_| Error::custom("Invalid UTF-8 in want line"))?;
        
        // Split on null byte to separate OID from capabilities
        let parts: Vec<&str> = line_str.trim().splitn(2, '\0').collect();
        let oid_str = parts[0];
        
        // Parse object ID
        let oid = gix_hash::ObjectId::from_hex(oid_str.as_bytes())
            .map_err(|_| Error::InvalidObjectId { oid: oid_str.to_string() })?;
        
        // Add to wants
        session.negotiation.wants.insert(oid);
        
        // Parse capabilities if present (only on first want)
        if parts.len() > 1 && session.capabilities == ClientCapabilities::default() {
            self.parse_capabilities(parts[1], &mut session.capabilities)?;
        }
        
        Ok(())
    }
    
    /// Parse client capabilities from capability string
    fn parse_capabilities(&self, caps_str: &str, capabilities: &mut ClientCapabilities) -> Result<()> {
        for cap in caps_str.split_whitespace() {
            match cap {
                "multi_ack" => capabilities.multi_ack = MultiAckMode::Basic,
                "multi_ack_detailed" => capabilities.multi_ack = MultiAckMode::Detailed,
                "thin-pack" => capabilities.thin_pack = true,
                "side-band" => capabilities.side_band = SideBandMode::Basic,
                "side-band-64k" => capabilities.side_band = SideBandMode::SideBand64k,
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
                        _ => return Err(Error::UnsupportedCapability { capability: cap.to_string() }),
                    }
                }
                _ => {
                    // Unknown capabilities are ignored for forward compatibility
                }
            }
        }
        Ok(())
    }
    
    /// Parse shallow line
    fn parse_shallow_line(&self, line: &[u8], session: &mut SessionContext) -> Result<()> {
        let oid_str = std::str::from_utf8(line.trim_ascii())
            .map_err(|_| Error::custom("Invalid UTF-8 in shallow line"))?;
        
        let oid = gix_hash::ObjectId::from_hex(oid_str.as_bytes())
            .map_err(|_| Error::InvalidObjectId { oid: oid_str.to_string() })?;
            
        session.negotiation.shallow.insert(oid);
        Ok(())
    }
    
    /// Parse deepen line
    fn parse_deepen_line(&self, line: &[u8], session: &mut SessionContext) -> Result<()> {
        let depth_str = std::str::from_utf8(line.trim_ascii())
            .map_err(|_| Error::custom("Invalid UTF-8 in deepen line"))?;
            
        let depth: u32 = depth_str.parse()
            .map_err(|_| Error::custom("Invalid depth value"))?;
            
        session.negotiation.deepen = Some(DeepenSpec::Depth(depth));
        Ok(())
    }
    
    /// Parse deepen-since line
    fn parse_deepen_since_line(&self, line: &[u8], session: &mut SessionContext) -> Result<()> {
        let timestamp_str = std::str::from_utf8(line.trim_ascii())
            .map_err(|_| Error::custom("Invalid UTF-8 in deepen-since line"))?;
            
        let timestamp: i64 = timestamp_str.parse()
            .map_err(|_| Error::custom("Invalid timestamp value"))?;
            
        let time = gix_date::Time::new(timestamp, 0);
        session.negotiation.deepen = Some(DeepenSpec::Since(time));
        Ok(())
    }
    
    /// Parse deepen-not line
    fn parse_deepen_not_line(&self, line: &[u8], session: &mut SessionContext) -> Result<()> {
        let ref_str = std::str::from_utf8(line.trim_ascii())
            .map_err(|_| Error::custom("Invalid UTF-8 in deepen-not line"))?;
            
        if let Some(DeepenSpec::Not(ref mut refs)) = session.negotiation.deepen {
            refs.push(ref_str.into());
        } else {
            session.negotiation.deepen = Some(DeepenSpec::Not(vec![ref_str.into()]));
        }
        Ok(())
    }
    
    /// Handle have/ack negotiation loop
    fn handle_haves<R: BufRead, W: Write>(
        &self,
        reader: &mut StreamingPeekableIter<R>,
        writer: &mut W,
        session: &mut SessionContext,
    ) -> Result<()> {
        let mut common_found = false;
        
        while let Some(line_result) = reader.read_line() {
            let line = line_result??;
            if matches!(line, gix_packetline::PacketLineRef::Flush) {
                break;
            }
            
            if let Some(line_data) = line.as_slice() {
                if let Some(have_line) = line_data.strip_prefix(b"have ") {
                    let oid = self.parse_have_line(have_line, session)?;
                        
                // Check if we have this object
                if self.repository.objects.contains(&oid) {
                    session.negotiation.common.insert(oid);
                    common_found = true;
                    
                    // Send appropriate ACK based on capabilities
                    match session.capabilities.multi_ack {
                        MultiAckMode::None => {
                            if self.can_send_pack(session)? {
                                self.send_ack(writer, &oid, AckStatus::Common)?;
                            }
                        }
                        MultiAckMode::Basic => {
                            self.send_ack(writer, &oid, AckStatus::Continue)?;
                        }
                        MultiAckMode::Detailed => {
                            if self.can_send_pack(session)? {
                                self.send_ack(writer, &oid, AckStatus::Ready)?;
                                return Ok(());
                            } else {
                                self.send_ack(writer, &oid, AckStatus::Common)?;
                            }
                        }
                    }
                    } else {
                        session.negotiation.haves.insert(oid);
                    }
                } else if line_data.trim_ascii() == b"done" {
                    session.negotiation.done = true;
                    break;
                }
            }
        }
        
        // Send final response
        if session.negotiation.done {
            if common_found && self.can_send_pack(session)? {
                // Send final ACK
                if let Some(common_oid) = session.negotiation.common.iter().next() {
                    self.send_ack(writer, common_oid, AckStatus::Common)?;
                }
            } else {
                // Send NAK
                self.send_nak(writer)?;
            }
        }
        
        Ok(())
    }
    
    /// Parse have line and return object ID
    fn parse_have_line(&self, line: &[u8], session: &mut SessionContext) -> Result<gix_hash::ObjectId> {
        let oid_str = std::str::from_utf8(line.trim_ascii())
            .map_err(|_| Error::custom("Invalid UTF-8 in have line"))?;
        
        let oid = gix_hash::ObjectId::from_hex(oid_str.as_bytes())
            .map_err(|_| Error::InvalidObjectId { oid: oid_str.to_string() })?;
            
        session.negotiation.haves.insert(oid);
        Ok(oid)
    }
    
    /// Check if we can send pack (have enough common objects)
    fn can_send_pack(&self, session: &SessionContext) -> Result<bool> {
        // Simple heuristic: if we have some common objects, we can send a pack
        // In a real implementation, this would be more sophisticated
        Ok(!session.negotiation.common.is_empty() || session.negotiation.done)
    }
    
    /// Send ACK response
    fn send_ack<W: Write>(&self, writer: &mut W, oid: &gix_hash::ObjectId, status: AckStatus) -> Result<()> {
        let status_str = match status {
            AckStatus::Common => "",
            AckStatus::Continue => " continue",
            AckStatus::Ready => " ready",
        };
        
        let response = format!("ACK {}{}\n", oid.to_hex(), status_str);
        gix_packetline::encode::data_to_write(response.as_bytes(), writer)?;
        Ok(())
    }
    
    /// Send NAK response
    fn send_nak<W: Write>(&self, writer: &mut W) -> Result<()> {
        gix_packetline::encode::data_to_write(b"NAK\n", writer)?;
        Ok(())
    }
    
    /// Generate and send pack file
    fn send_pack<W: Write>(&self, writer: &mut W, session: &SessionContext) -> Result<()> {
        let pack_generator = pack_generation::PackGenerator::new(self.repository, self.options);
        pack_generator.generate_pack(writer, session)?;
        Ok(())
    }
}

impl<'a> ProtocolHandler for Handler<'a> {
    fn handle_session<R: Read, W: Write>(
        &mut self,
        input: R,
        mut output: W,
        session: &mut SessionContext,
    ) -> Result<()> {
        let mut buffered_input = BufReader::new(input);
        
        if self.options.advertise_refs {
            // Just advertise refs and exit (for git ls-remote, etc.)
            self.advertise_refs(&mut output)?;
        } else {
            // Full upload-pack session
            
            // Step 1: Advertise refs and capabilities
            self.advertise_refs(&mut output)?;
            
            // Step 2: Handle negotiation
            self.handle_negotiation(&mut buffered_input, &mut output, session)?;
            
            // Step 3: Generate and send pack
            if !session.negotiation.wants.is_empty() && session.negotiation.done {
                self.send_pack(&mut output, session)?;
            }
        }
        
        Ok(())
    }
}
