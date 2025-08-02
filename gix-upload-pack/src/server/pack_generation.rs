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
use gix::{objs::Kind, Repository};
use gix_features::{
    parallel,
    progress::{self},
};
use gix_pack::data::output;
use std::sync::atomic::AtomicBool;
use std::{
    collections::{HashSet, VecDeque},
    io::Write,
};

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

impl<'a> PackGenerator<'a> {
    /// Create a new pack generator
    pub fn new(repository: &'a Repository, options: &'a ServerOptions) -> Self {
        Self { repository, options }
    }

    /// Generate a pack file using gix-pack infrastructure
    pub fn generate_pack<W: Write>(&self, mut writer: W, session: &SessionContext) -> Result<PackStats> {
        let start_time = std::time::Instant::now();

        eprintln!("DEBUG: generate_pack() - Starting pack generation");
        
        // Step 1: Collect object IDs that need to be packed with progress reporting
        eprintln!("DEBUG: generate_pack() - About to call enumerate()");
        let object_ids = self.enumerate(&mut writer, session)?;
        eprintln!("DEBUG: generate_pack() - enumerate() returned {} objects", object_ids.len());

        if object_ids.is_empty() {
            // Return empty pack
            return self.write_empty_pack(writer, session);
        }

        // Step 2: Use gix-pack's count::objects to analyze the objects
        eprintln!("DEBUG: generate_pack() - About to call count_objects()");
        let (counts, count_stats) = self.count_objects(object_ids, &mut writer, session)?;
        eprintln!(
            "Debug: Object counting complete - {} total objects",
            count_stats.total_objects
        );

        // Step 3: Compress and stream pack data using gix-pack's FromEntriesIter
        eprintln!("DEBUG: generate_pack() - About to call stream_pack_data()");
        let pack_stats = self.stream_pack_data(&mut writer, counts, count_stats.total_objects, session)?;

        // Step 5: Send final status message (Git-compatible)
        eprintln!("DEBUG: generate_pack() - About to call send_final_status()");
        self.send_final_status(&mut writer, &pack_stats, session)?;

        let generation_time = start_time.elapsed();

        eprintln!("DEBUG: generate_pack() - Completed pack generation");
        Ok(PackStats {
            object_count: pack_stats.object_count,
            pack_size: pack_stats.pack_size,
            delta_objects: pack_stats.delta_objects,
            compression_ratio: pack_stats.compression_ratio,
            generation_time_ms: generation_time.as_millis() as u64,
        })
    }

    /// Collect object IDs using our existing traversal logic with progress reporting
    fn enumerate<W: Write>(
        &self,
        writer: &mut W,
        session: &SessionContext,
    ) -> Result<Vec<gix_hash::ObjectId>> {
        eprintln!("DEBUG: Starting enumerate() method");
        
        // Create formatter for progress reporting
        let mut formatter = ResponseFormatter::new_with_progress_control(
            writer,
            session.capabilities.side_band,
            session.capabilities.no_progress,
        );

        // Start progress reporting for object collection
        let mut progress_reporter = ProgressReporter::new(
            &mut formatter,
            "Enumerating objects".to_string(),
            None, // We don't know total count upfront
        );

        // Use our existing object collection logic but just return the IDs
        let mut objects = HashSet::new();
        let mut visited = HashSet::new();

        // Start from wanted objects
        let mut update_counter = 0;
        const UPDATE_INTERVAL: usize = 1000; // Only check for progress every 1000 objects
        
        for want in &session.negotiation.wants {
            if !session.negotiation.common.contains(want) && !session.negotiation.haves.contains(want) {
                if self.repository.find_object(*want).is_ok() {
                    self.traverse_from_object(*want, &mut objects, &mut visited, session)?;
                    
                    // Only update progress occasionally to avoid performance overhead
                    update_counter += 1;
                    if update_counter % UPDATE_INTERVAL == 0 {
                        eprintln!("DEBUG: enumerate() - calling progress_reporter.update({}) [batched every {} objects]", objects.len(), UPDATE_INTERVAL);
                        progress_reporter.update(objects.len())?;
                    }
                }
            }
        }
        
        // Final update to ensure we report the final count
        eprintln!("DEBUG: enumerate() - final progress_reporter.update({})", objects.len());
        progress_reporter.update(objects.len())?;

        eprintln!("DEBUG: enumerate() - calling progress_reporter.finish()");
        progress_reporter.finish()?;

        // Apply filters if needed
        let mut filtered_objects: Vec<_> = objects.into_iter().collect();

        // Apply object filter if specified
        if let Some(filter) = &session.capabilities.filter {
            filtered_objects = self.apply_object_filter(filtered_objects, filter.as_ref())?;
        }

        eprintln!("DEBUG: enumerate() method completed with {} objects", filtered_objects.len());
        Ok(filtered_objects)
    }

    /// Use gix-pack's count::objects for intelligent object analysis with progress reporting
    fn count_objects<W: Write>(
        &self,
        object_ids: Vec<gix_hash::ObjectId>,
        writer: &mut W,
        session: &SessionContext,
    ) -> Result<(Vec<output::Count>, output::count::objects::Outcome)> {
        // Set up sideband progress reporting
        let mut formatter = ResponseFormatter::new_with_progress_control(
            writer,
            session.capabilities.side_band,
            session.capabilities.no_progress,
        );
        let mut progress_reporter =
            ProgressReporter::new(&mut formatter, "Counting objects".to_string(), Some(object_ids.len()));

        // Start the gix-pack counting in a separate thread
        let find_adapter = RepositoryFindAdapter::new(self.repository);
        let objects_iter = object_ids.into_iter().map(Ok);

        let (counts, stats) = output::count::objects(
            find_adapter,
            Box::new(objects_iter),
            &progress::Discard,
            &AtomicBool::new(false),
            output::count::objects::Options {
                input_object_expansion: output::count::objects::ObjectExpansion::TreeContents,
                thread_limit: Some(std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)),
                chunk_size: 10,
                ..Default::default()
            },
        ).map_err(|e| {
            Error::Pack(format!("Object counting failed: {}", e))
        })?;

        let _actual_count = counts.iter().fold(ObjectCount::default(), |mut c, _e| {
            c.add(gix_pack::data::output::entry::Kind::Base(gix_object::Kind::Blob));
            let _ = progress_reporter.update(c.total());

            c
        });

        // Send final completion message (Git-style)
        progress_reporter.finish()?;

        // Report final completion
        eprintln!(
            "Progress: Object counting completed - {} total objects",
            stats.total_objects
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
        let find_adapter = RepositoryFindAdapter::new(self.repository);

        let mut entries_iter = output::entry::iter_from_counts(
            counts,
            find_adapter,
            Box::new(progress::Discard),
            output::entry::iter_from_counts::Options {
                allow_thin_pack: session.capabilities.thin_pack,
                ..Default::default()
            },
        );

        // Use InOrderIter to properly sort the parallel chunks by sequence ID, following the example
        let entries: Vec<_> = parallel::InOrderIter::from(entries_iter.by_ref())
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| Error::Pack(format!("Entry generation failed: {}", e)))?
            .into_iter()
            .flatten()
            .collect();

        let actual_count = entries.len();

        let mut formatter = ResponseFormatter::new_with_progress_control(
            &mut writer,
            session.capabilities.side_band,
            session.capabilities.no_progress,
        );

        // send compressing status to sideband (this is the compression/writing phase)
        let mut progress_reporter = ProgressReporter::new(
            &mut formatter,
            "Compressing objects".to_string(),
            Some(total_objects),
        );

        // Count entry types for debugging, following the example pattern
        let entry_stats = entries.iter().fold(ObjectCount::default(), |mut c, e| {
            c.add(e.kind);
            progress_reporter.update(c.total()).unwrap_or(());
            c
        });

        eprintln!("Debug: Entry stats: {:?}", entry_stats);
        progress_reporter.finish()?;

        // Create formatter for pack data writing (for proper sideband handling)  
        let formatter = ResponseFormatter::new_with_progress_control(
            &mut writer,
            session.capabilities.side_band,
            session.capabilities.no_progress,
        );

        // Use gix-pack's streaming pack writer with the formatter, following the example pattern
        let mut pack_writer = output::bytes::FromEntriesIter::new(
            std::iter::once(Ok::<_, output::entry::iter_from_counts::Error>(entries)),
            formatter,
            actual_count as u32,
            gix_pack::data::Version::V2,
            self.repository.object_hash(),
        );

        let mut total_bytes_written = 0u64;

        // Stream the pack data, following the example pattern
        for result in &mut pack_writer {
            let bytes_written = result.map_err(|e| {
                Error::Pack(format!("Pack streaming failed: {}", e))
            })?;
            total_bytes_written += bytes_written;
        }

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

    fn traverse_from_object(
        &self,
        start_oid: gix_hash::ObjectId,
        objects: &mut HashSet<gix_hash::ObjectId>,
        visited: &mut HashSet<gix_hash::ObjectId>,
        session: &SessionContext,
    ) -> Result<()> {
        let mut queue = VecDeque::new();
        queue.push_back(start_oid);

        while let Some(oid) = queue.pop_front() {
            if visited.contains(&oid) {
                continue;
            }

            visited.insert(oid);

            // Stop if this is a common object
            if session.negotiation.common.contains(&oid) || session.negotiation.haves.contains(&oid) {
                continue;
            }

            // Add object to pack
            objects.insert(oid);

            // Traverse children based on object type
            let obj = match self.repository.find_object(oid) {
                Ok(obj) => obj,
                Err(_) => continue, // Skip missing objects
            };

            match obj.kind {
                Kind::Commit => {
                    let commit = obj
                        .try_into_commit()
                        .map_err(|e| Error::custom(format!("Failed to parse commit: {}", e)))?;

                    // Add tree
                    queue.push_back(commit.tree()?.id().into());

                    // Add parents
                    for parent_id in commit.parent_ids() {
                        queue.push_back(parent_id.detach());
                    }
                }
                Kind::Tree => {
                    let tree = obj
                        .try_into_tree()
                        .map_err(|e| Error::custom(format!("Failed to parse tree: {}", e)))?;

                    // Add all tree entries
                    for entry in tree.iter() {
                        queue.push_back(entry?.oid().into());
                    }
                }
                Kind::Tag => {
                    let tag = obj
                        .try_into_tag()
                        .map_err(|e| Error::custom(format!("Failed to parse tag: {}", e)))?;

                    // Add tagged object
                    if let Ok(target) = tag.target_id() {
                        queue.push_back(target.detach());
                    }
                }
                Kind::Blob => {
                    // Blobs have no children
                }
            }
        }

        Ok(())
    }

    fn apply_object_filter(&self, objects: Vec<gix_hash::ObjectId>, filter: &BStr) -> Result<Vec<gix_hash::ObjectId>> {
        // Simple filter implementation - can be enhanced
        let filter_str = filter.to_str_lossy();

        if filter_str.starts_with("blob:none") {
            // Filter out all blobs
            let filtered: Result<Vec<_>> = objects
                .into_iter()
                .filter_map(|oid| match self.repository.find_object(oid) {
                    Ok(obj) => {
                        if obj.kind != Kind::Blob {
                            Some(Ok(oid))
                        } else {
                            None
                        }
                    }
                    Err(e) => Some(Err(Error::custom(format!("Failed to find object: {}", e)))),
                })
                .collect();
            filtered
        } else {
            // Return all objects for unsupported filters
            Ok(objects)
        }
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
