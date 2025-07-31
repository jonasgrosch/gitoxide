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
use gix_packetline::PacketLineRef;
use std::io::Write;

/// Response formatter for upload-pack protocol
pub struct ResponseFormatter<'a, W: Write> {
    writer: &'a mut W,
    side_band_mode: SideBandMode,
    session_id: Option<&'a BStr>,
}

/// Side-band channel types
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SideBandChannel {
    /// Main data channel
    Data = 1,
    /// Progress messages
    Progress = 2,
    /// Error messages  
    Error = 3,
}

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
        match self.side_band_mode {
            SideBandMode::None => {
                // Send data directly
                gix_packetline::encode::data_to_write(data, &mut *self.writer)?;
            }
            SideBandMode::Basic | SideBandMode::SideBand64k => {
                // Send with side-band multiplexing
                self.send_side_band(SideBandChannel::Data, data)?;
            }
        }
        Ok(())
    }
    
    /// Send a progress message
    pub fn send_progress(&mut self, message: &str) -> Result<()> {
        match self.side_band_mode {
            SideBandMode::None => {
                // Can't send progress without side-band
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
        let max_data_size = match self.side_band_mode {
            SideBandMode::Basic => 999,  // 1000 - 1 byte for channel
            SideBandMode::SideBand64k => 65519, // 65520 - 1 byte for channel
            SideBandMode::None => unreachable!(),
        };
        
        // Split data into chunks if necessary
        for chunk in data.chunks(max_data_size) {
            let mut packet_data = vec![channel as u8];
            packet_data.extend_from_slice(chunk);
            
            gix_packetline::encode::data_to_write(&packet_data, &mut *self.writer)?;
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
        let status_str = match status {
            AckStatus::Common => "",
            AckStatus::Continue => " continue",
            AckStatus::Ready => " ready",
        };
        
        let response = format!("ACK {}{}\n", oid.to_hex(), status_str);
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
        let mut line = format!("{} {}", reference.target.to_hex(), reference.name.to_str_lossy());
        
        if let Some(peeled) = &reference.peeled {
            line.push_str(&format!(" peeled:{}", peeled.to_hex()));
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
            SideBandMode::Basic => 999,   // 1000 - 1 for side-band channel
            SideBandMode::SideBand64k => 65519, // 65520 - 1 for side-band channel
        }
    }
    
    /// Check if progress messages are supported
    pub fn supports_progress(&self) -> bool {
        self.side_band_mode != SideBandMode::None
    }
    
    /// Check if error messages are supported
    pub fn supports_errors(&self) -> bool {
        self.side_band_mode != SideBandMode::None
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
pub struct ProgressReporter<'a, W: Write> {
    formatter: &'a mut ResponseFormatter<'a, W>,
    operation: String,
    total: Option<u64>,
    current: u64,
    last_report_time: std::time::Instant,
    report_interval: std::time::Duration,
}

impl<'a, W: Write> ProgressReporter<'a, W> {
    /// Create a new progress reporter
    pub fn new(
        formatter: &'a mut ResponseFormatter<'a, W>,
        operation: String,
        total: Option<u64>,
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
    pub fn update(&mut self, current: u64) -> Result<()> {
        self.current = current;
        
        let now = std::time::Instant::now();
        if now.duration_since(self.last_report_time) >= self.report_interval {
            self.report()?;
            self.last_report_time = now;
        }
        
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

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_response_formatter_basic() {
        let mut buffer = Vec::new();
        let mut formatter = ResponseFormatter::new(&mut buffer, SideBandMode::None);
        
        formatter.send_line("test message").unwrap();
        formatter.send_flush().unwrap();
        
        // Verify packet format
        assert!(!buffer.is_empty());
    }
    
    #[test] 
    fn test_progress_reporter() {
        let mut buffer = Vec::new();
        let mut formatter = ResponseFormatter::new(&mut buffer, SideBandMode::SideBand64k);
        let mut progress = ProgressReporter::new(&mut formatter, "Testing".to_string(), Some(100));
        
        progress.update(50).unwrap();
        progress.finish().unwrap();
        
        // Verify progress messages were sent
        assert!(!buffer.is_empty());
    }
    
    #[test]
    fn test_error_responses() {
        assert_eq!(ErrorResponse::object_not_found("abc123"), "error: Object abc123 not found");
        assert_eq!(ErrorResponse::unsupported_capability("test"), "error: Capability 'test' not supported");
    }
}
