//! Advanced pack file generation using gix-pack infrastructure
//!
//! This module replaces our manual pack generation with the sophisticated
//! gix-pack system, providing delta compression, streaming output, and
//! better performance.

use crate::{
    config::ServerOptions,
    error::{Error, Result},
    services::pack::ProgressReporter,
    services::packet_io::EnhancedPacketWriter,
    types::*,
};
use gix::Repository;
use gix_features::{
    parallel,
    progress::{self},
};
use gix_pack::data::output;
use std::io::Write;
use std::sync::atomic::AtomicBool;

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
    // Constructor is handled by PackGenerator::create_optimized_find_adapter
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
}

/// Git-native pack configuration values for compatibility
#[derive(Debug, Clone)]
struct PackConfig {
    threads: usize,
    window: usize,
}

impl<'a> PackGenerator<'a> {
    /// Create a new pack generator
    pub fn new(repository: &'a Repository, _options: &'a ServerOptions) -> Self {
        Self { repository }
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
        }
    }

    /// Generate a pack file using EnhancedPacketWriter for proper sideband handling
    pub fn generate_pack<W: Write>(
        &self,
        writer: &mut EnhancedPacketWriter<W>,
        session: &SessionContext,
    ) -> Result<PackStats> {
        let object_ids = self.prepare_minimal_objects(session)?;

        if object_ids.is_empty() {
            // Return empty pack
            return self.write_empty_pack(writer.inner_mut(), session);
        }

        // Step 2: Use gix-pack's count::objects to analyze and expand the objects
        // This replaces our manual enumeration - gix-pack will do tree traversal for us
        let (counts, count_stats) = self.count_objects_with_expansion(object_ids, writer, session)?;

        // Step 3: Compress and stream pack data using gix-pack's FromEntriesIter
        let pack_stats = self.stream_pack_data(writer, counts, count_stats.total_objects, session)?;

        // Step 4: Send final status message (Git-compatible)
        self.send_final_status(writer, &pack_stats, session)?;

        Ok(PackStats {
            object_count: pack_stats.object_count,
            pack_size: pack_stats.pack_size,
            delta_objects: pack_stats.delta_objects,
            compression_ratio: pack_stats.compression_ratio,
        })
    }

    /// Prepare objects using optimized commit traversal
    fn prepare_minimal_objects(&self, session: &SessionContext) -> Result<Vec<gix_hash::ObjectId>> {
        let prepare_start: std::time::Instant = std::time::Instant::now();

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
        writer: &mut EnhancedPacketWriter<W>,
        session: &SessionContext,
    ) -> Result<(Vec<output::Count>, output::count::objects::Outcome)> {
        let count_start = std::time::Instant::now();

        // Progress reporting will be handled directly through EnhancedPacketWriter

        // Start the gix-pack counting with optimized adapter and Git-native configuration
        let find_adapter = self.create_optimized_find_adapter();
        let pack_config = self.get_pack_config();

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
        // Send progress message if progress is enabled
        if !session.capabilities.no_progress {
            writer.send_progress(&format!("Enumerating objects: {}, done.", stats.total_objects))?;
        }

        let mut progress_reporter =
            ProgressReporter::new(writer, "Counting objects".to_string(), Some(stats.total_objects));

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

    /// Stream pack data using gix-pack's FromEntriesIter
    fn stream_pack_data<W: Write>(
        &self,
        writer: &mut EnhancedPacketWriter<W>,
        counts: Vec<output::Count>,
        total_objects: usize,
        session: &SessionContext,
    ) -> Result<PackGenerationStats> {
        let find_adapter = self.create_optimized_find_adapter();
        let pack_config = self.get_pack_config();

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

        // send compressing status to sideband (this is the compression/writing phase)
        let mut progress_reporter =
            ProgressReporter::new(writer, "Compressing objects".to_string(), Some(total_objects));

        // Count entry types for debugging, following the example pattern
        let entry_stats = entries.iter().fold(ObjectCount::default(), |mut c, e| {
            c.add(e.kind);
            progress_reporter.update(c.total()).unwrap_or(());
            c
        });

        progress_reporter.finish()?;

        // CRITICAL FIX: Use a temporary buffer to collect all pack data first,
        // then write it in properly sized sideband packets
        let pack_writer_start = std::time::Instant::now();

        // Write pack data to a temporary buffer first
        let mut pack_buffer = Vec::new();
        let mut pack_writer = output::bytes::FromEntriesIter::new(
            std::iter::once(Ok::<_, output::entry::iter_from_counts::Error>(entries)),
            &mut pack_buffer,
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

        // Stream the pack data to the buffer first
        let streaming_start = std::time::Instant::now();
        for result in &mut pack_writer {
            let bytes_written = result.map_err(|e| Error::Pack(format!("Pack streaming failed: {}", e)))?;
            total_bytes_written += bytes_written;
        }
        let streaming_duration = streaming_start.elapsed();

        // Get the final pack digest before we use the buffer
        let pack_digest = pack_writer
            .digest()
            .ok_or_else(|| Error::Pack("Pack generation incomplete".to_string()))?;

        eprintln!(
            "Pack streaming timing: Pack data generation took {:?}, {} bytes",
            streaming_duration,
            pack_buffer.len()
        );

        // Now write the complete pack data through the sideband writer in proper chunks
        let sideband_start = std::time::Instant::now();
        writer.send_data(&pack_buffer)?;
        let sideband_duration = sideband_start.elapsed();
        eprintln!(
            "Pack streaming timing: Sideband transmission took {:?}",
            sideband_duration
        );

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
    fn write_empty_pack<W: Write>(&self, writer: W, _session: &SessionContext) -> Result<PackStats> {
        // Write empty pack: header + no entries + checksum
        let empty_entries: Vec<output::Entry> = Vec::new();
        let entries_iter = std::iter::once(Ok(empty_entries));

        let mut pack_writer = output::bytes::FromEntriesIter::new(
            entries_iter,
            writer,
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
        })
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
        writer: &mut EnhancedPacketWriter<W>,
        stats: &PackGenerationStats,
        _session: &SessionContext,
    ) -> Result<()> {
        // For now, we don't track delta stats like Git does, so we'll use approximations
        // Git sends: "Total X (delta Y), reused Z (delta W), pack-reused 0 (from 0)"
        let total = stats.object_count;
        let delta_count = stats.delta_objects; // This might be 0 for now

        let status_message = format!(
            "Total {} (delta {}), reused {} (delta {}), pack-reused 0 (from 0)",
            total, delta_count, total, delta_count
        );

        writer.send_progress(&status_message)?;

        // Send final flush packet to indicate completion
        writer.write_flush()?;

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
