//! Want/have negotiation logic for upload-pack
//!
//! This module handles the negotiation phase where the client communicates
//! what objects it wants and what objects it already has, allowing the server
//! to determine the minimal set of objects to send.

use crate::{
    config::ServerOptions,
    error::{Error, Result},
    types::*,
};
use bstr::{BString, ByteSlice};
use gix::Repository;
use gix_packetline_blocking::{PacketLineRef, StreamingPeekableIter};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::{BufRead, Write},
};

/// Negotiation engine for want/have protocol
pub struct NegotiationEngine<'a> {
    repository: &'a Repository,
    // options: &'a ServerOptions,
}

/// Negotiation state during the protocol exchange
#[derive(Debug, Default)]
pub struct NegotiationState {
    /// Objects the client wants
    pub wants: HashSet<gix_hash::ObjectId>,
    /// Objects the client has
    pub haves: HashSet<gix_hash::ObjectId>,
    /// Common objects between client and server
    pub common: HashSet<gix_hash::ObjectId>,
    /// Shallow commits for shallow clones
    pub shallow: HashSet<gix_hash::ObjectId>,
    /// New shallow commits to be created
    pub new_shallow: HashSet<gix_hash::ObjectId>,
    /// Unshallow commits to be removed from shallow list
    pub unshallow: HashSet<gix_hash::ObjectId>,
    /// Deepen specification
    pub deepen: Option<DeepenSpec>,
    /// Whether client sent "done"
    pub done: bool,
    /// Round number for negotiation statistics
    pub round: u32,
    /// Number of common commits found
    pub common_count: u32,
}

/// Result of want/have negotiation
#[derive(Debug)]
pub struct NegotiationResult {
    /// Objects to include in pack
    pub pack_objects: HashSet<gix_hash::ObjectId>,
    /// Shallow commits for the client
    pub shallow_commits: HashSet<gix_hash::ObjectId>,
    /// Whether negotiation is complete
    pub complete: bool,
    /// Negotiation statistics
    pub stats: NegotiationStats,
}

/// Statistics about the negotiation process
#[derive(Debug, Default)]
pub struct NegotiationStats {
    /// Number of negotiation rounds
    pub rounds: u32,
    /// Number of wants received
    pub want_count: u32,
    /// Number of haves processed
    pub have_count: u32,
    /// Number of common objects found
    pub common_count: u32,
    /// Time spent in negotiation
    pub negotiation_time: std::time::Duration,
}

impl<'a> NegotiationEngine<'a> {
    /// Create a new negotiation engine
    pub fn new(repository: &'a Repository, options: &'a ServerOptions) -> Self {
        Self { repository }
    }
    
    /// Perform complete negotiation for protocol v1
    pub fn negotiate_v1<R: BufRead, W: Write>(
        &mut self,
        reader: &mut StreamingPeekableIter<R>,
        writer: &mut W,
        capabilities: &ClientCapabilities,
    ) -> Result<NegotiationResult> {
        let start_time = std::time::Instant::now();
        let mut state = NegotiationState::default();
        
        // Phase 1: Collect wants and setup
        self.collect_wants_v1(reader, &mut state, capabilities)?;
        
        // Phase 2: Process haves and send ACKs
        self.process_haves_v1(reader, writer, &mut state, capabilities)?;
        
        // Phase 3: Finalize negotiation
        let result = self.finalize_negotiation(&state)?;
        
        let negotiation_time = start_time.elapsed();
        Ok(NegotiationResult {
            pack_objects: result.pack_objects,
            shallow_commits: result.shallow_commits,
            complete: result.complete,
            stats: NegotiationStats {
                rounds: state.round,
                want_count: state.wants.len() as u32,
                have_count: state.haves.len() as u32,
                common_count: state.common.len() as u32,
                negotiation_time,
            },
        })
    }
    
    /// Collect want lines and initial parameters
    fn collect_wants_v1<R: BufRead>(
        &mut self,
        reader: &mut StreamingPeekableIter<R>,
        state: &mut NegotiationState,
        capabilities: &ClientCapabilities,
    ) -> Result<()> {
        while let Some(line) = reader.read_line() {
            match line? {
                line => {
                    match line.map_err(|e| Error::custom(format!("Packetline decode error: {}", e)))? {
                        PacketLineRef::Flush => {
                            break;
                        }
                        PacketLineRef::Data(data) => {
                            if data.starts_with(b"want ") {
                                let rest = &data[5..]; // skip "want "
                                if let Some(space_pos) = rest.iter().position(|&b| b == b' ') {
                                    let oid_str = &rest[..space_pos];
                                    if let Ok(oid) = gix_hash::ObjectId::from_hex(oid_str) {
                                        state.wants.insert(oid);
                                    }
                                }
                            }
                            // Parse other commands...
                        }
                        _ => {
                            // Handle other packet line types if needed
                        }
                    }
                }
            }
        }
        
        // Validate wants
        self.validate_wants(state, capabilities)?;
        
        Ok(())
    }
    
    // /// Parse want line and add to state
    // fn parse_want_line(&self, line: &[u8], state: &mut NegotiationState) -> Result<()> {
    //     let line_str = std::str::from_utf8(line.trim_ascii())
    //         .map_err(|_| Error::custom("Invalid UTF-8 in want line"))?;
        
    //     // Parse just the OID (capabilities are only on first want in v1)
    //     let oid_str = line_str.split_whitespace().next()
    //         .ok_or_else(|| Error::custom("Empty want line"))?;
        
    //     let oid = gix_hash::ObjectId::from_hex(oid_str.as_bytes())
    //         .map_err(|_| Error::InvalidObjectId { oid: oid_str.to_string() })?;
        
    //     state.wants.insert(oid);
    //     Ok(())
    // }
    
    // /// Parse shallow line
    // fn parse_shallow_line(&self, line: &[u8], state: &mut NegotiationState) -> Result<()> {
    //     let oid_str = std::str::from_utf8(line.trim_ascii())
    //         .map_err(|_| Error::custom("Invalid UTF-8 in shallow line"))?;
        
    //     let oid = gix_hash::ObjectId::from_hex(oid_str.as_bytes())
    //         .map_err(|_| Error::InvalidObjectId { oid: oid_str.to_string() })?;
            
    //     state.shallow.insert(oid);
    //     Ok(())
    // }
    
    // /// Parse deepen line
    // fn parse_deepen_line(&self, line: &[u8], state: &mut NegotiationState) -> Result<()> {
    //     let depth_str = std::str::from_utf8(line.trim_ascii())
    //         .map_err(|_| Error::custom("Invalid UTF-8 in deepen line"))?;
            
    //     let depth: u32 = depth_str.parse()
    //         .map_err(|_| Error::custom("Invalid depth value"))?;
            
    //     state.deepen = Some(DeepenSpec::Depth(depth));
    //     Ok(())
    // }
    
    // /// Parse deepen-since line
    // fn parse_deepen_since_line(&self, line: &[u8], state: &mut NegotiationState) -> Result<()> {
    //     let timestamp_str = std::str::from_utf8(line.trim_ascii())
    //         .map_err(|_| Error::custom("Invalid UTF-8 in deepen-since line"))?;
            
    //     let timestamp: i64 = timestamp_str.parse()
    //         .map_err(|_| Error::custom("Invalid timestamp value"))?;
            
    //     let time = gix_date::Time::new(timestamp, 0);
    //     state.deepen = Some(DeepenSpec::Since(time));
    //     Ok(())
    // }
    
    // /// Parse deepen-not line
    // fn parse_deepen_not_line(&self, line: &[u8], state: &mut NegotiationState) -> Result<()> {
    //     let ref_str = std::str::from_utf8(line.trim_ascii())
    //         .map_err(|_| Error::custom("Invalid UTF-8 in deepen-not line"))?;
            
    //     if let Some(DeepenSpec::Not(ref mut refs)) = state.deepen {
    //         refs.push(ref_str.into());
    //     } else {
    //         state.deepen = Some(DeepenSpec::Not(vec![ref_str.into()]));
    //     }
    //     Ok(())
    // }
    
    /// Validate wanted objects
    fn validate_wants(&self, state: &NegotiationState, capabilities: &ClientCapabilities) -> Result<()> {
        for want in &state.wants {
            // Check if object exists
            if !self.repository.has_object(*want) {
                // Check if SHA1-in-want is allowed
                if !capabilities.allow_tip_sha1_in_want 
                    && !capabilities.allow_reachable_sha1_in_want 
                    && !capabilities.allow_tip_sha1_in_want {
                    return Err(Error::ObjectNotFound { oid: *want });
                }
                
                // For SHA1-in-want, validate based on capability level
                if capabilities.allow_tip_sha1_in_want {
                    self.validate_tip_sha1_in_want(*want)?;
                } else if capabilities.allow_reachable_sha1_in_want {
                    self.validate_reachable_sha1_in_want(*want)?;
                }
                // allow_any_sha1_in_want allows any SHA1, so no validation needed
            }
        }
        
        Ok(())
    }
    
    /// Validate tip SHA1 in want (must be a ref tip)
    fn validate_tip_sha1_in_want(&self, oid: gix_hash::ObjectId) -> Result<()> {
        // Check if OID is a tip of any reference
        for reference in self.repository.references().map_err(|e| Error::custom(format!("Reference error: {}", e)))?.all().map_err(|e| Error::custom(format!("Reference iteration error: {}", e)))? {
            let reference = reference.map_err(|e| Error::custom(format!("Reference access error: {}", e)))?;
            match reference.target() {
                gix::refs::TargetRef::Object(ref_oid) => {
                    if oid == ref_oid {
                        return Ok(());
                    }
                }
                _ => {
                    // Skip symbolic or other reference types
                }
            }
        }
        
        Err(Error::ObjectNotFound { oid })
    }
    
    /// Validate reachable SHA1 in want (must be reachable from a ref)
    fn validate_reachable_sha1_in_want(&self, oid: gix_hash::ObjectId) -> Result<()> {
        // Check if OID is reachable from any reference
        for reference in self.repository.references().map_err(|e| Error::RefPackedBuffer(e))?.all().map_err(|e| Error::RefIterInit(e))? {
            let reference = reference?;
            if let gix::refs::TargetRef::Object(ref_oid) = reference.target() {
                if self.is_ancestor_or_equal(oid, ref_oid.to_owned())? {
                    return Ok(());
                }
            }
        }
        
        Err(Error::ObjectNotFound { oid })
    }
    
    /// Check if an object is an ancestor of or equal to another
    fn is_ancestor_or_equal(&self, ancestor: gix_hash::ObjectId, descendant: gix_hash::ObjectId) -> Result<bool> {
        if ancestor == descendant {
            return Ok(true);
        }
        
        // Use revision walking to check ancestry
        let revwalk = self.repository.rev_walk([descendant]);
        
        for commit_result in revwalk.all()? {
            let commit_id = commit_result
                .map_err(|e| Error::Odb(format!("Revision walk error: {}", e)))?
                .detach();
            
            if commit_id.id == ancestor {
                return Ok(true);
            }
        }
        
        Ok(false)
    }
    
    /// Process have lines and send appropriate ACKs
    fn process_haves_v1<R: BufRead, W: Write>(
        &mut self,
        reader: &mut StreamingPeekableIter<R>,
        writer: &mut W,
        state: &mut NegotiationState,
        capabilities: &ClientCapabilities,
    ) -> Result<()> {
        let mut consecutive_unknowns = 0;
        let max_consecutive_unknowns = 256; // Git's default
        
        while let Some(line) = reader.read_line() {
            match line? {
                line => {
                    match line.map_err(|e| Error::custom(format!("Packetline decode error: {}", e)))? {
                        PacketLineRef::Flush => {
                            break;
                        }
                        PacketLineRef::Data(line_data) => {
                    
                    if let Some(have_line) = line_data.strip_prefix(b"have ") {
                        let oid = self.parse_have_line(have_line, state)?;
                        
                        if self.repository.has_object(oid) {
                            // We have this object - it's common
                            state.common.insert(oid);
                            state.common_count += 1;
                            consecutive_unknowns = 0;
                            
                            // Send ACK based on multi-ack mode
                            match capabilities.multi_ack {
                                MultiAckMode::None => {
                                    // Send ACK only when we're ready
                                    if self.should_send_pack(state)? {
                                        self.send_ack(writer, &oid, AckStatus::Common)?;
                                        return Ok(());
                                    }
                                }
                                MultiAckMode::Basic => {
                                    self.send_ack(writer, &oid, AckStatus::Continue)?;
                                }
                                MultiAckMode::Detailed => {
                                    if self.should_send_pack(state)? {
                                        self.send_ack(writer, &oid, AckStatus::Ready)?;
                                        return Ok(());
                                    } else {
                                        self.send_ack(writer, &oid, AckStatus::Common)?;
                                    }
                                }
                            }
                        } else {
                            // We don't have this object
                            consecutive_unknowns += 1;
                            
                            // If too many unknown objects, consider stopping
                            if consecutive_unknowns > max_consecutive_unknowns {
                                break;
                            }
                        }
                        
                        state.haves.insert(oid);
                    } else if line_data.trim_ascii() == b"done" {
                        state.done = true;
                        break;
                    }
                        }
                        _ => {
                            // Handle other packet line types if needed
                        }
                    }
                }
            }
            
            state.round += 1;
        }
        
        // Send final response
        if state.done {
            if !state.common.is_empty() && self.should_send_pack(state)? {
                // Send final ACK
                if let Some(common_oid) = state.common.iter().next() {
                    self.send_ack(writer, common_oid, AckStatus::Common)?;
                }
            } else {
                // Send NAK
                self.send_nak(writer)?;
            }
        }
        
        Ok(())
    }
    
    /// Parse have line and return object ID
    fn parse_have_line(&self, line: &[u8], state: &mut NegotiationState) -> Result<gix_hash::ObjectId> {
        let oid_str = std::str::from_utf8(line.trim_ascii())
            .map_err(|_| Error::custom("Invalid UTF-8 in have line"))?;
        
        let oid = gix_hash::ObjectId::from_hex(oid_str.as_bytes())
            .map_err(|_| Error::InvalidObjectId { oid: oid_str.to_string() })?;
            
        Ok(oid)
    }
    
    /// Determine if we should send a pack
    fn should_send_pack(&self, state: &NegotiationState) -> Result<bool> {
        // Simple heuristic: send pack if we have some common objects or client said done
        Ok(!state.common.is_empty() || state.done)
    }
    
    /// Send ACK packet
    fn send_ack<W: Write>(&self, writer: &mut W, oid: &gix_hash::ObjectId, status: AckStatus) -> Result<()> {
        let status_str = match status {
            AckStatus::Common => "",
            AckStatus::Continue => " continue", 
            AckStatus::Ready => " ready",
        };
        
        let response = format!("ACK {}{}\n", oid.to_hex(), status_str);
        gix_packetline::encode::data_to_write(response.as_bytes(), writer)?;
        Ok(())
    }
    
    /// Send NAK packet
    fn send_nak<W: Write>(&self, writer: &mut W) -> Result<()> {
        gix_packetline::encode::data_to_write(b"NAK\n", writer)?;
        Ok(())
    }
    
    /// Finalize negotiation and compute pack objects
    fn finalize_negotiation(&self, state: &NegotiationState) -> Result<NegotiationResult> {
        let mut pack_objects = HashSet::new();
        
        // Start from wanted objects
        for want in &state.wants {
            self.collect_reachable_objects(*want, &mut pack_objects, state)?;
        }
        
        // Remove objects the client already has
        for have in &state.common {
            self.remove_reachable_objects(*have, &mut pack_objects)?;
        }
        
        // Handle shallow/deepen constraints
        let shallow_commits = self.compute_shallow_commits(state, &pack_objects)?;
        
        Ok(NegotiationResult {
            pack_objects,
            shallow_commits,
            complete: state.done,
            stats: NegotiationStats::default(), // Will be filled by caller
        })
    }
    
    /// Collect all objects reachable from a starting object
    fn collect_reachable_objects(
        &self,
        start_oid: gix_hash::ObjectId,
        objects: &mut HashSet<gix_hash::ObjectId>,
        state: &NegotiationState,
    ) -> Result<()> {
        let mut queue = VecDeque::new();
        let mut visited = HashSet::new();
        
        queue.push_back(start_oid);
        
        while let Some(oid) = queue.pop_front() {
            if visited.contains(&oid) || state.common.contains(&oid) {
                continue;
            }
            
            visited.insert(oid);
            objects.insert(oid);
            
            // Stop at shallow commits
            if state.shallow.contains(&oid) {
                continue;
            }
            
            // Traverse object based on type
            if let Ok(obj) = self.repository.find_object(oid) {
                match obj.kind {
                    gix::object::Kind::Commit => {
                        if let Ok(commit) = obj.try_into_commit() {
                            // Add tree
                            queue.push_back(commit.tree()?.id().into());
                            
                            // Add parents
                            for parent in commit.parent_ids() {
                                queue.push_back(parent.detach());
                            }
                        }
                    }
                    gix::object::Kind::Tree => {
                        if let Ok(tree) = obj.try_into_tree() {
                            for entry in tree.iter() {
                                if let Ok(entry) = entry {
                                    queue.push_back(entry.oid().to_owned());
                                }
                            }
                        }
                    }
                    gix::object::Kind::Tag => {
                        if let Ok(tag) = obj.try_into_tag() {
                            if let Ok(target) = tag.target_id() {
                                queue.push_back(target.detach());
                            }
                        }
                    }
                    gix::object::Kind::Blob => {
                        // Blobs have no children
                    }
                }
            }
        }
        
        Ok(())
    }
    
    /// Remove objects reachable from a common object
    fn remove_reachable_objects(
        &self,
        start_oid: gix_hash::ObjectId,
        objects: &mut HashSet<gix_hash::ObjectId>,
    ) -> Result<()> {
        let mut queue = VecDeque::new();
        let mut visited = HashSet::new();
        
        queue.push_back(start_oid);
        
        while let Some(oid) = queue.pop_front() {
            if visited.contains(&oid) {
                continue;
            }
            
            visited.insert(oid);
            objects.remove(&oid);
            
            // Only continue if this object was in our pack
            if !objects.contains(&oid) {
                continue;
            }
            
            // Traverse based on object type  
            if let Ok(obj) = self.repository.find_object(oid) {
                match obj.kind {
                    gix::object::Kind::Commit => {
                        if let Ok(commit) = obj.try_into_commit() {
                            queue.push_back(commit.tree()?.id().into());
                            for parent in commit.parent_ids() {
                                queue.push_back(parent.detach());
                            }
                        }
                    }
                    gix::object::Kind::Tree => {
                        if let Ok(tree) = obj.try_into_tree() {
                            for entry in tree.iter() {
                                if let Ok(entry) = entry {
                                    queue.push_back(entry.oid().to_owned());
                                }
                            }
                        }
                    }
                    gix::object::Kind::Tag => {
                        if let Ok(tag) = obj.try_into_tag() {
                            if let Ok(target) = tag.target_id() {
                                queue.push_back(target.detach());
                            }
                        }
                    }
                    gix::object::Kind::Blob => {}
                }
            }
        }
        
        Ok(())
    }
    
    /// Compute shallow commits for the result
    fn compute_shallow_commits(
        &self,
        state: &NegotiationState,
        pack_objects: &HashSet<gix_hash::ObjectId>,
    ) -> Result<HashSet<gix_hash::ObjectId>> {
        let mut shallow_commits = state.shallow.clone();
        
        // Apply deepen constraints
        if let Some(ref deepen_spec) = state.deepen {
            match deepen_spec {
                DeepenSpec::Depth(depth) => {
                    shallow_commits = self.compute_depth_shallow(*depth, &state.wants)?;
                }
                DeepenSpec::Since(since_time) => {
                    shallow_commits = self.compute_time_shallow(*since_time, &state.wants)?;
                }
                DeepenSpec::Not(exclude_refs) => {
                    shallow_commits = self.compute_exclude_shallow(exclude_refs, &state.wants)?;
                }
            }
        }
        
        Ok(shallow_commits)
    }
    
    /// Compute shallow commits based on depth limit
    fn compute_depth_shallow(
        &self,
        depth: u32,
        wants: &HashSet<gix_hash::ObjectId>,
    ) -> Result<HashSet<gix_hash::ObjectId>> {
        let mut shallow = HashSet::new();
        
        for want in wants {
            if let Ok(obj) = self.repository.find_object(*want) {
                if obj.kind == gix::object::Kind::Commit {
                    self.collect_shallow_at_depth(*want, depth, &mut shallow)?;
                }
            }
        }
        
        Ok(shallow)
    }
    
    /// Collect commits that should be shallow at given depth
    fn collect_shallow_at_depth(
        &self,
        start: gix_hash::ObjectId,
        max_depth: u32,
        shallow: &mut HashSet<gix_hash::ObjectId>,
    ) -> Result<()> {
        let mut queue = VecDeque::new();
        let mut depths = HashMap::new();
        
        queue.push_back((start, 0));
        depths.insert(start, 0);
        
        while let Some((oid, depth)) = queue.pop_front() {
            if depth >= max_depth {
                shallow.insert(oid);
                continue;
            }
            
            if let Ok(obj) = self.repository.find_object(oid) {
                if let Ok(commit) = obj.try_into_commit() {
                    for parent_id in commit.parent_ids() {
                        let parent_id = parent_id.detach();
                        let parent_depth = depth + 1;
                        
                        if let Some(&existing_depth) = depths.get(&parent_id) {
                            if existing_depth <= parent_depth {
                                continue;
                            }
                        }
                        
                        depths.insert(parent_id, parent_depth);
                        queue.push_back((parent_id, parent_depth));
                    }
                }
            }
        }
        
        Ok(())
    }
    
    /// Compute shallow commits based on time limit
    fn compute_time_shallow(
        &self,
        since_time: gix_date::Time,
        wants: &HashSet<gix_hash::ObjectId>,
    ) -> Result<HashSet<gix_hash::ObjectId>> {
        let mut shallow = HashSet::new();
        
        for want in wants {
            self.collect_shallow_since(*want, since_time, &mut shallow)?;
        }
        
        Ok(shallow)
    }
    
    /// Collect commits that should be shallow based on time
    fn collect_shallow_since(
        &self,
        start: gix_hash::ObjectId,
        since_time: gix_date::Time,
        shallow: &mut HashSet<gix_hash::ObjectId>,
    ) -> Result<()> {
        let mut queue = VecDeque::new();
        let mut visited = HashSet::new();
        
        queue.push_back(start);
        
        while let Some(oid) = queue.pop_front() {
            if visited.contains(&oid) {
                continue;
            }
            
            visited.insert(oid);
            
            if let Ok(obj) = self.repository.find_object(oid) {
                if let Ok(commit) = obj.try_into_commit() {
                    let commit_time = commit.time().unwrap_or_default();
                    
                    if commit_time.seconds < since_time.seconds {
                        shallow.insert(oid);
                        continue;
                    }
                    
                    for parent_id in commit.parent_ids() {
                        queue.push_back(parent_id.detach());
                    }
                }
            }
        }
        
        Ok(())
    }
    
    /// Compute shallow commits based on exclude refs
    fn compute_exclude_shallow(
        &self,
        exclude_refs: &[BString],
        wants: &HashSet<gix_hash::ObjectId>,
    ) -> Result<HashSet<gix_hash::ObjectId>> {
        let mut shallow = HashSet::new();
        let mut excluded_commits = HashSet::new();
        
        // Collect all commits reachable from exclude refs
        for ref_name in exclude_refs {
            if let Ok(reference) = self.repository.refs.find(ref_name.as_bstr()) {
                if let gix::refs::Target::Object(oid) = reference.target {
                    self.collect_excluded_commits(oid.to_owned(), &mut excluded_commits)?;
                }
            }
        }
        
        // Find boundary commits (reachable from wants but not from excluded)
        for want in wants {
            self.find_boundary_commits(*want, &excluded_commits, &mut shallow)?;
        }
        
        Ok(shallow)
    }
    
    /// Collect all commits reachable from an excluded ref
    fn collect_excluded_commits(
        &self,
        start: gix_hash::ObjectId,
        excluded: &mut HashSet<gix_hash::ObjectId>,
    ) -> Result<()> {
        let mut queue = VecDeque::new();
        queue.push_back(start);
        
        while let Some(oid) = queue.pop_front() {
            if excluded.contains(&oid) {
                continue;
            }
            
            excluded.insert(oid);
            
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
    
    /// Find boundary commits between wanted and excluded sets
    fn find_boundary_commits(
        &self,
        start: gix_hash::ObjectId,
        excluded: &HashSet<gix_hash::ObjectId>,
        shallow: &mut HashSet<gix_hash::ObjectId>,
    ) -> Result<()> {
        let mut queue = VecDeque::new();
        let mut visited = HashSet::new();
        
        queue.push_back(start);
        
        while let Some(oid) = queue.pop_front() {
            if visited.contains(&oid) || excluded.contains(&oid) {
                continue;
            }
            
            visited.insert(oid);
            
            if let Ok(obj) = self.repository.find_object(oid) {
                if let Ok(commit) = obj.try_into_commit() {
                    let mut has_excluded_parent = false;
                    
                    for parent_id in commit.parent_ids() {
                        let parent_id = parent_id.detach();
                        
                        if excluded.contains(&parent_id) {
                            has_excluded_parent = true;
                        } else {
                            queue.push_back(parent_id);
                        }
                    }
                    
                    if has_excluded_parent {
                        shallow.insert(oid);
                    }
                }
            }
        }
        
        Ok(())
    }
}
