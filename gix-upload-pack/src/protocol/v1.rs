//! Protocol version 1 implementation
//!
//! This module implements the Git wire protocol version 1, which is the traditional
//! stateful protocol used by Git for communication between client and server.

use crate::{
    config::ServerOptions,
    error::{Error, Result},
    protocol::ProtocolHandler,
    services::{CapabilityManager, pack::PackGenerator, {packet_io::{EnhancedPacketReader, EnhancedPacketWriter}}},
    types::*,
};
use gix::Repository;
use std::io::{Read, Write};

// Async support removed - now fully synchronous

/// Protocol V1 handler with dependency injection
pub struct Handler<'a> {
    options: &'a ServerOptions,
    capability_manager: &'a CapabilityManager<'a>,
    command_parser: &'a crate::services::CommandParser<'a>,
    reference_manager: &'a crate::services::ReferenceManager<'a>,
    pack_generator: &'a PackGenerator<'a>,
    packet_io_factory: &'a crate::services::PacketIOFactory,
}

impl<'a> Handler<'a> {
    /// Create a new V1 protocol handler with dependency injection
    pub fn new(
        _repository: &'a Repository,
        options: &'a ServerOptions,
        capability_manager: &'a CapabilityManager<'a>,
        command_parser: &'a crate::services::CommandParser<'a>,
        reference_manager: &'a crate::services::ReferenceManager<'a>,
        pack_generator: &'a PackGenerator<'a>,
        packet_io_factory: &'a crate::services::PacketIOFactory,
    ) -> Self {
        Self {
            options,
            capability_manager,
            command_parser,
            reference_manager,
            pack_generator,
            packet_io_factory,
        }
    }

    /// Advertise references and capabilities using passed EnhancedPacketWriter
    fn advertise_refs<W: Write>(&self, writer: &mut EnhancedPacketWriter<W>) -> Result<()> {
        // Check if we're explicitly using protocol v1 (not v0/default)
        let is_explicit_v1 = crate::server::protocol_detection::ProtocolDetector::is_explicit_v1();

        // For explicit v1, send version announcement first
        if is_explicit_v1 {
            writer.write_protocol_message(b"version 1\n")?;
        }

        let capabilities = &self.options.capabilities;

        // Get all references using injected reference manager
        let refs = self.reference_manager.collect_advertised_references()?;

        // Get capability strings from capability manager (streamlined approach)
        let cap_strings = self.capability_manager.get_v1_capability_strings(capabilities);
        let caps_str = cap_strings.join(" ");
        let lines = self.reference_manager.format_v1_advertisement(&refs, &caps_str)?;
        
        for line in lines {
            writer.write_protocol_message(format!("{}\n", line).as_bytes())?;
        }

        // Send flush packet to end advertisement
        writer.write_flush()?;
        Ok(())
    }





    /// Handle want/have negotiation phase using EnhancedPacketWriter
    fn handle_negotiation<R: Read, W: Write>(
        &self,
        line_reader: &mut EnhancedPacketReader<R>,
        writer: &mut EnhancedPacketWriter<W>,
        session: &mut SessionContext,
    ) -> Result<()> {
        // Phase 1: Collect wants and capabilities
        self.collect_wants(line_reader, session)?;

        // Update writer's sideband mode based on negotiated capabilities
        // For advertise-refs mode, never use sideband (Git protocol requirement)
        let sideband_mode = if self.options.advertise_refs {
            crate::types::SideBandMode::None
        } else {
            session.capabilities.side_band
        };
        writer.set_sideband_mode(sideband_mode);

        // Phase 2: Handle haves and send acks using EnhancedPacketWriter
        self.handle_haves(line_reader, writer, session)?;

        Ok(())
    }

    /// Collect want lines from client
    fn collect_wants<R: Read>(&self, reader: &mut EnhancedPacketReader<R>, session: &mut SessionContext) -> Result<()> {
        eprintln!("Debug: Starting collect_wants");
        let mut packet_count = 0;
        loop {
            match reader.read_line() {
                Some(line_result) => {
                    packet_count += 1;
                    let line = line_result??;
                    eprintln!("Debug: Packet {}: {:?}", packet_count, line);
                    if EnhancedPacketReader::<R>::is_flush_packet(&line) {
                        eprintln!("Debug: Received flush packet in collect_wants after {} packets", packet_count);
                        break;
                    }

                    if let Some(line_data) = line.as_slice() {
                        eprintln!("Debug: Received packet in collect_wants: {:?}", 
                                 String::from_utf8_lossy(line_data));
                        if let Some(want_line) = line_data.strip_prefix(b"want ") {
                            // Use centralized command parser
                            self.command_parser.parse_want_line(want_line, session)?;
                            
                            // Handle capabilities parsing for first want line
                            let line_str = std::str::from_utf8(want_line).map_err(|_| Error::custom("Invalid UTF-8 in want line"))?;
                            let line_str = line_str.trim();
                            let capabilities_str = if line_str.len() > 40 { line_str[40..].trim() } else { "" };
                            
                            // Parse capabilities if present (only on first want line)
                            if !capabilities_str.is_empty() && session.capabilities == ClientCapabilities::default() {
                                // Use centralized capability parsing from CapabilityManager
                                session.capabilities = self.capability_manager.parse_client_capabilities(capabilities_str)?;
                            }
                        } else if let Some(shallow_line) = line_data.strip_prefix(b"shallow ") {
                            // Use centralized command parser
                            self.command_parser.parse_shallow_line(shallow_line, session)?;
                        } else if let Some(deepen_line) = line_data.strip_prefix(b"deepen ") {
                            // Use centralized command parser
                            self.command_parser.parse_deepen_line(deepen_line, session)?;
                        } else if let Some(deepen_since_line) = line_data.strip_prefix(b"deepen-since ") {
                            // Use centralized command parser
                            self.command_parser.parse_deepen_since_line(deepen_since_line, session)?;
                        } else if let Some(deepen_not_line) = line_data.strip_prefix(b"deepen-not ") {
                            // Use centralized command parser
                            self.command_parser.parse_deepen_not_line(deepen_not_line, session)?;
                        } else if line_data.trim_ascii() == b"done" {
                            // Client sent "done" directly after wants (no have phase)
                            // Use centralized command parser
                            self.command_parser.parse_done_line(session)?;
                            break;
                        } else if line_data.starts_with(b"have ") {
                            // This is a have line, end want collection and let handle_haves process it
                            let have_line = &line_data[5..]; // Remove "have " prefix
                            // Use centralized command parser
                            let _is_common = self.command_parser.parse_have_line(have_line, session)?;
                            
                            // Object existence check is now handled by centralized parser
                            break;
                        }
                    }
                }
                None => {
                    eprintln!("Debug: read_line() returned None after {} packets", packet_count);
                    break;
                }
            }
        }

        eprintln!("Debug: Finished collect_wants after {} packets, done={}", packet_count, session.negotiation.done);
        Ok(())
    }



    /// Handle have/ack negotiation loop using EnhancedPacketWriter
    fn handle_haves<R: Read, W: Write>(
        &self,
        reader: &mut EnhancedPacketReader<R>,
        writer: &mut EnhancedPacketWriter<W>,
        session: &mut SessionContext,
    ) -> Result<()> {
        let mut common_found = false;

        while let Some(line_result) = reader.read_line() {
            let line = line_result??;
            if EnhancedPacketReader::<R>::is_flush_packet(&line) {
                eprintln!("Debug: Received flush packet in handle_haves");
                break;
            }

            if let Some(line_data) = line.as_slice() {
                eprintln!("Debug: Received packet in handle_haves: {:?}", 
                         String::from_utf8_lossy(line_data));
                if let Some(have_line) = line_data.strip_prefix(b"have ") {
                    // Use centralized command parser
                    let is_common = self.command_parser.parse_have_line(have_line, session)?;
                    
                    // Extract OID for compatibility with existing logic
                    let oid_str = std::str::from_utf8(have_line.trim_ascii())
                        .map_err(|_| Error::custom("Invalid UTF-8 in have line"))?;
                    let oid = gix_hash::ObjectId::from_hex(oid_str.as_bytes())
                        .map_err(|_| Error::InvalidObjectId { oid: oid_str.to_string() })?;

                    // Object existence check is now handled by centralized parser
                    if is_common {
                        common_found = true;

                        // Send appropriate ACK based on capabilities using EnhancedPacketWriter
                        match session.capabilities.multi_ack {
                            MultiAckMode::None => {
                                if self.can_send_pack(session)? {
                                    writer.send_ack(&oid, AckStatus::Common)?;
                                }
                            }
                            MultiAckMode::Basic => {
                                writer.send_ack(&oid, AckStatus::Continue)?;
                            }
                            MultiAckMode::Detailed => {
                                if self.can_send_pack(session)? {
                                    writer.send_ack(&oid, AckStatus::Ready)?;
                                    return Ok(());
                                } else {
                                    writer.send_ack(&oid, AckStatus::Common)?;
                                }
                            }
                        }
                    }
                } else if line_data.trim_ascii() == b"done" {
                    eprintln!("Debug: Received 'done' packet in handle_haves");
                    // Use centralized command parser
                    self.command_parser.parse_done_line(session)?;
                    break;
                }
            }
        }

        // Send final response using EnhancedPacketWriter
        eprintln!("Debug: About to check final response, negotiation.done={}", session.negotiation.done);
        if session.negotiation.done {
            eprintln!("Debug: Negotiation done, common_found={}, can_send_pack={}", 
                     common_found, self.can_send_pack(session)?);
            if common_found && self.can_send_pack(session)? {
                eprintln!("Debug: Sending ACK for common object");
                if let Some(common_oid) = session.negotiation.common.iter().next() {
                    writer.send_ack(common_oid, AckStatus::Common)?;
                }
            } else {
                eprintln!("Debug: Sending NAK");
                writer.send_nak()?;
            }
        } else {
            eprintln!("Debug: Negotiation not done, no final response");
        }

        Ok(())
    }



    /// Check if we can send pack (have enough common objects)
    fn can_send_pack(&self, session: &SessionContext) -> Result<bool> {
        // Protocol compliance logic based on Git's behavior:
        // 1. Negotiation must be done
        // 2. For non-sideband modes: only send pack if we have common objects (send NAK only otherwise)
        // 3. For sideband modes: always send pack when negotiation is done (Git's behavior)

        if !session.negotiation.done {
            return Ok(false);
        }

        match session.capabilities.side_band {
            SideBandMode::None => {
                // Non-sideband mode: only send pack if we have common objects
                Ok(!session.negotiation.common.is_empty())
            }
            SideBandMode::Basic | SideBandMode::SideBand64k => {
                // Sideband mode: Git sends pack even without common objects
                Ok(true)
            }
        }
    }

    /// Generate and send pack file using EnhancedPacketWriter
    fn send_pack<W: Write>(&self, writer: &mut EnhancedPacketWriter<W>, session: &SessionContext) -> Result<()> {
        let pack_generator = self.pack_generator;
        pack_generator.generate_pack(writer, session)?;
        Ok(())
    }

    /// Handle session with injected packet I/O
    pub fn handle_session_with_io<R: Read, W: Write>(
        &mut self,
        reader: &mut EnhancedPacketReader<R>,
        writer: &mut EnhancedPacketWriter<W>,
        session: &mut SessionContext,
    ) -> Result<()> {
        if self.options.advertise_refs {
            // Just advertise refs and exit (for git ls-remote, etc.)
            self.advertise_refs(writer)?;
        } else if session.stateless_rpc {
            // Stateless RPC mode: client sends complete request, server responds directly
            // Handle negotiation using EnhancedPacketWriter
            self.handle_negotiation(reader, writer, session)?;

            // Generate and send pack if needed
            if !session.negotiation.wants.is_empty() && self.can_send_pack(session)? {
                self.send_pack(writer, session)?;
            }
        } else {
            // Full stateful upload-pack session

            // Step 1: Advertise refs and capabilities
            self.advertise_refs(writer)?;

            // Step 2: Handle negotiation
            self.handle_negotiation(reader, writer, session)?;

            // Step 3: Generate and send pack if needed
            if !session.negotiation.wants.is_empty() && self.can_send_pack(session)? {
                self.send_pack(writer, session)?;
            }
        }

        Ok(())
    }
}

impl<'a> ProtocolHandler for Handler<'a> {
    fn handle_session<R: Read, W: Write>(&mut self, input: R, output: W, session: &mut SessionContext) -> Result<()> {
        // Use injected packet I/O factory
        let mut reader = self.packet_io_factory.create_reader(input, false);

        // For advertise-refs mode, never use sideband (Git protocol requirement)
        let sideband_mode = if self.options.advertise_refs {
            crate::types::SideBandMode::None
        } else {
            // Start with no sideband - will be updated after capability negotiation
            crate::types::SideBandMode::None
        };

        let mut writer = self.packet_io_factory.create_writer(output, sideband_mode);
        
        // Delegate to the method with injected I/O
        self.handle_session_with_io(&mut reader, &mut writer, session)
    }
}
