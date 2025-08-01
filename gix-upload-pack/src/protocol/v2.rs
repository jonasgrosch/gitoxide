//! Protocol version 2 implementation
//!
//! This module implements the Git wire protocol version 2, which is a more
//! modern, stateless protocol that provides better performance and extensibility.

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
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
};

/// Protocol V2 handler
pub struct Handler<'a> {
    repository: &'a Repository,
    options: &'a ServerOptions,
}

/// Protocol V2 commands
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    LsRefs,
    Fetch,
    ServerInfo,
    ObjectInfo,
}

impl std::str::FromStr for Command {
    type Err = Error;
    
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "ls-refs" => Ok(Command::LsRefs),
            "fetch" => Ok(Command::Fetch),
            "server-info" => Ok(Command::ServerInfo),
            "object-info" => Ok(Command::ObjectInfo),
            _ => Err(Error::UnsupportedCommand { command: s.to_string() }),
        }
    }
}

impl<'a> Handler<'a> {
    /// Create a new V2 protocol handler
    pub fn new(repository: &'a Repository, options: &'a ServerOptions) -> Self {
        Self { repository, options }
    }
    
    /// Send capability advertisement
    fn advertise_capabilities<W: Write>(&self, writer: &mut W) -> Result<()> {
        let capabilities = &self.options.capabilities;
        
        // Version announcement
        gix_packetline::encode::data_to_write(b"version 2\n", &mut *writer)?;
        
        // Agent capability
        let agent_line = format!("agent={}\n", capabilities.agent.to_str_lossy());
        gix_packetline::encode::data_to_write(agent_line.as_bytes(), &mut *writer)?;
        
        // Supported object formats
        if !capabilities.object_format.is_empty() {
            gix_packetline::encode::data_to_write(b"object-format=sha1\n", &mut *writer)?;
        }
        
        // Commands
        gix_packetline::encode::data_to_write(b"ls-refs=unborn\n", &mut *writer)?;
        
        // Fetch command with capabilities
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
        
        // Server-option capability
        gix_packetline::encode::data_to_write(b"server-option\n", &mut *writer)?;
        
        // Object-info command if supported
        if capabilities.object_info {
            gix_packetline::encode::data_to_write(b"object-info\n", &mut *writer)?;
        }
        
        // Session ID if available
        if let Some(session_id) = &capabilities.session_id {
            let session_line = format!("session-id={}\n", session_id.to_str_lossy());
            gix_packetline::encode::data_to_write(session_line.as_bytes(), &mut *writer)?;
        }
        
        // End capabilities with flush
        gix_packetline::PacketLineRef::Flush.write_to(&mut *writer)?;
        
        Ok(())
    }
    
    /// Handle ls-refs command
    fn handle_ls_refs<R: BufRead, W: Write>(
        &self,
        reader: &mut StreamingPeekableIter<R>,
        writer: &mut W,
        args: &HashMap<String, String>,
    ) -> Result<()> {
        // Parse arguments
        let symrefs = args.get("symrefs").is_some();
        let peel = args.get("peel").is_some();
        let unborn = args.get("unborn").is_some();
        
        let ref_prefixes = args.get("ref-prefix")
            .map(|s| s.split(' ').map(|p| p.to_string()).collect::<Vec<_>>())
            .unwrap_or_default();
        
        // Get references
        let refs = self.collect_refs_v2(&ref_prefixes)?;
        
        // Send references
        for reference in refs {
            let mut line = format!("{} {}", reference.target.to_hex(), reference.name.to_str_lossy());
            
            // Add symref-target for HEAD if requested
            if symrefs && reference.name == "HEAD" {
                if let Ok(head) = self.repository.head() {
                    if let gix::head::Kind::Symbolic(target_ref) = head.kind {
                        line.push_str(&format!(" symref-target:{}", target_ref.name.as_bstr().to_str_lossy()));
                    }
                }
            }
            
            // Add peeled info if requested and available
            if peel {
                if let Some(peeled) = &reference.peeled {
                    line.push_str(&format!(" peeled:{}", peeled.to_hex()));
                }
            }
            
            line.push('\n');
            gix_packetline::encode::data_to_write(line.as_bytes(), &mut *writer)?;
        }
        
        // Handle symbolic refs if requested
        if symrefs {
            self.send_symrefs(&mut *writer)?;
        }
        
        // Handle unborn HEAD if requested
        if unborn {
            self.send_unborn_head(&mut *writer)?;
        }
        
        // End with flush
        gix_packetline::PacketLineRef::Flush.write_to(writer)?;
        
        Ok(())
    }
    
    /// Collect references for V2 protocol
    fn collect_refs_v2(&self, prefixes: &[String]) -> Result<Vec<Reference>> {
        let mut refs = Vec::new();
        
        // In V2, we should include HEAD as a regular reference
        if let Ok(head) = self.repository.head() {
            match head.kind {
                gix::head::Kind::Symbolic(target_ref) => {
                    // Add HEAD as a reference pointing to the same commit as the target
                    if let gix::refs::Target::Object(oid) = &target_ref.target {
                        refs.push(Reference {
                            name: "HEAD".into(),
                            target: *oid,
                            peeled: None,
                        });
                    }
                }
                gix::head::Kind::Detached { target, .. } => {
                    // Detached HEAD
                    refs.push(Reference {
                        name: "HEAD".into(),
                        target,
                        peeled: None,
                    });
                }
                gix::head::Kind::Unborn(_) => {
                    // Skip unborn HEAD in V2
                }
            }
        }
        
        // Get the reference iterator
        let refs_binding = self.repository.references().map_err(|e| Error::RefPackedBuffer(e))?;
        let ref_iter = if prefixes.is_empty() {
            refs_binding.all().map_err(|e| Error::RefIterInit(e))?
        } else {
            // For multiple prefixes, we need to collect from each prefix
            let mut all_refs = Vec::new();
            let all_refs_iter = refs_binding.all().map_err(|e| Error::RefIterInit(e))?;
            
            for reference in all_refs_iter {
                if let Ok(reference) = reference {
                    let ref_name = reference.name().as_bstr().to_str_lossy();
                    for prefix in prefixes {
                        if ref_name.starts_with(prefix) {
                            all_refs.push(reference);
                            break;
                        }
                    }
                }
            }
            
            // Process the collected refs
            for reference in all_refs {
                let name = reference.name().as_bstr();
                
                // Check if reference should be hidden
                if self.options.is_ref_hidden(name.to_str_lossy().as_ref()) {
                    continue;
                }
                
                match reference.target() {
                    gix::refs::TargetRef::Symbolic(_) => {
                        // Skip symbolic refs in V2 (handled separately)
                        continue;
                    }
                    gix::refs::TargetRef::Object(oid) => {
                        let target = oid.to_owned();
                        let name = name.to_owned();
                        
                        // Get peeled value for tags
                        let peeled = if name.starts_with_str("refs/tags/") {
                            if let Ok(obj) = self.repository.find_object(target) {
                                if obj.kind == gix::object::Kind::Tag {
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
            
            return Ok(refs);
        };
        
        for reference in ref_iter {
            let reference = reference.map_err(|e| Error::Boxed(e))?;
            let name = reference.name().as_bstr();
            
            // Check if reference should be hidden
            if self.options.is_ref_hidden(name.to_str_lossy().as_ref()) {
                continue;
            }
            
            match reference.target() {
                gix::refs::TargetRef::Symbolic(_) => {
                    // Skip symbolic refs in V2 (handled separately)
                    continue;
                }
                gix::refs::TargetRef::Object(oid) => {
                    let target = oid.to_owned();
                    let name = name.to_owned();
                    
                    // Get peeled value for tags
                    let peeled = if name.starts_with_str("refs/tags/") {
                        if let Ok(obj) = self.repository.find_object(target) {
                            if obj.kind == gix::object::Kind::Tag {
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
    
    /// Send symbolic references
    fn send_symrefs<W: Write>(&self, writer: &mut W) -> Result<()> {
        // Find symbolic references - use all() method instead of iter()
        for reference in self.repository.references().map_err(|e| Error::RefPackedBuffer(e))?.all().map_err(|e| Error::RefIterInit(e))? {
            let reference = reference.map_err(|e| Error::Boxed(e))?;
            
            if let gix::refs::TargetRef::Symbolic(target) = reference.target() {
                let name = reference.name().as_bstr();
                let target_name = target.as_bstr();
                
                // Don't send hidden refs
                if self.options.is_ref_hidden(name.to_str_lossy().as_ref()) {
                    continue;
                }
                
                let symref_line = format!("symref-target:{} {}\n", name.to_str_lossy(), target_name.to_str_lossy());
                gix_packetline::encode::data_to_write(symref_line.as_bytes(), &mut *writer)?;
            }
        }
        
        Ok(())
    }
    
    /// Send unborn HEAD information
    fn send_unborn_head<W: Write>(&self, writer: &mut W) -> Result<()> {
        // Check if HEAD is unborn (points to non-existent ref)
        if let Ok(head_ref) = self.repository.refs.find("HEAD") {
            if let gix::refs::Target::Symbolic(target) = head_ref.target {
                // Check if target exists
                if self.repository.refs.find(target.as_bstr()).is_err() {
                    let unborn_line = format!("unborn {}\n", target.as_bstr().to_str_lossy());
                    gix_packetline::encode::data_to_write(unborn_line.as_bytes(), writer)?;
                }
            }
        }
        
        Ok(())
    }
    
    /// Handle fetch command
    fn handle_fetch<R: BufRead, W: Write>(
        &self,
        reader: &mut StreamingPeekableIter<R>,
        writer: &mut W,
        args: &HashMap<String, String>,
        session: &mut SessionContext,
    ) -> Result<()> {
        eprintln!("Debug: handle_fetch called with args: {:?}", args);
        
        // Parse fetch arguments
        let thin_pack = args.get("thin-pack").is_some();
        let ofs_delta = args.get("ofs-delta").is_some();
        let include_tag = args.get("include-tag").is_some();
        let sideband_all = args.get("sideband-all").is_some();
        let wait_for_done = args.get("wait-for-done").is_some();
        
        // Parse filter if present
        let filter = args.get("filter").map(|f| f.as_str().into());
        
        // Set session capabilities based on arguments
        session.capabilities.thin_pack = thin_pack;
        session.capabilities.ofs_delta = ofs_delta;
        session.capabilities.include_tag = include_tag;
        session.capabilities.filter = filter;
        
        if sideband_all {
            session.capabilities.side_band = SideBandMode::SideBand64k;
        }
        
        // Read fetch parameters
        self.read_fetch_parameters(reader, session)?;
        
        eprintln!("Debug: After reading parameters, wants: {:?}, haves: {:?}", 
                  session.negotiation.wants.len(), 
                  session.negotiation.haves.len());
        
        // Perform negotiation if needed
        if !session.negotiation.wants.is_empty() {
            // In V2, we can send pack immediately if we have wants
            // and don't need negotiation (simplified for this example)
            
            // Send acknowledgments section
            gix_packetline::encode::data_to_write(b"acknowledgments\n", &mut *writer)?;
            
            // Find common commits (simplified)
            for have in &session.negotiation.haves {
                if self.repository.objects.contains(have) {
                    session.negotiation.common.insert(*have);
                    let ack_line = format!("ACK {}\n", have.to_hex());
                    gix_packetline::encode::data_to_write(ack_line.as_bytes(), &mut *writer)?;
                }
            }
            
            // End acknowledgments
            gix_packetline::PacketLineRef::Flush.write_to(&mut *writer)?;
            
            // Send packfile section
            gix_packetline::encode::data_to_write(b"packfile\n", &mut *writer)?;
            
            eprintln!("Debug: About to generate pack for {} wants", session.negotiation.wants.len());
            
            // Generate and send pack
            let pack_generator = pack_generation::PackGenerator::new(self.repository, self.options);
            let pack_stats = pack_generator.generate_pack(&mut *writer, session)?;
            
            eprintln!("Debug: Pack generation complete - stats: {:?}", pack_stats);
        }
        
        Ok(())
    }
    
    /// Read fetch parameters from client
    fn read_fetch_parameters<R: BufRead>(
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
                    self.parse_want_line_v2(want_line, session)?;
                } else if let Some(have_line) = line_data.strip_prefix(b"have ") {
                    self.parse_have_line_v2(have_line, session)?;
                } else if let Some(shallow_line) = line_data.strip_prefix(b"shallow ") {
                    self.parse_shallow_line_v2(shallow_line, session)?;
                } else if let Some(deepen_line) = line_data.strip_prefix(b"deepen ") {
                    self.parse_deepen_line_v2(deepen_line, session)?;
                } else if let Some(deepen_since_line) = line_data.strip_prefix(b"deepen-since ") {
                    self.parse_deepen_since_line_v2(deepen_since_line, session)?;
                } else if let Some(deepen_not_line) = line_data.strip_prefix(b"deepen-not ") {
                    self.parse_deepen_not_line_v2(deepen_not_line, session)?;
                } else if line_data.trim_ascii() == b"done" {
                    session.negotiation.done = true;
                    break;
                }
            }
        }
        
        Ok(())
    }
    
    /// Parse want line for V2
    fn parse_want_line_v2(&self, line: &[u8], session: &mut SessionContext) -> Result<()> {
        let oid_str = std::str::from_utf8(line.trim_ascii())
            .map_err(|_| Error::custom("Invalid UTF-8 in want line"))?;
        
        let oid = gix_hash::ObjectId::from_hex(oid_str.as_bytes())
            .map_err(|_| Error::InvalidObjectId { oid: oid_str.to_string() })?;
        
        session.negotiation.wants.insert(oid);
        Ok(())
    }
    
    /// Parse have line for V2
    fn parse_have_line_v2(&self, line: &[u8], session: &mut SessionContext) -> Result<()> {
        let oid_str = std::str::from_utf8(line.trim_ascii())
            .map_err(|_| Error::custom("Invalid UTF-8 in have line"))?;
        
        let oid = gix_hash::ObjectId::from_hex(oid_str.as_bytes())
            .map_err(|_| Error::InvalidObjectId { oid: oid_str.to_string() })?;
        
        session.negotiation.haves.insert(oid);
        Ok(())
    }
    
    /// Parse shallow line for V2
    fn parse_shallow_line_v2(&self, line: &[u8], session: &mut SessionContext) -> Result<()> {
        let oid_str = std::str::from_utf8(line.trim_ascii())
            .map_err(|_| Error::custom("Invalid UTF-8 in shallow line"))?;
        
        let oid = gix_hash::ObjectId::from_hex(oid_str.as_bytes())
            .map_err(|_| Error::InvalidObjectId { oid: oid_str.to_string() })?;
            
        session.negotiation.shallow.insert(oid);
        Ok(())
    }
    
    /// Parse deepen line for V2
    fn parse_deepen_line_v2(&self, line: &[u8], session: &mut SessionContext) -> Result<()> {
        let depth_str = std::str::from_utf8(line.trim_ascii())
            .map_err(|_| Error::custom("Invalid UTF-8 in deepen line"))?;
            
        let depth: u32 = depth_str.parse()
            .map_err(|_| Error::custom("Invalid depth value"))?;
            
        session.negotiation.deepen = Some(DeepenSpec::Depth(depth));
        Ok(())
    }
    
    /// Parse deepen-since line for V2
    fn parse_deepen_since_line_v2(&self, line: &[u8], session: &mut SessionContext) -> Result<()> {
        let timestamp_str = std::str::from_utf8(line.trim_ascii())
            .map_err(|_| Error::custom("Invalid UTF-8 in deepen-since line"))?;
            
        let timestamp: i64 = timestamp_str.parse()
            .map_err(|_| Error::custom("Invalid timestamp value"))?;
            
        let time = gix_date::Time::new(timestamp, 0);
        session.negotiation.deepen = Some(DeepenSpec::Since(time));
        Ok(())
    }
    
    /// Parse deepen-not line for V2
    fn parse_deepen_not_line_v2(&self, line: &[u8], session: &mut SessionContext) -> Result<()> {
        let ref_str = std::str::from_utf8(line.trim_ascii())
            .map_err(|_| Error::custom("Invalid UTF-8 in deepen-not line"))?;
            
        if let Some(DeepenSpec::Not(ref mut refs)) = session.negotiation.deepen {
            refs.push(ref_str.into());
        } else {
            session.negotiation.deepen = Some(DeepenSpec::Not(vec![ref_str.into()]));
        }
        Ok(())
    }
    
    /// Handle object-info command
    fn handle_object_info<R: BufRead, W: Write>(
        &self,
        reader: &mut StreamingPeekableIter<R>,
        writer: &mut W,
        args: &HashMap<String, String>,
    ) -> Result<()> {
        // Parse requested attributes
        let mut attributes = Vec::new();
        if let Some(attrs) = args.get("attributes") {
            for attr in attrs.split(' ') {
                attributes.push(attr.to_string());
            }
        }
        
        // Read object OIDs
        let mut oids = Vec::new();
        while let Some(line_result) = reader.read_line() {
            let line = line_result??;
            if matches!(line, gix_packetline::PacketLineRef::Flush) {
                break;
            }
            
            if let Some(line_data) = line.as_slice() {
                if let Some(oid_line) = line_data.strip_prefix(b"oid ") {
                let oid_str = std::str::from_utf8(oid_line.trim_ascii())
                    .map_err(|_| Error::custom("Invalid UTF-8 in oid line"))?;
                
                let oid = gix_hash::ObjectId::from_hex(oid_str.as_bytes())
                    .map_err(|_| Error::InvalidObjectId { oid: oid_str.to_string() })?;
                
                    oids.push(oid);
                }
            }
        }
        
        // Send object information
        for oid in oids {
            if let Ok(obj) = self.repository.find_object(oid) {
                let mut info_line = format!("{}", oid.to_hex());
                
                for attr in &attributes {
                    match attr.as_str() {
                        "size" => {
                            info_line.push_str(&format!(" size {}", obj.data.len()));
                        }
                        "type" => {
                            info_line.push_str(&format!(" type {}", obj.kind));
                        }
                        _ => {
                            // Unknown attribute, skip
                        }
                    }
                }
                
                info_line.push('\n');
                gix_packetline::encode::data_to_write(info_line.as_bytes(), &mut *writer)?;
            }
        }
        
        // End with flush
        gix_packetline::PacketLineRef::Flush.write_to(writer)?;
        
        Ok(())
    }
    
    /// Parse command arguments from argument lines
    fn parse_command_arguments<R: BufRead>(
        &self,
        reader: &mut StreamingPeekableIter<R>,
    ) -> Result<HashMap<String, String>> {
        let mut args = HashMap::new();
        
        while let Some(line_result) = reader.read_line() {
            let line = line_result??;
            if matches!(line, gix_packetline::PacketLineRef::Flush) {
                break;
            }
            
            if let Some(line_data) = line.as_slice() {
                let line_str = std::str::from_utf8(line_data)
                .map_err(|_| Error::custom("Invalid UTF-8 in argument line"))?
                .trim();
            
            if let Some(equals_pos) = line_str.find('=') {
                let key = line_str[..equals_pos].to_string();
                let value = line_str[equals_pos + 1..].to_string();
                args.insert(key, value);
                } else {
                    // Flag argument (no value)
                    args.insert(line_str.to_string(), String::new());
                }
            }
        }
        
        Ok(args)
    }
    
    /// Parse command arguments for V2, stopping at fetch parameters
    fn parse_command_arguments_v2<R: BufRead>(
        &self,
        reader: &mut StreamingPeekableIter<R>,
    ) -> Result<HashMap<String, String>> {
        let mut args = HashMap::new();
        
        while let Some(line_result) = reader.read_line() {
            let line = line_result??;
            if matches!(line, gix_packetline::PacketLineRef::Flush) {
                break;
            }
            
            if let Some(line_data) = line.as_slice() {
                let line_str = std::str::from_utf8(line_data)
                    .map_err(|_| Error::custom("Invalid UTF-8 in argument line"))?
                    .trim();
                
                // Stop if we hit fetch parameters (want, have, done, etc.)
                if line_str.starts_with("want ") || 
                   line_str.starts_with("have ") || 
                   line_str.starts_with("done") ||
                   line_str.starts_with("shallow ") ||
                   line_str.starts_with("deepen") {
                    // Put the line back for fetch parameter parsing
                    // We can't actually put it back, so we'll handle this differently
                    eprintln!("Debug: Found fetch parameter line, stopping arg parsing: {}", line_str);
                    break;
                }
                
                if let Some(equals_pos) = line_str.find('=') {
                    let key = line_str[..equals_pos].to_string();
                    let value = line_str[equals_pos + 1..].to_string();
                    args.insert(key, value);
                } else {
                    // Flag argument (no value)
                    args.insert(line_str.to_string(), String::new());
                }
            }
        }
        
        Ok(args)
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
        let mut line_reader = gix_packetline::StreamingPeekableIter::new(
            &mut buffered_input,
            &[PacketLineRef::Flush],
            false, // trace
        );
        
        // Protocol V2 only advertises capabilities in non-stateless RPC mode
        // In stateless RPC mode (--stateless-rpc), we wait for client command first
        if !session.stateless_rpc {
            self.advertise_capabilities(&mut output)?;
        }
        
        // Wait for command
        let mut command = None;
        while let Some(line_result) = line_reader.read_line() {
            let line = line_result??;
            if matches!(line, gix_packetline::PacketLineRef::Flush) {
                break;
            }
            
            if let Some(line_data) = line.as_slice() {
                if let Some(cmd_line) = line_data.strip_prefix(b"command=") {
                let cmd_str = std::str::from_utf8(cmd_line.trim_ascii())
                    .map_err(|_| Error::custom("Invalid UTF-8 in command line"))?;
                
                    command = Some(cmd_str.parse::<Command>()?);
                    break;
                }
            }
        }
        
        let command = command.ok_or_else(|| Error::custom("No command specified"))?;
        
        // Parse command arguments - but STOP at first flush or want/have line
        let args = self.parse_command_arguments_v2(&mut line_reader)?;
        
        eprintln!("Debug: Parsed command arguments: {:?}", args);
        
        // Handle the command
        match command {
            Command::LsRefs => {
                self.handle_ls_refs(&mut line_reader, &mut output, &args)?;
            }
            Command::Fetch => {
                self.handle_fetch(&mut line_reader, &mut output, &args, session)?;
            }
            Command::ObjectInfo => {
                self.handle_object_info(&mut line_reader, &mut output, &args)?;
            }
            Command::ServerInfo => {
                // server-info is not a standard V2 command, return error
                return Err(Error::UnsupportedCommand { command: "server-info".to_string() });
            }
        }
        
        Ok(())
    }
}
