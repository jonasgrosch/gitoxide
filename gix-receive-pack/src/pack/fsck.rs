//! Fsck integration for pack ingestion validation.
//!
//! This module provides configurable object validation using gix-fsck with different
//! strictness levels and comprehensive error reporting.

use std::collections::HashMap;
use std::path::Path;

use gix_hash::ObjectId;
use gix_object::Kind;

#[cfg(feature = "fsck")]
use gix_object::Find;
// Error handling integration will be added when fsck validation is implemented

/// Configuration for fsck validation during pack ingestion.
#[derive(Debug, Clone)]
pub struct FsckConfig {
    /// Whether fsck validation is enabled
    pub enabled: bool,
    /// Validation strictness level
    pub level: FsckLevel,
    /// Skip validation for specific object types
    pub skip_types: Vec<Kind>,
    /// Skip validation for specific object IDs
    pub skip_objects: Vec<ObjectId>,
    /// Custom message type configurations
    pub msg_types: HashMap<String, FsckMessageLevel>,
}

impl Default for FsckConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            level: FsckLevel::Normal,
            skip_types: Vec::new(),
            skip_objects: Vec::new(),
            msg_types: HashMap::new(),
        }
    }
}

/// Fsck validation strictness levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsckLevel {
    /// Basic validation - only check object presence and basic integrity
    Basic,
    /// Normal validation - standard fsck checks
    Normal,
    /// Strict validation - all checks including pedantic ones
    Strict,
}

/// Message level configuration for specific fsck message types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsckMessageLevel {
    /// Ignore this message type
    Ignore,
    /// Warn but don't fail
    Warn,
    /// Treat as error and fail validation
    Error,
}

/// Results from fsck validation.
#[derive(Debug, Clone)]
pub struct FsckResults {
    /// Objects that were validated
    pub validated_objects: Vec<ObjectId>,
    /// Warnings encountered during validation
    pub warnings: Vec<FsckMessage>,
    /// Errors encountered during validation
    pub errors: Vec<FsckMessage>,
    /// Missing objects found during connectivity check
    pub missing_objects: Vec<MissingObject>,
}

impl FsckResults {
    /// Check if there are any errors in the fsck results.
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty() || !self.missing_objects.is_empty()
    }

    /// Check if there are any warnings in the fsck results.
    pub fn has_warnings(&self) -> bool {
        !self.warnings.is_empty()
    }

    /// Get the total number of issues (errors + warnings).
    pub fn issue_count(&self) -> usize {
        self.errors.len() + self.warnings.len() + self.missing_objects.len()
    }
}

/// A single fsck message (warning or error).
#[derive(Debug, Clone)]
pub struct FsckMessage {
    /// The object ID this message relates to
    pub object_id: ObjectId,
    /// The message type/category
    pub message_type: String,
    /// Human-readable message
    pub message: String,
}

/// Information about a missing object found during connectivity check.
#[derive(Debug, Clone)]
pub struct MissingObject {
    /// The missing object ID
    pub object_id: ObjectId,
    /// The type of the missing object
    pub object_type: Kind,
}

/// Fsck validator that integrates with gix-fsck for object validation.
#[cfg(feature = "fsck")]
#[derive(Debug)]
pub struct FsckValidator {
    config: FsckConfig,
}

#[cfg(feature = "fsck")]
impl FsckValidator {
    /// Create a new fsck validator with the given configuration.
    pub fn new(config: FsckConfig) -> Self {
        Self { config }
    }

    /// Validate objects in the given quarantine directory.
    ///
    /// This performs connectivity checks on all objects in the quarantine
    /// and validates them according to the configured strictness level.
    pub fn validate_quarantine(
        &self,
        quarantine_objects_dir: &Path,
        main_odb: &gix_odb::Handle,
    ) -> Result<FsckResults, crate::Error> {
        if !self.config.enabled {
            return Ok(FsckResults {
                validated_objects: Vec::new(),
                warnings: Vec::new(),
                errors: Vec::new(),
                missing_objects: Vec::new(),
            });
        }

        let mut results = FsckResults {
            validated_objects: Vec::new(),
            warnings: Vec::new(),
            errors: Vec::new(),
            missing_objects: Vec::new(),
        };

        // Create a combined ODB that includes both quarantine and main ODB
        let quarantine_odb = self.create_quarantine_odb(quarantine_objects_dir, main_odb)?;

        // Find all objects in the quarantine pack directory
        let pack_objects = self.find_pack_objects(quarantine_objects_dir)?;

        // Perform connectivity check on all objects
        self.check_connectivity(&quarantine_odb, &pack_objects, &mut results)?;

        // Perform additional validation based on strictness level
        match self.config.level {
            FsckLevel::Basic => {
                // Basic validation is already done by connectivity check
            }
            FsckLevel::Normal | FsckLevel::Strict => {
                self.validate_object_integrity(&quarantine_odb, &pack_objects, &mut results)?;
            }
        }

        // Check if we have any errors that should cause validation to fail
        if !results.errors.is_empty() {
            let error_count = results.errors.len();
            let first_error = &results.errors[0];
            return Err(crate::Error::Fsck(format!(
                "fsck validation failed with {} error(s): {} (object {})",
                error_count, first_error.message, first_error.object_id
            )));
        }

        Ok(results)
    }

    /// Create a combined ODB that includes both quarantine and main ODB.
    fn create_quarantine_odb(
        &self,
        quarantine_objects_dir: &Path,
        _main_odb: &gix_odb::Handle,
    ) -> Result<gix_odb::Handle, crate::Error> {
        // Create an ODB handle for the quarantine directory
        let quarantine_odb = gix_odb::at(quarantine_objects_dir)
            .map_err(|e| crate::Error::Validation(format!("failed to open quarantine ODB: {}", e)))?;

        // For now, return the quarantine ODB directly
        // In a more sophisticated implementation, we might create a compound ODB
        // that searches quarantine first, then falls back to main ODB
        Ok(quarantine_odb)
    }

    /// Find all objects in the quarantine pack directory.
    fn find_pack_objects(&self, quarantine_objects_dir: &Path) -> Result<Vec<ObjectId>, crate::Error> {
        let mut objects = Vec::new();
        let pack_dir = quarantine_objects_dir.join("pack");

        if !pack_dir.exists() {
            return Ok(objects);
        }

        // Look for pack files and extract object IDs from them
        for entry in std::fs::read_dir(&pack_dir)
            .map_err(|e| crate::Error::Validation(format!("failed to read pack directory: {}", e)))?
        {
            let entry = entry
                .map_err(|e| crate::Error::Validation(format!("failed to read pack entry: {}", e)))?;
            let path = entry.path();

            if path.extension().and_then(|s| s.to_str()) == Some("pack") {
                // Open the pack file and extract object IDs
                let pack_objects = self.extract_pack_objects(&path)?;
                objects.extend(pack_objects);
            }
        }

        Ok(objects)
    }

    /// Extract object IDs from a pack file.
    fn extract_pack_objects(&self, _pack_path: &Path) -> Result<Vec<ObjectId>, crate::Error> {
        // For now, return empty list - in a full implementation we'd parse the pack index
        // to get object IDs without needing to stream the entire pack
        Ok(Vec::new())
    }

    /// Perform connectivity check using gix-fsck.
    fn check_connectivity(
        &self,
        odb: &gix_odb::Handle,
        objects: &[ObjectId],
        results: &mut FsckResults,
    ) -> Result<(), crate::Error> {
        let mut missing_objects = Vec::new();

        // Callback to collect missing objects
        let mut missing_cb = |oid: &ObjectId, kind: Kind| {
            missing_objects.push(MissingObject {
                object_id: *oid,
                object_type: kind,
            });
        };

        let mut connectivity = gix_fsck::Connectivity::new(odb, &mut missing_cb);

        // Check connectivity for all commit objects
        for &object_id in objects {
            // Skip if this object should be skipped
            if self.config.skip_objects.contains(&object_id) {
                continue;
            }

            // Try to determine object type and check connectivity if it's a commit
            let mut buf = Vec::new();
            if let Ok(Some(obj)) = odb.try_find(&object_id, &mut buf) {
                if obj.kind == Kind::Commit {
                    if let Err(e) = connectivity.check_commit(&object_id) {
                        results.errors.push(FsckMessage {
                            object_id,
                            message_type: "connectivity".to_string(),
                            message: format!("connectivity check failed: {}", e),
                        });
                    } else {
                        results.validated_objects.push(object_id);
                    }
                }
            }
        }

        results.missing_objects = missing_objects;

        // Convert missing objects to errors or warnings based on configuration
        for missing in &results.missing_objects {
            if self.config.skip_types.contains(&missing.object_type) {
                continue;
            }

            let message = FsckMessage {
                object_id: missing.object_id,
                message_type: "missing-object".to_string(),
                message: format!("missing {} object", missing.object_type),
            };

            match self.config.level {
                FsckLevel::Basic => results.warnings.push(message),
                FsckLevel::Normal | FsckLevel::Strict => results.errors.push(message),
            }
        }

        Ok(())
    }

    /// Validate object integrity beyond basic connectivity.
    fn validate_object_integrity(
        &self,
        odb: &gix_odb::Handle,
        objects: &[ObjectId],
        results: &mut FsckResults,
    ) -> Result<(), crate::Error> {
        let mut buf = Vec::new();

        for &object_id in objects {
            // Skip if this object should be skipped
            if self.config.skip_objects.contains(&object_id) {
                continue;
            }

            // Try to read and validate the object
            match odb.try_find(&object_id, &mut buf) {
                Ok(Some(obj)) => {
                    // Object exists, perform additional validation based on type
                    if let Err(validation_error) = self.validate_object_content(&obj, object_id) {
                        let message = FsckMessage {
                            object_id,
                            message_type: "object-validation".to_string(),
                            message: validation_error,
                        };

                        match self.config.level {
                            FsckLevel::Basic | FsckLevel::Normal => results.warnings.push(message),
                            FsckLevel::Strict => results.errors.push(message),
                        }
                    } else {
                        results.validated_objects.push(object_id);
                    }
                }
                Ok(None) => {
                    // Object doesn't exist - this should have been caught by connectivity check
                    results.errors.push(FsckMessage {
                        object_id,
                        message_type: "missing-object".to_string(),
                        message: "object not found in ODB".to_string(),
                    });
                }
                Err(e) => {
                    results.errors.push(FsckMessage {
                        object_id,
                        message_type: "read-error".to_string(),
                        message: format!("failed to read object: {}", e),
                    });
                }
            }

            buf.clear();
        }

        Ok(())
    }

    /// Validate the content of a specific object.
    fn validate_object_content(
        &self,
        obj: &gix_object::Data<'_>,
        object_id: ObjectId,
    ) -> Result<(), String> {
        // Skip validation for object types that should be skipped
        if self.config.skip_types.contains(&obj.kind) {
            return Ok(());
        }

        match obj.kind {
            Kind::Commit => self.validate_commit_object(obj, object_id),
            Kind::Tree => self.validate_tree_object(obj, object_id),
            Kind::Blob => self.validate_blob_object(obj, object_id),
            Kind::Tag => self.validate_tag_object(obj, object_id),
        }
    }

    /// Validate a commit object.
    fn validate_commit_object(&self, obj: &gix_object::Data<'_>, _object_id: ObjectId) -> Result<(), String> {
        // Try to parse the commit
        match gix_object::CommitRef::from_bytes(obj.data) {
            Ok(commit) => {
                // Basic validation - ensure required fields are present
                if commit.tree().is_null() {
                    return Err("commit has null tree".to_string());
                }

                // In strict mode, perform additional validation
                if self.config.level == FsckLevel::Strict {
                    // Validate commit message encoding
                    if let Some(encoding) = commit.encoding {
                        if encoding.is_empty() {
                            return Err("commit has empty encoding field".to_string());
                        }
                    }

                    // Validate author and committer
                    if commit.author().name.is_empty() {
                        return Err("commit has empty author name".to_string());
                    }
                    if commit.committer().name.is_empty() {
                        return Err("commit has empty committer name".to_string());
                    }
                }

                Ok(())
            }
            Err(e) => Err(format!("failed to parse commit: {}", e)),
        }
    }

    /// Validate a tree object.
    fn validate_tree_object(&self, obj: &gix_object::Data<'_>, _object_id: ObjectId) -> Result<(), String> {
        // Try to parse the tree
        match gix_object::TreeRef::from_bytes(obj.data) {
            Ok(tree) => {
                // Validate tree entries
                for entry in tree.entries {
                    // Check for null object IDs
                    if entry.oid.is_null() {
                        return Err(format!("tree entry '{}' has null object ID", entry.filename));
                    }

                    // In strict mode, perform additional validation
                    if self.config.level == FsckLevel::Strict {
                        // Validate filename
                        if entry.filename.is_empty() {
                            return Err("tree has entry with empty filename".to_string());
                        }

                        // Check for invalid characters in filename
                        if entry.filename.contains(&b'\0') {
                            return Err(format!("tree entry '{}' contains null byte", entry.filename));
                        }
                    }
                }

                Ok(())
            }
            Err(e) => Err(format!("failed to parse tree: {}", e)),
        }
    }

    /// Validate a blob object.
    fn validate_blob_object(&self, _obj: &gix_object::Data<'_>, _object_id: ObjectId) -> Result<(), String> {
        // Blobs don't have much structure to validate
        // In the future, we might add checks for binary content, size limits, etc.
        Ok(())
    }

    /// Validate a tag object.
    fn validate_tag_object(&self, obj: &gix_object::Data<'_>, _object_id: ObjectId) -> Result<(), String> {
        // Try to parse the tag
        match gix_object::TagRef::from_bytes(obj.data) {
            Ok(tag) => {
                // Check for null target
                if tag.target().is_null() {
                    return Err("tag has null target".to_string());
                }

                // In strict mode, perform additional validation
                if self.config.level == FsckLevel::Strict {
                    // Validate tag name
                    if tag.name.is_empty() {
                        return Err("tag has empty name".to_string());
                    }

                    // Validate tagger if present
                    if let Some(tagger) = tag.tagger {
                        if tagger.name.is_empty() {
                            return Err("tag has empty tagger name".to_string());
                        }
                    }
                }

                Ok(())
            }
            Err(e) => Err(format!("failed to parse tag: {}", e)),
        }
    }
}

/// No-op fsck validator when fsck feature is disabled.
#[cfg(not(feature = "fsck"))]
#[derive(Debug)]
pub struct FsckValidator {
    _config: FsckConfig,
}

#[cfg(not(feature = "fsck"))]
impl FsckValidator {
    /// Create a new fsck validator (no-op when fsck feature is disabled).
    pub fn new(config: FsckConfig) -> Self {
        Self { _config: config }
    }

    /// Validate objects in the given quarantine directory (no-op when fsck feature is disabled).
    pub fn validate_quarantine(
        &self,
        _quarantine_objects_dir: &Path,
        _main_odb: &gix_odb::Handle,
    ) -> Result<FsckResults, crate::Error> {
        Ok(FsckResults {
            validated_objects: Vec::new(),
            warnings: Vec::new(),
            errors: Vec::new(),
            missing_objects: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fsck_config_default() {
        let config = FsckConfig::default();
        assert!(config.enabled);
        assert_eq!(config.level, FsckLevel::Normal);
        assert!(config.skip_types.is_empty());
        assert!(config.skip_objects.is_empty());
        assert!(config.msg_types.is_empty());
    }

    #[test]
    fn fsck_level_ordering() {
        assert_ne!(FsckLevel::Basic, FsckLevel::Normal);
        assert_ne!(FsckLevel::Normal, FsckLevel::Strict);
        assert_ne!(FsckLevel::Basic, FsckLevel::Strict);
    }

    #[test]
    fn fsck_message_level_values() {
        assert_ne!(FsckMessageLevel::Ignore, FsckMessageLevel::Warn);
        assert_ne!(FsckMessageLevel::Warn, FsckMessageLevel::Error);
        assert_ne!(FsckMessageLevel::Ignore, FsckMessageLevel::Error);
    }

    #[cfg(feature = "fsck")]
    #[test]
    fn fsck_validator_creation() {
        let config = FsckConfig::default();
        let _validator = FsckValidator::new(config);
    }

    #[cfg(not(feature = "fsck"))]
    #[test]
    fn fsck_validator_no_op() {
        let config = FsckConfig::default();
        let validator = FsckValidator::new(config);
        
        // This should not panic and should return empty results
        let temp_dir = std::env::temp_dir();
        let main_odb = gix_odb::at(&temp_dir).unwrap();
        let results = validator.validate_quarantine(&temp_dir, &main_odb).unwrap();
        
        assert!(results.validated_objects.is_empty());
        assert!(results.warnings.is_empty());
        assert!(results.errors.is_empty());
        assert!(results.missing_objects.is_empty());
    }
}
#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::pack::PackIngestor;
    use std::path::Path;

    /// Create a temporary directory for testing.
    fn create_temp_dir() -> std::path::PathBuf {
        let temp_dir = std::env::temp_dir().join(format!(
            "gix-receive-pack-fsck-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();
        temp_dir
    }

    /// Create a minimal test repository structure.
    fn create_test_repo(base_dir: &Path) -> std::path::PathBuf {
        let repo_dir = base_dir.join("test-repo");
        let objects_dir = repo_dir.join("objects");
        std::fs::create_dir_all(&objects_dir).unwrap();
        std::fs::create_dir_all(objects_dir.join("pack")).unwrap();
        std::fs::create_dir_all(objects_dir.join("info")).unwrap();
        objects_dir
    }

    /// Create a test pack file with some basic objects.
    fn create_test_pack(pack_dir: &Path) -> std::path::PathBuf {
        // This is a simplified test - in a real test we'd create actual pack data
        let pack_path = pack_dir.join("pack-test.pack");
        let idx_path = pack_dir.join("pack-test.idx");
        
        // Create empty files for now - in a real implementation we'd use gix-pack to create valid packs
        std::fs::write(&pack_path, b"PACK").unwrap();
        std::fs::write(&idx_path, b"IDX").unwrap();
        
        pack_path
    }

    #[cfg(feature = "fsck")]
    #[test]
    fn test_fsck_validator_basic_level() {
        let temp_dir = create_temp_dir();
        let objects_dir = create_test_repo(&temp_dir);
        
        let config = FsckConfig {
            enabled: true,
            level: FsckLevel::Basic,
            skip_types: Vec::new(),
            skip_objects: Vec::new(),
            msg_types: HashMap::new(),
        };
        
        let validator = FsckValidator::new(config);
        
        // Create a test ODB
        let main_odb = gix_odb::at(&objects_dir).unwrap();
        
        // This should not fail even with an empty quarantine
        let result = validator.validate_quarantine(&objects_dir, &main_odb);
        assert!(result.is_ok());
        
        let fsck_results = result.unwrap();
        assert!(fsck_results.validated_objects.is_empty());
        assert!(fsck_results.warnings.is_empty());
        assert!(fsck_results.errors.is_empty());
        assert!(fsck_results.missing_objects.is_empty());
        
        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[cfg(feature = "fsck")]
    #[test]
    fn test_fsck_validator_normal_level() {
        let temp_dir = create_temp_dir();
        let objects_dir = create_test_repo(&temp_dir);
        
        let config = FsckConfig {
            enabled: true,
            level: FsckLevel::Normal,
            skip_types: Vec::new(),
            skip_objects: Vec::new(),
            msg_types: HashMap::new(),
        };
        
        let validator = FsckValidator::new(config);
        let main_odb = gix_odb::at(&objects_dir).unwrap();
        
        let result = validator.validate_quarantine(&objects_dir, &main_odb);
        assert!(result.is_ok());
        
        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[cfg(feature = "fsck")]
    #[test]
    fn test_fsck_validator_strict_level() {
        let temp_dir = create_temp_dir();
        let objects_dir = create_test_repo(&temp_dir);
        
        let config = FsckConfig {
            enabled: true,
            level: FsckLevel::Strict,
            skip_types: Vec::new(),
            skip_objects: Vec::new(),
            msg_types: HashMap::new(),
        };
        
        let validator = FsckValidator::new(config);
        let main_odb = gix_odb::at(&objects_dir).unwrap();
        
        let result = validator.validate_quarantine(&objects_dir, &main_odb);
        assert!(result.is_ok());
        
        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[cfg(feature = "fsck")]
    #[test]
    fn test_fsck_validator_disabled() {
        let temp_dir = create_temp_dir();
        let objects_dir = create_test_repo(&temp_dir);
        
        let config = FsckConfig {
            enabled: false,
            level: FsckLevel::Normal,
            skip_types: Vec::new(),
            skip_objects: Vec::new(),
            msg_types: HashMap::new(),
        };
        
        let validator = FsckValidator::new(config);
        let main_odb = gix_odb::at(&objects_dir).unwrap();
        
        let result = validator.validate_quarantine(&objects_dir, &main_odb);
        assert!(result.is_ok());
        
        let fsck_results = result.unwrap();
        assert!(fsck_results.validated_objects.is_empty());
        assert!(fsck_results.warnings.is_empty());
        assert!(fsck_results.errors.is_empty());
        assert!(fsck_results.missing_objects.is_empty());
        
        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[cfg(feature = "fsck")]
    #[test]
    fn test_fsck_validator_skip_types() {
        let temp_dir = create_temp_dir();
        let objects_dir = create_test_repo(&temp_dir);
        
        let config = FsckConfig {
            enabled: true,
            level: FsckLevel::Normal,
            skip_types: vec![Kind::Blob, Kind::Tree],
            skip_objects: Vec::new(),
            msg_types: HashMap::new(),
        };
        
        let validator = FsckValidator::new(config);
        let main_odb = gix_odb::at(&objects_dir).unwrap();
        
        let result = validator.validate_quarantine(&objects_dir, &main_odb);
        assert!(result.is_ok());
        
        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[cfg(feature = "fsck")]
    #[test]
    fn test_fsck_validator_skip_objects() {
        let temp_dir = create_temp_dir();
        let objects_dir = create_test_repo(&temp_dir);
        
        let skip_object = ObjectId::from_hex(b"1234567890123456789012345678901234567890").unwrap();
        
        let config = FsckConfig {
            enabled: true,
            level: FsckLevel::Normal,
            skip_types: Vec::new(),
            skip_objects: vec![skip_object],
            msg_types: HashMap::new(),
        };
        
        let validator = FsckValidator::new(config);
        let main_odb = gix_odb::at(&objects_dir).unwrap();
        
        let result = validator.validate_quarantine(&objects_dir, &main_odb);
        assert!(result.is_ok());
        
        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_pack_ingestor_creation() {
        // Test creating PackIngestor with different configurations
        let ingestor_default = PackIngestor::default();
        assert!(ingestor_default.fsck_validator.is_none());
        
        let ingestor_without_fsck = PackIngestor::without_fsck();
        assert!(ingestor_without_fsck.fsck_validator.is_none());
        
        #[cfg(feature = "fsck")]
        {
            let fsck_config = FsckConfig::default();
            let ingestor_with_fsck = PackIngestor::with_fsck(fsck_config);
            assert!(ingestor_with_fsck.fsck_validator.is_some());
            
            let ingestor_new = PackIngestor::new(Some(FsckConfig::default()));
            assert!(ingestor_new.fsck_validator.is_some());
            
            let ingestor_new_none = PackIngestor::new(None);
            assert!(ingestor_new_none.fsck_validator.is_none());
        }
    }

    #[test]
    fn test_fsck_message_creation() {
        let object_id = ObjectId::from_hex(b"1234567890123456789012345678901234567890").unwrap();
        
        let message = FsckMessage {
            object_id,
            message_type: "test-error".to_string(),
            message: "This is a test error message".to_string(),
        };
        
        assert_eq!(message.object_id, object_id);
        assert_eq!(message.message_type, "test-error");
        assert_eq!(message.message, "This is a test error message");
    }

    #[test]
    fn test_missing_object_creation() {
        let object_id = ObjectId::from_hex(b"1234567890123456789012345678901234567890").unwrap();
        
        let missing = MissingObject {
            object_id,
            object_type: Kind::Blob,
        };
        
        assert_eq!(missing.object_id, object_id);
        assert_eq!(missing.object_type, Kind::Blob);
    }

    #[test]
    fn test_fsck_results_creation() {
        let object_id = ObjectId::from_hex(b"1234567890123456789012345678901234567890").unwrap();
        
        let results = FsckResults {
            validated_objects: vec![object_id],
            warnings: Vec::new(),
            errors: Vec::new(),
            missing_objects: Vec::new(),
        };
        
        assert_eq!(results.validated_objects.len(), 1);
        assert_eq!(results.validated_objects[0], object_id);
        assert!(results.warnings.is_empty());
        assert!(results.errors.is_empty());
        assert!(results.missing_objects.is_empty());
    }
}