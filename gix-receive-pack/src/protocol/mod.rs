 // M1: Protocol module surface (blocking-first). Async parity will be added behind feature flags.
pub mod capabilities;
pub mod advertise;
// M2: Options and commands parsing (blocking-first).
pub mod options;
pub mod commands;

use gix_hash::ObjectId;

/// A single advertised reference with its object id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefRecord {
    pub oid: ObjectId,
    pub name: String,
}

impl RefRecord {
    pub fn new(oid: ObjectId, name: impl Into<String>) -> Self {
        Self { oid, name: name.into() }
    }
}

/// Predicate to determine whether a ref is hidden and should not be advertised.
pub type HiddenRefPredicate = dyn Fn(&RefRecord) -> bool + Send + Sync;

/// Re-exports for crate users.
pub use capabilities::{CapabilityOrdering, CapabilitySet};
pub use advertise::Advertiser;
pub use options::Options;
pub use commands::{CommandList, CommandUpdate};