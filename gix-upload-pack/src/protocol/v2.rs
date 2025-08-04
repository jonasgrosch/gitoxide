//! Protocol version 2 implementation
//!
//! This module implements the Git wire protocol version 2, which is a more
//! modern, stateless protocol that provides better performance and extensibility.

use crate::{
    config::ServerOptions,
    error::{Error, Result},
    protocol::ProtocolHandler,
    services::{
        pack::PackGenerator,
        packet_io::{EnhancedPacketReader, EnhancedPacketWriter},
        CapabilityManager,
    },
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

/// Protocol V2 handler with dependency injection
pub struct Handler<'a> {
    repository: &'a Repository,
    options: &'a ServerOptions,
    capability_manager: &'a CapabilityManager<'a>,
    command_parser: &'a crate::services::CommandParser<'a>,
    reference_manager: &'a crate::services::ReferenceManager<'a>,
    pack_generator: &'a PackGenerator<'a>,
    packet_io_factory: &'a crate::services::PacketIOFactory,
}

impl<'a> Handler<'a> {
    /// Create a new V2 protocol handler with dependency injection
    pub fn new(
        repository: &'a Repository,
        options: &'a ServerOptions,
        capability_manager: &'a CapabilityManager<'a>,
        command_parser: &'a crate::services::CommandParser<'a>,
        reference_manager: &'a crate::services::ReferenceManager<'a>,
        pack_generator: &'a PackGenerator<'a>,
        packet_io_factory: &'a crate::services::PacketIOFactory,
    ) -> Self {
        Self {
            repository,
            options,
            capability_manager,
            command_parser,
            reference_manager,
            pack_generator,
            packet_io_factory,
        }
    }

    /// Send capability advertisement using streamlined approach
    fn advertise_capabilities<W: Write>(&self, writer: &mut W) -> Result<()> {
        // Use injected packet I/O factory
        let mut packet_writer = self.packet_io_factory.create_temp_writer(writer);

        // Get capability lines from capability manager (no direct writing)
        let capability_lines = self
            .capability_manager
            .get_v2_capability_lines(&self.options.capabilities);

        // Send capability lines through packet writer
        for line in capability_lines {
            packet_writer.write_protocol_message(format!("{}\n", line).as_bytes())?;
        }

        // End capabilities with flush
        packet_writer.write_flush()?;

        Ok(())
    }

    /// Advertise protocol v2 capabilities and commands (equivalent to --advertise-refs for v2)
    fn advertise_refs<W: Write>(&self, writer: &mut EnhancedPacketWriter<W>) -> Result<()> {
        // For advertise-refs mode, use the exact format that native git uses
        // This is simpler than the full capability negotiation format

        // Version line
        writer.write_protocol_message(b"version 2\n")?;

        // Agent capability
        let agent = format!("agent=git/gitoxide-{}\n", crate::VERSION);
        writer.write_protocol_message(agent.as_bytes())?;

        // Commands in the exact order that native git uses
        writer.write_protocol_message(b"ls-refs=unborn\n")?;
        writer.write_protocol_message(b"fetch=shallow wait-for-done\n")?;
        writer.write_protocol_message(b"server-option\n")?;
        writer.write_protocol_message(b"object-format=sha1\n")?;
        writer.write_protocol_message(b"object-info\n")?;

        // End with flush packet
        writer.write_flush()?;

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
        let _unborn = args.get("unborn").is_some();

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

        // Get references using injected reference manager
        let refs = self.reference_manager.collect_references_with_prefixes(&ref_prefixes)?;

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
            // Use injected packet I/O factory for consistent packet encoding
            let mut packet_writer = self.packet_io_factory.create_temp_writer(&mut *writer);
            packet_writer.write_protocol_message(line.as_bytes())?;
        }

        // End with flush
        // Use injected packet I/O factory for consistent packet encoding
        let mut packet_writer = self.packet_io_factory.create_temp_writer(writer);
        packet_writer.write_flush()?;

        Ok(())
    }

    /// Handle fetch command
    fn handle_fetch<R: BufRead, W: Write>(
        &self,
        reader: &mut StreamingPeekableIter<R>,
        writer: &mut EnhancedPacketWriter<W>,
        args: &HashMap<String, String>,
        session: &mut SessionContext,
    ) -> Result<()> {
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
        let no_progress = args.get("no-progress").is_some();
        let sideband_all = args.get("sideband-all").is_some();
        let _wait_for_done = args.get("wait-for-done").is_some();

        // Parse filter if present
        let filter = args.get("filter").map(|f| f.as_str().into());

        // Set session capabilities based on arguments
        session.capabilities.thin_pack = thin_pack;
        session.capabilities.ofs_delta = ofs_delta;
        session.capabilities.include_tag = include_tag;
        session.capabilities.no_progress = no_progress;
        session.capabilities.filter = filter;

        // Protocol v2 defaults to sideband support (matches Git behavior)
        // Git always uses sideband in v2 protocol for progress messages
        session.capabilities.side_band = if sideband_all {
            SideBandMode::SideBand64k
        } else {
            // Default to SideBand64k for protocol v2 (Git's behavior)
            SideBandMode::SideBand64k
        };

        // Update writer's sideband mode based on negotiated capabilities
        writer.set_sideband_mode(session.capabilities.side_band);

        // Read fetch parameters
        self.read_fetch_parameters(reader, session)?;

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
                // Use injected packet I/O factory for consistent packet encoding
                let mut packet_writer = self.packet_io_factory.create_temp_writer(&mut *writer);
                packet_writer.write_protocol_message(b"acknowledgments\n")?;

                for ack in acks {
                    let ack_line = format!("ACK {}\n", ack.to_hex());
                    // Use injected packet I/O factory for consistent packet encoding
                    let mut packet_writer = self.packet_io_factory.create_temp_writer(&mut *writer);
                    packet_writer.write_protocol_message(ack_line.as_bytes())?;
                }

                // End acknowledgments
                // Use injected packet I/O factory for consistent packet encoding
                let mut packet_writer = self.packet_io_factory.create_temp_writer(&mut *writer);
                packet_writer.write_flush()?;
            }

            // Send packfile section
            writer.write_protocol_message(b"packfile\n")?;

            // Generate and send pack using EnhancedPacketWriter for proper sideband handling
            let pack_generator = self.pack_generator;
            let pack_stats = pack_generator.generate_pack(writer, session)?;

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
                    // Use centralized command parser
                    self.command_parser.parse_want_line(want_line, session)?;
                } else if let Some(have_line) = line_data.strip_prefix(b"have ") {
                    // Use centralized command parser
                    let _is_common = self.command_parser.parse_have_line(have_line, session)?;
                } else if let Some(shallow_line) = line_data.strip_prefix(b"shallow ") {
                    // Use centralized command parser
                    self.command_parser.parse_shallow_line(shallow_line, session)?;
                } else if let Some(deepen_line) = line_data.strip_prefix(b"deepen ") {
                    // Use centralized command parser
                    self.command_parser.parse_deepen_line(deepen_line, session)?;
                } else if let Some(deepen_since_line) = line_data.strip_prefix(b"deepen-since ") {
                    // Use centralized command parser
                    self.command_parser
                        .parse_deepen_since_line(deepen_since_line, session)?;
                } else if let Some(deepen_not_line) = line_data.strip_prefix(b"deepen-not ") {
                    // Use centralized command parser
                    self.command_parser.parse_deepen_not_line(deepen_not_line, session)?;
                } else if line_data.trim_ascii() == b"done" {
                    // Use centralized command parser
                    self.command_parser.parse_done_line(session)?;
                    break;
                }
            }
        }

        Ok(())
    }

    /// Handle session with injected packet I/O
    pub fn handle_session_with_io<R: Read, W: Write>(
        &mut self,
        reader: EnhancedPacketReader<R>,
        writer: &mut EnhancedPacketWriter<W>,
        session: &mut SessionContext,
    ) -> Result<()> {
        // Check if we're in advertise-refs mode
        if self.options.advertise_refs {
            // Just advertise capabilities and commands, then exit
            self.advertise_refs(writer)?;
            return Ok(());
        }

        let input = reader.into_inner();
        let mut buffered_input = BufReader::new(input);
        let mut line_reader = gix_packetline::StreamingPeekableIter::new(
            &mut buffered_input,
            &[PacketLineRef::Flush],
            false, // trace
        );

        // Protocol V2 only advertises capabilities in non-stateless RPC mode
        // In stateless RPC mode (--stateless-rpc), we wait for client command first
        if !session.stateless_rpc {
            self.advertise_capabilities(writer.inner_mut())?;
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

        // Handle the command
        match command {
            Some(Command::LsRefs) => {
                self.handle_ls_refs(&mut line_reader, writer.inner_mut(), &args)?;
            }
            Some(Command::Fetch) => {
                self.handle_fetch(&mut line_reader, writer, &args, session)?;
            }
            None => {
                return Err(Error::custom(format!("Unsupported command: {:?}", command)));
            }
        }

        Ok(())
    }
}

impl<'a> ProtocolHandler for Handler<'a> {
    fn handle_session<R: Read, W: Write>(&mut self, input: R, output: W, session: &mut SessionContext) -> Result<()> {
        // Use injected packet I/O factory
        let reader = self.packet_io_factory.create_reader(input, false);

        // For advertise-refs mode, never use sideband (Git protocol requirement)
        let sideband_mode = if self.options.advertise_refs {
            crate::types::SideBandMode::None
        } else {
            // Start with no sideband - will be updated after capability negotiation
            crate::types::SideBandMode::None
        };

        let mut writer = self.packet_io_factory.create_writer(output, sideband_mode);

        // Delegate to the method with injected I/O
        self.handle_session_with_io(reader, &mut writer, session)
    }
}
