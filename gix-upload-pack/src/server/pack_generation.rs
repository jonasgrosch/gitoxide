//! Advanced pack file generation using gix-pack infrastructure
//!
//! This module replaces our manual pack generation with the sophisticated
//! gix-pack system, providing delta compression, streaming output, and
//! better performance.

use crate::{
    config::ServerOptions,
    error::{Error, Result},
    server::response::{ProgressReporter, ResponseFormatter},
    types::*,
};
use bstr::{BStr, ByteSlice};
use gix::Repository;
use gix_features::{
    parallel,
    progress::{self},
};
use gix_pack::data::output;
use std::sync::atomic::AtomicBool;
use std::{collections::HashSet, io::Write};

/// Adapter to make Repository objects compatible with gix_pack::Find trait
#[derive(Clone)]
struct RepositoryFindAdapter {
    objects: gix::odb::Handle,
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
struct ObjectCount {
    trees: usize,
    commits: usize,
    blobs: usize,
    tags: usize,
    delta_ref: usize,
    delta_oid: usize,
}

impl ObjectCount {
    fn total(&self) -> usize {
        self.tags + self.trees + self.commits + self.blobs + self.delta_ref + self.delta_oid
    }
    fn add(&mut self, kind: output::entry::Kind) {
        use gix_object::Kind::*;
        use output::entry::Kind::*;
        match kind {
            Base(Tree) => self.trees += 1,
            Base(Commit) => self.commits += 1,
            Base(Blob) => self.blobs += 1,
            Base(Tag) => self.tags += 1,
            DeltaRef { .. } => self.delta_ref += 1,
            DeltaOid { .. } => self.delta_oid += 1,
        }
    }
}

impl RepositoryFindAdapter {
    fn new(repository: &Repository) -> Self {
        let mut objects = repository.objects.clone().into_inner();
        // Configure the handle to prevent pack unloading, which is required for
        // advanced pack operations like delta compression
        objects.prevent_pack_unload();

        Self { objects }
    }
}

impl gix_pack::Find for RepositoryFindAdapter {
    fn contains(&self, id: &gix_hash::oid) -> bool {
        self.objects.contains(id)
    }

    fn try_find_cached<'a>(
        &self,
        id: &gix_hash::oid,
        buffer: &'a mut Vec<u8>,
        pack_cache: &mut dyn gix_pack::cache::DecodeEntry,
    ) -> std::result::Result<
        Option<(gix_object::Data<'a>, Option<gix_pack::data::entry::Location>)>,
        Box<dyn std::error::Error + Send + Sync + 'static>,
    > {
        // Delegate to the Handle's implementation
        self.objects
            .try_find_cached(id, buffer, pack_cache)
            .map_err(|e| e.into())
    }

    fn location_by_oid(&self, id: &gix_hash::oid, buf: &mut Vec<u8>) -> Option<gix_pack::data::entry::Location> {
        // Delegate to the Handle's implementation (now that prevent_pack_unload is called)
        self.objects.location_by_oid(id, buf)
    }

    fn pack_offsets_and_oid(&self, pack_id: u32) -> Option<Vec<(gix_pack::data::Offset, gix_hash::ObjectId)>> {
        // Delegate to the Handle's implementation (now that prevent_pack_unload is called)
        self.objects.pack_offsets_and_oid(pack_id)
    }

    fn entry_by_location(&self, location: &gix_pack::data::entry::Location) -> Option<gix_pack::find::Entry> {
        // Delegate to the Handle's implementation (now that prevent_pack_unload is called)
        self.objects.entry_by_location(location)
    }
}

/// Pack generator using gix-pack infrastructure for advanced pack generation
pub struct PackGenerator<'a> {
    repository: &'a Repository,
    options: &'a ServerOptions,
}

/// Statistics about pack generation
#[derive(Debug, Default)]
pub struct PackStats {
    /// Number of objects in the pack
    pub object_count: u32,
    /// Total size of the pack
    pub pack_size: u64,
    /// Number of objects with delta compression
    pub delta_objects: u32,
    /// Compression ratio achieved
    pub compression_ratio: f64,
    /// Time taken for pack generation
    pub generation_time_ms: u64,
}

/// Git-native pack configuration values for compatibility
#[derive(Debug, Clone)]
struct PackConfig {
    threads: usize,
    window: usize,
    depth: u32,
    window_memory: u64,
    delta_cache_size: u64,
    delta_cache_limit: u32,
}

impl<'a> PackGenerator<'a> {
    /// Create a new pack generator
    pub fn new(repository: &'a Repository, options: &'a ServerOptions) -> Self {
        Self { repository, options }
    }

    /// Create an optimized RepositoryFindAdapter with buffer pool optimization
    fn create_optimized_find_adapter(&self) -> RepositoryFindAdapter {
        let mut objects = self.repository.objects.clone().into_inner();
        // Configure the handle to prevent pack unloading and optimize buffer usage
        objects.prevent_pack_unload();

        RepositoryFindAdapter { objects }
    }

    /// Get Git-native pack configuration values optimized for performance
    fn get_pack_config(&self) -> PackConfig {
        let config = self.repository.config_snapshot();
        let available_threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);

        PackConfig {
            // Optimize thread count: use available cores but cap at 8 to avoid overhead
            threads: config
                .integer("pack.threads")
                .unwrap_or(available_threads as i64)
                .max(1)
                .min(8) as usize,
            // Larger window for better delta compression but not too large to avoid memory pressure
            window: config.integer("pack.window").unwrap_or(50).max(10).min(250) as usize,
            // Reasonable depth for good compression without excessive CPU time
            depth: config.integer("pack.depth").unwrap_or(50).max(10).min(100) as u32,
            // Optimize memory settings for performance
            window_memory: config.integer("pack.windowMemory").unwrap_or(512 * 1024 * 1024).max(0) as u64,
            delta_cache_size: config
                .integer("pack.deltaCacheSize")
                .unwrap_or(512 * 1024 * 1024)
                .max(0) as u64,
            delta_cache_limit: config.integer("pack.deltaCacheLimit").unwrap_or(2000).max(0) as u32,
        }
    }

    /// Generate a pack file using gix-pack infrastructure with optimized buffer management
    pub fn generate_pack<W: Write>(&self, mut writer: W, session: &SessionContext) -> Result<PackStats> {
        let start_time = std::time::Instant::now();

        // Step 1: Prepare minimal object IDs (just wants, let gix-pack do the expansion)
        let prepare_start = std::time::Instant::now();
        let object_ids = self.prepare_minimal_objects(session)?;
        let prepare_duration = prepare_start.elapsed();
        eprintln!("Pack generation timing: Object preparation took {:?}", prepare_duration);

        if object_ids.is_empty() {
            // Return empty pack
            return self.write_empty_pack(writer, session);
        }

        // Step 2: Use gix-pack's count::objects to analyze and expand the objects
        // This replaces our manual enumeration - gix-pack will do tree traversal for us
        let count_start = std::time::Instant::now();
        let (counts, count_stats) = self.count_objects_with_expansion(object_ids, &mut writer, session)?;
        let count_duration = count_start.elapsed();
        eprintln!(
            "Pack generation timing: Object counting with expansion took {:?}",
            count_duration
        );

        // Step 3: Compress and stream pack data using gix-pack's FromEntriesIter
        let pack_start = std::time::Instant::now();
        let pack_stats = self.stream_pack_data(&mut writer, counts, count_stats.total_objects, session)?;
        let pack_duration = pack_start.elapsed();
        eprintln!("Pack generation timing: Pack streaming took {:?}", pack_duration);

        // Step 4: Send final status message (Git-compatible)
        let status_start = std::time::Instant::now();
        self.send_final_status(&mut writer, &pack_stats, session)?;
        let status_duration = status_start.elapsed();
        eprintln!("Pack generation timing: Final status took {:?}", status_duration);

        let generation_time = start_time.elapsed();
        eprintln!(
            "Pack generation timing: Total pack generation took {:?}",
            generation_time
        );

        Ok(PackStats {
            object_count: pack_stats.object_count,
            pack_size: pack_stats.pack_size,
            delta_objects: pack_stats.delta_objects,
            compression_ratio: pack_stats.compression_ratio,
            generation_time_ms: generation_time.as_millis() as u64,
        })
    }

    /// Prepare objects using optimized commit traversal
    fn prepare_minimal_objects(&self, session: &SessionContext) -> Result<Vec<gix_hash::ObjectId>> {
        let prepare_start = std::time::Instant::now();

        // Create sets for efficient lookup
        let haves: std::collections::HashSet<_> = session.negotiation.haves.iter().collect();
        let common: std::collections::HashSet<_> = session.negotiation.common.iter().collect();

        let mut all_objects = Vec::new();

        // Process each want - separate commits from other objects
        let mut commit_wants = Vec::new();
        let mut non_commit_wants = Vec::new();

        for want in &session.negotiation.wants {
            // Skip objects we already have
            if haves.contains(want) || common.contains(want) {
                continue;
            }

            // Verify the object exists
            use gix_object::Exists;
            if !self.repository.exists(want) {
                continue;
            }

            // Separate commits from other objects
            if let Ok(_commit) = self.repository.find_commit(*want) {
                commit_wants.push(*want);
            } else {
                non_commit_wants.push(*want);
            }
        }

        // For commits, use gix repository's revision walker for better performance
        if !commit_wants.is_empty() {
            let traverse_start = std::time::Instant::now();

            // Create excluded commits list for efficient filtering
            let excluded_commits: Vec<_> = haves
                .iter()
                .chain(common.iter())
                .filter(|id| {
                    // Only exclude commits, not other object types
                    self.repository.find_commit(***id).is_ok()
                })
                .map(|id| **id)
                .collect();

            // Use gix repository's optimized revision walker
            let walk = self
                .repository
                .rev_walk(commit_wants)
                .with_hidden(excluded_commits)
                .sorting(gix::revision::walk::Sorting::ByCommitTime(
                    gix_traverse::commit::simple::CommitTimeOrder::NewestFirst,
                ))
                .all()
                .map_err(|e| Error::custom(format!("Revision walk setup failed: {}", e)))?;

            // Collect all reachable commits efficiently
            for commit_info in walk {
                let commit_info = commit_info.map_err(|e| Error::custom(format!("Revision walk failed: {}", e)))?;
                all_objects.push(commit_info.id);
            }

            let traverse_duration = traverse_start.elapsed();
            eprintln!("Prepare objects timing: Commit traversal took {:?}", traverse_duration);
        }

        // Add non-commit objects directly
        all_objects.extend(non_commit_wants);

        let prepare_duration = prepare_start.elapsed();
        eprintln!(
            "Prepare objects timing: Collected {} total objects in {:?}",
            all_objects.len(),
            prepare_duration
        );

        Ok(all_objects)
    }

    /// Use gix-pack's count::objects with TreeContents expansion to do all the work
    fn count_objects_with_expansion<W: Write>(
        &self,
        object_ids: Vec<gix_hash::ObjectId>,
        writer: &mut W,
        session: &SessionContext,
    ) -> Result<(Vec<output::Count>, output::count::objects::Outcome)> {
        let count_start = std::time::Instant::now();

        // Set up sideband progress reporting
        let formatter_start = std::time::Instant::now();
        let mut formatter = ResponseFormatter::new_with_progress_control(
            writer,
            session.capabilities.side_band,
            session.capabilities.no_progress,
        );
        let mut progress_reporter =
            ProgressReporter::new(&mut formatter, "Counting objects".to_string(), Some(object_ids.len()));
        let formatter_duration = formatter_start.elapsed();
        eprintln!("Count objects timing: Formatter setup took {:?}", formatter_duration);

        // Start the gix-pack counting with optimized adapter and Git-native configuration
        let adapter_start = std::time::Instant::now();
        let find_adapter = self.create_optimized_find_adapter();
        let adapter_duration = adapter_start.elapsed();
        eprintln!(
            "Count objects timing: Find adapter creation took {:?}",
            adapter_duration
        );

        // Use Git-native pack configuration for optimal compatibility
        let config_start = std::time::Instant::now();
        let pack_config = self.get_pack_config();
        let config_duration = config_start.elapsed();
        eprintln!("Count objects timing: Pack config retrieval took {:?}", config_duration);

        // For now, always use TreeContents to match our original behavior
        // The TreeAdditionsComparedToAncestor mode might be filtering too aggressively
        let expansion_mode = output::count::objects::ObjectExpansion::TreeContents;

        let counting_start = std::time::Instant::now();

        // Use the object_ids we collected from prepare_minimal_objects
        // This should contain all the commits we traversed
        let objects_iter = object_ids
            .into_iter()
            .map(|id| Ok::<_, Box<dyn std::error::Error + Send + Sync + 'static>>(id));

        let (mut counts, stats) = output::count::objects(
            find_adapter.clone(),
            Box::new(objects_iter),
            &progress::Discard,
            &AtomicBool::new(false),
            output::count::objects::Options {
                input_object_expansion: expansion_mode,
                thread_limit: Some(pack_config.threads.min(8)), // Limit threads to avoid overhead
                chunk_size: pack_config.window.max(50),         // Larger chunks for better efficiency
            },
        )
        .map_err(|e| Error::Pack(format!("Object counting failed: {}", e)))?;
        let counting_duration = counting_start.elapsed();
        eprintln!("Count objects timing: Actual counting took {:?}", counting_duration);

        // Now we need to filter out objects that the client already has
        if !session.negotiation.haves.is_empty() || !session.negotiation.common.is_empty() {
            let filter_start = std::time::Instant::now();
            counts = self.filter_existing_objects(counts, session)?;
            let filter_duration = filter_start.elapsed();
            eprintln!("Count objects timing: Object filtering took {:?}", filter_duration);
        }

        let _actual_count = counts.iter().fold(ObjectCount::default(), |mut c, _e| {
            c.add(gix_pack::data::output::entry::Kind::Base(gix_object::Kind::Blob));
            let _ = progress_reporter.update(c.total());

            c
        });

        // Send final completion message (Git-style)
        progress_reporter.finish()?;

        // Report final completion
        let count_total_duration = count_start.elapsed();
        eprintln!(
            "Count objects timing: Total counting took {:?} - {} total objects (expanded from {} input objects)",
            count_total_duration, stats.total_objects, stats.input_objects
        );

        Ok((counts, stats))
    }

    /// DEPRECATED: Collect object IDs using gix revision walking with progress reporting
    /// This method is now replaced by prepare_minimal_objects + count_objects_with_expansion
    #[allow(dead_code)]
    fn enumerate<W: Write>(&self, writer: &mut W, session: &SessionContext) -> Result<Vec<gix_hash::ObjectId>> {
        let enumerate_start = std::time::Instant::now();

        // Create formatter for progress reporting
        let formatter_start = std::time::Instant::now();
        let mut formatter = ResponseFormatter::new_with_progress_control(
            writer,
            session.capabilities.side_band,
            session.capabilities.no_progress,
        );
        let formatter_duration = formatter_start.elapsed();
        eprintln!("Enumeration timing: Formatter creation took {:?}", formatter_duration);

        // Start progress reporting for object collection
        let mut progress_reporter = ProgressReporter::new(
            &mut formatter,
            "Enumerating objects".to_string(),
            None, // We don't know total count upfront
        );

        // Use optimized HashSet with better capacity estimation
        let estimated_capacity = session.negotiation.wants.len() * 100; // Rough estimate
        let mut objects = HashSet::with_capacity(estimated_capacity);

        // Filter wants to only include objects we don't already have and dereference tags to commits
        let wants_processing_start = std::time::Instant::now();
        let mut commit_wants = Vec::new();
        let mut non_commit_objects = Vec::new();

        for want in &session.negotiation.wants {
            if session.negotiation.common.iter().any(|c| c == want)
                || session.negotiation.haves.iter().any(|h| h == want)
            {
                continue;
            }

            // Use type-specific methods for better performance and type safety
            if let Ok(commit) = self.repository.find_commit(*want) {
                commit_wants.push(*want);
            } else if let Ok(tag) = self.repository.find_tag(*want) {
                // Add the tag itself to objects
                non_commit_objects.push(*want);

                // Try to get the target and add it if it's a commit
                if let Ok(target_id) = tag.target_id() {
                    let target_id = target_id.detach();
                    if let Ok(_) = self.repository.find_commit(target_id) {
                        commit_wants.push(target_id);
                    } else {
                        use gix_object::Exists;
                        if self.repository.exists(&target_id) {
                            non_commit_objects.push(target_id);
                        }
                    }
                }
            } else {
                use gix_object::Exists;
                if self.repository.exists(want) {
                    // For trees and blobs, just add them directly
                    non_commit_objects.push(*want);
                }
            }
            // Skip missing objects (no else clause needed)
        }

        // Add non-commit objects directly to our set
        for obj_id in non_commit_objects {
            objects.insert(obj_id);

            // If it's a tree, traverse it - use type-specific method
            if let Ok(_tree) = self.repository.find_tree(obj_id) {
                self.traverse_tree(obj_id, &mut objects)?;
            }
        }

        let wants_processing_duration = wants_processing_start.elapsed();
        eprintln!(
            "Enumeration timing: Wants processing took {:?}",
            wants_processing_duration
        );

        if commit_wants.is_empty() {
            progress_reporter.finish()?;
            let filtered_objects: Vec<_> = objects.into_iter().collect();
            let enumerate_total_duration = enumerate_start.elapsed();
            eprintln!(
                "Enumeration timing: Total enumeration (no commits) took {:?}",
                enumerate_total_duration
            );
            return Ok(filtered_objects);
        }

        // Create revision walker starting from commit wants, excluding haves and common
        let revwalk_setup_start = std::time::Instant::now();
        let mut excluded_commits: Vec<_> = session
            .negotiation
            .haves
            .iter()
            .chain(session.negotiation.common.iter())
            .filter(|id| {
                // Only exclude commits, not other object types - use type-specific method
                self.repository.find_commit(**id).is_ok()
            })
            .copied()
            .collect();
        excluded_commits.sort();
        excluded_commits.dedup();

        // Use optimized revision walker with performance settings
        let walk = self
            .repository
            .rev_walk(commit_wants)
            .with_hidden(excluded_commits)
            .sorting(gix::revision::walk::Sorting::ByCommitTime(
                gix_traverse::commit::simple::CommitTimeOrder::NewestFirst,
            )) // More efficient for recent commits
            .all()?;
        let revwalk_setup_duration = revwalk_setup_start.elapsed();
        eprintln!(
            "Enumeration timing: Revision walk setup took {:?}",
            revwalk_setup_duration
        );

        let mut update_counter = 0;
        const UPDATE_INTERVAL: usize = 10000; // Reduce progress update frequency for better performance

        // Walk commits and collect all reachable objects with aggressive optimizations
        let revwalk_start = std::time::Instant::now();
        let mut commit_batch = Vec::with_capacity(500); // Larger batches for better performance
        let mut tree_cache = std::collections::HashMap::new(); // Cache tree traversals

        for commit_info in walk {
            let commit_info = commit_info.map_err(|e| Error::custom(format!("Revision walk failed: {}", e)))?;
            commit_batch.push(commit_info.id);

            // Process commits in larger batches for better cache locality
            if commit_batch.len() >= 500 {
                self.process_commit_batch_cached(&commit_batch, &mut objects, &mut tree_cache)?;
                commit_batch.clear();

                // Update progress much less frequently
                update_counter += 500;
                if update_counter % UPDATE_INTERVAL == 0 {
                    progress_reporter.update(objects.len())?;
                }
            }
        }

        // Process remaining commits
        if !commit_batch.is_empty() {
            self.process_commit_batch_cached(&commit_batch, &mut objects, &mut tree_cache)?;
        }

        let revwalk_duration = revwalk_start.elapsed();
        eprintln!("Enumeration timing: Revision walk took {:?}", revwalk_duration);

        progress_reporter.set_current(objects.len());
        progress_reporter.finish()?;

        // Apply filters if needed
        let filter_start = std::time::Instant::now();
        let mut filtered_objects: Vec<_> = objects.into_iter().collect();

        // Apply object filter if specified
        if let Some(filter) = &session.capabilities.filter {
            filtered_objects = self.apply_object_filter(filtered_objects, filter.as_ref())?;
        }
        let filter_duration = filter_start.elapsed();
        eprintln!("Enumeration timing: Object filtering took {:?}", filter_duration);

        let enumerate_total_duration = enumerate_start.elapsed();
        eprintln!(
            "Enumeration timing: Total enumeration took {:?}",
            enumerate_total_duration
        );

        Ok(filtered_objects)
    }

    /// DEPRECATED: Use gix-pack's count::objects for intelligent object analysis with progress reporting
    /// This method is now replaced by count_objects_with_expansion
    #[allow(dead_code)]
    fn count_objects<W: Write>(
        &self,
        object_ids: Vec<gix_hash::ObjectId>,
        writer: &mut W,
        session: &SessionContext,
    ) -> Result<(Vec<output::Count>, output::count::objects::Outcome)> {
        let count_start = std::time::Instant::now();

        // Set up sideband progress reporting
        let formatter_start = std::time::Instant::now();
        let mut formatter = ResponseFormatter::new_with_progress_control(
            writer,
            session.capabilities.side_band,
            session.capabilities.no_progress,
        );
        let mut progress_reporter =
            ProgressReporter::new(&mut formatter, "Counting objects".to_string(), Some(object_ids.len()));
        let formatter_duration = formatter_start.elapsed();
        eprintln!("Count objects timing: Formatter setup took {:?}", formatter_duration);

        // Start the gix-pack counting with optimized adapter and Git-native configuration
        let adapter_start = std::time::Instant::now();
        let find_adapter = self.create_optimized_find_adapter();
        let objects_iter = object_ids
            .into_iter()
            .map(|id| Ok::<_, Box<dyn std::error::Error + Send + Sync + 'static>>(id));
        let adapter_duration = adapter_start.elapsed();
        eprintln!(
            "Count objects timing: Find adapter creation took {:?}",
            adapter_duration
        );

        // Use Git-native pack configuration for optimal compatibility
        let config_start = std::time::Instant::now();
        let pack_config = self.get_pack_config();
        let config_duration = config_start.elapsed();
        eprintln!("Count objects timing: Pack config retrieval took {:?}", config_duration);

        let counting_start = std::time::Instant::now();
        let (counts, stats) = output::count::objects(
            find_adapter,
            Box::new(objects_iter),
            &progress::Discard,
            &AtomicBool::new(false),
            output::count::objects::Options {
                input_object_expansion: output::count::objects::ObjectExpansion::TreeContents,
                thread_limit: Some(pack_config.threads.min(8)), // Limit threads to avoid overhead
                chunk_size: pack_config.window.max(50),         // Larger chunks for better efficiency
                ..Default::default()
            },
        )
        .map_err(|e| Error::Pack(format!("Object counting failed: {}", e)))?;
        let counting_duration = counting_start.elapsed();
        eprintln!("Count objects timing: Actual counting took {:?}", counting_duration);

        let _actual_count = counts.iter().fold(ObjectCount::default(), |mut c, _e| {
            c.add(gix_pack::data::output::entry::Kind::Base(gix_object::Kind::Blob));
            let _ = progress_reporter.update(c.total());

            c
        });

        // Send final completion message (Git-style)
        progress_reporter.finish()?;

        // Report final completion
        let count_total_duration = count_start.elapsed();
        eprintln!(
            "Count objects timing: Total counting took {:?} - {} total objects",
            count_total_duration, stats.total_objects
        );

        Ok((counts, stats))
    }

    /// Stream pack data using gix-pack's FromEntriesIter
    fn stream_pack_data<W: Write>(
        &self,
        mut writer: W,
        counts: Vec<output::Count>,
        total_objects: usize,
        session: &SessionContext,
    ) -> Result<PackGenerationStats> {
        let adapter_start = std::time::Instant::now();
        let find_adapter = self.create_optimized_find_adapter();
        let adapter_duration = adapter_start.elapsed();
        eprintln!(
            "Pack streaming timing: Find adapter creation took {:?}",
            adapter_duration
        );

        // Use Git-native pack configuration for entry generation
        let config_start = std::time::Instant::now();
        let pack_config = self.get_pack_config();
        let config_duration = config_start.elapsed();
        eprintln!(
            "Pack streaming timing: Pack config retrieval took {:?}",
            config_duration
        );

        let entries_iter_start = std::time::Instant::now();
        let mut entries_iter = output::entry::iter_from_counts(
            counts,
            find_adapter,
            Box::new(progress::Discard),
            output::entry::iter_from_counts::Options {
                allow_thin_pack: session.capabilities.thin_pack,
                thread_limit: Some(pack_config.threads.min(8)), // Limit threads to avoid overhead
                chunk_size: pack_config.window.max(100),        // Larger chunks for better efficiency
                ..Default::default()
            },
        );
        let entries_iter_duration = entries_iter_start.elapsed();
        eprintln!(
            "Pack streaming timing: Entries iterator creation took {:?}",
            entries_iter_duration
        );

        // Use InOrderIter to properly sort the parallel chunks by sequence ID, following the example
        let entries_collect_start = std::time::Instant::now();
        let entries: Vec<_> = parallel::InOrderIter::from(entries_iter.by_ref())
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| Error::Pack(format!("Entry generation failed: {}", e)))?
            .into_iter()
            .flatten()
            .collect();
        let entries_collect_duration = entries_collect_start.elapsed();
        eprintln!(
            "Pack streaming timing: Entries collection took {:?}",
            entries_collect_duration
        );

        let actual_count = entries.len();

        let mut formatter = ResponseFormatter::new_with_progress_control(
            &mut writer,
            session.capabilities.side_band,
            session.capabilities.no_progress,
        );

        // send compressing status to sideband (this is the compression/writing phase)
        let mut progress_reporter =
            ProgressReporter::new(&mut formatter, "Compressing objects".to_string(), Some(total_objects));

        // Count entry types for debugging, following the example pattern
        let entry_stats = entries.iter().fold(ObjectCount::default(), |mut c, e| {
            c.add(e.kind);
            progress_reporter.update(c.total()).unwrap_or(());
            c
        });

        eprintln!("Debug: Entry stats: {:?}", entry_stats);
        progress_reporter.finish()?;

        // Create formatter for pack data writing (for proper sideband handling)
        let formatter_start = std::time::Instant::now();
        let formatter = ResponseFormatter::new_with_progress_control(
            &mut writer,
            session.capabilities.side_band,
            session.capabilities.no_progress,
        );
        let formatter_duration = formatter_start.elapsed();
        eprintln!(
            "Pack streaming timing: Formatter creation took {:?}",
            formatter_duration
        );

        // Use gix-pack's streaming pack writer with the formatter, following the example pattern
        let pack_writer_start = std::time::Instant::now();
        let mut pack_writer = output::bytes::FromEntriesIter::new(
            std::iter::once(Ok::<_, output::entry::iter_from_counts::Error>(entries)),
            formatter,
            actual_count as u32,
            gix_pack::data::Version::V2,
            self.repository.object_hash(),
        );
        let pack_writer_duration = pack_writer_start.elapsed();
        eprintln!(
            "Pack streaming timing: Pack writer creation took {:?}",
            pack_writer_duration
        );

        let mut total_bytes_written = 0u64;

        // Stream the pack data, following the example pattern
        let streaming_start = std::time::Instant::now();
        for result in &mut pack_writer {
            let bytes_written = result.map_err(|e| Error::Pack(format!("Pack streaming failed: {}", e)))?;
            total_bytes_written += bytes_written;
        }
        let streaming_duration = streaming_start.elapsed();
        eprintln!(
            "Pack streaming timing: Actual pack streaming took {:?}",
            streaming_duration
        );

        // Get the final pack digest
        let pack_digest = pack_writer
            .digest()
            .ok_or_else(|| Error::Pack("Pack generation incomplete".to_string()))?;

        eprintln!(
            "Debug: Pack generation complete - {} bytes written, digest: {}",
            total_bytes_written,
            pack_digest.to_hex()
        );

        Ok(PackGenerationStats {
            object_count: actual_count as u32,
            pack_size: total_bytes_written,
            delta_objects: entry_stats.delta_ref as u32,
            compression_ratio: 0.0,
        })
    }

    // Async support removed - stream_pack_data is now the only implementation

    /// Write an empty pack when no objects need to be sent
    fn write_empty_pack<W: Write>(&self, mut writer: W, session: &SessionContext) -> Result<PackStats> {
        // Write empty pack: header + no entries + checksum
        let empty_entries: Vec<output::Entry> = Vec::new();
        let entries_iter = std::iter::once(Ok(empty_entries));

        // Create formatter for empty pack writing (for proper sideband handling)
        let formatter = ResponseFormatter::new_with_progress_control(
            &mut writer,
            session.capabilities.side_band,
            session.capabilities.no_progress,
        );

        let mut pack_writer = output::bytes::FromEntriesIter::new(
            entries_iter,
            formatter,
            0,
            gix_pack::data::Version::V2,
            self.repository.object_hash(),
        );

        // Write the empty pack
        for result in &mut pack_writer {
            result.map_err(|e: gix_pack::data::output::bytes::Error<std::convert::Infallible>| {
                Error::Pack(format!("Empty pack generation failed: {}", e))
            })?;
        }

        Ok(PackStats {
            object_count: 0,
            pack_size: 32, // Empty pack size (header + checksum)
            delta_objects: 0,
            compression_ratio: 1.0,
            generation_time_ms: 0,
        })
    }

    // Helper methods (similar to our existing implementation)

    /// Process a batch of commits with tree caching for maximum performance
    fn process_commit_batch_cached(
        &self,
        commit_ids: &[gix_hash::ObjectId],
        objects: &mut HashSet<gix_hash::ObjectId>,
        tree_cache: &mut std::collections::HashMap<gix_hash::ObjectId, Vec<gix_hash::ObjectId>>,
    ) -> Result<()> {
        // Reuse buffer across all commits in the batch
        let mut buf = self.repository.empty_reusable_buffer();

        for &commit_id in commit_ids {
            // Add the commit itself
            objects.insert(commit_id);

            // Get the commit object efficiently with shared buffer
            let commit = self
                .repository
                .find_commit(commit_id)
                .map_err(|e| Error::custom(format!("Failed to find commit: {}", e)))?;

            // Add the tree and all its contents with caching
            let tree_id = commit
                .tree_id()
                .map_err(|e| Error::custom(format!("Failed to get tree ID: {}", e)))?
                .detach();

            // Check cache first
            if let Some(cached_objects) = tree_cache.get(&tree_id) {
                // Use cached tree traversal result
                for &obj_id in cached_objects {
                    objects.insert(obj_id);
                }
            } else {
                // Traverse tree and cache the result
                let initial_size = objects.len();
                self.traverse_tree_optimized(tree_id, objects, &mut buf)?;

                // Cache the newly discovered objects for this tree
                let new_objects: Vec<_> = objects.iter().skip(initial_size).copied().collect();
                tree_cache.insert(tree_id, new_objects);
            }
        }

        Ok(())
    }

    /// Traverse a tree and collect all reachable objects using optimized gix-traverse
    fn traverse_tree(&self, tree_id: gix_hash::ObjectId, objects: &mut HashSet<gix_hash::ObjectId>) -> Result<()> {
        let mut buf = self.repository.empty_reusable_buffer();
        self.traverse_tree_optimized(tree_id, objects, &mut buf)
    }

    /// Optimized tree traversal with buffer reuse and early termination
    fn traverse_tree_optimized(
        &self,
        tree_id: gix_hash::ObjectId,
        objects: &mut HashSet<gix_hash::ObjectId>,
        buf: &mut Vec<u8>,
    ) -> Result<()> {
        // Skip if we've already processed this tree (common in Git histories)
        if !objects.insert(tree_id) {
            return Ok(()); // Already processed - this is a major optimization for Git repos with shared trees
        }

        // Get the raw tree data for gix-traverse with provided buffer
        buf.clear(); // Reuse the buffer
        let tree_data = {
            use gix_object::Find;
            self.repository
                .try_find(&tree_id, buf)
                .map_err(|e| Error::custom(format!("Failed to find tree: {}", e)))?
                .ok_or_else(|| Error::custom("Tree not found".to_string()))?
        };

        if tree_data.kind != gix_object::Kind::Tree {
            return Err(Error::custom("Object is not a tree".to_string()));
        }

        let tree_iter = gix_object::TreeRefIter::from_bytes(tree_data.data);

        // Use optimized traversal with pre-allocated recorder
        let mut recorder = gix_traverse::tree::Recorder::default();

        // Use repository's optimized traversal with shared buffer management
        gix_traverse::tree::breadthfirst(
            tree_iter,
            gix_traverse::tree::breadthfirst::State::default(),
            self.repository, // Repository implements Find trait efficiently
            &mut recorder,
        )
        .map_err(|e| Error::custom(format!("Tree traversal failed: {}", e)))?;

        // Add all discovered objects to our set efficiently
        objects.reserve(recorder.records.len()); // Pre-allocate space
        for record in recorder.records {
            objects.insert(record.oid.into());
        }

        Ok(())
    }

    fn apply_object_filter(&self, objects: Vec<gix_hash::ObjectId>, filter: &BStr) -> Result<Vec<gix_hash::ObjectId>> {
        // Optimized filter implementation using efficient object header queries
        let filter_str = filter.to_str_lossy();

        if filter_str.starts_with("blob:none") {
            // Filter out all blobs using efficient header-only queries with batching
            let mut filtered = Vec::with_capacity(objects.len() / 2); // Estimate fewer objects after filtering
            let mut buf = self.repository.empty_reusable_buffer(); // Reuse buffer

            for oid in objects {
                // Use header query instead of full object loading for efficiency
                use gix_object::FindHeader;
                match self.repository.try_header(&oid) {
                    Ok(Some(header)) if header.kind != gix_object::Kind::Blob => {
                        filtered.push(oid);
                    }
                    _ => {} // Skip blobs and objects we can't read
                }
            }
            Ok(filtered)
        } else {
            // Return all objects for unsupported filters
            Ok(objects)
        }
    }

    /// Filter out objects that the client already has
    fn filter_existing_objects(
        &self,
        counts: Vec<output::Count>,
        session: &SessionContext,
    ) -> Result<Vec<output::Count>> {
        // If no haves/common, no filtering needed
        if session.negotiation.haves.is_empty() && session.negotiation.common.is_empty() {
            return Ok(counts);
        }

        // Build a set of all objects reachable from haves and common
        let mut existing_objects = std::collections::HashSet::new();

        // Add haves and common directly
        for have in &session.negotiation.haves {
            existing_objects.insert(*have);
        }
        for common in &session.negotiation.common {
            existing_objects.insert(*common);
        }

        // For each have/common that's a commit, add all reachable objects
        let mut to_traverse = Vec::new();
        to_traverse.extend(session.negotiation.haves.iter());
        to_traverse.extend(session.negotiation.common.iter());

        for &obj_id in &to_traverse {
            // If it's a commit, traverse its history
            if let Ok(commit) = self.repository.find_commit(obj_id) {
                // Add the commit's tree and all its contents
                if let Ok(tree_id) = commit.tree_id() {
                    self.collect_tree_objects(tree_id.detach(), &mut existing_objects)?;
                }
            }
            // If it's a tree, traverse it
            else if let Ok(_tree) = self.repository.find_tree(obj_id) {
                self.collect_tree_objects(obj_id, &mut existing_objects)?;
            }
            // For other objects, just add them
            else {
                existing_objects.insert(obj_id);
            }
        }

        eprintln!(
            "Filter objects: Excluding {} existing objects from {} total",
            existing_objects.len(),
            counts.len()
        );

        // Filter out existing objects
        let filtered: Vec<_> = counts
            .into_iter()
            .filter(|count| !existing_objects.contains(&count.id))
            .collect();

        eprintln!("Filter objects: Kept {} objects after filtering", filtered.len());

        Ok(filtered)
    }

    /// Collect all objects reachable from a tree
    fn collect_tree_objects(
        &self,
        tree_id: gix_hash::ObjectId,
        objects: &mut std::collections::HashSet<gix_hash::ObjectId>,
    ) -> Result<()> {
        // Skip if already processed
        if !objects.insert(tree_id) {
            return Ok(());
        }

        // Use gix-traverse to efficiently collect all tree contents
        let mut buf = self.repository.empty_reusable_buffer();
        let tree_data = {
            use gix_object::Find;
            self.repository
                .try_find(&tree_id, &mut buf)
                .map_err(|e| Error::custom(format!("Failed to find tree: {}", e)))?
                .ok_or_else(|| Error::custom("Tree not found".to_string()))?
        };

        if tree_data.kind != gix_object::Kind::Tree {
            return Ok(());
        }

        let tree_iter = gix_object::TreeRefIter::from_bytes(tree_data.data);
        let mut recorder = gix_traverse::tree::Recorder::default();

        gix_traverse::tree::breadthfirst(
            tree_iter,
            gix_traverse::tree::breadthfirst::State::default(),
            self.repository,
            &mut recorder,
        )
        .map_err(|e| Error::custom(format!("Tree traversal failed: {}", e)))?;

        // Add all discovered objects
        for record in recorder.records {
            objects.insert(record.oid.into());
        }

        Ok(())
    }

    /// Send final status message compatible with Git
    fn send_final_status<W: Write>(
        &self,
        writer: &mut W,
        stats: &PackGenerationStats,
        session: &SessionContext,
    ) -> Result<()> {
        // Create formatter for status message (for proper sideband handling)
        let mut formatter = ResponseFormatter::new_with_progress_control(
            writer,
            session.capabilities.side_band,
            session.capabilities.no_progress,
        );

        // For now, we don't track delta stats like Git does, so we'll use approximations
        // Git sends: "Total X (delta Y), reused Z (delta W), pack-reused 0 (from 0)"
        let total = stats.object_count;
        let delta_count = stats.delta_objects; // This might be 0 for now

        let status_message = format!(
            "Total {} (delta {}), reused {} (delta {}), pack-reused 0 (from 0)",
            total, delta_count, total, delta_count
        );

        formatter.send_progress(&status_message)?;

        // Send final flush packet to indicate completion
        use gix_packetline::PacketLineRef;
        PacketLineRef::Flush.write_to(writer)?;

        Ok(())
    }
}

// Helper struct for internal statistics
struct PackGenerationStats {
    object_count: u32,
    pack_size: u64,
    delta_objects: u32,
    compression_ratio: f64,
}
