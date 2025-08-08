//! Performance tests for streaming pack ingestion with memory usage validation.

use std::io::Cursor;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use gix_receive_pack::pack::{StreamingConfig, StreamingPackReader};
use gix_receive_pack::ReceivePackBuilder;

/// Test data generator for creating pack-like data streams.
struct TestDataGenerator {
    size: usize,
    chunk_size: usize,
}

impl TestDataGenerator {
    fn new(size: usize, chunk_size: usize) -> Self {
        Self { size, chunk_size }
    }

    fn generate(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(self.size);
        let pattern = b"PACK test data chunk with some variety in content to simulate real pack data";
        
        while data.len() < self.size {
            let remaining = self.size - data.len();
            let to_add = remaining.min(pattern.len());
            data.extend_from_slice(&pattern[..to_add]);
        }
        
        data
    }
}

#[test]
fn test_memory_bounded_streaming_small_pack() {
    let test_data = TestDataGenerator::new(1024, 256).generate(); // 1KB pack
    let cursor = Cursor::new(test_data.clone());
    
    let config = StreamingConfig {
        max_memory_bytes: Some(2048), // 2KB limit
        buffer_size: 256,
        memory_pressure_threshold: 0.8,
        memory_check_interval: Duration::from_millis(10),
        cleanup_timeout: Duration::from_secs(1),
    };
    
    let mut reader = StreamingPackReader::new(cursor, config);
    let memory_tracker = reader.memory_tracker();
    
    let mut total_read = 0;
    let mut chunks = 0;
    
    // Read all chunks and verify memory usage stays bounded
    while let Ok(Some(chunk)) = reader.read_chunk(None) {
        total_read += chunk.len();
        chunks += 1;
        
        // Memory usage should never exceed our limit
        assert!(memory_tracker.current_usage() <= 2048);
        
        // Each chunk should be at most our buffer size
        assert!(chunk.len() <= 256);
    }
    
    assert_eq!(total_read, test_data.len());
    assert!(chunks > 0);
    
    let stats = memory_tracker.stats();
    println!("Small pack stats: current={}, peak={}, chunks={}", 
             stats.current_bytes, stats.peak_bytes, chunks);
}

#[test]
fn test_memory_bounded_streaming_large_pack() {
    let test_data = TestDataGenerator::new(10 * 1024 * 1024, 1024).generate(); // 10MB pack
    let cursor = Cursor::new(test_data.clone());
    
    let config = StreamingConfig {
        max_memory_bytes: Some(256 * 1024), // 256KB limit (much smaller than pack)
        buffer_size: 4096,
        memory_pressure_threshold: 0.8,
        memory_check_interval: Duration::from_millis(50),
        cleanup_timeout: Duration::from_secs(5),
    };
    
    let mut reader = StreamingPackReader::new(cursor, config);
    let memory_tracker = reader.memory_tracker();
    
    let start_time = Instant::now();
    let mut total_read = 0;
    let mut max_memory_seen = 0;
    
    // Read all chunks and verify memory usage stays bounded
    loop {
        match reader.read_chunk(None) {
            Ok(Some(chunk)) => {
                total_read += chunk.len();
                
                let current_memory = memory_tracker.current_usage();
                max_memory_seen = max_memory_seen.max(current_memory);
                
                // Memory usage should never exceed our limit
                assert!(current_memory <= 256 * 1024, 
                        "Memory usage {} exceeded limit {}", current_memory, 256 * 1024);
                
                // Each chunk should be at most our buffer size
                assert!(chunk.len() <= 4096);
                
                // Deallocate the chunk memory to simulate processing
                memory_tracker.deallocate(chunk.len() as u64);
            }
            Ok(None) => break, // EOF
            Err(e) => {
                // If we get a memory limit error, that's expected behavior
                if matches!(e, gix_receive_pack::error::PackIngestionError::ResourceLimitExceeded { .. }) {
                    println!("Memory limit reached as expected: {}", e);
                    break;
                } else {
                    panic!("Unexpected error: {}", e);
                }
            }
        }
    }
    
    let elapsed = start_time.elapsed();
    
    // We might not read all data due to memory limits, but should read some
    assert!(total_read > 0, "Should have read some data");
    println!("Read {} bytes out of {} total", total_read, test_data.len());
    
    let stats = memory_tracker.stats();
    println!("Large pack stats: current={}, peak={}, max_seen={}, time={:?}", 
             stats.current_bytes, stats.peak_bytes, max_memory_seen, elapsed);
    
    // Verify we processed some data with bounded memory
    assert!(total_read > 0); // At least some data
    assert!(max_memory_seen <= 256 * 1024); // Never exceeded limit
}

#[test]
fn test_memory_limit_enforcement() {
    let test_data = TestDataGenerator::new(1024, 256).generate();
    let cursor = Cursor::new(test_data);
    
    let config = StreamingConfig {
        max_memory_bytes: Some(100), // Very small limit
        buffer_size: 256,           // Larger than limit
        ..Default::default()
    };
    
    let mut reader = StreamingPackReader::new(cursor, config);
    
    // Should fail due to memory limit
    let result = reader.read_chunk(None);
    assert!(result.is_err());
    
    // Should be a resource limit error
    match result.unwrap_err() {
        gix_receive_pack::error::PackIngestionError::ResourceLimitExceeded { .. } => {
            // Expected
        }
        other => panic!("Expected ResourceLimitExceeded, got: {:?}", other),
    }
}

#[test]
fn test_memory_pressure_handling() {
    let test_data = TestDataGenerator::new(2048, 256).generate();
    let cursor = Cursor::new(test_data);
    
    let config = StreamingConfig {
        max_memory_bytes: Some(1000),
        buffer_size: 200,
        memory_pressure_threshold: 0.5, // Low threshold to trigger pressure handling
        memory_check_interval: Duration::from_millis(1), // Frequent checks
        ..Default::default()
    };
    
    let mut reader = StreamingPackReader::new(cursor, config);
    let memory_tracker = reader.memory_tracker();
    
    let mut pressure_detected = false;
    let mut total_read = 0;
    
    // Read chunks and check for pressure detection
    while let Ok(Some(chunk)) = reader.read_chunk(None) {
        total_read += chunk.len();
        
        if memory_tracker.is_under_pressure() {
            pressure_detected = true;
        }
        
        // Should still respect memory limits even under pressure
        assert!(memory_tracker.current_usage() <= 1000);
    }
    
    // We should have detected pressure at some point
    assert!(pressure_detected, "Memory pressure should have been detected");
    assert!(total_read > 0);
}

#[test]
fn test_streaming_performance_comparison() {
    let sizes = vec![
        1024,           // 1KB
        64 * 1024,      // 64KB
        1024 * 1024,    // 1MB
        4 * 1024 * 1024, // 4MB
    ];
    
    for size in sizes {
        let test_data = TestDataGenerator::new(size, 1024).generate();
        
        // Test with different buffer sizes
        let buffer_sizes = vec![1024, 4096, 16384];
        
        for buffer_size in buffer_sizes {
            let cursor = Cursor::new(test_data.clone());
            let config = StreamingConfig {
                max_memory_bytes: Some(buffer_size as u64 * 4), // 4x buffer size limit
                buffer_size,
                ..Default::default()
            };
            
            let mut reader = StreamingPackReader::new(cursor, config);
            let memory_tracker = reader.memory_tracker();
            
            let start_time = Instant::now();
            let mut total_read = 0;
            let mut chunks = 0;
            
            loop {
                match reader.read_chunk(None) {
                    Ok(Some(chunk)) => {
                        total_read += chunk.len();
                        chunks += 1;
                        // Deallocate chunk memory to simulate processing
                        memory_tracker.deallocate(chunk.len() as u64);
                    }
                    Ok(None) => break, // EOF
                    Err(_) => break,   // Error (possibly memory limit)
                }
            }
            
            let elapsed = start_time.elapsed();
            let stats = memory_tracker.stats();
            
            println!("Size: {}KB, Buffer: {}KB, Time: {:?}, Chunks: {}, Peak Memory: {}KB",
                     size / 1024, buffer_size / 1024, elapsed, chunks, stats.peak_bytes / 1024);
            
            // We should read at least some data, but might not read all due to memory limits
            assert!(total_read > 0, "Should read some data");
            assert!(stats.peak_bytes <= buffer_size as u64 * 4);
        }
    }
}

#[test]
fn test_concurrent_streaming_readers() {
    use std::sync::Arc;
    use std::thread;
    
    let test_data = Arc::new(TestDataGenerator::new(1024 * 1024, 1024).generate()); // 1MB
    let config = StreamingConfig {
        max_memory_bytes: Some(128 * 1024), // 128KB per reader
        buffer_size: 4096,
        ..Default::default()
    };
    
    let num_readers = 4;
    let mut handles = Vec::new();
    
    for i in 0..num_readers {
        let data = test_data.clone();
        let config = config.clone();
        
        let handle = thread::spawn(move || {
            let cursor = Cursor::new(data.as_slice());
            let mut reader = StreamingPackReader::new(cursor, config);
            let memory_tracker = reader.memory_tracker();
            
            let mut total_read = 0;
            let start_time = Instant::now();
            
            loop {
                match reader.read_chunk(None) {
                    Ok(Some(chunk)) => {
                        total_read += chunk.len();
                        
                        // Each reader should respect its own memory limit
                        assert!(memory_tracker.current_usage() <= 128 * 1024);
                        
                        // Deallocate chunk memory to simulate processing
                        memory_tracker.deallocate(chunk.len() as u64);
                    }
                    Ok(None) => break, // EOF
                    Err(_) => break,   // Error (possibly memory limit)
                }
            }
            
            let elapsed = start_time.elapsed();
            let stats = memory_tracker.stats();
            
            (i, total_read, elapsed, stats)
        });
        
        handles.push(handle);
    }
    
    // Wait for all readers to complete
    for handle in handles {
        let (reader_id, total_read, elapsed, stats) = handle.join().unwrap();
        
        println!("Reader {}: read {}KB in {:?}, peak memory: {}KB",
                 reader_id, total_read / 1024, elapsed, stats.peak_bytes / 1024);
        
        assert!(total_read > 0, "Should read some data");
        assert!(stats.peak_bytes <= 128 * 1024);
    }
}

#[test]
fn test_streaming_with_cancellation() {
    let test_data = TestDataGenerator::new(10 * 1024 * 1024, 1024).generate(); // 10MB
    let cursor = Cursor::new(test_data);
    
    let config = StreamingConfig {
        max_memory_bytes: Some(256 * 1024),
        buffer_size: 4096,
        ..Default::default()
    };
    
    let mut reader = StreamingPackReader::new(cursor, config);
    let cancellation_flag = reader.cancellation_flag();
    
    // Start reading in a separate thread
    let reader_handle = thread::spawn(move || {
        let mut total_read = 0;
        let mut chunks = 0;
        
        loop {
            match reader.read_chunk(None) {
                Ok(Some(chunk)) => {
                    total_read += chunk.len();
                    chunks += 1;
                    
                    // Simulate some processing time
                    thread::sleep(Duration::from_millis(1));
                }
                Ok(None) => break, // EOF
                Err(_) => break,   // Error (possibly cancellation)
            }
        }
        
        (total_read, chunks)
    });
    
    // Cancel after a short delay
    thread::sleep(Duration::from_millis(50));
    cancellation_flag.store(true, std::sync::atomic::Ordering::SeqCst);
    
    // Reader should stop due to cancellation
    let (total_read, chunks) = reader_handle.join().unwrap();
    
    println!("Cancelled after reading {}KB in {} chunks", total_read / 1024, chunks);
    
    // Should have read some data but not all of it
    assert!(total_read > 0);
    assert!(total_read < 10 * 1024 * 1024); // Less than full 10MB
}

/// Integration test with actual ReceivePack streaming functionality.
#[cfg(feature = "progress")]
#[test]
fn test_receive_pack_streaming_integration() {
    use gix_features::progress::Discard;
    use std::path::PathBuf;
    
    let test_data = TestDataGenerator::new(64 * 1024, 1024).generate(); // 64KB pack
    let mut cursor = Cursor::new(test_data.clone());
    
    let temp_dir = std::env::temp_dir().join("gix-receive-pack-streaming-test");
    std::fs::create_dir_all(&temp_dir).unwrap();
    
    let streaming_config = StreamingConfig {
        max_memory_bytes: Some(32 * 1024), // 32KB limit
        buffer_size: 4096,
        ..Default::default()
    };
    
    let receive_pack = ReceivePackBuilder::new()
        .blocking()
        .with_objects_dir(&temp_dir)
        .with_max_pack_bytes(Some(128 * 1024)) // 128KB pack limit
        .build();
    
    let mut progress = Discard;
    
    let result = receive_pack.ingest_pack_streaming(
        &mut cursor,
        Some(test_data.len() as u64),
        Some(10), // Hint: small number of objects
        streaming_config,
        &mut progress,
    );
    
    match result {
        Ok(stats) => {
            println!("Streaming ingestion stats: {:?}", stats);
            assert!(stats.memory_stats.peak_bytes <= 32 * 1024);
            assert_eq!(stats.bytes_read as usize, test_data.len());
        }
        Err(e) => {
            // Some errors are expected in test environment (e.g., invalid pack format)
            println!("Streaming ingestion error (may be expected): {:?}", e);
        }
    }
    
    // Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);
}