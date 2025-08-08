// M3: Pack ingestion & quarantine implementation.
//
// This module provides:
// - Policy to choose between index-pack and unpack-objects based on transfer.unpackLimit.
// - Quarantine lifecycle with activation (tmp ODB + alternates), migration on success, and drop on failure.
// - Blocking ingestion from a BufRead using gix-pack::Bundle into the quarantine, with thin-pack base lookup via gix-odb.
//
// Notes
// - Keep constructors free of I/O; activation performs the filesystem work.
// - We route UnpackObjects to IndexPack for now; a dedicated unpack path can be added later if needed.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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
}

impl IngestionPolicy {
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
}

/// Stubbed pack ingestor for compile-only paths, and full ingestion behind `progress`.
#[derive(Debug, Default)]
pub struct PackIngestor;

impl PackIngestor {
    /// Placeholder no-op for index-pack ingestion used by scaffold-only entrypoints.
    pub fn index_pack_stub() -> Result<(), crate::Error> {
        Ok(())
    }

    /// Placeholder no-op for unpack-objects ingestion used by scaffold-only entrypoints.
    pub fn unpack_objects_stub() -> Result<(), crate::Error> {
        Ok(())
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
        input: &mut dyn io::BufRead,
        quarantine_objects_dir: &Path,
        pack_size: Option<u64>,
        thin_pack_lookup: Option<gix_odb::Handle>,
        progress: &mut dyn gix_features::progress::prodash::DynNestedProgress,
    ) -> Result<(), crate::Error> {
        use gix_pack::bundle::write::{Options, Outcome};
        use std::sync::atomic::AtomicBool;

        // gix-pack expects the 'objects/pack' directory as target.
        let pack_dir = quarantine_objects_dir.join("pack");
        fs::create_dir_all(&pack_dir)?;

        let should_interrupt = AtomicBool::new(false);

        let options = Options {
            // Defaults: Verify mode, index version default, object hash default
            ..Default::default()
        };

        let out: Outcome = match thin_pack_lookup {
            Some(handle) => {
                // Use thin-pack lookup to fix up deltas against the main ODB if required.
                gix_pack::Bundle::write_to_directory(
                    input,
                    Some(pack_dir.as_path()),
                    progress,
                    &should_interrupt,
                    Some(handle),
                    options,
                )
                .map_err(|e| crate::Error::Protocol(e.to_string()))?
            }
            None => {
                gix_pack::Bundle::write_to_directory(
                    input,
                    Some(pack_dir.as_path()),
                    progress,
                    &should_interrupt,
                    Option::<gix_odb::Handle>::None,
                    options,
                )
                .map_err(|e| crate::Error::Protocol(e.to_string()))?
            }
        };

        // A successful write implies the pack and index exist in the quarantine pack dir.
        // Nothing else to do here; migration is handled by the Quarantine lifecycle.
        let _ = out;
        Ok(())
    }
}

/// Quarantine container for received objects.
///
/// The quarantine is a temporary ODB with an 'info/alternates' pointing to the main ODB, so that thin packs
/// and lookups can resolve missing bases while keeping new objects isolated. On success, its packs are migrated
/// into the main ODB; on failure, it is dropped.
#[derive(Debug, Default)]
pub struct Quarantine {
    /// The main repository objects directory (.git/objects)
    pub main_objects_dir: PathBuf,
    /// The quarantine objects directory (…/tmp-objdir-…/objects)
    pub objects_dir: PathBuf,
    /// Path to the quarantine 'info/alternates' file
    alternates_file: PathBuf,
    /// Whether activation succeeded
    active: bool,
}

impl Quarantine {
    /// Create a quarantine instance targeted at the given main objects directory (.git/objects).
    ///
    /// No I/O is performed here. Call `activate()` to create the quarantine on disk.
    pub fn new(main_objects_dir: impl Into<PathBuf>) -> Self {
        let main = main_objects_dir.into();
        let base = std::env::temp_dir().join(format!(
            "gix-receive-pack-quarantine-{}-{}",
            std::process::id(),
            Self::ts()
        ));
        let objects_dir = base.join("objects");
        let alternates_file = objects_dir.join("info").join("alternates");

        Self {
            main_objects_dir: main,
            objects_dir,
            alternates_file,
            active: false,
        }
    }

    fn ts() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    }

    /// Activate the quarantine environment:
    /// - Creates the quarantine objects directory with 'info' and 'pack' subdirectories.
    /// - Writes 'info/alternates' to point to the main objects directory.
    pub fn activate(&mut self) -> Result<(), crate::Error> {
        // directories
        fs::create_dir_all(self.objects_dir.join("info"))?;
        fs::create_dir_all(self.objects_dir.join("pack"))?;

        // 'info/alternates' must contain the path to the main objects directory, one per line.
        {
        let mut line = self.main_objects_dir.display().to_string();
        if !line.ends_with('\n') {
            line.push('\n');
        }
        fs::write(&self.alternates_file, line.as_bytes())?;
    }
        self.active = true;
        Ok(())
    }

    /// Migrate quarantined packs into the main objects/pack directory and clean up quarantine.
    pub fn migrate_on_success(&mut self) -> Result<(), crate::Error> {
        if !self.active {
            return Ok(());
        }
        let src_pack = self.objects_dir.join("pack");
        let dst_pack = self.main_objects_dir.join("pack");
        fs::create_dir_all(&dst_pack)?;

        if src_pack.is_dir() {
            for entry in fs::read_dir(&src_pack)? {
                let entry = entry?;
                let path = entry.path();
                if let Some(name) = path.file_name() {
                    // Only move pack artifacts: pack-*.pack, pack-*.idx, pack-*.keep
                    let name_s = name.to_string_lossy();
                    if name_s.starts_with("pack-") && (name_s.ends_with(".pack") || name_s.ends_with(".idx") || name_s.ends_with(".keep")) {
                        let dst = dst_pack.join(name);
                        // Try rename first; fallback to copy+remove if needed.
                        match fs::rename(&path, &dst) {
                            Ok(_) => {}
                            Err(_) => {
                                fs::copy(&path, &dst)?;
                                fs::remove_file(&path)?;
                            }
                        }
                    }
                }
            }
        }

        // Remove alternates and quarantine directories.
        let _ = fs::remove_file(&self.alternates_file);
        let base = self
            .objects_dir
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| self.objects_dir.clone());
        let _ = fs::remove_dir_all(base);
        self.active = false;
        Ok(())
    }

    /// Drop quarantined content on failure and clean up on disk.
    pub fn drop_on_failure(&mut self) -> Result<(), crate::Error> {
        if !self.active {
            return Ok(());
        }
        let _ = fs::remove_file(&self.alternates_file);
        let base = self
            .objects_dir
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| self.objects_dir.clone());
        let _ = fs::remove_dir_all(base);
        self.active = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_choose_path() {
        let pol = IngestionPolicy { unpack_limit: Some(100) };
        assert!(matches!(pol.choose_path(Some(50)), PackIngestPath::UnpackObjects));
        assert!(matches!(pol.choose_path(Some(150)), PackIngestPath::IndexPack));
        assert!(matches!(pol.choose_path(None), PackIngestPath::IndexPack));

        let pol2 = IngestionPolicy { unpack_limit: None };
        assert!(matches!(pol2.choose_path(Some(1)), PackIngestPath::IndexPack));
    }

    #[test]
    fn quarantine_paths_form() {
        let q = Quarantine::new("/tmp/main-objects");
        assert!(q.alternates_file.display().to_string().contains("info/alternates"));
        assert!(q.objects_dir.display().to_string().contains("objects"));
        assert_eq!(q.main_objects_dir, PathBuf::from("/tmp/main-objects"));
    }
}