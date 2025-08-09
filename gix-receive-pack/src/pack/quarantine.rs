use std::path::PathBuf;

/// Quarantine directory for safe pack ingestion.
/// 
/// This provides a temporary directory structure that can be safely cleaned up
/// if the operation fails, or migrated to the main objects directory on success.
pub struct Quarantine {
    /// The main objects directory (.git/objects)
    main_objects_dir: PathBuf,
    /// The quarantine objects directory (temporary)
    pub objects_dir: PathBuf,
    /// Whether the quarantine is currently active
    active: bool,
}

impl Quarantine {
    /// Create a new quarantine for the given objects directory.
    pub fn new(main_objects_dir: PathBuf) -> Self {
        Self {
            main_objects_dir,
            objects_dir: PathBuf::new(),
            active: false,
        }
    }
    
    /// Activate the quarantine by creating the temporary directory structure.
    pub fn activate(&mut self) -> Result<(), std::io::Error> {
        if self.active {
            return Ok(());
        }
        
        // Create quarantine directory using a simple approach
        let quarantine_dir = self.main_objects_dir.join("quarantine").join(format!("tmp-{}", std::process::id()));
        std::fs::create_dir_all(&quarantine_dir)?;
        
        // Setup alternates file to point to main objects directory
        let alternates_file = quarantine_dir.join("info/alternates");
        std::fs::create_dir_all(alternates_file.parent().unwrap())?;
        std::fs::write(&alternates_file, self.main_objects_dir.to_string_lossy().as_bytes())?;
        
        self.objects_dir = quarantine_dir;
        self.active = true;
        
        Ok(())
    }
    
    /// Check if the quarantine is currently active.
    pub fn is_active(&self) -> bool {
        self.active
    }
    
    /// Migrate the quarantine contents to the main objects directory on success.
    pub fn migrate_on_success(&mut self) -> Result<(), std::io::Error> {
        if !self.active {
            return Ok(());
        }
        
        // Move all objects from quarantine to main objects directory
        if self.objects_dir.exists() {
            for entry in std::fs::read_dir(&self.objects_dir)? {
                let entry = entry?;
                let path = entry.path();
                
                // Skip the info directory (contains alternates)
                if path.file_name().unwrap() == "info" {
                    continue;
                }
                
                let dest = self.main_objects_dir.join(entry.file_name());
                if path.is_dir() {
                    // Move directory recursively
                    self.move_dir_recursive(&path, &dest)?;
                } else {
                    // Move file
                    std::fs::rename(&path, &dest)?;
                }
            }
            
            // Clean up quarantine directory
            std::fs::remove_dir_all(&self.objects_dir)?;
        }
        
        self.active = false;
        Ok(())
    }
    
    /// Drop the quarantine on failure, cleaning up temporary files.
    pub fn drop_on_failure(&mut self) -> Result<(), std::io::Error> {
        if !self.active {
            return Ok(());
        }
        
        // Remove the entire quarantine directory
        if self.objects_dir.exists() {
            std::fs::remove_dir_all(&self.objects_dir)?;
        }
        
        self.active = false;
        Ok(())
    }
    
    /// Helper to move directories recursively.
    fn move_dir_recursive(&self, src: &std::path::Path, dest: &std::path::Path) -> Result<(), std::io::Error> {
        std::fs::create_dir_all(dest)?;
        
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let src_path = entry.path();
            let dest_path = dest.join(entry.file_name());
            
            if src_path.is_dir() {
                self.move_dir_recursive(&src_path, &dest_path)?;
            } else {
                std::fs::rename(&src_path, &dest_path)?;
            }
        }
        
        std::fs::remove_dir(src)?;
        Ok(())
    }
}

impl Drop for Quarantine {
    fn drop(&mut self) {
        if self.active {
            let _ = self.drop_on_failure();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    
    #[test]
    fn test_quarantine_lifecycle() {
        let temp = tempdir().unwrap();
        let objects_dir = temp.path().join("objects");
        std::fs::create_dir_all(&objects_dir).unwrap();
        
        let mut quarantine = Quarantine::new(objects_dir.clone());
        assert!(!quarantine.is_active());
        
        // Activate quarantine
        quarantine.activate().unwrap();
        assert!(quarantine.is_active());
        assert!(quarantine.objects_dir.exists());
        
        // Check alternates file
        let alternates_file = quarantine.objects_dir.join("info/alternates");
        assert!(alternates_file.exists());
        let content = std::fs::read_to_string(&alternates_file).unwrap();
        assert!(content.contains("objects"));
        
        // Migrate on success
        quarantine.migrate_on_success().unwrap();
        assert!(!quarantine.is_active());
    }
    
    #[test]
    fn test_quarantine_cleanup_on_drop() {
        let temp = tempdir().unwrap();
        let objects_dir = temp.path().join("objects");
        std::fs::create_dir_all(&objects_dir).unwrap();
        
        let quarantine_path = {
            let mut quarantine = Quarantine::new(objects_dir);
            quarantine.activate().unwrap();
            quarantine.objects_dir.clone()
        }; // quarantine dropped here
        
        // Directory should be cleaned up
        assert!(!quarantine_path.exists());
    }
}