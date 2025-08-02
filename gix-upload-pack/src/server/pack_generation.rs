//! Advanced pack file generation using gix-pack infrastructure
//!
//! This module replaces our manual pack generation with the sophisticated
//! gix-pack system, providing delta compression, streaming output, and
//! better performance.

use crate::{
    config::ServerOptions,
    error::{Error, Result},
    types::*,
    server::response::{ProgressReporter, ResponseFormatter},
};
use bstr::{BStr, ByteSlice};
use gix::{Repository, objs::Kind};
use gix_pack::data::output;
use gix_features::{progress::{self, Count}, parallel};
use std::{
    collections::{HashSet, VecDeque},
    io::Write,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// Adapter to make Repository objects compatible with gix_pack::Find trait
#[derive(Clone)]
struct RepositoryFindAdapter {
    objects: gix::odb::Handle,
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
    ) -> std::result::Result<Option<(gix_object::Data<'a>, Option<gix_pack::data::entry::Location>)>, Box<dyn std::error::Error + Send + Sync + 'static>> {
        // Delegate to the Handle's implementation 
        self.objects.try_find_cached(id, buffer, pack_cache)
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
    pub fn generate_pack<W: Write>(
        &self,
        mut writer: W,
        session: &SessionContext,
    ) -> Result<PackStats> {
        let start_time = std::time::Instant::now();
        
        // Step 1: Collect object IDs that need to be packed with progress reporting
        let object_ids = self.collect_object_ids(&mut writer, session)?;
        eprintln!("Debug: Collected {} objects for advanced packing", object_ids.len());
        
        if object_ids.is_empty() {
            // Return empty pack
            return self.write_empty_pack(writer);
        }
        
        // Step 2: Use gix-pack's count::objects to analyze the objects
        let (counts, count_stats) = self.count_objects(object_ids, &mut writer, session)?;
        eprintln!("Debug: Object counting complete - {} total objects", count_stats.total_objects);
        
        // Step 3: Convert counts to pack entries using gix-pack
        let entries_iter = self.create_entries_iterator(counts, session)?;

        // Step 4: Stream pack data using gix-pack's FromEntriesIter
        let pack_stats = self.stream_pack_data(writer, entries_iter, count_stats.total_objects, session)?;
        
        let generation_time = start_time.elapsed();
        
        Ok(PackStats {
            object_count: pack_stats.object_count,
            pack_size: pack_stats.pack_size,
            delta_objects: pack_stats.delta_objects,
            compression_ratio: pack_stats.compression_ratio,
            generation_time_ms: generation_time.as_millis() as u64,
        })
    }
    
    /// Collect object IDs using our existing traversal logic with progress reporting
    fn collect_object_ids<W: Write>(&self, writer: &mut W, session: &SessionContext) -> Result<Vec<gix_hash::ObjectId>> {
        // Create formatter for progress reporting
        let mut formatter = ResponseFormatter::new(
            writer,
            session.capabilities.side_band,
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
        for want in &session.negotiation.wants {
            if !session.negotiation.common.contains(want) && !session.negotiation.haves.contains(want) {
                if self.repository.find_object(*want).is_ok() {
                    self.traverse_from_object(*want, &mut objects, &mut visited, session)?;
                    progress_reporter.update(objects.len())?;
                }
            }
        }
        
        progress_reporter.finish()?;
        
        // Apply filters if needed
        let mut filtered_objects: Vec<_> = objects.into_iter().collect();
        
        // Apply object filter if specified
        if let Some(filter) = &session.capabilities.filter {
            filtered_objects = self.apply_object_filter(filtered_objects, filter.as_ref())?;
        }
        
        Ok(filtered_objects)
    }
    
    /// Use gix-pack's count::objects for intelligent object analysis with progress reporting
    fn count_objects<W: Write>(
        &self,
        object_ids: Vec<gix_hash::ObjectId>,
        writer: &mut W,
        session: &SessionContext,
    ) -> Result<(Vec<output::Count>, output::count::objects::Outcome)> {
        
        // Create a Discard progress that implements Count trait
        let console_progress = progress::Discard;
        
        // Get the shared counter that gix-pack will actually increment
        let shared_counter = console_progress.counter();
        
        // Set up sideband progress reporting
        let mut formatter = ResponseFormatter::new(
            writer,
            session.capabilities.side_band,
        );
        let mut progress_reporter = ProgressReporter::new(
            &mut formatter,
            "Counting objects".to_string(),
            Some(object_ids.len()),
        );
        progress_reporter.update(0)?;
        
        // Start the gix-pack counting in a separate thread
        let counting_complete = Arc::new(AtomicBool::new(false));
        let find_adapter = RepositoryFindAdapter::new(self.repository);
        let objects_iter = object_ids.into_iter().map(Ok);
        let counting_complete_for_gix = counting_complete.clone();
        
        let gix_thread = thread::spawn(move || {
            let result = output::count::objects(
                find_adapter,
                Box::new(objects_iter),
                &console_progress,
                &AtomicBool::new(false),
                output::count::objects::Options {
                    input_object_expansion: output::count::objects::ObjectExpansion::TreeContents,
                    thread_limit: Some(std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)),
                    chunk_size: 1000,
                    ..Default::default()
                },
            );
            
            counting_complete_for_gix.store(true, Ordering::Relaxed);
            result
        });
        
        // Main thread: send periodic progress updates to sideband while counting is running
        let mut last_reported = 0usize;
        let mut last_update_time = std::time::Instant::now();
        
        while !counting_complete.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(1)); // Check every 1ms for fast updates
            
            let current_count = shared_counter.load(Ordering::Relaxed);
            let now = std::time::Instant::now();
            
            // Send progress update if count increased significantly or time passed
            if current_count != last_reported && 
               (current_count / 1000 > last_reported / 1000 || 
                now.duration_since(last_update_time).as_millis() >= 500) {
                
                // Log to console
                eprintln!("Incremental: Counted {} objects so far...", current_count);
                
                // Send to sideband
                if let Err(e) = progress_reporter.update(current_count) {
                    eprintln!("Warning: Failed to send progress update: {}", e);
                }
                
                last_reported = current_count;
                last_update_time = now;
            }
        }
        
        // Wait for the counting thread to complete
        let (counts, stats) = gix_thread.join()
            .unwrap_or_else(|e| {
                eprintln!("GIX counting thread panicked: {:?}", e);
                Err(output::count::objects::Error::Interrupted)
            })
            .map_err(|e| Error::Pack(format!("Object counting failed: {}", e)))?;
        
        // Report final completion
        eprintln!("Progress: Object counting completed - {} total objects", stats.total_objects);
        
        Ok((counts, stats))
    }
    
    /// Create entries iterator using gix-pack's advanced entry generation
    fn create_entries_iterator(
        &self,
        counts: Vec<output::Count>,
        session: &SessionContext,
    ) -> Result<impl Iterator<Item = std::result::Result<(usize, Vec<output::Entry>), output::entry::iter_from_counts::Error>>> {
        // Use gix-pack's iter_from_counts for sophisticated entry generation
        let find_adapter = RepositoryFindAdapter::new(self.repository);
        let entries_iter = output::entry::iter_from_counts(
            counts,
            find_adapter,
            Box::new(progress::Discard),
            output::entry::iter_from_counts::Options {
                version: gix_pack::data::Version::V2,
                mode: output::entry::iter_from_counts::Mode::PackCopyAndBaseObjects,
                allow_thin_pack: session.capabilities.thin_pack, // Respect client's thin-pack capability
                thread_limit: Some(std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)),
                chunk_size: 1000,
            },
        );
        
        // If client doesn't support OFS deltas, we need to post-process entries
        if !session.capabilities.ofs_delta {
            eprintln!("Debug: Client doesn't support OFS deltas - entries will be converted to base objects");
            // TODO: Implement OFS delta conversion wrapper
        }
        
        Ok(entries_iter)
    }
    
    /// Stream pack data using gix-pack's FromEntriesIter
    fn stream_pack_data<W: Write, I, E>(
        &self,
        mut writer: W,
        entries_iter: I,
        total_objects: usize,
        session: &SessionContext,
    ) -> Result<PackGenerationStats>
    where
        I: Iterator<Item = std::result::Result<(usize, Vec<output::Entry>), E>>,
        E: std::error::Error + 'static,
    {
        // Use InOrderIter to properly sort the parallel chunks by sequence ID
        let sorted_iter = parallel::InOrderIter::from(entries_iter);

        // First phase: collect all entries with progress reporting
        let (all_entries, actual_count) = {
            let mut formatter = ResponseFormatter::new(
                &mut writer,
                session.capabilities.side_band,
            );

            // send compressing status to sideband (this is the compression/writing phase)
            let mut progress_reporter = ProgressReporter::new(
                &mut formatter,
                "Compressing objects".to_string(),
                Some(total_objects), // total_objects is already usize
            );
            
            // Now collect all entries in the correct order
            let mut batch_count = 0;
            let mut total_entries = 0;
            let mut all_entries = Vec::new();
            
            for result in sorted_iter {
                let entries = result
                    .map_err(|e| Error::Pack(format!("Entry generation failed: {}", e)))?;
                
                batch_count += 1;
                total_entries += entries.len();
                progress_reporter.update(total_entries)?;
                
                eprintln!("Debug: Sorted Batch {}: {} entries", batch_count, entries.len());
                
                // Check for invalid entries or weird indices
                for (i, entry) in entries.iter().enumerate() {
                    if entry.is_invalid() {
                        eprintln!("Debug: Found invalid entry at batch {}, index {}", batch_count, i);
                    }
                }

                all_entries.extend(entries);
            }

            progress_reporter.finish()?;
            
            eprintln!("Debug: Total batches: {}, Total entries collected: {}, Expected: {}", 
                      batch_count, total_entries, total_objects);

            let actual_count = all_entries.len() as u32;
            eprintln!("Debug: Using actual count {} for pack generation", actual_count);
            
            // Progress reporter and formatter will be dropped here, releasing the borrow on writer
            (all_entries, actual_count)
        };

        // Second phase: create pack writer with the now-available writer
        // Create iterator that yields a single batch of all entries
        let entries_iter = std::iter::once(Ok(all_entries));

        // Use gix-pack's streaming pack writer with correct count
        let mut pack_writer = output::bytes::FromEntriesIter::new(
            entries_iter,
            writer,
            actual_count,
            gix_pack::data::Version::V2,
            self.repository.object_hash(),
        );
        
        let mut total_bytes_written = 0u64;
        
        // Stream the pack data (no progress reporting during this phase to avoid borrow conflicts)
        for result in &mut pack_writer {
            let bytes_written = result
                .map_err(|e: gix_pack::data::output::bytes::Error<std::convert::Infallible>| Error::Pack(format!("Pack streaming failed: {}", e)))?;
            total_bytes_written += bytes_written;
        }
        
        // Get the final pack digest
        let pack_digest = pack_writer.digest()
            .ok_or_else(|| Error::Pack("Pack generation incomplete".to_string()))?;
        
        eprintln!("Debug: Pack generation complete - {} bytes written, digest: {}", 
                  total_bytes_written, pack_digest.to_hex());
        
        Ok(PackGenerationStats {
            object_count: actual_count,
            pack_size: total_bytes_written,
            delta_objects: 0,
            compression_ratio: 0.0,
        })
    }

    // Async support removed - stream_pack_data is now the only implementation
    
    /// Write an empty pack when no objects need to be sent
    fn write_empty_pack<W: Write>(&self, writer: W) -> Result<PackStats> {
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
            result.map_err(|e: gix_pack::data::output::bytes::Error<std::convert::Infallible>| Error::Pack(format!("Empty pack generation failed: {}", e)))?;
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
                    let commit = obj.try_into_commit()
                        .map_err(|e| Error::custom(format!("Failed to parse commit: {}", e)))?;
                    
                    // Add tree
                    queue.push_back(commit.tree()?.id().into());
                    
                    // Add parents
                    for parent_id in commit.parent_ids() {
                        queue.push_back(parent_id.detach());
                    }
                }
                Kind::Tree => {
                    let tree = obj.try_into_tree()
                        .map_err(|e| Error::custom(format!("Failed to parse tree: {}", e)))?;
                    
                    // Add all tree entries
                    for entry in tree.iter() {
                        queue.push_back(entry?.oid().into());
                    }
                }
                Kind::Tag => {
                    let tag = obj.try_into_tag()
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
    
    fn apply_object_filter(
        &self,
        objects: Vec<gix_hash::ObjectId>,
        filter: &BStr,
    ) -> Result<Vec<gix_hash::ObjectId>> {
        // Simple filter implementation - can be enhanced
        let filter_str = filter.to_str_lossy();
        
        if filter_str.starts_with("blob:none") {
            // Filter out all blobs
            let filtered: Result<Vec<_>> = objects
                .into_iter()
                .filter_map(|oid| {
                    match self.repository.find_object(oid) {
                        Ok(obj) => {
                            if obj.kind != Kind::Blob {
                                Some(Ok(oid))
                            } else {
                                None
                            }
                        }
                        Err(e) => Some(Err(Error::custom(format!("Failed to find object: {}", e)))),
                    }
                })
                .collect();
            filtered
        } else {
            // Return all objects for unsupported filters
            Ok(objects)
        }
    }
}

// Helper struct for internal statistics
struct PackGenerationStats {
    object_count: u32,
    pack_size: u64,
    delta_objects: u32,
    compression_ratio: f64,
}
