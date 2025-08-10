//! Shared visibility primitives for advertising references safely.

use gix_hash::ObjectId;
use std::sync::Arc;

/// A reference record with its object id and name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefRecord {
    /// The object id the reference points to.
    pub id: ObjectId,
    /// The fully qualified reference name.
    pub name: String,
}

impl RefRecord {
    /// Create a new reference record.
    pub fn new(id: ObjectId, name: impl Into<String>) -> Self {
        Self { id, name: name.into() }
    }
}

/// A predicate that determines whether a ref should be hidden.
pub type HiddenRefPredicate = dyn Fn(&RefRecord) -> bool + Send + Sync;

/// Resolver for collecting visible reference roots from a repository.
///
/// This is moved from receive-pack to a shared location.
pub struct VisibleRoots<'r> {
    repo: &'r gix::Repository,
    hidden: Arc<HiddenRefPredicate>,
}

impl<'r> VisibleRoots<'r> {
    /// Create a new resolver.
    pub fn new(repo: &'r gix::Repository, hidden: Arc<HiddenRefPredicate>) -> Self {
        Self { repo, hidden }
    }

    /// Collect visible refs as (name, id) pairs.
    pub fn collect(&self) -> Result<Vec<(String, ObjectId)>, String> {
        let mut out = Vec::new();
        let refs = self
            .repo
            .references()
            .map_err(|e| format!("reference iteration: {e}"))?;
        let iter = refs
            .all()
            .map_err(|e| format!("reference iteration: {e}"))?;
        for reference_result in iter {
            let reference = reference_result.map_err(|e| format!("reference iteration: {e}"))?;
            let refname = reference.name().as_bstr().to_string();
            let id = match reference.target() {
                gix::refs::TargetRef::Object(oid) => oid.to_owned(),
                gix::refs::TargetRef::Symbolic(target) => match self.repo.find_reference(target) {
                    Ok(target_ref) => match target_ref.target() {
                        gix::refs::TargetRef::Object(oid) => oid.to_owned(),
                        _ => continue,
                    },
                    Err(_) => continue,
                },
            };
            let record = RefRecord::new(id, &refname);
            if !(self.hidden)(&record) {
                out.push((refname, record.id));
            }
        }
        Ok(out)
    }
}


