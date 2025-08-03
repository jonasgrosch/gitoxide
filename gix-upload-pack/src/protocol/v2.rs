//! Protocol version 2 implementation
//!
//! This module implements the Git wire protocol version 2, which is a more
//! modern, stateless protocol that provides better performance and extensibility.

use crate::{
    config::ServerOptions,
    error::{Error, Result},
    protocol::ProtocolHandler,
    server::{capabilities::CapabilityManager, pack_generation},
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

// Async support removed - now fully synchronous

/// Protocol V2 handler
pub struct Handler<'a> {
    repository: &'a Repository,
    options: &'a ServerOptions,
    capability_manager: CapabilityManager<'a>,
}

impl<'a> Handler<'a> {
    /// Create a new V2 protocol handler
    pub fn new(repository: &'a Repository, options: &'a ServerOptions) -> Self {
        Self {
            repository,
            options,
            capability_manager: CapabilityManager::new(repository, options),
        }
    }

    /// Send capability advertisement using gix-protocol integration
    fn advertise_capabilities<W: Write>(&self, writer: &mut W) -> Result<()> {
        // Build capabilities using the integrated capability manager
        let server_caps = self.capability_manager.build_server_capabilities(ProtocolVersion::V2)?;

        // Send capabilities directly - they're already in the right format from gix-protocol
        // Write each capability line
        for capability in server_caps.iter() {
            let cap_line = format!("{}\n", capability.name().to_str_lossy());
            gix_packetline::encode::data_to_write(cap_line.as_bytes(), &mut *writer)?;
        }

        // End capabilities with flush
        gix_packetline::PacketLineRef::Flush.write_to(&mut *writer)?;

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
                        refs.push(ProtocolRef::Symbolic {
                            full_ref_name: "HEAD".into(),
                            target: target_ref.name.as_bstr().to_owned(),
                            tag: None,
                            object: *oid,
                        });
                    }
                }
                gix::head::Kind::Detached { target, .. } => {
                    // Detached HEAD
                    refs.push(ProtocolRef::Direct {
                        full_ref_name: "HEAD".into(),
                        object: target,
                    });
                }
                gix::head::Kind::Unborn(_) => {
                    // Skip unborn HEAD in V2
                }
            }
        }

        // Get the reference iterator
        let refs_binding = self.repository.references().map_err(|e| Error::RefPackedBuffer(e))?;

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
                gix::refs::TargetRef::Symbolic(target_ref_name) => {
                    // We need to fetch the target object id for symbolic refs
                    let object: Option<std::result::Result<gix::Reference<'_>, gix_ref::file::find::existing::Error>> =
                        reference.follow();

                    if let Some(Ok(resolved_ref)) = object {
                        refs.push(ProtocolRef::Symbolic {
                            full_ref_name: name.to_owned(),
                            target: target_ref_name.as_bstr().to_owned(),
                            tag: None,
                            object: resolved_ref.target().id().to_owned(),
                        });
                    }
                }
                gix::refs::TargetRef::Object(oid) => {
                    let target = oid.to_owned();
                    let name = name.to_owned();

                    // Get peeled value for tags - optimized
                    let peeled = if name.starts_with_str("refs/tags/") {
                        // Use type-specific method for better performance
                        self.repository
                            .find_tag(target)
                            .ok()
                            .and_then(|tag| tag.target_id().ok())
                            .map(|id| id.detach())
                    } else {
                        None
                    };

                    if let Some(peeled) = peeled {
                        refs.push(ProtocolRef::Peeled {
                            full_ref_name: name,
                            tag: target,
                            object: peeled,
                        });
                    } else {
                        refs.push(ProtocolRef::Direct {
                            full_ref_name: name,
                            object: target,
                        });
                    }
                }
            }
        }

        Ok(refs)
    }

    /// Parse want line for V2
    fn parse_want_line_v2(&self, line: &[u8], session: &mut SessionContext) -> Result<()> {
        let oid_str =
            std::str::from_utf8(line.trim_ascii()).map_err(|_| Error::custom("Invalid UTF-8 in want line"))?;

        let oid = gix_hash::ObjectId::from_hex(oid_str.as_bytes()).map_err(|_| Error::InvalidObjectId {
            oid: oid_str.to_string(),
        })?;

        session.negotiation.wants.insert(oid);
        Ok(())
    }

    /// Parse have line for V2
    fn parse_have_line_v2(&self, line: &[u8], session: &mut SessionContext) -> Result<()> {
        let oid_str =
            std::str::from_utf8(line.trim_ascii()).map_err(|_| Error::custom("Invalid UTF-8 in have line"))?;

        let oid = gix_hash::ObjectId::from_hex(oid_str.as_bytes()).map_err(|_| Error::InvalidObjectId {
            oid: oid_str.to_string(),
        })?;

        session.negotiation.haves.insert(oid);
        Ok(())
    }

    /// Parse shallow line for V2
    fn parse_shallow_line_v2(&self, line: &[u8], session: &mut SessionContext) -> Result<()> {
        let oid_str =
            std::str::from_utf8(line.trim_ascii()).map_err(|_| Error::custom("Invalid UTF-8 in shallow line"))?;

        let oid = gix_hash::ObjectId::from_hex(oid_str.as_bytes()).map_err(|_| Error::InvalidObjectId {
            oid: oid_str.to_string(),
        })?;

        session.negotiation.shallow.insert(oid);
        Ok(())
    }

    /// Parse deepen line for V2
    fn parse_deepen_line_v2(&self, line: &[u8], session: &mut SessionContext) -> Result<()> {
        let depth_str =
            std::str::from_utf8(line.trim_ascii()).map_err(|_| Error::custom("Invalid UTF-8 in deepen line"))?;

        let depth: u32 = depth_str.parse().map_err(|_| Error::custom("Invalid depth value"))?;

        session.negotiation.deepen = Some(DeepenSpec::Depth(depth));
        Ok(())
    }

    /// Parse deepen-since line for V2
    fn parse_deepen_since_line_v2(&self, line: &[u8], session: &mut SessionContext) -> Result<()> {
        let timestamp_str =
            std::str::from_utf8(line.trim_ascii()).map_err(|_| Error::custom("Invalid UTF-8 in deepen-since line"))?;

        let timestamp: i64 = timestamp_str
            .parse()
            .map_err(|_| Error::custom("Invalid timestamp value"))?;

        let time = gix_date::Time::new(timestamp, 0);
        session.negotiation.deepen = Some(DeepenSpec::Since(time));
        Ok(())
    }

    /// Parse deepen-not line for V2
    fn parse_deepen_not_line_v2(&self, line: &[u8], session: &mut SessionContext) -> Result<()> {
        let ref_str =
            std::str::from_utf8(line.trim_ascii()).map_err(|_| Error::custom("Invalid UTF-8 in deepen-not line"))?;

        if let Some(DeepenSpec::Not(ref mut refs)) = session.negotiation.deepen {
            refs.push(ref_str.into());
        } else {
            session.negotiation.deepen = Some(DeepenSpec::Not(vec![ref_str.into()]));
        }
        Ok(())
    }

    /// Parse command arguments for V2
    fn parse_command_arguments_v2<R: BufRead>(
        &self,
        reader: &mut StreamingPeekableIter<R>,
    ) -> Result<HashMap<String, String>> {
        let mut args = HashMap::new();

        loop {
            // Peek at the next line to see if it's a command argument or fetch parameter
            let should_break = match reader.peek_line() {
                Some(Ok(Ok(line))) => {
                    if matches!(line, gix_packetline::PacketLineRef::Flush) {
                        true // Will consume the flush below
                    } else if let Some(line_data) = line.as_slice() {
                        let line_str = std::str::from_utf8(line_data)
                            .map_err(|_| Error::custom("Invalid UTF-8 in argument line"))?
                            .trim();

                        // Stop parsing arguments if we hit fetch-specific commands
                        if line_str.starts_with("want ")
                            || line_str.starts_with("have ")
                            || line_str.starts_with("shallow ")
                            || line_str.starts_with("deepen")
                            || line_str == "done"
                        {
                            eprintln!("Debug: Stopping argument parsing at fetch parameter: {}", line_str);
                            true // Don't consume this line, leave it for read_fetch_parameters
                        } else {
                            false // This is an argument line, will process it below
                        }
                    } else {
                        false
                    }
                }
                Some(Ok(Err(_))) => {
                    true // Decode error, break
                }
                Some(Err(_)) => {
                    true // IO error, break
                }
                None => {
                    true // EOF
                }
            };

            if should_break {
                // Only consume flush packets, leave fetch parameters for later
                if let Some(Ok(Ok(line))) = reader.peek_line() {
                    if matches!(line, gix_packetline::PacketLineRef::Flush) {
                        reader.read_line(); // consume the flush
                    }
                }
                break;
            }

            // Now consume the line we know is an argument
            if let Some(line_result) = reader.read_line() {
                let line = line_result??;
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
            } else {
                break;
            }
        }

        Ok(args)
    }

    /// Handle ls-refs command
    fn handle_ls_refs<R: BufRead, W: Write>(
        &self,
        _reader: &mut StreamingPeekableIter<R>,
        writer: &mut W,
        args: &HashMap<String, String>,
    ) -> Result<()> {
        // Get server capabilities for validation
        let server_caps = self.capability_manager.build_server_capabilities(ProtocolVersion::V2)?;

        // Validate ls-refs command arguments
        let args_vec: Vec<bstr::BString> = args
            .iter()
            .map(|(k, v)| {
                if v.is_empty() {
                    k.as_bytes().into()
                } else {
                    format!("{}={}", k, v).into()
                }
            })
            .collect();

        self.capability_manager
            .validate_v2_command(Command::LsRefs, &args_vec, &server_caps)?;

        // Parse arguments
        let symrefs = args.get("symrefs").is_some();
        let peel = args.get("peel").is_some();
        let unborn = args.get("unborn").is_some();

        // Collect all ref-prefix arguments (they come as separate keys like "ref-prefix HEAD", "ref-prefix refs/heads/")
        let ref_prefixes: Vec<String> = args
            .keys()
            .filter_map(|key| {
                if key.starts_with("ref-prefix ") {
                    Some(key.strip_prefix("ref-prefix ").unwrap().to_string())
                } else {
                    None
                }
            })
            .collect();

        // Get references
        let refs = self.collect_refs_v2(&ref_prefixes)?;

        // Send references
        for reference in refs {
            let (ref_name, target_oid, peeled_oid) = reference.unpack();

            let target_oid = target_oid.ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "Unborn reference has no target")
            })?;
            let mut line = format!("{} {}", target_oid.to_hex(), ref_name.to_str_lossy());

            // Add symref-target attribute for symbolic references when symrefs is requested
            if symrefs {
                if let ProtocolRef::Symbolic { target, .. } = &reference {
                    line.push_str(&format!(" symref-target:{}", target.to_str_lossy()));
                }
            }

            // Add peeled attribute for peeled references when peel is requested
            if peel {
                if let Some(peeled_oid) = peeled_oid {
                    line.push_str(&format!(" peeled:{}", peeled_oid.to_hex()));
                }
            }

            line.push('\n');
            gix_packetline::encode::data_to_write(line.as_bytes(), &mut *writer)?;
        }

        // End with flush
        gix_packetline::PacketLineRef::Flush.write_to(writer)?;

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

        // Get server capabilities for validation
        let server_caps = self.capability_manager.build_server_capabilities(ProtocolVersion::V2)?;

        // Get initial arguments and validate
        let initial_args = self
            .capability_manager
            .get_initial_v2_arguments(Command::Fetch, &server_caps);
        let args_vec: Vec<bstr::BString> = args
            .iter()
            .map(|(k, v)| {
                if v.is_empty() {
                    k.as_bytes().into()
                } else {
                    format!("{}={}", k, v).into()
                }
            })
            .chain(initial_args.into_iter())
            .collect();

        self.capability_manager
            .validate_v2_command(Command::Fetch, &args_vec, &server_caps)?;

        // Parse fetch arguments
        let thin_pack = args.get("thin-pack").is_some();
        let ofs_delta = args.get("ofs-delta").is_some();
        let include_tag = args.get("include-tag").is_some();
        let sideband_all = args.get("sideband-all").is_some();
        let _wait_for_done = args.get("wait-for-done").is_some();

        // Parse filter if present
        let filter = args.get("filter").map(|f| f.as_str().into());

        // Set session capabilities based on arguments
        session.capabilities.thin_pack = thin_pack;
        session.capabilities.ofs_delta = ofs_delta;
        session.capabilities.include_tag = include_tag;
        session.capabilities.filter = filter;

        // Protocol v2 defaults to side-band support (matches Git behavior)
        // Client can explicitly request sideband-all, but we enable it by default
        session.capabilities.side_band = if sideband_all {
            SideBandMode::SideBand64k
        } else {
            // Default to SideBand64k for protocol v2 (Git's behavior)
            SideBandMode::SideBand64k
        };

        eprintln!(
            "Debug: Set session.capabilities.side_band to {:?} (sideband_all: {})",
            session.capabilities.side_band, sideband_all
        );

        // Read fetch parameters
        self.read_fetch_parameters(reader, session)?;

        eprintln!(
            "Debug: After reading parameters, wants: {:?}, haves: {:?}",
            session.negotiation.wants.len(),
            session.negotiation.haves.len()
        );

        // Perform negotiation if needed
        if !session.negotiation.wants.is_empty() {
            // In V2, we can send pack immediately if we have wants
            // and don't need negotiation (simplified for this example)

            // Find common commits and collect acknowledgments
            let mut acks = Vec::new();
            for have in &session.negotiation.haves {
                if self.repository.objects.contains(have) {
                    session.negotiation.common.insert(*have);
                    acks.push(*have);
                }
            }

            // Only send acknowledgments section if we have acknowledgments to send
            if !acks.is_empty() {
                // Send acknowledgments section
                gix_packetline::encode::data_to_write(b"acknowledgments\n", &mut *writer)?;

                for ack in acks {
                    let ack_line = format!("ACK {}\n", ack.to_hex());
                    gix_packetline::encode::data_to_write(ack_line.as_bytes(), &mut *writer)?;
                }

                // End acknowledgments
                gix_packetline::PacketLineRef::Flush.write_to(&mut *writer)?;
            }

            // Send packfile section
            gix_packetline::encode::data_to_write(b"packfile\n", &mut *writer)?;

            eprintln!(
                "Debug: About to generate pack for {} wants",
                session.negotiation.wants.len()
            );

            // Generate and send pack directly to writer (bypassing formatter for binary data)
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

                    command = match cmd_str {
                        "ls-refs" => Some(Command::LsRefs),
                        "fetch" => Some(Command::Fetch),
                        _ => None, // Unsupported command
                    };
                    break;
                }
            }
        }

        // Parse command arguments - but STOP at first flush or want/have line
        let args = self.parse_command_arguments_v2(&mut line_reader)?;

        eprintln!("Debug: Parsed command arguments: {:?}", args);

        // Handle the command
        match command {
            Some(Command::LsRefs) => {
                self.handle_ls_refs(&mut line_reader, &mut output, &args)?;
            }
            Some(Command::Fetch) => {
                self.handle_fetch(&mut line_reader, &mut output, &args, session)?;
            }
            None => {
                return Err(Error::custom(format!("Unsupported command: {:?}", command)));
            }
        }

        Ok(())
    }
}
