// M3: Pack ingestion & quarantine scaffolding.
//
// This module provides compile-only stubs for:
// - Path selection between index-pack and unpack-objects based on transfer.unpackLimit.
// - A quarantine lifecycle with activate/migrate/drop.
// - Ingestor entry points to be wired to gix-pack in later milestones.
//
// Keep constructors free of I/O; all methods here are no-ops returning Ok(()).

/// Which path to use to ingest an incoming pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackIngestPath {
    /// Use `index-pack` style ingestion to create a pack index and keep the pack.
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
    /// Rules (scaffold):
    /// - If both a limit and a count are present and count <= limit, use UnpackObjects.
    /// - Otherwise, fall back to IndexPack.
    pub fn choose_path(&self, object_count_hint: Option<u64>) -> PackIngestPath {
        match (self.unpack_limit, object_count_hint) {
            (Some(limit), Some(count)) if count <= limit => PackIngestPath::UnpackObjects,
            _ => PackIngestPath::IndexPack,
        }
    }
}

/// Stubbed pack ingestor that will call into gix-pack in a future milestone.
#[derive(Debug, Default)]
pub struct PackIngestor;

impl PackIngestor {
    /// Placeholder for index-pack ingestion.
    pub fn index_pack() -> Result<(), crate::Error> {
        Ok(())
    }

    /// Placeholder for unpack-objects ingestion.
    pub fn unpack_objects() -> Result<(), crate::Error> {
        Ok(())
    }
}

/// Quarantine container for received objects.
///
/// In a future milestone this will use gix-tempfile to create a temporary object store
/// and configure alternates to the main ODB, migrating objects on success.
#[derive(Debug, Default)]
pub struct Quarantine;

impl Quarantine {
    /// Create a new quarantine container (no I/O in scaffold).
    pub fn new() -> Self {
        Quarantine
    }

    /// Activate the quarantine environment (no-op in scaffold).
    pub fn activate(&mut self) -> Result<(), crate::Error> {
        Ok(())
    }

    /// Migrate quarantined objects into the main store on success (no-op in scaffold).
    pub fn migrate_on_success(&mut self) -> Result<(), crate::Error> {
        Ok(())
    }

    /// Drop quarantined objects on failure (no-op in scaffold).
    pub fn drop_on_failure(&mut self) -> Result<(), crate::Error> {
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
}