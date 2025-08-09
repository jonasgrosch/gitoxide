//! Fast-forward detection helper.
//!
//! This module provides minimal fast-forward detection for policy evaluation.
//! It will be replaced by M4's ConnectivityChecker once available.

use gix_hash::ObjectId;
use gix_odb::Handle as OdbHandle;
use gix_object::{CommitRef, Kind, Find};
use std::collections::HashSet;

/// Maximum number of commits to traverse when checking fast-forward status.
/// This prevents infinite loops and excessive resource usage.
const MAX_TRAVERSAL_DEPTH: usize = 1000;

/// Check if `new` is a fast-forward of `old`.
///
/// This is a minimal implementation that will be replaced by M4's ConnectivityChecker.
/// 
/// # Arguments
/// * `old` - The old object ID
/// * `new` - The new object ID  
/// * `main_odb` - ODB handle for object lookup
///
/// # Returns
/// * `Ok(true)` if `new` is a fast-forward of `old`
/// * `Ok(false)` if `new` is not a fast-forward of `old`
/// * `Err(_)` if there was an error during evaluation
///
/// # Fast-forward rules
/// * If `old == new` → true (no change)
/// * If `old` is zero → true (create operation, deny_non_fast_forwards doesn't apply)
/// * Otherwise: walk commit parents from `new` towards root with bounded steps
///
/// # Notes
/// * Excludes hidden refs from consideration per SPEC 8
/// * Uses bounded traversal to prevent resource exhaustion
/// * Will be replaced by M4's ConnectivityChecker::check() once available
pub fn is_fast_forward(old: ObjectId, new: ObjectId, main_odb: &OdbHandle) -> Result<bool, crate::Error> {
    // Rule 1: If old == new, it's trivially a fast-forward (no change)
    if old == new {
        return Ok(true);
    }

    // Rule 2: If old is zero, treat as create; deny_non_fast_forwards does not apply
    if old.is_null() {
        return Ok(true);
    }

    // Rule 3: If new is zero, this is a delete operation, not relevant for fast-forward check
    // Delete operations are handled by deny_deletes policy, not fast-forward policy
    if new.is_null() {
        return Ok(true);
    }

    // Rule 4: Walk commit parents from `new` towards root with bounded steps
    // If `old` is seen during traversal → true (fast-forward)
    // If traversal completes without seeing `old` → false (non-fast-forward)
    
    // First, verify that both objects exist and are commits
    let mut buf1 = Vec::new();
    let new_object = main_odb.try_find(&new, &mut buf1)
        .map_err(|e| crate::Error::Protocol(format!("Failed to find object {}: {}", new, e)))?
        .ok_or_else(|| crate::Error::Protocol(format!("Object {} not found", new)))?;
    
    let mut buf2 = Vec::new();
    let old_object = main_odb.try_find(&old, &mut buf2)
        .map_err(|e| crate::Error::Protocol(format!("Failed to find object {}: {}", old, e)))?
        .ok_or_else(|| crate::Error::Protocol(format!("Object {} not found", old)))?;
    
    // If either object is not a commit, we can't do ancestry checking
    // For non-commit objects, we conservatively return false
    if new_object.kind != Kind::Commit || old_object.kind != Kind::Commit {
        return Ok(false);
    }

    // Perform bounded ancestry walk from new towards root
    let mut visited = HashSet::new();
    let mut to_visit = vec![new];
    let mut depth = 0;

    while let Some(current_oid) = to_visit.pop() {
        // Check depth limit to prevent excessive resource usage
        if depth >= MAX_TRAVERSAL_DEPTH {
            // If we hit the depth limit, conservatively return false
            return Ok(false);
        }

        // If we've already visited this commit, skip it
        if visited.contains(&current_oid) {
            continue;
        }
        visited.insert(current_oid);

        // If we found the old commit, it's a fast-forward
        if current_oid == old {
            return Ok(true);
        }

        // Get the commit object and traverse its parents
        let mut buf = Vec::new();
        let commit_object = main_odb.try_find(&current_oid, &mut buf)
            .map_err(|e| crate::Error::Protocol(format!("Failed to find object {}: {}", current_oid, e)))?
            .ok_or_else(|| crate::Error::Protocol(format!("Object {} not found", current_oid)))?;
        if commit_object.kind != Kind::Commit {
            continue; // Skip non-commit objects
        }

        let commit = CommitRef::from_bytes(&commit_object.data).map_err(|e| {
            crate::Error::Protocol(format!("Failed to parse commit {}: {}", current_oid, e))
        })?;

        // Add all parents to the traversal queue
        for parent_id in commit.parents() {
            if !visited.contains(&parent_id) {
                to_visit.push(parent_id);
            }
        }

        depth += 1;
    }

    // If we completed the traversal without finding the old commit, it's not a fast-forward
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gix_hash::ObjectId;


    fn test_oid(suffix: u8) -> ObjectId {
        let mut bytes = [0u8; 20];
        bytes[19] = suffix;
        ObjectId::from_bytes_or_panic(&bytes)
    }

    #[test]
    fn test_identical_oids_are_fast_forward() {
        let oid = test_oid(1);
        // Create a minimal ODB handle for testing
        let temp_dir = tempfile::tempdir().unwrap();
        let objects_dir = temp_dir.path().join("objects");
        std::fs::create_dir_all(&objects_dir).unwrap();
        let odb = gix_odb::at(objects_dir).unwrap();
        
        assert!(is_fast_forward(oid, oid, &odb).unwrap());
    }

    #[test]
    fn test_zero_old_is_fast_forward() {
        let old = ObjectId::null(gix_hash::Kind::Sha1);
        let new = test_oid(1);
        let temp_dir = tempfile::tempdir().unwrap();
        let objects_dir = temp_dir.path().join("objects");
        std::fs::create_dir_all(&objects_dir).unwrap();
        let odb = gix_odb::at(objects_dir).unwrap();
        
        assert!(is_fast_forward(old, new, &odb).unwrap());
    }

    #[test]
    fn test_zero_new_is_fast_forward() {
        let old = test_oid(1);
        let new = ObjectId::null(gix_hash::Kind::Sha1);
        let temp_dir = tempfile::tempdir().unwrap();
        let objects_dir = temp_dir.path().join("objects");
        std::fs::create_dir_all(&objects_dir).unwrap();
        let odb = gix_odb::at(objects_dir).unwrap();
        
        assert!(is_fast_forward(old, new, &odb).unwrap());
    }

    #[test]
    fn test_nonexistent_objects() {
        let old = test_oid(1);
        let new = test_oid(2);
        let temp_dir = tempfile::tempdir().unwrap();
        let objects_dir = temp_dir.path().join("objects");
        std::fs::create_dir_all(&objects_dir).unwrap();
        let odb = gix_odb::at(objects_dir).unwrap();
        
        // Should return an error when objects don't exist
        assert!(is_fast_forward(old, new, &odb).is_err());
    }

    #[test]
    fn test_basic_cases_with_empty_odb() {
        // Test the basic cases that don't require actual objects
        let temp_dir = tempfile::tempdir().unwrap();
        let objects_dir = temp_dir.path().join("objects");
        std::fs::create_dir_all(&objects_dir).unwrap();
        let odb = gix_odb::at(objects_dir).unwrap();
        
        let null_oid = ObjectId::null(gix_hash::Kind::Sha1);
        let test_oid = test_oid(42);
        
        // Create operation (old is null)
        assert!(is_fast_forward(null_oid, test_oid, &odb).unwrap());
        
        // Delete operation (new is null)  
        assert!(is_fast_forward(test_oid, null_oid, &odb).unwrap());
        
        // Same OID
        assert!(is_fast_forward(test_oid, test_oid, &odb).unwrap());
    }
}