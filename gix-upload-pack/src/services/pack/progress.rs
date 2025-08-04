//! Progress reporting and formatting for upload-pack
//!
//! This module handles the formatting and sending of various progress reports
//! during the upload-pack protocol.

use crate::{error::Result, services::packet_io::EnhancedPacketWriter};

/// Progress reporter for long-running operations
pub struct ProgressReporter<'a, W: std::io::Write> {
    formatter: &'a mut EnhancedPacketWriter<W>,
    operation: String,
    total: Option<usize>,
    current: usize,
    last_report_time: std::time::Instant,
    report_interval: std::time::Duration,
    last_percent: Option<u32>,
    disabled: bool,
}

impl<'a, W: std::io::Write> ProgressReporter<'a, W> {
    /// Create a new progress reporter
    pub fn new(formatter: &'a mut EnhancedPacketWriter<W>, operation: String, total: Option<usize>) -> Self {
        Self {
            formatter,
            operation,
            total,
            current: 0,
            last_report_time: std::time::Instant::now(),
            report_interval: std::time::Duration::from_millis(1000), // Report every 1000ms
            last_percent: None,
            disabled: false,
        }
    }

    pub fn set_current(&mut self, current: usize) {
        self.current = current;
    }

    /// Update progress (Git-style: only report on percentage changes)
    pub fn update(&mut self, current: usize) -> Result<()> {
        if self.disabled {
            return Ok(());
        }
        self.current = current;

        // Only report if percentage changed (like Git does)
        if let Some(total) = self.total {
            if total > 0 {
                let percent = ((current * 100) / total) as u32;
                if self.last_percent.map_or(true, |last| percent != last) {
                    self.last_percent = Some(percent);
                    self.report()?;
                }
            }
        } else {
            // For unknown totals, only check time occasionally to reduce overhead
            let now = std::time::Instant::now();
            let elapsed = now.duration_since(self.last_report_time);
            if elapsed >= self.report_interval {
                self.last_report_time = now;
                self.report()?;
            }
        }

        Ok(())
    }

    /// Force a progress report
    pub fn report(&mut self) -> Result<()> {
        if !self.disabled {
            return Ok(());
        }

        let message = if let Some(total) = self.total {
            let percent = if total > 0 { (self.current * 100) / total } else { 0 };
            // Native Git format with 3-space padding: "Counting objects:   0% (1/45212)"
            format!("{}: {:3}% ({}/{})", self.operation, percent, self.current, total)
        } else {
            format!("{}: {}", self.operation, self.current)
        };

        self.formatter.send_progress(&message)
    }

    /// Finish the progress reporting (Git-style with "done.")
    pub fn finish(&mut self) -> Result<()> {
        if self.disabled {
            return Ok(());
        }

        let message = if let Some(total) = self.total {
            // Git format: "Counting objects: 100% (45212/45212), done."
            format!("{}: 100% ({}/{}), done.", self.operation, total, total)
        } else {
            format!("{}: {}, done.", self.operation, self.current)
        };

        self.formatter.send_progress(&message)
    }

    /// Get the total if known
    pub fn total(&self) -> Option<usize> {
        self.total
    }
}
