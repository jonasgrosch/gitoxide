//! Response handling and formatting for upload-pack
//!
//! This module handles the formatting and sending of various response types
//! during the upload-pack protocol, including error responses, progress updates,
//! and protocol-specific messages.

use crate::{
    error::Result,
    types::*,
};
use bstr::{BStr, ByteSlice};
//use gix_features::progress::{Count, Progress};
use gix_packetline::PacketLineRef;
use std::io::{Write};

/// Response formatter for upload-pack protocol
pub struct ResponseFormatter<'a, W: Write> {
    writer: &'a mut W,
    side_band_mode: SideBandMode,
    session_id: Option<&'a BStr>,
}

use crate::types::{SideBandMode, SideBandChannel};

impl<'a, W: Write> ResponseFormatter<'a, W> {
    /// Create a new response formatter
    pub fn new(writer: &'a mut W, side_band_mode: SideBandMode) -> Self {
        Self {
            writer,
            side_band_mode,
            session_id: None,
        }
    }
    
    /// Set session ID for response correlation
    pub fn with_session_id(mut self, session_id: &'a BStr) -> Self {
        self.session_id = Some(session_id);
        self
    }
    
    /// Send a data packet
    pub fn send_data(&mut self, data: &[u8]) -> Result<()> {
        // Use consistent chunking size for all modes, matching native Git's ~8KB chunks
        // This improves compatibility and performance
        const OPTIMAL_CHUNK_SIZE: usize = 8196;
        
        match self.side_band_mode {
            SideBandMode::None => {
                // For large data, chunk it to avoid packet line size limits
                if data.len() <= OPTIMAL_CHUNK_SIZE {
                    gix_packetline::encode::data_to_write(data, &mut *self.writer)?;
                } else {
                    // Chunk large data into multiple packet lines
                    for chunk in data.chunks(OPTIMAL_CHUNK_SIZE) {
                        gix_packetline::encode::data_to_write(chunk, &mut *self.writer)?;
                    }
                }
            }
            SideBandMode::Basic | SideBandMode::SideBand64k => {
                // Pack data should be sent via side-band channel 1 (Data channel)
                // This matches Git's behavior where pack files are multiplexed with progress
                // Use consistent chunking for better compatibility with native Git
                self.send_side_band_chunked(SideBandChannel::Data, data, OPTIMAL_CHUNK_SIZE)?;
            }
        }
        Ok(())
    }
    
    /// Send a progress message
    pub fn send_progress(&mut self, message: &str) -> Result<()> {
        match self.side_band_mode {
            SideBandMode::None => {
                // Cannot send progress without side-band
            }
            SideBandMode::Basic | SideBandMode::SideBand64k => {
                let progress_msg = format!("{}\n", message);
                self.send_side_band(SideBandChannel::Progress, progress_msg.as_bytes())?;
            }
        }
        Ok(())
    }
    
    /// Send an error message
    pub fn send_error(&mut self, error: &str) -> Result<()> {
        match self.side_band_mode {
            SideBandMode::None => {
                // Send as regular packet
                let error_msg = format!("error: {}\n", error);
                gix_packetline::encode::data_to_write(error_msg.as_bytes(), &mut *self.writer)?;
            }
            SideBandMode::Basic | SideBandMode::SideBand64k => {
                let error_msg = format!("error: {}\n", error);
                self.send_side_band(SideBandChannel::Error, error_msg.as_bytes())?;
            }
        }
        Ok(())
    }
    
    /// Send a side-band packet
    fn send_side_band(&mut self, channel: SideBandChannel, data: &[u8]) -> Result<()> {
        let max_data_size = self.side_band_mode.max_data_size()
            .expect("send_side_band called when side-band mode is None");
        
        // Split data into chunks if necessary and use gix-packetline encoding
        for chunk in data.chunks(max_data_size) {
            gix_packetline::encode::band_to_write(channel, chunk, &mut *self.writer)?;
        }
        
        Ok(())
    }
    
    /// Send a side-band packet with custom chunk size
    fn send_side_band_chunked(&mut self, channel: SideBandChannel, data: &[u8], chunk_size: usize) -> Result<()> {
        let max_data_size = self.side_band_mode.max_data_size()
            .expect("send_side_band_chunked called when side-band mode is None");
        
        // Use the smaller of our preferred chunk size and the protocol maximum
        let effective_chunk_size = chunk_size.min(max_data_size);
        
        // Split data into chunks and use gix-packetline encoding
        for chunk in data.chunks(effective_chunk_size) {
            gix_packetline::encode::band_to_write(channel, chunk, &mut *self.writer)?;
        }
        
        Ok(())
    }
    
    /// Send flush packet
    pub fn send_flush(&mut self) -> Result<()> {
        PacketLineRef::Flush.write_to(&mut *self.writer)?;
        Ok(())
    }
    
    /// Send delimiter packet (protocol V2)
    pub fn send_delimiter(&mut self) -> Result<()> {
        PacketLineRef::Delimiter.write_to(&mut *self.writer)?;
        Ok(())
    }
    
    /// Send a response end packet (protocol V2)
    pub fn send_response_end(&mut self) -> Result<()> {
        PacketLineRef::ResponseEnd.write_to(&mut *self.writer)?;
        Ok(())
    }
    
    /// Send ACK response
    pub fn send_ack(&mut self, oid: &gix_hash::ObjectId, status: AckStatus) -> Result<()> {
        let response = match status {
            AckStatus::Common => format!("ACK {}\n", oid.to_hex()),
            AckStatus::Continue => format!("ACK {} continue\n", oid.to_hex()),
            AckStatus::Ready => format!("ACK {} ready\n", oid.to_hex()),
        };
        
        self.send_data(response.as_bytes())
    }
    
    /// Send NAK response
    pub fn send_nak(&mut self) -> Result<()> {
        self.send_data(b"NAK\n")
    }
    
    /// Send shallow response
    pub fn send_shallow(&mut self, oid: &gix_hash::ObjectId) -> Result<()> {
        let response = format!("shallow {}\n", oid.to_hex());
        self.send_data(response.as_bytes())
    }
    
    /// Send unshallow response
    pub fn send_unshallow(&mut self, oid: &gix_hash::ObjectId) -> Result<()> {
        let response = format!("unshallow {}\n", oid.to_hex());
        self.send_data(response.as_bytes())
    }
    
    /// Send a reference line (for ls-refs)
    pub fn send_ref(&mut self, reference: &Reference) -> Result<()> {
        let (ref_name, target_oid, peeled) = reference.unpack();
        let target_oid = target_oid.ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "Unborn reference has no target"))?;
        let mut line = format!("{} {}", target_oid.to_hex(), ref_name.to_str_lossy());
        
        if let Some(peeled_oid) = peeled {
            line.push_str(&format!(" peeled:{}", peeled_oid.to_hex()));
        }
        
        line.push('\n');
        self.send_data(line.as_bytes())
    }
    
    /// Send symref information
    pub fn send_symref(&mut self, name: &BStr, target: &BStr) -> Result<()> {
        let line = format!("symref-target:{} {}\n", name.to_str_lossy(), target.to_str_lossy());
        self.send_data(line.as_bytes())
    }
    
    /// Send unborn HEAD information
    pub fn send_unborn(&mut self, ref_name: &BStr) -> Result<()> {
        let line = format!("unborn {}\n", ref_name.to_str_lossy());
        self.send_data(line.as_bytes())
    }
    
    /// Send server information
    pub fn send_server_info(&mut self, key: &str, value: &str) -> Result<()> {
        let line = format!("{} {}\n", key, value);
        self.send_data(line.as_bytes())
    }
    
    /// Send object information
    pub fn send_object_info(&mut self, oid: &gix_hash::ObjectId, info: &ObjectInfo) -> Result<()> {
        let mut line = oid.to_hex().to_string();
        
        if let Some(size) = info.size {
            line.push_str(&format!(" size {}", size));
        }
        
        if let Some(ref obj_type) = info.object_type {
            line.push_str(&format!(" type {}", obj_type));
        }
        
        line.push('\n');
        self.send_data(line.as_bytes())
    }
    
    /// Send section header (protocol V2)
    pub fn send_section(&mut self, section_name: &str) -> Result<()> {
        let line = format!("{}\n", section_name);
        self.send_data(line.as_bytes())
    }
    
    /// Send a generic response line
    pub fn send_line(&mut self, line: &str) -> Result<()> {
        let line_with_newline = if line.ends_with('\n') {
            line.to_string()
        } else {
            format!("{}\n", line)
        };
        self.send_data(line_with_newline.as_bytes())
    }
    
    /// Send binary data (like pack files)
    pub fn send_binary(&mut self, data: &[u8]) -> Result<()> {
        match self.side_band_mode {
            SideBandMode::None => {
                // Send raw binary data
                self.writer.write_all(data)?;
            }
            SideBandMode::Basic | SideBandMode::SideBand64k => {
                // Send through side-band
                self.send_side_band(SideBandChannel::Data, data)?;
            }
        }
        Ok(())
    }
    
    /// Get the maximum packet size for this formatter
    pub fn max_packet_size(&self) -> usize {
        match self.side_band_mode {
            SideBandMode::None => 65520, // Standard Git packet line limit
            SideBandMode::Basic => 999,   // 1000 - 1 byte for channel
            SideBandMode::SideBand64k => 65519, // 65520 - 1 byte for channel
        }
    }
    
    /// Check if progress messages are supported
    pub fn supports_progress(&self) -> bool {
        self.side_band_mode.supports_channel(SideBandChannel::Progress)
    }
    
    /// Check if error messages are supported
    pub fn supports_errors(&self) -> bool {
        self.side_band_mode.supports_channel(SideBandChannel::Error)
    }
}

/// Object information for object-info responses
#[derive(Debug, Default, Clone)]
pub struct ObjectInfo {
    /// Size of the object
    pub size: Option<u64>,
    /// Type of the object
    pub object_type: Option<String>,
}

/// Progress reporter for long-running operations
pub struct ProgressReporter<'a, W: std::io::Write> {
    formatter: &'a mut ResponseFormatter<'a, W>,
    operation: String,
    total: Option<usize>, // Changed from u64 to usize to match Step
    current: usize,       // Changed from u64 to usize to match Step
    last_report_time: std::time::Instant,
    report_interval: std::time::Duration,
}

impl<'a, W: std::io::Write> ProgressReporter<'a, W> {
    /// Create a new progress reporter
    pub fn new(
        formatter: &'a mut ResponseFormatter<'a, W>,
        operation: String,
        total: Option<usize>, // Changed from u64 to usize
    ) -> Self {
        Self {
            formatter,
            operation,
            total,
            current: 0,
            last_report_time: std::time::Instant::now(),
            report_interval: std::time::Duration::from_millis(100), // Report every 100ms
        }
    }
    
    /// Update progress
    pub fn update(&mut self, current: usize) -> Result<()> { // Changed from u64 to usize
        self.current = current;
        
        self.report()?;
        
        Ok(())
    }
    
    /// Force a progress report
    pub fn report(&mut self) -> Result<()> {
        if !self.formatter.supports_progress() {
            return Ok(());
        }
        
        let message = if let Some(total) = self.total {
            format!("{}: {}/{} ({:.1}%)", 
                   self.operation, 
                   self.current, 
                   total,
                   (self.current as f64 / total as f64) * 100.0)
        } else {
            format!("{}: {}", self.operation, self.current)
        };
        
        self.formatter.send_progress(&message)
    }
    
    /// Finish the progress reporting
    pub fn finish(&mut self) -> Result<()> {
        if !self.formatter.supports_progress() {
            return Ok(());
        }
        
        let message = if let Some(total) = self.total {
            format!("{}: {} complete", self.operation, total)
        } else {
            format!("{}: {} complete", self.operation, self.current)
        };
        
        self.formatter.send_progress(&message)
    }
}

/// Error response helper
pub struct ErrorResponse;

impl ErrorResponse {
    /// Format a generic error response
    pub fn generic(message: &str) -> String {
        format!("error: {}", message)
    }
    
    /// Format an object not found error
    pub fn object_not_found(oid: &str) -> String {
        format!("error: Object {} not found", oid)
    }
    
    /// Format a reference not found error
    pub fn ref_not_found(ref_name: &str) -> String {
        format!("error: Reference {} not found", ref_name)
    }
    
    /// Format an unsupported capability error
    pub fn unsupported_capability(capability: &str) -> String {
        format!("error: Capability '{}' not supported", capability)
    }
    
    /// Format a protocol error
    pub fn protocol_error(details: &str) -> String {
        format!("error: Protocol error: {}", details)
    }
    
    /// Format a permission denied error
    pub fn permission_denied(resource: &str) -> String {
        format!("error: Permission denied: {}", resource)
    }
    
    /// Format a repository error
    pub fn repository_error(details: &str) -> String {
        format!("error: Repository error: {}", details)
    }
}
