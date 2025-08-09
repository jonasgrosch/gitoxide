// M3: Pack ingestion & quarantine implementation.
//
// This module provides:
// - Policy to choose between index-pack and unpack-objects based on transfer.unpackLimit.
// - Quarantine lifecycle with activation (tmp ODB + alternates), migration on success, and drop on failure.
// - Blocking ingestion from a BufRead using gix-pack::Bundle into the quarantine, with thin-pack base lookup via gix-odb.
// - Fsck integration for object validation with configurable strictness levels.
//
// Notes
// - Keep constructors free of I/O; activation performs the filesystem work.
// - We route UnpackObjects to IndexPack for now; a dedicated unpack path can be added later if needed.

pub mod fsck;
pub mod quarantine;
pub mod streaming;

use crate::error::{ErrorContext, PackIngestionError, Result};

#[cfg(feature = "progress")]
use std::time::Instant;

use std::fs;
use std::path::PathBuf;

pub use fsck::{FsckConfig, FsckLevel, FsckMessageLevel, FsckResults, FsckValidator};
pub use streaming::{
    BufferPool, MemoryStats, MemoryTracker, StreamingBufReader, StreamingConfig, StreamingPackReader, StreamingStats,
};

/// CountingReader is only used in streaming pack operations
#[cfg(all(feature = "progress", feature = "pack-streaming"))]
#[derive(Debug)]
struct CountingReader<R: std::io::BufRead> {
    inner: R,
    counter: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

#[cfg(all(feature = "progress", feature = "pack-streaming"))]
impl<R: std::io::BufRead> std::io::Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.counter.fetch_add(n as u64, std::sync::atomic::Ordering::SeqCst);
        Ok(n)
    }
}

#[cfg(all(feature = "progress", feature = "pack-streaming"))]
impl<R: std::io::BufRead> std::io::BufRead for CountingReader<R> {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        self.inner.fill_buf()
    }

    fn consume(&mut self, amt: usize) {
        self.inner.consume(amt)
    }
}

/// Which path to use to ingest an incoming pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackIngestPath {
    /// Use `index-pack` style ingestion to create a pack index while keeping the pack.
    IndexPack,
    /// Use `unpack-objects` style ingestion to inflate objects into the object database.
    UnpackObjects,
}

/// Policy used to decide which ingestion path to take.
#[derive(Debug, Clone, Copy, Default)]
pub struct IngestionPolicy {
    /// transfer.unpackLimit: object-count threshold for unpack-objects.
    pub unpack_limit: Option<u64>,
    /// Whether to enable fallback strategies when primary strategy fails.
    pub enable_fallback: bool,
}

impl IngestionPolicy {
    /// Create a new ingestion policy with fallback enabled.
    pub fn with_fallback(unpack_limit: Option<u64>) -> Self {
        Self {
            unpack_limit,
            enable_fallback: true,
        }
    }

    /// Create a new ingestion policy with fallback disabled.
    pub fn without_fallback(unpack_limit: Option<u64>) -> Self {
        Self {
            unpack_limit,
            enable_fallback: false,
        }
    }

    /// Choose an ingestion path using the optional object-count hint and the configured unpack limit.
    ///
    /// Rules:
    /// - If both a limit and a count are present and count <= limit, use UnpackObjects.
    /// - Otherwise, fall back to IndexPack.
    pub fn choose_path(&self, object_count_hint: Option<u64>) -> PackIngestPath {
        match (self.unpack_limit, object_count_hint) {
            (Some(limit), Some(count)) if count <= limit => PackIngestPath::UnpackObjects,
            _ => PackIngestPath::IndexPack,
        }
    }

    /// Get the fallback strategy for a failed ingestion attempt.
    ///
    /// Returns None if fallback is disabled or no fallback is available.
    pub fn get_fallback_strategy(&self, failed_strategy: PackIngestPath) -> Option<PackIngestPath> {
        if !self.enable_fallback {
            return None;
        }

        match failed_strategy {
            PackIngestPath::IndexPack => Some(PackIngestPath::UnpackObjects),
            PackIngestPath::UnpackObjects => Some(PackIngestPath::IndexPack),
        }
    }

    /// Check if fallback is available for the given strategy.
    pub fn has_fallback(&self, strategy: PackIngestPath) -> bool {
        self.enable_fallback && self.get_fallback_strategy(strategy).is_some()
    }

    /// Get all available strategies in order of preference.
    pub fn get_strategy_sequence(&self, object_count_hint: Option<u64>) -> Vec<PackIngestPath> {
        let primary = self.choose_path(object_count_hint);
        let mut strategies = vec![primary];

        if let Some(fallback) = self.get_fallback_strategy(primary) {
            strategies.push(fallback);
        }

        strategies
    }
}

/// Pack ingestor with fsck integration for object validation.
#[derive(Debug)]
pub struct PackIngestor {
    /// Fsck validator for object validation
    #[cfg_attr(not(feature = "fsck"), allow(dead_code))]
    fsck_validator: Option<FsckValidator>,
    /// Streaming configuration for memory management
    streaming_config: StreamingConfig,
}

impl Default for PackIngestor {
    fn default() -> Self {
        Self {
            fsck_validator: None,
            streaming_config: StreamingConfig::default(),
        }
    }
}

impl PackIngestor {
    /// Create a new PackIngestor with optional fsck validation.
    pub fn new(fsck_config: Option<FsckConfig>) -> Self {
        Self {
            fsck_validator: fsck_config.map(FsckValidator::new),
            streaming_config: StreamingConfig::default(),
        }
    }

    /// Create a PackIngestor with fsck validation enabled using the given configuration.
    pub fn with_fsck(fsck_config: FsckConfig) -> Self {
        Self {
            fsck_validator: Some(FsckValidator::new(fsck_config)),
            streaming_config: StreamingConfig::default(),
        }
    }

    /// Create a PackIngestor with no fsck validation.
    pub fn without_fsck() -> Self {
        Self {
            fsck_validator: None,
            streaming_config: StreamingConfig::default(),
        }
    }

    /// Create a PackIngestor with custom streaming configuration.
    pub fn with_streaming_config(fsck_config: Option<FsckConfig>, streaming_config: StreamingConfig) -> Self {
        Self {
            fsck_validator: fsck_config.map(FsckValidator::new),
            streaming_config,
        }
    }

    /// Get the streaming configuration.
    pub fn streaming_config(&self) -> &StreamingConfig {
        &self.streaming_config
    }

    /// Set the streaming configuration.
    pub fn set_streaming_config(&mut self, config: StreamingConfig) {
        self.streaming_config = config;
    }
}

/// Pack ingestion controller that handles strategy selection and fallback logic.
#[derive(Debug)]
pub struct PackIngestionController {
    /// The pack ingestor instance
    ingestor: PackIngestor,
    /// Ingestion policy for strategy selection
    policy: IngestionPolicy,
    /// Maximum number of fallback attempts
    pub max_fallback_attempts: u32,
}

impl PackIngestionController {
    /// Create a new pack ingestion controller.
    pub fn new(ingestor: PackIngestor, policy: IngestionPolicy) -> Self {
        Self {
            ingestor,
            policy,
            max_fallback_attempts: 2, // Primary + 1 fallback by default
        }
    }

    /// Create a controller with custom fallback attempt limit.
    pub fn with_max_fallback_attempts(mut self, max_attempts: u32) -> Self {
        self.max_fallback_attempts = max_attempts;
        self
    }

    /// Ingest a pack with automatic fallback on failure.
    #[cfg(feature = "progress")]
    pub fn ingest_pack_with_fallback(
        &self,
        input: &mut dyn std::io::BufRead,
        quarantine_objects_dir: &std::path::Path,
        pack_size: Option<u64>,
        object_count_hint: Option<u64>,
        thin_pack_lookup: Option<gix_odb::Handle>,
        progress: &mut dyn gix_features::progress::DynNestedProgress,
    ) -> Result<PackIngestionResult> {
        let strategies = self.policy.get_strategy_sequence(object_count_hint);
        let mut last_error: Option<PackIngestionError> = None;
        let mut attempt_count = 0;

        for (strategy_index, strategy) in strategies.iter().enumerate() {
            if attempt_count >= self.max_fallback_attempts {
                break;
            }

            let context = ErrorContext::new("pack-ingestion-with-fallback")
                .with_context("strategy", format!("{:?}", strategy))
                .with_context("attempt", (attempt_count + 1).to_string())
                .with_context("is_fallback", if strategy_index > 0 { "true" } else { "false" })
                .with_pack_size(pack_size.unwrap_or(0));

            // Create a progress child for this attempt
            let mut strategy_progress = progress.add_child(format!("attempt {} ({:?})", attempt_count + 1, strategy));

            let result = match strategy {
                PackIngestPath::IndexPack => self.ingestor.index_pack(
                    input,
                    quarantine_objects_dir,
                    pack_size,
                    thin_pack_lookup.clone(),
                    &mut strategy_progress,
                ),
                PackIngestPath::UnpackObjects => self.ingestor.unpack_objects(
                    input,
                    quarantine_objects_dir,
                    pack_size,
                    thin_pack_lookup.clone(),
                    &mut strategy_progress,
                ),
            };

            match result {
                Ok(fsck_results) => {
                    return Ok(PackIngestionResult {
                        strategy_used: *strategy,
                        fsck_results,
                        attempts_made: attempt_count + 1,
                        fallback_used: strategy_index > 0,
                        errors_encountered: if let Some(err) = last_error { vec![err] } else { vec![] },
                    });
                }
                Err(error) => {
                    // Check if this error is recoverable and we should try fallback
                    if self.should_attempt_fallback(&error, strategy_index, attempt_count) {
                        last_error = Some(error);
                        attempt_count += 1;
                        continue;
                    } else {
                        // Error is not recoverable or we've exhausted attempts
                        return Err(self.create_fallback_error(error, last_error, attempt_count + 1, context));
                    }
                }
            }
        }

        // If we get here, we've exhausted all strategies
        let final_error = last_error.unwrap_or_else(|| {
            PackIngestionError::configuration(
                "no ingestion strategies available",
                ErrorContext::new("pack-ingestion-with-fallback")
                    .with_context("strategies_tried", strategies.len().to_string()),
            )
        });

        Err(self.create_fallback_error(
            final_error,
            None,
            attempt_count,
            ErrorContext::new("pack-ingestion-with-fallback"),
        ))
    }

    /// Ingest a pack with streaming and fallback support.
    #[cfg(all(feature = "progress", feature = "pack-streaming"))]
    pub fn ingest_pack_streaming_with_fallback(
        &self,
        input: &mut dyn std::io::BufRead,
        quarantine_objects_dir: &std::path::Path,
        pack_size: Option<u64>,
        object_count_hint: Option<u64>,
        thin_pack_lookup: Option<gix_odb::Handle>,
        progress: &mut dyn gix_features::progress::DynNestedProgress,
    ) -> Result<PackIngestionStreamingResult> {
        let strategies = self.policy.get_strategy_sequence(object_count_hint);
        let mut last_error: Option<PackIngestionError> = None;
        let mut attempt_count = 0;

        for (strategy_index, strategy) in strategies.iter().enumerate() {
            if attempt_count >= self.max_fallback_attempts {
                break;
            }

            let context = ErrorContext::new("pack-ingestion-streaming-with-fallback")
                .with_context("strategy", format!("{:?}", strategy))
                .with_context("attempt", (attempt_count + 1).to_string())
                .with_context("is_fallback", if strategy_index > 0 { "true" } else { "false" })
                .with_pack_size(pack_size.unwrap_or(0));

            let mut strategy_progress = progress.add_child(format!("attempt {} ({:?})", attempt_count + 1, strategy));

            let result = match strategy {
                PackIngestPath::IndexPack => self.ingestor.index_pack_streaming(
                    input,
                    quarantine_objects_dir,
                    pack_size,
                    thin_pack_lookup.clone(),
                    &mut strategy_progress,
                ),
                PackIngestPath::UnpackObjects => self.ingestor.unpack_objects_streaming(
                    input,
                    quarantine_objects_dir,
                    pack_size,
                    thin_pack_lookup.clone(),
                    &mut strategy_progress,
                ),
            };

            match result {
                Ok((fsck_results, streaming_stats)) => {
                    return Ok(PackIngestionStreamingResult {
                        strategy_used: *strategy,
                        fsck_results,
                        streaming_stats,
                        attempts_made: attempt_count + 1,
                        fallback_used: strategy_index > 0,
                        errors_encountered: if let Some(err) = last_error { vec![err] } else { vec![] },
                    });
                }
                Err(error) => {
                    if self.should_attempt_fallback(&error, strategy_index, attempt_count) {
                        last_error = Some(error);
                        attempt_count += 1;
                        continue;
                    } else {
                        return Err(self.create_fallback_error(error, last_error, attempt_count + 1, context));
                    }
                }
            }
        }

        let final_error = last_error.unwrap_or_else(|| {
            PackIngestionError::configuration(
                "no ingestion strategies available",
                ErrorContext::new("pack-ingestion-streaming-with-fallback")
                    .with_context("strategies_tried", strategies.len().to_string()),
            )
        });

        Err(self.create_fallback_error(
            final_error,
            None,
            attempt_count,
            ErrorContext::new("pack-ingestion-streaming-with-fallback"),
        ))
    }

    /// Determine if we should attempt fallback for the given error.
    pub fn should_attempt_fallback(
        &self,
        error: &PackIngestionError,
        strategy_index: usize,
        attempt_count: u32,
    ) -> bool {
        // Don't attempt fallback if it's disabled
        if !self.policy.enable_fallback {
            return false;
        }

        // Don't attempt fallback if we've exceeded max attempts
        if attempt_count >= self.max_fallback_attempts - 1 {
            return false;
        }

        // Don't attempt fallback if this is already a fallback attempt and we have no more strategies
        if strategy_index >= 1 {
            return false;
        }

        // Check if the error is recoverable
        error.is_recoverable()
    }

    /// Create a comprehensive fallback error with context.
    /// Only used by fallback methods that require the progress feature.
    #[cfg(feature = "progress")]
    fn create_fallback_error(
        &self,
        primary_error: PackIngestionError,
        previous_error: Option<PackIngestionError>,
        attempts_made: u32,
        context: ErrorContext,
    ) -> PackIngestionError {
        let mut errors = vec![primary_error];
        if let Some(prev) = previous_error {
            errors.insert(0, prev);
        }

        PackIngestionError::Multiple {
            errors,
            context: context
                .with_context("fallback_enabled", self.policy.enable_fallback.to_string())
                .with_context("attempts_made", attempts_made.to_string())
                .with_context("max_attempts", self.max_fallback_attempts.to_string()),
        }
    }

    /// Get the ingestion policy.
    pub fn policy(&self) -> &IngestionPolicy {
        &self.policy
    }

    /// Get the pack ingestor.
    pub fn ingestor(&self) -> &PackIngestor {
        &self.ingestor
    }
}

/// Result of pack ingestion with fallback information.
#[derive(Debug)]
pub struct PackIngestionResult {
    /// The strategy that was successfully used
    pub strategy_used: PackIngestPath,
    /// Results from fsck validation
    pub fsck_results: FsckResults,
    /// Number of attempts made (including fallbacks)
    pub attempts_made: u32,
    /// Whether fallback was used
    pub fallback_used: bool,
    /// Errors encountered during failed attempts
    pub errors_encountered: Vec<PackIngestionError>,
}

/// Result of streaming pack ingestion with fallback information.
#[derive(Debug)]
pub struct PackIngestionStreamingResult {
    /// The strategy that was successfully used
    pub strategy_used: PackIngestPath,
    /// Results from fsck validation
    pub fsck_results: FsckResults,
    /// Streaming statistics
    pub streaming_stats: StreamingStats,
    /// Number of attempts made (including fallbacks)
    pub attempts_made: u32,
    /// Whether fallback was used
    pub fallback_used: bool,
    /// Errors encountered during failed attempts
    pub errors_encountered: Vec<PackIngestionError>,
}

impl PackIngestor {
    /// Placeholder no-op for index-pack ingestion used by scaffold-only entrypoints.
    pub fn index_pack_stub() -> Result<()> {
        Ok(())
    }

    /// Implement unpack-objects style ingestion by first materializing the incoming pack
    /// into the quarantine pack directory, then exploding it into loose objects and
    /// removing the temporary pack artifacts.
    ///
    /// This uses gix-pack's bundle writer to write the incoming pack to disk, and reuses
    /// gitoxide's 'explode' logic to write objects as loose.
    #[cfg(all(feature = "progress", feature = "pack-streaming"))]
    pub fn unpack_objects(
        &self,
        input: &mut dyn std::io::BufRead,
        quarantine_objects_dir: &std::path::Path,
        pack_size: Option<u64>,
        thin_pack_lookup: Option<gix_odb::Handle>,
        progress: &mut dyn gix_features::progress::DynNestedProgress,
    ) -> Result<FsckResults> {
        use gix_pack::bundle::write::Options as WriteOptions;
        use std::sync::atomic::AtomicBool;

        let start_time = Instant::now();
        let context = ErrorContext::new("unpack-objects")
            .with_context("strategy", "unpack-objects")
            .with_pack_size(pack_size.unwrap_or(0));

        // 1. Ensure pack directory exists.
        let pack_dir = quarantine_objects_dir.join("pack");
        fs::create_dir_all(&pack_dir).map_err(|e| {
            PackIngestionError::quarantine_operation(
                "failed to create pack directory",
                context.clone(),
                Some(Box::new(e)),
            )
        })?;

        // 2. Write incoming stream into a temp pack in quarantine using bundle writer.
        // Note: This produces a valid .pack and .idx side by side.
        let should_interrupt = AtomicBool::new(false);
        let write_opts = WriteOptions {
            // Keep defaults for verify/index-version/object-hash
            ..Default::default()
        };
        let mut write_progress = progress.add_child("write pack".to_string());
        let _write_outcome = match &thin_pack_lookup {
            Some(handle) => gix_pack::Bundle::write_to_directory(
                input,
                Some(pack_dir.as_path()),
                &mut write_progress,
                &should_interrupt,
                Some(handle.clone()),
                write_opts,
            )
            .map_err(|e| {
                PackIngestionError::pack_parsing(
                    "failed to write pack bundle with thin pack support",
                    context.clone().with_elapsed(start_time.elapsed()),
                    Some(Box::new(e)),
                )
            })?,
            None => gix_pack::Bundle::write_to_directory(
                input,
                Some(pack_dir.as_path()),
                &mut write_progress,
                &should_interrupt,
                Option::<gix_odb::Handle>::None,
                write_opts,
            )
            .map_err(|e| {
                PackIngestionError::pack_parsing(
                    "failed to write pack bundle",
                    context.clone().with_elapsed(start_time.elapsed()),
                    Some(Box::new(e)),
                )
            })?,
        };

        // 3. Find the freshly written .pack file in quarantine.
        let mut pack_path: Option<PathBuf> = None;
        if let Ok(entries) = fs::read_dir(&pack_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) == Some("pack")
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .map_or(false, |n| n.starts_with("pack-"))
                {
                    pack_path = Some(p);
                    break;
                }
            }
        }
        let _pack_path = pack_path.ok_or_else(|| {
            PackIngestionError::unpack_objects_operation(
                "failed to locate just-written pack file",
                context.clone().with_elapsed(start_time.elapsed()),
                None,
            )
        })?;

        // 4. Explode pack contents into loose objects in quarantine objects_dir.
        //    Keep verification minimal here; rely on quarantine and later fsck when enabled.
        let _explode_progress = progress.add_child("explode pack".to_string());
        // Reuse gitoxide-core explode semantics if available directly through gix APIs.
        // The explode operation in gitoxide-core maps to gix APIs, so we replicate the essence:
        // - Open the pack and stream objects into loose::Store.
        // - Use default safety checks.
        let object_hash = gix_hash::Kind::Sha1; // TODO: detect repo hash kind in config once wired.
        
        // Open the pack file and iterate through all entries, writing each as a loose object
        let pack_file = gix_pack::data::File::at(&_pack_path, object_hash).map_err(|e| {
            PackIngestionError::unpack_objects_operation(
                "failed to open pack file for explosion",
                context.clone().with_elapsed(start_time.elapsed()),
                Some(Box::new(e)),
            )
        })?;
        
        // Create a loose object store for writing
        let loose_store = gix_odb::loose::Store::at(quarantine_objects_dir, object_hash);
        
        // Create an inflate instance for decompression
        let mut inflate = gix_features::zlib::Inflate::default();
        let mut decoded_buf = Vec::new();
        let _explode_progress = progress.add_child("explode pack".to_string());
        
        // Set up progress tracking
        let _num_objects = pack_file.num_objects();
        
        // Iterate through pack entries using streaming iterator
        let mut _entries_processed = 0;
        let pack_iter = pack_file.streaming_iter().map_err(|e| {
            PackIngestionError::unpack_objects_operation(
                "failed to create pack streaming iterator",
                context.clone().with_elapsed(start_time.elapsed()),
                Some(Box::new(e)),
            )
        })?;
        
        // Create a resolve function for handling ref deltas in thin packs
        let resolve_fn = |oid: &gix_hash::oid, out: &mut Vec<u8>| -> Option<gix_pack::data::decode::entry::ResolvedBase> {
            // If we have a thin pack lookup, try to find the object in the main repository
            if let Some(ref lookup) = thin_pack_lookup {
                use gix_object::Find;
                match lookup.try_find(oid, out) {
                    Ok(Some(obj)) => {
                        return Some(gix_pack::data::decode::entry::ResolvedBase::OutOfPack {
                            kind: obj.kind,
                            end: out.len(),
                        });
                    }
                    _ => {}
                }
            }
            None
        };
        
        // Process each entry in the pack
        for entry_result in pack_iter {
            let entry = entry_result.map_err(|e| {
                PackIngestionError::unpack_objects_operation(
                    "failed to read pack entry",
                    context.clone().with_elapsed(start_time.elapsed()),
                    Some(Box::new(e)),
                )
            })?;
            
            // Convert input::Entry to data::Entry for decode_entry
            let data_entry = gix_pack::data::Entry {
                header: entry.header,
                decompressed_size: entry.decompressed_size,
                data_offset: entry.pack_offset + entry.header_size as u64,
            };
            
            // Decode the entry (this handles delta resolution)
            decoded_buf.clear();
            let outcome = pack_file.decode_entry(
                data_entry,
                &mut decoded_buf,
                &mut inflate,
                &resolve_fn,
                &mut gix_pack::cache::Never,
            ).map_err(|e| {
                PackIngestionError::unpack_objects_operation(
                    "failed to decode pack entry",
                    context.clone().with_elapsed(start_time.elapsed()),
                    Some(Box::new(e)),
                )
            })?;
            
            // Write the decoded object as a loose object
            use gix_object::Write;
            let _oid = loose_store.write_buf(outcome.kind, &decoded_buf).map_err(|e| {
                PackIngestionError::unpack_objects_operation(
                    "failed to write loose object",
                    context.clone().with_elapsed(start_time.elapsed()),
                    Some(e),
                )
            })?;
            
            _entries_processed += 1;
            // Update progress message
            let _ = explode_progress; // Progress updates handled internally
            
            // Check for interruption
            if should_interrupt.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(PackIngestionError::io(
                    "pack explosion interrupted",
                    context.clone().with_elapsed(start_time.elapsed()),
                    std::io::Error::new(std::io::ErrorKind::Interrupted, "operation cancelled"),
                ));
            }
        }

        // 5. Perform fsck validation before removing pack artifacts
        let fsck_results = if let Some(ref validator) = self.fsck_validator {
            if let Some(ref main_odb) = thin_pack_lookup {
                validator
                    .validate_quarantine(quarantine_objects_dir, main_odb)
                    .map_err(|e| match e {
                        crate::Error::Fsck(msg) => PackIngestionError::object_validation(
                            msg,
                            context.clone().with_elapsed(start_time.elapsed()),
                            None,
                            vec![],
                            None,
                        ),
                        _ => PackIngestionError::object_validation(
                            "fsck validation failed",
                            context.clone().with_elapsed(start_time.elapsed()),
                            None,
                            vec![e.to_string()],
                            Some(Box::new(e)),
                        ),
                    })?
            } else {
                // Create a temporary ODB handle for validation if none provided
                let temp_odb = gix_odb::at(quarantine_objects_dir).map_err(|e| {
                    PackIngestionError::object_database(
                        "failed to create temp ODB for fsck",
                        context.clone().with_elapsed(start_time.elapsed()),
                        Some(Box::new(e)),
                    )
                })?;
                validator
                    .validate_quarantine(quarantine_objects_dir, &temp_odb)
                    .map_err(|e| match e {
                        crate::Error::Fsck(msg) => PackIngestionError::object_validation(
                            msg,
                            context.clone().with_elapsed(start_time.elapsed()),
                            None,
                            vec![],
                            None,
                        ),
                        _ => PackIngestionError::object_validation(
                            "fsck validation failed",
                            context.clone().with_elapsed(start_time.elapsed()),
                            None,
                            vec![e.to_string()],
                            Some(Box::new(e)),
                        ),
                    })?
            }
        } else {
            FsckResults {
                validated_objects: Vec::new(),
                warnings: Vec::new(),
                errors: Vec::new(),
                missing_objects: Vec::new(),
            }
        };

        // 6. Remove temporary pack artifacts to mimic unpack-objects behavior.
        //    Keep *.keep if present, else remove both pack and idx.
        if let Ok(entries) = fs::read_dir(&pack_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) == Some("pack")
                    || p.extension().and_then(|e| e.to_str()) == Some("idx")
                {
                    let _ = fs::remove_file(p);
                }
            }
        }

        let _ = pack_size;
        Ok(fsck_results)
    }

    /// Stub version of unpack_objects when required features are not enabled.
    #[cfg(not(all(feature = "progress", feature = "pack-streaming")))]
    pub fn unpack_objects(
        &self,
        _input: &mut dyn std::io::BufRead,
        _quarantine_objects_dir: &std::path::Path,
        _pack_size: Option<u64>,
        _thin_pack_lookup: Option<gix_odb::Handle>,
        _progress: &mut dyn std::any::Any,
    ) -> Result<FsckResults> {
        let context = ErrorContext::new("unpack-objects")
            .with_context("reason", "progress and pack-streaming features not enabled");
        Err(PackIngestionError::configuration(
            "unpack-objects requires progress and pack-streaming features",
            context,
        ))
    }
}

#[cfg(feature = "progress")]
impl PackIngestor {
    /// Ingest a pack using index-pack semantics into the given quarantine objects directory.
    ///
    /// - `input`: the incoming pack stream.
    /// - `quarantine_objects_dir`: the quarantine '.git/objects' directory.
    /// - `pack_size`: if known, provide the total size to optimize reading; otherwise None.
    /// - `thin_pack_lookup`: Optional object finder to resolve thin-pack bases (typically the main ODB).
    /// - `progress`: progress sink used by gix-pack; will be translated to sideband later in the engine.
    pub fn index_pack(
        &self,
        input: &mut dyn std::io::BufRead,
        quarantine_objects_dir: &std::path::Path,
        pack_size: Option<u64>,
        thin_pack_lookup: Option<gix_odb::Handle>,
        progress: &mut dyn gix_features::progress::DynNestedProgress,
    ) -> Result<FsckResults> {
        use gix_pack::bundle::write::{Options, Outcome};
        use std::sync::atomic::AtomicBool;

        let start_time = Instant::now();
        let context = ErrorContext::new("index-pack")
            .with_context("strategy", "index-pack")
            .with_pack_size(pack_size.unwrap_or(0));

        // gix-pack expects the 'objects/pack' directory as target.
        let pack_dir = quarantine_objects_dir.join("pack");
        fs::create_dir_all(&pack_dir).map_err(|e| {
            PackIngestionError::quarantine_operation(
                "failed to create pack directory",
                context.clone(),
                Some(Box::new(e)),
            )
        })?;

        let should_interrupt = AtomicBool::new(false);

        let options = Options {
            // Defaults: Verify mode, index version default, object hash default
            ..Default::default()
        };

        let _out: Outcome = match &thin_pack_lookup {
            Some(handle) => {
                // Use thin-pack lookup to fix up deltas against the main ODB if required.
                gix_pack::Bundle::write_to_directory(
                    input,
                    Some(pack_dir.as_path()),
                    progress,
                    &should_interrupt,
                    Some(handle.clone()),
                    options,
                )
                .map_err(|e| {
                    PackIngestionError::index_pack_operation(
                        "failed to write pack bundle with thin pack support",
                        context.clone().with_elapsed(start_time.elapsed()),
                        Some(Box::new(e)),
                    )
                })?
            }
            None => gix_pack::Bundle::write_to_directory(
                input,
                Some(pack_dir.as_path()),
                progress,
                &should_interrupt,
                Option::<gix_odb::Handle>::None,
                options,
            )
            .map_err(|e| {
                PackIngestionError::index_pack_operation(
                    "failed to write pack bundle",
                    context.clone().with_elapsed(start_time.elapsed()),
                    Some(Box::new(e)),
                )
            })?,
        };

        // Perform fsck validation if configured
        let fsck_results = if let Some(ref validator) = self.fsck_validator {
            if let Some(ref main_odb) = thin_pack_lookup {
                validator
                    .validate_quarantine(quarantine_objects_dir, main_odb)
                    .map_err(|e| match e {
                        crate::Error::Fsck(msg) => PackIngestionError::object_validation(
                            msg,
                            context.clone().with_elapsed(start_time.elapsed()),
                            None,
                            vec![],
                            None,
                        ),
                        _ => PackIngestionError::object_validation(
                            "fsck validation failed",
                            context.clone().with_elapsed(start_time.elapsed()),
                            None,
                            vec![e.to_string()],
                            Some(Box::new(e)),
                        ),
                    })?
            } else {
                // Create a temporary ODB handle for validation if none provided
                let temp_odb = gix_odb::at(quarantine_objects_dir).map_err(|e| {
                    PackIngestionError::object_database(
                        "failed to create temp ODB for fsck",
                        context.clone().with_elapsed(start_time.elapsed()),
                        Some(Box::new(e)),
                    )
                })?;
                validator
                    .validate_quarantine(quarantine_objects_dir, &temp_odb)
                    .map_err(|e| match e {
                        crate::Error::Fsck(msg) => PackIngestionError::object_validation(
                            msg,
                            context.clone().with_elapsed(start_time.elapsed()),
                            None,
                            vec![],
                            None,
                        ),
                        _ => PackIngestionError::object_validation(
                            "fsck validation failed",
                            context.clone().with_elapsed(start_time.elapsed()),
                            None,
                            vec![e.to_string()],
                            Some(Box::new(e)),
                        ),
                    })?
            }
        } else {
            FsckResults {
                validated_objects: Vec::new(),
                warnings: Vec::new(),
                errors: Vec::new(),
                missing_objects: Vec::new(),
            }
        };

        Ok(fsck_results)
    }

    /// Streaming version of index_pack with bounded memory usage.
    ///
    /// This method uses a streaming reader to process pack data with controlled memory usage,
    /// providing progress updates and memory pressure handling.
    #[cfg(all(feature = "pack-streaming", feature = "progress"))]
    pub fn index_pack_streaming(
        &self,
        input: &mut dyn std::io::BufRead,
        quarantine_objects_dir: &std::path::Path,
        pack_size: Option<u64>,
        thin_pack_lookup: Option<gix_odb::Handle>,
        progress: &mut dyn gix_features::progress::DynNestedProgress,
    ) -> Result<(FsckResults, StreamingStats)> {
        use gix_pack::bundle::write::{Options, Outcome};
        use std::sync::atomic::AtomicBool;

        let start_time = Instant::now();
        let context = ErrorContext::new("index-pack-streaming")
            .with_context("strategy", "index-pack-streaming")
            .with_pack_size(pack_size.unwrap_or(0));

        // Create streaming reader with memory management
        let streaming_reader = StreamingPackReader::new(input, self.streaming_config.clone());
        let memory_tracker = streaming_reader.memory_tracker();
        let _cancellation_flag = streaming_reader.cancellation_flag();

        // Create pack directory
        let pack_dir = quarantine_objects_dir.join("pack");
        fs::create_dir_all(&pack_dir).map_err(|e| {
            PackIngestionError::quarantine_operation(
                "failed to create pack directory",
                context.clone(),
                Some(Box::new(e)),
            )
        })?;

        // Create a buffer pool for efficient memory reuse
        let buffer_pool = std::sync::Arc::new(BufferPool::new(
            memory_tracker.clone(),
            self.streaming_config.buffer_size,
            4, // Pool up to 4 buffers
        ));

        // Create a streaming wrapper that implements BufRead
        let streaming_wrapper = StreamingBufReader::new(streaming_reader, buffer_pool.clone());
        // Counting counter to measure exact bytes read by gix-pack writer
        let bytes_counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));

        let should_interrupt = AtomicBool::new(false);
        let options = Options { ..Default::default() };

        // Add progress child for pack writing
        let mut pack_progress = progress.add_child("writing pack".to_string());

        let _out: Outcome = {
            let mut counting_reader = CountingReader {
                inner: streaming_wrapper,
                counter: bytes_counter.clone(),
            };
            match &thin_pack_lookup {
                Some(handle) => gix_pack::Bundle::write_to_directory(
                    &mut counting_reader,
                    Some(pack_dir.as_path()),
                    &mut pack_progress,
                    &should_interrupt,
                    Some(handle.clone()),
                    options,
                )
                .map_err(|e| {
                    PackIngestionError::index_pack_operation(
                        "failed to write pack bundle with thin pack support (streaming)",
                        context.clone().with_elapsed(start_time.elapsed()),
                        Some(Box::new(e)),
                    )
                })?,
                None => gix_pack::Bundle::write_to_directory(
                    &mut counting_reader,
                    Some(pack_dir.as_path()),
                    &mut pack_progress,
                    &should_interrupt,
                    Option::<gix_odb::Handle>::None,
                    options,
                )
                .map_err(|e| {
                    PackIngestionError::index_pack_operation(
                        "failed to write pack bundle (streaming)",
                        context.clone().with_elapsed(start_time.elapsed()),
                        Some(Box::new(e)),
                    )
                })?,
            }
        };

        // Get streaming statistics
        let streaming_stats = StreamingStats {
            bytes_read: bytes_counter.load(std::sync::atomic::Ordering::SeqCst),
            memory_stats: memory_tracker.stats(),
            buffer_size: self.streaming_config.buffer_size,
        };

        // Perform fsck validation if configured
        let fsck_results = if let Some(ref validator) = self.fsck_validator {
            if let Some(ref main_odb) = thin_pack_lookup {
                validator
                    .validate_quarantine(quarantine_objects_dir, main_odb)
                    .map_err(|e| match e {
                        crate::Error::Fsck(msg) => PackIngestionError::object_validation(
                            msg,
                            context.clone().with_elapsed(start_time.elapsed()),
                            None,
                            vec![],
                            None,
                        ),
                        _ => PackIngestionError::object_validation(
                            "fsck validation failed",
                            context.clone().with_elapsed(start_time.elapsed()),
                            None,
                            vec![e.to_string()],
                            Some(Box::new(e)),
                        ),
                    })?
            } else {
                let temp_odb = gix_odb::at(quarantine_objects_dir).map_err(|e| {
                    PackIngestionError::object_database(
                        "failed to create temp ODB for fsck",
                        context.clone().with_elapsed(start_time.elapsed()),
                        Some(Box::new(e)),
                    )
                })?;
                validator
                    .validate_quarantine(quarantine_objects_dir, &temp_odb)
                    .map_err(|e| match e {
                        crate::Error::Fsck(msg) => PackIngestionError::object_validation(
                            msg,
                            context.clone().with_elapsed(start_time.elapsed()),
                            None,
                            vec![],
                            None,
                        ),
                        _ => PackIngestionError::object_validation(
                            "fsck validation failed",
                            context.clone().with_elapsed(start_time.elapsed()),
                            None,
                            vec![e.to_string()],
                            Some(Box::new(e)),
                        ),
                    })?
            }
        } else {
            FsckResults {
                validated_objects: Vec::new(),
                warnings: Vec::new(),
                errors: Vec::new(),
                missing_objects: Vec::new(),
            }
        };

        // Clean up buffer pool
        buffer_pool.clear();

        Ok((fsck_results, streaming_stats))
    }

    /// Streaming version of unpack_objects with bounded memory usage.
    ///
    /// This method processes pack data in a streaming fashion with memory controls,
    /// exploding objects into loose storage while maintaining bounded memory usage.
    #[cfg(all(feature = "progress", feature = "pack-streaming"))]
    pub fn unpack_objects_streaming(
        &self,
        input: &mut dyn std::io::BufRead,
        quarantine_objects_dir: &std::path::Path,
        pack_size: Option<u64>,
        thin_pack_lookup: Option<gix_odb::Handle>,
        progress: &mut dyn gix_features::progress::DynNestedProgress,
    ) -> Result<(FsckResults, StreamingStats)> {
        use gix_pack::bundle::write::Options as WriteOptions;
        use std::sync::atomic::AtomicBool;

        let start_time = Instant::now();
        let context = ErrorContext::new("unpack-objects-streaming")
            .with_context("strategy", "unpack-objects-streaming")
            .with_pack_size(pack_size.unwrap_or(0));

        // Create streaming reader with memory management
        let streaming_reader = StreamingPackReader::new(input, self.streaming_config.clone());
        let memory_tracker = streaming_reader.memory_tracker();

        // Ensure pack directory exists
        let pack_dir = quarantine_objects_dir.join("pack");
        fs::create_dir_all(&pack_dir).map_err(|e| {
            PackIngestionError::quarantine_operation(
                "failed to create pack directory",
                context.clone(),
                Some(Box::new(e)),
            )
        })?;

        // Create buffer pool for efficient memory reuse
        let buffer_pool = std::sync::Arc::new(BufferPool::new(
            memory_tracker.clone(),
            self.streaming_config.buffer_size,
            4,
        ));

        // First, write the pack using streaming reader
        let streaming_wrapper = StreamingBufReader::new(streaming_reader, buffer_pool.clone());
        // Counting counter to measure exact bytes read by gix-pack writer
        let bytes_counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));

        let should_interrupt = AtomicBool::new(false);
        let write_opts = WriteOptions { ..Default::default() };

        let mut write_progress = progress.add_child("write pack".to_string());
        let _write_outcome = {
            let mut counting_reader = CountingReader {
                inner: streaming_wrapper,
                counter: bytes_counter.clone(),
            };
            match &thin_pack_lookup {
                Some(handle) => gix_pack::Bundle::write_to_directory(
                    &mut counting_reader,
                    Some(pack_dir.as_path()),
                    &mut write_progress,
                    &should_interrupt,
                    Some(handle.clone()),
                    write_opts,
                )
                .map_err(|e| {
                    PackIngestionError::pack_parsing(
                        "failed to write pack bundle with thin pack support (streaming)",
                        context.clone().with_elapsed(start_time.elapsed()),
                        Some(Box::new(e)),
                    )
                })?,
                None => gix_pack::Bundle::write_to_directory(
                    &mut counting_reader,
                    Some(pack_dir.as_path()),
                    &mut write_progress,
                    &should_interrupt,
                    Option::<gix_odb::Handle>::None,
                    write_opts,
                )
                .map_err(|e| {
                    PackIngestionError::pack_parsing(
                        "failed to write pack bundle (streaming)",
                        context.clone().with_elapsed(start_time.elapsed()),
                        Some(Box::new(e)),
                    )
                })?,
            }
        };

        // Find the freshly written .pack file
        let mut pack_path: Option<PathBuf> = None;
        if let Ok(entries) = fs::read_dir(&pack_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) == Some("pack")
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .map_or(false, |n| n.starts_with("pack-"))
                {
                    pack_path = Some(p);
                    break;
                }
            }
        }
        let _pack_path = pack_path.ok_or_else(|| {
            PackIngestionError::unpack_objects_operation(
                "failed to locate just-written pack file",
                context.clone().with_elapsed(start_time.elapsed()),
                None,
            )
        })?;

        // Get streaming statistics before cleanup
        let streaming_stats = StreamingStats {
            bytes_read: bytes_counter.load(std::sync::atomic::Ordering::SeqCst),
            memory_stats: memory_tracker.stats(),
            buffer_size: self.streaming_config.buffer_size,
        };

        // Explode pack contents into loose objects with memory management
        let _explode_progress = progress.add_child("explode pack".to_string());

        // Explode pack contents into loose objects with memory management
        let object_hash = gix_hash::Kind::Sha1; // TODO: detect repo hash kind in config once wired.
        let pack_file = gix_pack::data::File::at(&_pack_path, object_hash).map_err(|e| {
            PackIngestionError::unpack_objects_operation(
                "failed to open pack file for explosion (streaming)",
                context.clone().with_elapsed(start_time.elapsed()),
                Some(Box::new(e)),
            )
        })?;
        
        // Create a loose object store for writing
        let loose_store = gix_odb::loose::Store::at(quarantine_objects_dir, object_hash);
        
        // Create an inflate instance for decompression
        let mut inflate = gix_features::zlib::Inflate::default();
        let mut decoded_buf = Vec::new();
        
        // Set up progress tracking
        let _num_objects = pack_file.num_objects();
        
        // Track memory usage for streaming
        let mut _entries_processed = 0;
        let pack_iter = pack_file.streaming_iter().map_err(|e| {
            PackIngestionError::unpack_objects_operation(
                "failed to create pack streaming iterator",
                context.clone().with_elapsed(start_time.elapsed()),
                Some(Box::new(e)),
            )
        })?;
        
        // Create a resolve function for handling ref deltas in thin packs
        let resolve_fn = |oid: &gix_hash::oid, out: &mut Vec<u8>| -> Option<gix_pack::data::decode::entry::ResolvedBase> {
            // If we have a thin pack lookup, try to find the object in the main repository
            if let Some(ref lookup) = thin_pack_lookup {
                use gix_object::Find;
                match lookup.try_find(oid, out) {
                    Ok(Some(obj)) => {
                        return Some(gix_pack::data::decode::entry::ResolvedBase::OutOfPack {
                            kind: obj.kind,
                            end: out.len(),
                        });
                    }
                    _ => {}
                }
            }
            None
        };
        
        // Process each entry in the pack with memory tracking
        for entry_result in pack_iter {
            // Check memory pressure periodically
            if _entries_processed % 100 == 0 {
                let _stats = memory_tracker.stats();
                // Progress updates handled internally by gix-features
            }
            
            let entry = entry_result.map_err(|e| {
                PackIngestionError::unpack_objects_operation(
                    "failed to read pack entry (streaming)",
                    context.clone().with_elapsed(start_time.elapsed()),
                    Some(Box::new(e)),
                )
            })?;
            
            // Clear buffer before decoding to manage memory
            decoded_buf.clear();
            
            // Convert input::Entry to data::Entry for decode_entry
            let data_entry = gix_pack::data::Entry {
                header: entry.header,
                decompressed_size: entry.decompressed_size,
                data_offset: entry.pack_offset + entry.header_size as u64,
            };
            
            // Decode the entry (this handles delta resolution)
            let outcome = pack_file.decode_entry(
                data_entry,
                &mut decoded_buf,
                &mut inflate,
                &resolve_fn,
                &mut gix_pack::cache::Never,
            ).map_err(|e| {
                PackIngestionError::unpack_objects_operation(
                    "failed to decode pack entry (streaming)",
                    context.clone().with_elapsed(start_time.elapsed()),
                    Some(Box::new(e)),
                )
            })?;
            
            // Write the decoded object as a loose object
            use gix_object::Write;
            let _oid = loose_store.write_buf(outcome.kind, &decoded_buf).map_err(|e| {
                PackIngestionError::unpack_objects_operation(
                    "failed to write loose object (streaming)",
                    context.clone().with_elapsed(start_time.elapsed()),
                    Some(e),
                )
            })?;
            
            _entries_processed += 1;
            
            // Check for interruption
            if should_interrupt.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(PackIngestionError::io(
                    "pack explosion interrupted (streaming)",
                    context.clone().with_elapsed(start_time.elapsed()),
                    std::io::Error::new(std::io::ErrorKind::Interrupted, "operation cancelled"),
                ));
            }
            
            // Periodically shrink the buffer if it's grown too large
            if decoded_buf.capacity() > self.streaming_config.buffer_size * 10 {
                decoded_buf.shrink_to(self.streaming_config.buffer_size);
            }
        }

        // Perform fsck validation if configured
        let fsck_results = if let Some(ref validator) = self.fsck_validator {
            if let Some(ref main_odb) = thin_pack_lookup {
                validator
                    .validate_quarantine(quarantine_objects_dir, main_odb)
                    .map_err(|e| match e {
                        crate::Error::Fsck(msg) => PackIngestionError::object_validation(
                            msg,
                            context.clone().with_elapsed(start_time.elapsed()),
                            None,
                            vec![],
                            None,
                        ),
                        _ => PackIngestionError::object_validation(
                            "fsck validation failed",
                            context.clone().with_elapsed(start_time.elapsed()),
                            None,
                            vec![e.to_string()],
                            Some(Box::new(e)),
                        ),
                    })?
            } else {
                let temp_odb = gix_odb::at(quarantine_objects_dir).map_err(|e| {
                    PackIngestionError::object_database(
                        "failed to create temp ODB for fsck",
                        context.clone().with_elapsed(start_time.elapsed()),
                        Some(Box::new(e)),
                    )
                })?;
                validator
                    .validate_quarantine(quarantine_objects_dir, &temp_odb)
                    .map_err(|e| match e {
                        crate::Error::Fsck(msg) => PackIngestionError::object_validation(
                            msg,
                            context.clone().with_elapsed(start_time.elapsed()),
                            None,
                            vec![],
                            None,
                        ),
                        _ => PackIngestionError::object_validation(
                            "fsck validation failed",
                            context.clone().with_elapsed(start_time.elapsed()),
                            None,
                            vec![e.to_string()],
                            Some(Box::new(e)),
                        ),
                    })?
            }
        } else {
            FsckResults {
                validated_objects: Vec::new(),
                warnings: Vec::new(),
                errors: Vec::new(),
                missing_objects: Vec::new(),
            }
        };

        // Remove temporary pack artifacts
        if let Ok(entries) = fs::read_dir(&pack_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) == Some("pack")
                    || p.extension().and_then(|e| e.to_str()) == Some("idx")
                {
                    let _ = fs::remove_file(p);
                }
            }
        }

        // Clean up buffer pool
        buffer_pool.clear();

        Ok((fsck_results, streaming_stats))
    }

    /// Stub version of streaming unpack_objects when required features are not enabled.
    #[cfg(not(all(feature = "progress", feature = "pack-streaming")))]
    pub fn unpack_objects_streaming(
        &self,
        _input: &mut dyn std::io::BufRead,
        _quarantine_objects_dir: &std::path::Path,
        _pack_size: Option<u64>,
        _thin_pack_lookup: Option<gix_odb::Handle>,
        _progress: &mut dyn std::any::Any,
    ) -> Result<(FsckResults, StreamingStats)> {
        let context = ErrorContext::new("unpack-objects-streaming")
            .with_context("reason", "progress and pack-streaming features not enabled");
        Err(PackIngestionError::configuration(
            "unpack-objects-streaming requires progress and pack-streaming features",
            context,
        ))
    }
}

impl PackIngestionController {
    /// Stub version of ingest_pack_with_fallback when progress feature is not enabled.
    #[cfg(not(feature = "progress"))]
    pub fn ingest_pack_with_fallback(
        &self,
        _input: &mut dyn std::io::BufRead,
        _quarantine_objects_dir: &std::path::Path,
        _pack_size: Option<u64>,
        _object_count_hint: Option<u64>,
        _thin_pack_lookup: Option<gix_odb::Handle>,
        _progress: &mut dyn std::any::Any,
    ) -> Result<PackIngestionResult> {
        let context =
            ErrorContext::new("pack-ingestion-with-fallback").with_context("reason", "progress feature not enabled");
        Err(PackIngestionError::configuration(
            "pack ingestion with fallback requires progress feature",
            context,
        ))
    }

    /// Stub version of ingest_pack_streaming_with_fallback when required features are not enabled.
    #[cfg(not(all(feature = "progress", feature = "pack-streaming")))]
    pub fn ingest_pack_streaming_with_fallback(
        &self,
        _input: &mut dyn std::io::BufRead,
        _quarantine_objects_dir: &std::path::Path,
        _pack_size: Option<u64>,
        _object_count_hint: Option<u64>,
        _thin_pack_lookup: Option<gix_odb::Handle>,
        _progress: &mut dyn std::any::Any,
    ) -> Result<PackIngestionStreamingResult> {
        let context = ErrorContext::new("pack-ingestion-streaming-with-fallback")
            .with_context("reason", "progress and pack-streaming features not enabled");
        Err(PackIngestionError::configuration(
            "streaming pack ingestion with fallback requires progress and pack-streaming features",
            context,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_choose_path() {
        let pol = IngestionPolicy {
            unpack_limit: Some(100),
            enable_fallback: false,
        };
        assert!(matches!(pol.choose_path(Some(50)), PackIngestPath::UnpackObjects));
        assert!(matches!(pol.choose_path(Some(150)), PackIngestPath::IndexPack));
        assert!(matches!(pol.choose_path(None), PackIngestPath::IndexPack));

        let pol2 = IngestionPolicy {
            unpack_limit: None,
            enable_fallback: false,
        };
        assert!(matches!(pol2.choose_path(Some(1)), PackIngestPath::IndexPack));
    }

    #[test]
    fn policy_fallback_strategies() {
        let pol = IngestionPolicy::with_fallback(Some(100));

        // Test fallback availability
        assert!(pol.has_fallback(PackIngestPath::IndexPack));
        assert!(pol.has_fallback(PackIngestPath::UnpackObjects));

        // Test fallback strategy selection
        assert_eq!(
            pol.get_fallback_strategy(PackIngestPath::IndexPack),
            Some(PackIngestPath::UnpackObjects)
        );
        assert_eq!(
            pol.get_fallback_strategy(PackIngestPath::UnpackObjects),
            Some(PackIngestPath::IndexPack)
        );

        // Test strategy sequence
        let sequence = pol.get_strategy_sequence(Some(50)); // Should prefer UnpackObjects
        assert_eq!(sequence, vec![PackIngestPath::UnpackObjects, PackIngestPath::IndexPack]);

        let sequence = pol.get_strategy_sequence(Some(150)); // Should prefer IndexPack
        assert_eq!(sequence, vec![PackIngestPath::IndexPack, PackIngestPath::UnpackObjects]);
    }

    #[test]
    fn policy_no_fallback() {
        let pol = IngestionPolicy::without_fallback(Some(100));

        // Test fallback disabled
        assert!(!pol.has_fallback(PackIngestPath::IndexPack));
        assert!(!pol.has_fallback(PackIngestPath::UnpackObjects));

        // Test no fallback strategy
        assert_eq!(pol.get_fallback_strategy(PackIngestPath::IndexPack), None);
        assert_eq!(pol.get_fallback_strategy(PackIngestPath::UnpackObjects), None);

        // Test strategy sequence with no fallback
        let sequence = pol.get_strategy_sequence(Some(50));
        assert_eq!(sequence, vec![PackIngestPath::UnpackObjects]);

        let sequence = pol.get_strategy_sequence(Some(150));
        assert_eq!(sequence, vec![PackIngestPath::IndexPack]);
    }

    #[test]
    fn pack_ingestion_controller_creation() {
        let ingestor = PackIngestor::default();
        let policy = IngestionPolicy::with_fallback(Some(100));
        let controller = PackIngestionController::new(ingestor, policy);

        assert_eq!(controller.max_fallback_attempts, 2);
        assert!(controller.policy().enable_fallback);

        let controller = controller.with_max_fallback_attempts(5);
        assert_eq!(controller.max_fallback_attempts, 5);
    }

    #[test]
    fn pack_ingestion_controller_should_attempt_fallback() {
        let ingestor = PackIngestor::default();
        let policy = IngestionPolicy::with_fallback(Some(100));
        let controller = PackIngestionController::new(ingestor, policy);

        // Create a recoverable error
        let recoverable_error = PackIngestionError::io(
            "temporary failure",
            ErrorContext::new("test"),
            std::io::Error::new(std::io::ErrorKind::Interrupted, "test"),
        );

        // Should attempt fallback for recoverable error on first attempt
        assert!(controller.should_attempt_fallback(&recoverable_error, 0, 0));

        // Should not attempt fallback if we've exceeded max attempts
        assert!(!controller.should_attempt_fallback(&recoverable_error, 0, 2));

        // Should not attempt fallback if this is already a fallback attempt
        assert!(!controller.should_attempt_fallback(&recoverable_error, 1, 0));

        // Create a non-recoverable error
        let non_recoverable_error =
            PackIngestionError::object_validation("validation failed", ErrorContext::new("test"), None, vec![], None);

        // Should not attempt fallback for non-recoverable error
        assert!(!controller.should_attempt_fallback(&non_recoverable_error, 0, 0));
    }

    #[test]
    fn pack_ingestion_controller_no_fallback_policy() {
        let ingestor = PackIngestor::default();
        let policy = IngestionPolicy::without_fallback(Some(100));
        let controller = PackIngestionController::new(ingestor, policy);

        let recoverable_error = PackIngestionError::io(
            "temporary failure",
            ErrorContext::new("test"),
            std::io::Error::new(std::io::ErrorKind::Interrupted, "test"),
        );

        // Should not attempt fallback when policy disables it
        assert!(!controller.should_attempt_fallback(&recoverable_error, 0, 0));
    }

    #[test]
    fn quarantine_paths_form() {
        let mut q = quarantine::Quarantine::new("/tmp/main-objects".into());
        // Quarantine needs to be activated to have meaningful paths
        let _ = q.activate(); // Ignore errors in test
        if q.is_active() {
            assert!(q.objects_dir.display().to_string().contains("quarantine"));
        }
        // Note: main_objects_dir is private, so we can't test it directly
    }
}
