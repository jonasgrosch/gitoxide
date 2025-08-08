// M4: Shallow planning
//
// Build a plan from client-provided shallow/unshallow lines and validate consistency.
// This module intentionally doesn't perform any repository I/O; it only derives a
// plan that later components can apply against the repository's shallow state.

use std::collections::HashSet;

use gix_hash::ObjectId;

use crate::Error;
use crate::protocol::options::Options;

/// A plan describing how to update shallow boundaries.
///
/// - `to_add`: OIDs that should become shallow (as seen in `shallow <oid>`).
/// - `to_remove`: OIDs that should become unshallow (as seen in `unshallow <oid>`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShallowPlan {
    pub to_add: Vec<ObjectId>,
    pub to_remove: Vec<ObjectId>,
}

impl ShallowPlan {
    /// Build a shallow-update plan from parsed head-info `Options`.
    ///
    /// Rules and validation:
    /// - An OID must not be present in both `shallow` and `unshallow` lists.
    ///   If it is, this is a protocol/validation error.
    /// - Duplicates in either list are de-duplicated.
    pub fn from_options(opts: &Options) -> Result<Self, Error> {
        // De-duplicate while preserving the first-seen order.
        let mut seen_add = HashSet::<ObjectId>::new();
        let mut to_add = Vec::new();
        for oid in &opts.shallow {
            if seen_add.insert(*oid) {
                to_add.push(*oid);
            }
        }

        let mut seen_remove = HashSet::<ObjectId>::new();
        let mut to_remove = Vec::new();
        for oid in &opts.unshallow {
            if seen_remove.insert(*oid) {
                to_remove.push(*oid);
            }
        }

        // Validate there is no overlap.
        let set_add: HashSet<_> = to_add.iter().copied().collect();
        let set_remove: HashSet<_> = to_remove.iter().copied().collect();
        let conflict: Vec<_> = set_add.intersection(&set_remove).copied().collect();
        if !conflict.is_empty() {
            return Err(Error::Validation(format!(
                "conflicting shallow updates: OIDs present in both shallow and unshallow: {:?}",
                conflict
            )));
        }

        Ok(ShallowPlan { to_add, to_remove })
    }

    /// True if the plan doesn't perform any changes.
    pub fn is_empty(&self) -> bool {
        self.to_add.is_empty() && self.to_remove.is_empty()
    }

    /// Return a tuple of slices to ease downstream application or matrix building.
    pub fn as_slices(&self) -> (&[ObjectId], &[ObjectId]) {
        (&self.to_add, &self.to_remove)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oid(hex40: &str) -> ObjectId {
        ObjectId::from_hex(hex40.as_bytes()).expect("valid hex")
    }

    #[test]
    fn plan_from_options_basic() {
        let mut opts = Options::default();
        opts.add_shallow_oid(oid("1111111111111111111111111111111111111111"));
        opts.add_unshallow_oid(oid("2222222222222222222222222222222222222222"));

        let plan = ShallowPlan::from_options(&opts).unwrap();
        assert_eq!(plan.to_add, vec![oid("1111111111111111111111111111111111111111")]);
        assert_eq!(plan.to_remove, vec![oid("2222222222222222222222222222222222222222")]);
        assert!(!plan.is_empty());
    }

    #[test]
    fn duplicates_are_deduplicated() {
        let mut opts = Options::default();
        let a = oid("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        opts.add_shallow_oid(a);
        opts.add_shallow_oid(a);

        let plan = ShallowPlan::from_options(&opts).unwrap();
        assert_eq!(plan.to_add, vec![a]);
        assert!(plan.to_remove.is_empty());
    }

    #[test]
    fn conflict_detection() {
        let mut opts = Options::default();
        let o = oid("ffffffffffffffffffffffffffffffffffffffff");
        opts.add_shallow_oid(o);
        opts.add_unshallow_oid(o);

        let err = ShallowPlan::from_options(&opts).unwrap_err();
        match err {
            Error::Validation(_) => {}
            other => panic!("expected Validation error, got {other:?}"),
        }
    }
}