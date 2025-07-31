//! Pack file generation for upload-pack
//!
//! This module handles the generation of pack files to send to clients,
//! leveraging the existing gix-pack infrastructure for efficient pack creation.

use crate::{
    config::ServerOptions,
    error::{Error, Result},
    types::*,
};
use bstr::{BStr, BString, ByteSlice};
use gix::{Repository, objs::Kind};
use gix_object::FindExt;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::Write,
};

/// Simple pack entry for our implementation
#[derive(Debug, Clone)]
struct PackEntry {
    id: gix_hash::ObjectId,
    kind: gix_object::Kind,
    size: usize,
    compressed_data: Vec<u8>,
}

/// Pack generator for upload-pack operations
pub struct PackGenerator<'a> {
    repository: &'a Repository,
    options: &'a ServerOptions,
}

/// Object traversal context for pack generation
#[derive(Debug)]
struct TraversalContext {
    /// Objects to include in the pack
    wants: HashSet<gix_hash::ObjectId>,
    /// Objects the client already has
    haves: HashSet<gix_hash::ObjectId>,
    /// Common objects between client and server
    common: HashSet<gix_hash::ObjectId>,
    /// Shallow commits for shallow clones
    shallow: HashSet<gix_hash::ObjectId>,
    /// Deepen specification
    deepen: Option<DeepenSpec>,
    /// Whether to include tags
    include_tag: bool,
    /// Object filter specification
    filter: Option<BString>,
}

/// Statistics about pack generation
#[derive(Debug, Default)]
pub struct PackStats {
    /// Number of objects in the pack
    pub object_count: u32,
    /// Total size of the pack
    pub pack_size: u64,
    /// Number of commits included
    pub commit_count: u32,
    /// Number of trees included
    pub tree_count: u32,
    /// Number of blobs included
    pub blob_count: u32,
    /// Number of tags included
    pub tag_count: u32,
    /// Number of delta objects
    pub delta_count: u32,
}

impl<'a> PackGenerator<'a> {
    /// Create a new pack generator
    pub fn new(repository: &'a Repository, options: &'a ServerOptions) -> Self {
        Self { repository, options }
    }
    
    /// Generate a pack file for the given session
    pub fn generate_pack<W: Write>(
        &self,
        writer: &mut W,
        session: &SessionContext,
    ) -> Result<PackStats> {
        let context = TraversalContext {
            wants: session.negotiation.wants.clone(),
            haves: session.negotiation.haves.clone(),
            common: session.negotiation.common.clone(),
            shallow: session.negotiation.shallow.clone(),
            deepen: session.negotiation.deepen.clone(),
            include_tag: session.capabilities.include_tag,
            filter: session.capabilities.filter.clone(),
        };
        
        // Collect objects to pack
        let objects = self.collect_objects(&context)?;
        
        // Generate the pack
        self.write_pack(writer, &objects, &context)
    }
    
    /// Collect all objects that need to be included in the pack
    fn collect_objects(&self, context: &TraversalContext) -> Result<Vec<gix_hash::ObjectId>> {
        let mut objects = HashSet::new();
        let mut visited = HashSet::new();
        
        // Start from wanted objects
        for want in &context.wants {
            if !context.common.contains(want) && !context.haves.contains(want) {
                self.traverse_from_object(*want, &mut objects, &mut visited, context)?;
            }
        }
        
        // Include tags if requested
        if context.include_tag {
            self.include_reachable_tags(&mut objects, &mut visited, context)?;
        }
        
        // Apply filters
        let mut filtered_objects: Vec<_> = objects.into_iter().collect();
        
        // Apply object filter if specified
        if let Some(filter) = &context.filter {
            filtered_objects = self.apply_object_filter(filtered_objects, filter.as_ref())?;
        }
        
        // Apply shallow/deepen constraints
        if let Some(ref deepen_spec) = context.deepen {
            filtered_objects = self.apply_depth_constraints(filtered_objects, deepen_spec)?;
        }
        
        // Sort objects for optimal delta compression
        self.sort_objects_for_packing(&mut filtered_objects)?;
        
        Ok(filtered_objects)
    }
    
    /// Traverse from a single object, collecting all reachable objects
    fn traverse_from_object(
        &self,
        start_oid: gix_hash::ObjectId,
        objects: &mut HashSet<gix_hash::ObjectId>,
        visited: &mut HashSet<gix_hash::ObjectId>,
        context: &TraversalContext,
    ) -> Result<()> {
        let mut queue = VecDeque::new();
        queue.push_back(start_oid);
        
        while let Some(oid) = queue.pop_front() {
            if visited.contains(&oid) {
                continue;
            }
            
            visited.insert(oid);
            
            // Stop if this is a common object
            if context.common.contains(&oid) || context.haves.contains(&oid) {
                continue;
            }
            
            // Stop at shallow commits
            if context.shallow.contains(&oid) {
                objects.insert(oid);
                continue;
            }
            
            // Add object to pack
            objects.insert(oid);
            
            // Traverse children based on object type
            let obj = self.repository.find_object(oid)
                .map_err(|e| Error::custom(format!("Failed to find object: {}", e)))?;
            
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
    
    /// Include reachable tags in the pack
    fn include_reachable_tags(
        &self,
        objects: &mut HashSet<gix_hash::ObjectId>,
        visited: &mut HashSet<gix_hash::ObjectId>,
        context: &TraversalContext,
    ) -> Result<()> {
        // Collect all tag refs - use references().all() instead of refs.iter()
        let refs_binding = self.repository.references()
            .map_err(|e| Error::RefPackedBuffer(e))?;
        let refs_iter = refs_binding
            .all()
            .map_err(|e| Error::RefIterInit(e))?;
            
        let tag_refs: Vec<_> = refs_iter
            .filter_map(|r| r.ok())
            .filter(|r| r.name().as_bstr().starts_with_str("refs/tags/"))
            .collect();
        
        for tag_ref in tag_refs {
            if let gix_ref::TargetRef::Object(oid) = tag_ref.target() {
                let tag_oid = oid.to_owned();
                
                // Check if this tag points to any of our wanted objects
                if self.is_tag_reachable(tag_oid, &context.wants)? {
                    self.traverse_from_object(tag_oid, objects, visited, context)?;
                }
            }
        }
        
        Ok(())
    }
    
    /// Check if a tag is reachable from any of the wanted objects
    fn is_tag_reachable(
        &self,
        tag_oid: gix_hash::ObjectId,
        wants: &HashSet<gix_hash::ObjectId>,
    ) -> Result<bool> {
        // Simple implementation: check if the tag target is reachable from wants
        let tag_obj = self.repository.find_object(tag_oid)
            .map_err(|e| Error::custom(format!("Failed to find tag object: {}", e)))?;
        
        if let Ok(tag) = tag_obj.try_into_tag() {
            if let Ok(target_oid) = tag.target_id() {
                let target_oid = target_oid.detach();
                
                // Check if target is directly wanted
                if wants.contains(&target_oid) {
                    return Ok(true);
                }
                
                // For commits, check if any wanted commit is reachable from this target
                if let Ok(target_obj) = self.repository.find_object(target_oid) {
                    if target_obj.kind == Kind::Commit {
                        for want in wants {
                            if self.is_ancestor_of(target_oid, *want)? {
                                return Ok(true);
                            }
                        }
                    }
                }
            }
        }
        
        Ok(false)
    }
    
    /// Check if ancestor is an ancestor of descendant
    fn is_ancestor_of(
        &self,
        ancestor: gix_hash::ObjectId,
        descendant: gix_hash::ObjectId,
    ) -> Result<bool> {
        // Use gix revision walking to check ancestry
        let revwalk = self.repository.rev_walk([descendant]);
        
        for commit_result in revwalk.all()? {
            let commit_id = commit_result
                .map_err(|e| Error::custom(format!("Failed to walk revisions: {}", e)))?
                .detach();
            
            if commit_id.id == ancestor {
                return Ok(true);
            }
        }
        
        Ok(false)
    }
    
    /// Apply object filter to the object list
    fn apply_object_filter(
        &self,
        objects: Vec<gix_hash::ObjectId>,
        filter: &BStr,
    ) -> Result<Vec<gix_hash::ObjectId>> {
        // Parse filter specification
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
        } else if filter_str.starts_with("blob:limit=") {
            // Filter blobs by size
            let limit_str = &filter_str["blob:limit=".len()..];
            let size_limit: u64 = limit_str.parse()
                .map_err(|_| Error::custom("Invalid blob size limit"))?;
            
            let filtered: Result<Vec<_>> = objects
                .into_iter()
                .filter_map(|oid| {
                    match self.repository.find_object(oid) {
                        Ok(obj) => {
                            if obj.kind == Kind::Blob && obj.data.len() as u64 > size_limit {
                                None
                            } else {
                                Some(Ok(oid))
                            }
                        }
                        Err(e) => Some(Err(Error::custom(format!("Failed to find object: {}", e)))),
                    }
                })
                .collect();
            filtered
        } else if filter_str.starts_with("tree:") {
            // Tree depth filter
            let depth_str = &filter_str["tree:".len()..];
            let max_depth: u32 = depth_str.parse()
                .map_err(|_| Error::custom("Invalid tree depth"))?;
            
            self.apply_tree_depth_filter(objects, max_depth)
        } else {
            // Unknown filter, return all objects
            Ok(objects)
        }
    }
    
    /// Apply tree depth filter
    fn apply_tree_depth_filter(
        &self,
        objects: Vec<gix_hash::ObjectId>,
        max_depth: u32,
    ) -> Result<Vec<gix_hash::ObjectId>> {
        let mut filtered = Vec::new();
        let mut object_depths = HashMap::new();
        
        // First pass: calculate depths for all trees
        for oid in &objects {
            if let Ok(obj) = self.repository.find_object(*oid) {
                if obj.kind == Kind::Tree {
                    let depth = self.calculate_tree_depth(*oid, &mut HashMap::new())?;
                    object_depths.insert(*oid, depth);
                }
            }
        }
        
        // Second pass: filter based on depth
        for oid in objects {
            let include = match self.repository.find_object(oid) {
                Ok(obj) => match obj.kind {
                    Kind::Tree => {
                        object_depths.get(&oid).map_or(true, |&depth| depth <= max_depth)
                    }
                    _ => true, // Include non-tree objects
                },
                Err(_) => true, // Include if we can't determine type
            };
            
            if include {
                filtered.push(oid);
            }
        }
        
        Ok(filtered)
    }
    
    /// Calculate the depth of a tree object
    fn calculate_tree_depth(
        &self,
        tree_oid: gix_hash::ObjectId,
        cache: &mut HashMap<gix_hash::ObjectId, u32>,
    ) -> Result<u32> {
        if let Some(&depth) = cache.get(&tree_oid) {
            return Ok(depth);
        }
        
        let tree_obj = self.repository.find_object(tree_oid)
            .map_err(|e| Error::custom(format!("Failed to find tree object: {}", e)))?;
        
        let tree = tree_obj.try_into_tree()
            .map_err(|e| Error::custom(format!("Failed to parse tree: {}", e)))?;
        
        let mut max_child_depth = 0;
        
        for entry in tree.iter() {
            let entry = entry?;
            if entry.mode().is_tree() {
                let child_depth = self.calculate_tree_depth(entry.oid().to_owned(), cache)?;
                max_child_depth = max_child_depth.max(child_depth);
            }
        }
        
        let depth = max_child_depth + 1;
        cache.insert(tree_oid, depth);
        Ok(depth)
    }
    
    /// Apply depth constraints for shallow clones
    fn apply_depth_constraints(
        &self,
        objects: Vec<gix_hash::ObjectId>,
        deepen_spec: &DeepenSpec,
    ) -> Result<Vec<gix_hash::ObjectId>> {
        match deepen_spec {
            DeepenSpec::Depth(depth) => {
                self.apply_commit_depth_limit(objects, *depth)
            }
            DeepenSpec::Since(since_time) => {
                self.apply_time_limit(objects, *since_time)
            }
            DeepenSpec::Not(exclude_refs) => {
                self.apply_exclude_refs(objects, exclude_refs)
            }
        }
    }
    
    /// Apply commit depth limit
    fn apply_commit_depth_limit(
        &self,
        objects: Vec<gix_hash::ObjectId>,
        max_depth: u32,
    ) -> Result<Vec<gix_hash::ObjectId>> {
        let mut filtered = Vec::new();
        let mut commit_depths = HashMap::new();
        
        // Calculate depths for all commits
        for oid in &objects {
            if let Ok(obj) = self.repository.find_object(*oid) {
                if obj.kind == Kind::Commit {
                    let depth = self.calculate_commit_depth(*oid, &mut HashMap::new())?;
                    commit_depths.insert(*oid, depth);
                }
            }
        }
        
        // Filter objects based on commit depths
        for oid in objects {
            let include = match self.repository.find_object(oid) {
                Ok(obj) => match obj.kind {
                    Kind::Commit => {
                        commit_depths.get(&oid).map_or(true, |&depth| depth <= max_depth)
                    }
                    _ => {
                        // For non-commits, check if they're reachable from included commits
                        self.is_reachable_from_included_commits(oid, &commit_depths, max_depth)?
                    }
                },
                Err(_) => true,
            };
            
            if include {
                filtered.push(oid);
            }
        }
        
        Ok(filtered)
    }
    
    /// Calculate commit depth from root commits
    fn calculate_commit_depth(
        &self,
        commit_oid: gix_hash::ObjectId,
        cache: &mut HashMap<gix_hash::ObjectId, u32>,
    ) -> Result<u32> {
        if let Some(&depth) = cache.get(&commit_oid) {
            return Ok(depth);
        }
        
        let commit_obj = self.repository.find_object(commit_oid)
            .map_err(|e| Error::custom(format!("Failed to find commit object: {}", e)))?;
        
        let commit = commit_obj.try_into_commit()
            .map_err(|e| Error::custom(format!("Failed to parse commit: {}", e)))?;
        
        let parent_ids: Vec<_> = commit.parent_ids().map(|p| p.detach()).collect();
        
        if parent_ids.is_empty() {
            // Root commit
            cache.insert(commit_oid, 0);
            Ok(0)
        } else {
            // Find maximum parent depth
            let mut max_parent_depth = 0;
            for parent_id in parent_ids {
                let parent_depth = self.calculate_commit_depth(parent_id, cache)?;
                max_parent_depth = max_parent_depth.max(parent_depth);
            }
            
            let depth = max_parent_depth + 1;
            cache.insert(commit_oid, depth);
            Ok(depth)
        }
    }
    
    /// Check if an object is reachable from included commits
    fn is_reachable_from_included_commits(
        &self,
        oid: gix_hash::ObjectId,
        commit_depths: &HashMap<gix_hash::ObjectId, u32>,
        max_depth: u32,
    ) -> Result<bool> {
        // Simple implementation: assume trees and blobs are reachable
        // if any commit at acceptable depth exists
        Ok(commit_depths.values().any(|&depth| depth <= max_depth))
    }
    
    /// Apply time-based filtering
    fn apply_time_limit(
        &self,
        objects: Vec<gix_hash::ObjectId>,
        since_time: gix_date::Time,
    ) -> Result<Vec<gix_hash::ObjectId>> {
        let mut filtered = Vec::new();
        
        for oid in objects {
            let include = match self.repository.find_object(oid) {
                Ok(obj) => match obj.kind {
                    Kind::Commit => {
                        if let Ok(commit) = obj.try_into_commit() {
                            commit.time().unwrap_or_default().seconds >= since_time.seconds
                        } else {
                            true
                        }
                    }
                    _ => true, // Include non-commits
                },
                Err(_) => true,
            };
            
            if include {
                filtered.push(oid);
            }
        }
        
        Ok(filtered)
    }
    
    /// Apply exclude refs filter
    fn apply_exclude_refs(
        &self,
        objects: Vec<gix_hash::ObjectId>,
        exclude_refs: &[BString],
    ) -> Result<Vec<gix_hash::ObjectId>> {
        // Get OIDs of excluded refs
        let mut excluded_oids = HashSet::new();
        
        for ref_name in exclude_refs {
            if let Ok(reference) = self.repository.refs.find(ref_name.as_bstr()) {
                if let gix::refs::Target::Object(oid) = reference.target {
                    excluded_oids.insert(oid.to_owned());
                    
                    // Also exclude all ancestors of this commit
                    if let Ok(obj) = self.repository.find_object(oid) {
                        if obj.kind == Kind::Commit {
                            self.collect_ancestors(oid.to_owned(), &mut excluded_oids)?;
                        }
                    }
                }
            }
        }
        
        // Filter out excluded objects
        let filtered = objects
            .into_iter()
            .filter(|oid| !excluded_oids.contains(oid))
            .collect();
        
        Ok(filtered)
    }
    
    /// Collect all ancestors of a commit
    fn collect_ancestors(
        &self,
        commit_oid: gix_hash::ObjectId,
        ancestors: &mut HashSet<gix_hash::ObjectId>,
    ) -> Result<()> {
        let mut queue = VecDeque::new();
        queue.push_back(commit_oid);
        
        while let Some(oid) = queue.pop_front() {
            if ancestors.contains(&oid) {
                continue;
            }
            
            ancestors.insert(oid);
            
            if let Ok(obj) = self.repository.find_object(oid) {
                if let Ok(commit) = obj.try_into_commit() {
                    for parent_id in commit.parent_ids() {
                        queue.push_back(parent_id.detach());
                    }
                }
            }
        }
        
        Ok(())
    }
    
    /// Sort objects for optimal delta compression
    fn sort_objects_for_packing(&self, objects: &mut Vec<gix_hash::ObjectId>) -> Result<()> {
        // Sort by type first (commits, trees, blobs, tags)
        // Then by reverse chronological order for commits
        // Then by path similarity for trees and blobs
        
        objects.sort_by(|a, b| {
            let type_order = |oid: &gix_hash::ObjectId| -> u8 {
                if let Ok(obj) = self.repository.find_object(*oid) {
                    match obj.kind {
                        Kind::Commit => 0,
                        Kind::Tree => 1,
                        Kind::Blob => 2,
                        Kind::Tag => 3,
                    }
                } else {
                    4
                }
            };
            
            let a_type = type_order(a);
            let b_type = type_order(b);
            
            a_type.cmp(&b_type).then_with(|| {
                // For same types, use OID for consistent ordering
                a.cmp(b)
            })
        });
        
        Ok(())
    }
    
    /// Write the pack file
    fn write_pack<W: Write>(
        &self,
        writer: &mut W,
        objects: &[gix_hash::ObjectId],
        context: &TraversalContext,
    ) -> Result<PackStats> {
        let mut stats = PackStats::default();
        
        // Create pack entries from objects
        let mut entries = Vec::new();
        
        for oid in objects {
            let mut buf = Vec::new();
            let obj = self.repository.objects.find(oid, &mut buf)
                .map_err(|e| Error::Pack(format!("Failed to find object {}: {}", oid, e)))?;
                
            // Update statistics
            match obj.kind {
                gix_object::Kind::Commit => stats.commit_count += 1,
                gix_object::Kind::Tree => stats.tree_count += 1,
                gix_object::Kind::Blob => stats.blob_count += 1,
                gix_object::Kind::Tag => stats.tag_count += 1,
            }
            
            let compressed_data = self.compress_object_data(&obj.data)?;
            
            entries.push(PackEntry {
                id: *oid,
                kind: obj.kind,
                size: obj.data.len(),
                compressed_data,
            });
        }
        
        // Write pack header
        self.write_pack_header(writer, entries.len())?;
        
        // Write each entry
        for entry in &entries {
            self.write_pack_entry(writer, entry)?;
        }
        
        // Calculate pack size so far
        let mut pack_size = 12; // header size
        for entry in &entries {
            pack_size += self.calculate_entry_size(entry);
        }
        
        // Write pack trailer (SHA-1 checksum)
        self.write_pack_trailer(writer, &entries)?;
        pack_size += 20; // trailer size
        
        stats.pack_size = pack_size as u64;
        stats.object_count = objects.len() as u32;
        
        Ok(stats)
    }
    
    /// Write pack header
    fn write_pack_header<W: Write>(&self, writer: &mut W, num_objects: usize) -> Result<()> {
        // Pack signature "PACK"
        writer.write_all(b"PACK")?;
        
        // Version (4 bytes, big endian) - version 2
        writer.write_all(&2u32.to_be_bytes())?;
        
        // Number of objects (4 bytes, big endian)
        writer.write_all(&(num_objects as u32).to_be_bytes())?;
        
        Ok(())
    }
    
    /// Write a single pack entry
    fn write_pack_entry<W: Write>(&self, writer: &mut W, entry: &PackEntry) -> Result<()> {
        // Write entry header (type and size)
        self.write_entry_header(writer, entry)?;
        
        // Write compressed data
        writer.write_all(&entry.compressed_data)?;
        
        Ok(())
    }
    
    /// Write entry header (variable length encoding)
    fn write_entry_header<W: Write>(&self, writer: &mut W, entry: &PackEntry) -> Result<()> {
        let type_num = match entry.kind {
            gix_object::Kind::Commit => 1,
            gix_object::Kind::Tree => 2,
            gix_object::Kind::Blob => 3,
            gix_object::Kind::Tag => 4,
        };
        
        let mut size = entry.size as u64;
        let mut header_byte = ((type_num & 0x7) << 4) | (size & 0xf) as u8;
        size >>= 4;
        
        while size > 0 {
            header_byte |= 0x80; // continuation bit
            writer.write_all(&[header_byte])?;
            header_byte = (size & 0x7f) as u8;
            size >>= 7;
        }
        
        writer.write_all(&[header_byte])?;
        Ok(())
    }
    
    /// Write pack trailer (SHA-1 checksum of all preceding data)
    fn write_pack_trailer<W: Write>(&self, writer: &mut W, entries: &[PackEntry]) -> Result<()> {
        // Calculate checksum of the pack data
        let mut hasher = gix_hash::hasher(self.repository.object_hash());
        
        // Hash header
        hasher.update(b"PACK");
        hasher.update(&2u32.to_be_bytes());
        hasher.update(&(entries.len() as u32).to_be_bytes());
        
        // Hash all entries
        for entry in entries {
            // Hash entry header
            let type_num = match entry.kind {
                gix_object::Kind::Commit => 1,
                gix_object::Kind::Tree => 2,
                gix_object::Kind::Blob => 3,
                gix_object::Kind::Tag => 4,
            };
            
            let mut size = entry.size as u64;
            let mut header_byte = ((type_num & 0x7) << 4) | (size & 0xf) as u8;
            size >>= 4;
            
            let mut header_bytes = Vec::new();
            while size > 0 {
                header_byte |= 0x80;
                header_bytes.push(header_byte);
                header_byte = (size & 0x7f) as u8;
                size >>= 7;
            }
            header_bytes.push(header_byte);
            
            for byte in header_bytes {
                hasher.update(&[byte]);
            }
            
            // Hash compressed data
            hasher.update(&entry.compressed_data);
        }
        
        let checksum = hasher.try_finalize().map_err(|e| Error::Pack(format!("Failed to finalize hash: {}", e)))?;
        writer.write_all(checksum.as_slice())?;
        
        Ok(())
    }
    
    /// Compress object data using zlib
    fn compress_object_data(&self, data: &[u8]) -> Result<Vec<u8>> {
        use std::io::Write;
        use gix_features::zlib::stream::deflate;
        
        let mut encoder = deflate::Write::new(Vec::new());
        encoder.write_all(data)?;
        let compressed = encoder.into_inner();
        
        Ok(compressed)
    }
    
    /// Calculate the size of a pack entry
    fn calculate_entry_size(&self, entry: &PackEntry) -> usize {
        // Header size (variable length)
        let _type_num = match entry.kind {
            gix_object::Kind::Commit => 1,
            gix_object::Kind::Tree => 2,
            gix_object::Kind::Blob => 3,
            gix_object::Kind::Tag => 4,
        };
        
        let mut size = entry.size as u64;
        let mut header_size = 1;
        size >>= 4;
        
        while size > 0 {
            header_size += 1;
            size >>= 7;
        }
        
        header_size + entry.compressed_data.len()
    }
}
