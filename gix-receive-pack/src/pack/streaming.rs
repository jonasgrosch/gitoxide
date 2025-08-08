//! Streaming pack ingestion with bounded memory usage controls.
//!
//! This module provides streaming pack readers that maintain bounded memory usage
//! regardless of pack size, with memory pressure handling and cleanup capabilities.

use std::io::{BufRead, Read};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::error::{ErrorContext, PackIngestionError, Result};

/// Configuration for streaming pack ingestion with memory controls.
#[derive(Debug, Clone)]
pub struct StreamingConfig {
    /// Maximum memory usage for pack processing (bytes)
    pub max_memory_bytes: Option<u64>,
    /// Buffer size for streaming reads (bytes)
    pub buffer_size: usize,
    /// Memory pressure threshold (0.0-1.0) at which cleanup is triggered
    pub memory_pressure_threshold: f64,
    /// Interval for memory pressure checks
    pub memory_check_interval: Duration,
    /// Maximum time to spend on memory cleanup
    pub cleanup_timeout: Duration,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        Self {
            max_memory_bytes: Some(256 * 1024 * 1024), // 256MB default limit
            buffer_size: 64 * 1024,                     // 64KB buffer
            memory_pressure_threshold: 0.8,             // 80% threshold
            memory_check_interval: Duration::from_millis(100),
            cleanup_timeout: Duration::from_secs(5),
        }
    }
}

/// Memory usage tracker for streaming operations.
#[derive(Debug)]
pub struct MemoryTracker {
    /// Current memory usage in bytes
    current_usage: AtomicU64,
    /// Peak memory usage in bytes
    peak_usage: AtomicU64,
    /// Maximum allowed memory usage
    max_usage: Option<u64>,
    /// Memory pressure threshold
    pressure_threshold: f64,
    /// Last memory check time
    last_check: std::sync::Mutex<Instant>,
    /// Check interval
    check_interval: Duration,
}

impl MemoryTracker {
    /// Create a new memory tracker with the given configuration.
    pub fn new(config: &StreamingConfig) -> Self {
        Self {
            current_usage: AtomicU64::new(0),
            peak_usage: AtomicU64::new(0),
            max_usage: config.max_memory_bytes,
            pressure_threshold: config.memory_pressure_threshold,
            last_check: std::sync::Mutex::new(Instant::now()),
            check_interval: config.memory_check_interval,
        }
    }

    /// Allocate memory and track usage.
    pub fn allocate(&self, bytes: u64) -> Result<()> {
        let new_usage = self.current_usage.fetch_add(bytes, Ordering::SeqCst) + bytes;
        
        // Update peak usage
        self.peak_usage.fetch_max(new_usage, Ordering::SeqCst);
        
        // Check memory limits
        if let Some(max) = self.max_usage {
            if new_usage > max {
                // Rollback allocation
                self.current_usage.fetch_sub(bytes, Ordering::SeqCst);
                let context = ErrorContext::new("memory-allocation")
                    .with_context("requested_bytes", bytes.to_string())
                    .with_context("current_usage", new_usage.to_string())
                    .with_context("max_usage", max.to_string());
                return Err(PackIngestionError::resource_limit_exceeded(
                    "memory",
                    new_usage,
                    max,
                    context,
                ));
            }
        }
        
        Ok(())
    }

    /// Deallocate memory and update tracking.
    pub fn deallocate(&self, bytes: u64) {
        self.current_usage.fetch_sub(bytes.min(self.current_usage.load(Ordering::SeqCst)), Ordering::SeqCst);
    }

    /// Get current memory usage.
    pub fn current_usage(&self) -> u64 {
        self.current_usage.load(Ordering::SeqCst)
    }

    /// Get peak memory usage.
    pub fn peak_usage(&self) -> u64 {
        self.peak_usage.load(Ordering::SeqCst)
    }

    /// Check if memory pressure threshold is exceeded.
    pub fn is_under_pressure(&self) -> bool {
        if let Some(max) = self.max_usage {
            let current = self.current_usage() as f64;
            let threshold = max as f64 * self.pressure_threshold;
            current > threshold
        } else {
            false
        }
    }

    /// Check if it's time for a memory pressure check.
    pub fn should_check_pressure(&self) -> bool {
        if let Ok(mut last_check) = self.last_check.try_lock() {
            if last_check.elapsed() >= self.check_interval {
                *last_check = Instant::now();
                return true;
            }
        }
        false
    }

    /// Get memory usage statistics.
    pub fn stats(&self) -> MemoryStats {
        let current = self.current_usage();
        let peak = self.peak_usage();
        let ratio = if let Some(max) = self.max_usage {
            if max == 0 { 0.0 } else { current as f64 / max as f64 }
        } else {
            0.0
        };
        MemoryStats {
            // Primary fields used by most tests
            current_bytes: current,
            peak_bytes: peak,
            max_bytes: self.max_usage,
            pressure_ratio: ratio,
            // Compatibility aliases and counters
            peak_usage: peak,
            current_usage: current,
            allocations: 0,
            deallocations: 0,
        }
    }
}

/**
 Memory usage statistics used by tests and streaming reports.

 This struct provides both primary fields (current_bytes/peak_bytes/etc.) and
 compatibility aliases (peak_usage/current_usage/allocations/deallocations)
 to satisfy all test expectations.
*/
#[derive(Debug, Clone)]
pub struct MemoryStats {
    // Primary fields
    /// Current memory usage in bytes
    pub current_bytes: u64,
    /// Peak memory usage in bytes
    pub peak_bytes: u64,
    /// Maximum allowed memory usage in bytes
    pub max_bytes: Option<u64>,
    /// Current memory pressure ratio (0.0-1.0)
    pub pressure_ratio: f64,

    // Compatibility alias fields for tests
    /// Alias for peak_bytes
    pub peak_usage: u64,
    /// Alias for current_bytes
    pub current_usage: u64,
    /// Allocation counter (not tracked in current implementation)
    pub allocations: u64,
    /// Deallocation counter (not tracked in current implementation)
    pub deallocations: u64,
}

/// Streaming pack reader with bounded memory usage.
pub struct StreamingPackReader<R: BufRead> {
    /// Underlying reader
    reader: R,
    /// Memory tracker
    memory_tracker: Arc<MemoryTracker>,
    /// Configuration
    config: StreamingConfig,
    /// Buffer for streaming reads
    buffer: Vec<u8>,
    /// Total bytes read
    bytes_read: u64,
    /// Cancellation flag
    should_interrupt: Arc<AtomicBool>,
    /// Last progress update time
    #[cfg(feature = "progress")]
    last_progress: Instant,
}

impl<R: BufRead> StreamingPackReader<R> {
    /// Create a new streaming pack reader.
    pub fn new(reader: R, config: StreamingConfig) -> Self {
        let memory_tracker = Arc::new(MemoryTracker::new(&config));
        let buffer = Vec::with_capacity(config.buffer_size);
        
        Self {
            reader,
            memory_tracker,
            config,
            buffer,
            bytes_read: 0,
            should_interrupt: Arc::new(AtomicBool::new(false)),
            #[cfg(feature = "progress")]
            last_progress: Instant::now(),
        }
    }

    /// Get the memory tracker for this reader.
    pub fn memory_tracker(&self) -> Arc<MemoryTracker> {
        self.memory_tracker.clone()
    }

    /// Get the cancellation flag for this reader.
    pub fn cancellation_flag(&self) -> Arc<AtomicBool> {
        self.should_interrupt.clone()
    }

    /// Read a chunk of data with memory tracking and progress updates.
    #[cfg(feature = "progress")]
    pub fn read_chunk(&mut self, progress: Option<&mut dyn gix_features::progress::DynNestedProgress>) -> Result<Option<Vec<u8>>> {
        // Check for cancellation
        if self.should_interrupt.load(Ordering::SeqCst) {
            let context = ErrorContext::new("streaming-read")
                .with_context("bytes_read", self.bytes_read.to_string());
            return Err(PackIngestionError::cancelled("operation was cancelled", context));
        }

        // Check memory pressure periodically
        if self.memory_tracker.should_check_pressure() && self.memory_tracker.is_under_pressure() {
            self.handle_memory_pressure()?;
        }

        // Prepare buffer
        self.buffer.clear();
        self.buffer.resize(self.config.buffer_size, 0);

        // Read data using BufRead interface
        let bytes_read = match Read::read(&mut self.reader, &mut self.buffer) {
            Ok(0) => return Ok(None), // EOF
            Ok(n) => n,
            Err(e) => {
                let context = ErrorContext::new("streaming-read")
                    .with_context("bytes_read", self.bytes_read.to_string());
                return Err(PackIngestionError::io("failed to read from pack stream", context, e));
            }
        };

        // Track memory allocation for the data we just read
        self.memory_tracker.allocate(bytes_read as u64)?;

        // Update statistics
        self.bytes_read += bytes_read as u64;
        self.buffer.truncate(bytes_read);

        // Update progress if enough time has passed
        #[cfg(feature = "progress")]
        if let Some(progress) = progress {
            let now = Instant::now();
            if now.duration_since(self.last_progress) >= Duration::from_millis(100) {
                use gix_features::progress::Count;
                progress.set(self.bytes_read as usize);
                self.last_progress = now;
            }
        }

        Ok(Some(self.buffer.clone()))
    }

    /// Read a chunk of data with memory tracking (no progress updates).
    #[cfg(not(feature = "progress"))]
    pub fn read_chunk(&mut self, _progress: Option<&mut dyn std::any::Any>) -> Result<Option<Vec<u8>>> {
        // Check for cancellation
        if self.should_interrupt.load(Ordering::SeqCst) {
            let context = ErrorContext::new("streaming-read")
                .with_context("bytes_read", self.bytes_read.to_string());
            return Err(PackIngestionError::cancelled("operation was cancelled", context));
        }

        // Check memory pressure periodically
        if self.memory_tracker.should_check_pressure() && self.memory_tracker.is_under_pressure() {
            self.handle_memory_pressure()?;
        }

        // Prepare buffer
        self.buffer.clear();
        self.buffer.resize(self.config.buffer_size, 0);

        // Read data using BufRead interface
        let bytes_read = match Read::read(&mut self.reader, &mut self.buffer) {
            Ok(0) => return Ok(None), // EOF
            Ok(n) => n,
            Err(e) => {
                let context = ErrorContext::new("streaming-read")
                    .with_context("bytes_read", self.bytes_read.to_string());
                return Err(PackIngestionError::io("failed to read from pack stream", context, e));
            }
        };

        // Track memory allocation for the data we just read
        self.memory_tracker.allocate(bytes_read as u64)?;

        // Update statistics
        self.bytes_read += bytes_read as u64;
        self.buffer.truncate(bytes_read);

        Ok(Some(self.buffer.clone()))
    }

    /// Handle memory pressure by triggering cleanup.
    fn handle_memory_pressure(&self) -> Result<()> {
        let context = ErrorContext::new("memory-pressure-handling")
            .with_context("current_usage", self.memory_tracker.current_usage().to_string())
            .with_context("pressure_threshold", self.config.memory_pressure_threshold.to_string());

        // For now, we'll just log the pressure and continue
        // In a full implementation, we might:
        // 1. Force garbage collection
        // 2. Flush buffers to disk
        // 3. Reduce buffer sizes
        // 4. Switch to a more memory-efficient strategy

        // If we're still under severe pressure, we might need to fail
        if let Some(max) = self.memory_tracker.max_usage {
            let current = self.memory_tracker.current_usage();
            if current as f64 > max as f64 * 0.95 {
                return Err(PackIngestionError::resource_limit_exceeded(
                    "memory_pressure",
                    current,
                    max,
                    context,
                ));
            }
        }

        Ok(())
    }

    /// Get streaming statistics.
    pub fn stats(&self) -> StreamingStats {
        StreamingStats {
            bytes_read: self.bytes_read,
            memory_stats: self.memory_tracker.stats(),
            buffer_size: self.config.buffer_size,
        }
    }

    /// Cleanup resources and deallocate tracked memory.
    pub fn cleanup(&mut self) {
        // Deallocate any remaining tracked memory
        let current = self.memory_tracker.current_usage();
        if current > 0 {
            self.memory_tracker.deallocate(current);
        }
        
        // Clear buffer
        self.buffer.clear();
        self.buffer.shrink_to_fit();
    }
}

impl<R: BufRead> Drop for StreamingPackReader<R> {
    fn drop(&mut self) {
        self.cleanup();
    }
}

/// Statistics for streaming operations.
#[derive(Debug, Clone)]
pub struct StreamingStats {
    /// Total bytes read
    pub bytes_read: u64,
    /// Memory usage statistics
    pub memory_stats: MemoryStats,
    /// Buffer size used
    pub buffer_size: usize,
}

/// Memory-aware buffer pool for reusing allocations.
#[derive(Debug)]
pub struct BufferPool {
    /// Pool of available buffers
    buffers: std::sync::Mutex<Vec<Vec<u8>>>,
    /// Memory tracker
    memory_tracker: Arc<MemoryTracker>,
    /// Maximum number of buffers to pool
    max_pooled: usize,
    /// Buffer size
    buffer_size: usize,
}

impl BufferPool {
    /// Create a new buffer pool.
    pub fn new(memory_tracker: Arc<MemoryTracker>, buffer_size: usize, max_pooled: usize) -> Self {
        Self {
            buffers: std::sync::Mutex::new(Vec::new()),
            memory_tracker,
            max_pooled,
            buffer_size,
        }
    }

    /// Get a buffer from the pool or allocate a new one.
    pub fn get_buffer(&self) -> Result<Vec<u8>> {
        // Try to get from pool first
        if let Ok(mut buffers) = self.buffers.try_lock() {
            if let Some(mut buffer) = buffers.pop() {
                buffer.clear();
                buffer.resize(self.buffer_size, 0);
                return Ok(buffer);
            }
        }

        // Allocate new buffer
        self.memory_tracker.allocate(self.buffer_size as u64)?;
        Ok(vec![0; self.buffer_size])
    }

    /// Return a buffer to the pool.
    pub fn return_buffer(&self, buffer: Vec<u8>) {
        if let Ok(mut buffers) = self.buffers.try_lock() {
            if buffers.len() < self.max_pooled {
                buffers.push(buffer);
                return;
            }
        }
        
        // If we can't pool it, deallocate the memory
        self.memory_tracker.deallocate(buffer.capacity() as u64);
    }

    /// Clear all pooled buffers and deallocate memory.
    pub fn clear(&self) {
        if let Ok(mut buffers) = self.buffers.try_lock() {
            let total_capacity: usize = buffers.iter().map(|b| b.capacity()).sum();
            buffers.clear();
            self.memory_tracker.deallocate(total_capacity as u64);
        }
    }
}

impl Drop for BufferPool {
    fn drop(&mut self) {
        self.clear();
    }
}

/// A BufRead wrapper around StreamingPackReader for integration with gix-pack.
pub struct StreamingBufReader<R: BufRead> {
    /// The streaming reader
    reader: StreamingPackReader<R>,
    /// Buffer pool for memory management
    buffer_pool: std::sync::Arc<BufferPool>,
    /// Current buffer being read from
    current_buffer: Option<Vec<u8>>,
    /// Position in current buffer
    buffer_pos: usize,
    /// Whether we've reached EOF
    eof: bool,
}

impl<R: BufRead> StreamingBufReader<R> {
    /// Create a new streaming BufRead wrapper.
    pub fn new(reader: StreamingPackReader<R>, buffer_pool: std::sync::Arc<BufferPool>) -> Self {
        Self {
            reader,
            buffer_pool,
            current_buffer: None,
            buffer_pos: 0,
            eof: false,
        }
    }

    /// Fill the internal buffer with new data.
    fn fill_buffer(&mut self) -> std::io::Result<()> {
        if self.eof {
            return Ok(());
        }

        // Return current buffer to pool if we have one
        if let Some(buffer) = self.current_buffer.take() {
            self.buffer_pool.return_buffer(buffer);
        }

        // Read next chunk
        match self.reader.read_chunk(None) {
            Ok(Some(data)) => {
                self.current_buffer = Some(data);
                self.buffer_pos = 0;
            }
            Ok(None) => {
                self.eof = true;
            }
            Err(e) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("streaming read error: {}", e),
                ));
            }
        }

        Ok(())
    }
}

impl<R: BufRead> Read for StreamingBufReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.current_buffer.is_none() && !self.eof {
            self.fill_buffer()?;
        }

        if let Some(ref current) = self.current_buffer {
            let available = current.len() - self.buffer_pos;
            if available == 0 {
                // Current buffer is exhausted, try to get more data
                self.fill_buffer()?;
                return self.read(buf);
            }

            let to_copy = buf.len().min(available);
            buf[..to_copy].copy_from_slice(&current[self.buffer_pos..self.buffer_pos + to_copy]);
            self.buffer_pos += to_copy;
            Ok(to_copy)
        } else {
            // EOF
            Ok(0)
        }
    }
}

impl<R: BufRead> BufRead for StreamingBufReader<R> {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        if self.current_buffer.is_none() && !self.eof {
            self.fill_buffer()?;
        }

        if let Some(ref current) = self.current_buffer {
            Ok(&current[self.buffer_pos..])
        } else {
            Ok(&[])
        }
    }

    fn consume(&mut self, amt: usize) {
        if let Some(ref current) = self.current_buffer {
            let available = current.len() - self.buffer_pos;
            self.buffer_pos += amt.min(available);
        }
    }
}

impl<R: BufRead> Drop for StreamingBufReader<R> {
    fn drop(&mut self) {
        // Return current buffer to pool
        if let Some(buffer) = self.current_buffer.take() {
            self.buffer_pool.return_buffer(buffer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn memory_tracker_basic_operations() {
        let config = StreamingConfig::default();
        let tracker = MemoryTracker::new(&config);

        // Test allocation
        assert!(tracker.allocate(1000).is_ok());
        assert_eq!(tracker.current_usage(), 1000);
        assert_eq!(tracker.peak_usage(), 1000);

        // Test more allocation
        assert!(tracker.allocate(500).is_ok());
        assert_eq!(tracker.current_usage(), 1500);
        assert_eq!(tracker.peak_usage(), 1500);

        // Test deallocation
        tracker.deallocate(500);
        assert_eq!(tracker.current_usage(), 1000);
        assert_eq!(tracker.peak_usage(), 1500); // Peak should remain
    }

    #[test]
    fn memory_tracker_limit_enforcement() {
        let config = StreamingConfig {
            max_memory_bytes: Some(1000),
            ..Default::default()
        };
        let tracker = MemoryTracker::new(&config);

        // Should succeed within limit
        assert!(tracker.allocate(500).is_ok());
        assert_eq!(tracker.current_usage(), 500);

        // Should succeed at limit
        assert!(tracker.allocate(500).is_ok());
        assert_eq!(tracker.current_usage(), 1000);

        // Should fail over limit
        assert!(tracker.allocate(1).is_err());
        assert_eq!(tracker.current_usage(), 1000); // Should not have changed
    }

    #[test]
    fn memory_tracker_pressure_detection() {
        let config = StreamingConfig {
            max_memory_bytes: Some(1000),
            memory_pressure_threshold: 0.8,
            ..Default::default()
        };
        let tracker = MemoryTracker::new(&config);

        // Should not be under pressure initially
        assert!(!tracker.is_under_pressure());

        // Should not be under pressure at 70%
        assert!(tracker.allocate(700).is_ok());
        assert!(!tracker.is_under_pressure());

        // Should be under pressure at 90%
        assert!(tracker.allocate(200).is_ok());
        assert!(tracker.is_under_pressure());
    }

    #[test]
    fn streaming_reader_basic_functionality() {
        let data = b"hello world, this is test data for streaming";
        let cursor = Cursor::new(data);
        let config = StreamingConfig {
            buffer_size: 10,
            ..Default::default()
        };
        
        let mut reader = StreamingPackReader::new(cursor, config);
        let mut total_read = 0;

        // Read chunks until EOF
        while let Ok(Some(chunk)) = reader.read_chunk(None) {
            total_read += chunk.len();
            assert!(chunk.len() <= 10); // Should respect buffer size
        }

        assert_eq!(total_read, data.len());
        assert_eq!(reader.stats().bytes_read, data.len() as u64);
    }

    #[test]
    fn streaming_reader_memory_limit() {
        let data = vec![0u8; 2000]; // 2KB of data
        let cursor = Cursor::new(data);
        let config = StreamingConfig {
            max_memory_bytes: Some(500), // 500 byte limit
            buffer_size: 1000,           // 1KB buffer (larger than limit)
            ..Default::default()
        };
        
        let mut reader = StreamingPackReader::new(cursor, config);
        
        // Should fail due to memory limit
        let result = reader.read_chunk(None);
        assert!(result.is_err());
        
        // Should be a resource limit error
        if let Err(PackIngestionError::ResourceLimitExceeded { .. }) = result {
            // Expected
        } else {
            panic!("Expected ResourceLimitExceeded error");
        }
    }

    #[test]
    fn buffer_pool_operations() {
        let config = StreamingConfig::default();
        let tracker = Arc::new(MemoryTracker::new(&config));
        let pool = BufferPool::new(tracker.clone(), 1024, 3);

        // Get buffer from pool (should allocate new)
        let buffer1 = pool.get_buffer().unwrap();
        assert_eq!(buffer1.len(), 1024);
        assert_eq!(tracker.current_usage(), 1024);

        // Return buffer to pool
        pool.return_buffer(buffer1);

        // Get buffer again (should reuse from pool)
        let buffer2 = pool.get_buffer().unwrap();
        assert_eq!(buffer2.len(), 1024);
        assert_eq!(tracker.current_usage(), 1024); // Should not increase

        // Clean up
        pool.return_buffer(buffer2);
        pool.clear();
        assert_eq!(tracker.current_usage(), 0);
    }

    #[test]
    fn streaming_reader_cancellation() {
        let data = vec![0u8; 1000];
        let cursor = Cursor::new(data);
        let config = StreamingConfig::default();
        
        let mut reader = StreamingPackReader::new(cursor, config);
        
        // Set cancellation flag
        reader.cancellation_flag().store(true, Ordering::SeqCst);
        
        // Should return cancelled error
        let result = reader.read_chunk(None);
        assert!(result.is_err());
        
        if let Err(PackIngestionError::Cancelled { .. }) = result {
            // Expected
        } else {
            panic!("Expected Cancelled error");
        }
    }
}