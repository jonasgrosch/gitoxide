//! Reference management and advertisement
//!
//! This module centralizes all reference-related operations including
//! collection, filtering, and advertisement formatting.

use crate::{
    error::{Error, Result},
    types::*,
};
use bstr::{BStr, ByteSlice, ByteVec};
use gix::Repository;

/// Reference manager for handling reference operations
pub struct ReferenceManager<'a> {
    repository: &'a Repository,
    hidden_patterns: &'a [bstr::BString],
}

impl<'a> ReferenceManager<'a> {
    /// Create a new reference manager
    pub fn new(repository: &'a Repository, hidden_patterns: &'a [bstr::BString]) -> Self {
        Self {
            repository,
            hidden_patterns,
        }
    }

    /// Collect all references that should be advertised
    /// Following the same logical flow as v2 protocol without sorting
    pub fn collect_advertised_references(&self) -> Result<Vec<Reference>> {
        self.collect_references_with_prefixes(&[])
    }

    /// Collect references with optional prefix filtering (for v2 protocol)
    pub fn collect_references_with_prefixes(&self, prefixes: &[String]) -> Result<Vec<Reference>> {
        let mut refs = Vec::new();

        // Add HEAD first if it exists - following v2 pattern
        if let Ok(head) = self.repository.head() {
            match head.kind {
                gix::head::Kind::Symbolic(target_ref) => {
                    if let gix::refs::Target::Object(oid) = &target_ref.target {
                        refs.push(ProtocolRef::Symbolic {
                            full_ref_name: "HEAD".into(),
                            target: target_ref.name.as_bstr().to_owned(),
                            tag: None,
                            object: *oid,
                        });
                    }
                }
                gix::head::Kind::Detached { target, .. } => {
                    refs.push(ProtocolRef::Direct {
                        full_ref_name: "HEAD".into(),
                        object: target,
                    });
                }
                gix::head::Kind::Unborn(_) => {
                    // Skip unborn HEAD as it has no commit to advertise
                }
            }
        }

        // Get all other references - process in natural order like v2
        let reference_store = self.repository.references().map_err(|e| Error::RefPackedBuffer(e))?;
        let references = reference_store.all().map_err(|e| Error::RefIterInit(e))?;

        // Collect references that match prefixes (if any)
        let mut filtered_refs = Vec::new();
        for reference in references {
            if let Ok(reference) = reference {
                let ref_name = reference.name().as_bstr().to_str_lossy();

                // Apply prefix filtering if prefixes are specified
                if !prefixes.is_empty() {
                    let matches_prefix = prefixes.iter().any(|prefix| ref_name.starts_with(prefix));
                    if matches_prefix {
                        filtered_refs.push(reference);
                    }
                } else {
                    // No prefix filtering - include all references
                    filtered_refs.push(reference);
                }
            }
        }

        // Process the filtered references
        for reference in filtered_refs {
            let name = reference.name().as_bstr().to_owned();

            // Skip hidden references
            if self.is_ref_hidden(name.as_ref()) {
                continue;
            }

            match reference.target() {
                gix::refs::TargetRef::Symbolic(target_ref_name) => {
                    // Follow v2 pattern for symbolic refs
                    let object = reference.follow();
                    if let Some(Ok(resolved_ref)) = object {
                        refs.push(ProtocolRef::Symbolic {
                            full_ref_name: name,
                            target: target_ref_name.as_bstr().to_owned(),
                            tag: None,
                            object: resolved_ref.target().id().to_owned(),
                        });
                    }
                }
                gix::refs::TargetRef::Object(oid) => {
                    let target = oid.to_owned();

                    // Add the main reference
                    refs.push(ProtocolRef::Direct {
                        full_ref_name: name.clone(),
                        object: target,
                    });

                    // For tags, immediately add peeled version if it's an annotated tag
                    if name.starts_with_str("refs/tags/") {
                        if let Some(peeled) = self
                            .repository
                            .find_tag(target)
                            .ok()
                            .and_then(|tag| tag.target_id().ok())
                            .map(|id| id.detach())
                        {
                            // Add peeled tag immediately after the tag reference
                            let mut peeled_name = name.clone();
                            peeled_name.push_str("^{}");
                            refs.push(ProtocolRef::Direct {
                                full_ref_name: peeled_name,
                                object: peeled,
                            });
                        }
                    }
                }
            }
        }

        // No sorting - let gix return references in natural order
        Ok(refs)
    }

    /// Check if a reference should be hidden based on patterns
    fn is_ref_hidden(&self, ref_name: &BStr) -> bool {
        let ref_str = ref_name.to_str_lossy();

        for pattern in self.hidden_patterns {
            if self.matches_pattern(&ref_str, pattern.to_str_lossy().as_ref()) {
                return true;
            }
        }

        false
    }

    /// Check if a reference name matches a pattern (simple glob matching)
    fn matches_pattern(&self, ref_name: &str, pattern: &str) -> bool {
        if pattern.ends_with("*") {
            let prefix = &pattern[..pattern.len() - 1];
            ref_name.starts_with(prefix)
        } else if pattern.starts_with("*") {
            let suffix = &pattern[1..];
            ref_name.ends_with(suffix)
        } else {
            ref_name == pattern
        }
    }

    /// Format references for protocol v1 advertisement
    pub fn format_v1_advertisement(&self, refs: &[Reference], capabilities: &str) -> Result<Vec<String>> {
        let mut lines = Vec::new();

        if refs.is_empty() {
            // No refs case - send capabilities only
            let null_oid = gix_hash::ObjectId::null(self.repository.object_hash());
            lines.push(format!("{} capabilities^{{}}\0{}", null_oid.to_hex(), capabilities));
        } else {
            // Send first ref with capabilities
            let first_ref = &refs[0];
            let (name, target, _) = first_ref.unpack();
            let null_oid = gix_hash::ObjectId::null(self.repository.object_hash());
            let target_oid = target.unwrap_or(&null_oid);
            lines.push(format!(
                "{} {}\0{}",
                target_oid.to_hex(),
                name.to_str_lossy(),
                capabilities
            ));

            // Send remaining refs without capabilities
            for reference in refs.iter().skip(1) {
                let (name, target, _) = reference.unpack();
                let target_oid = target.unwrap_or(&null_oid);
                lines.push(format!("{} {}", target_oid.to_hex(), name.to_str_lossy()));
            }

            // Peeled refs are already included in the main reference list
        }

        Ok(lines)
    }

    /// Format references for protocol v2 ls-refs command
    pub fn format_v2_ls_refs(
        &self,
        refs: &[Reference],
        args: &std::collections::HashMap<String, String>,
    ) -> Result<Vec<String>> {
        let mut lines = Vec::new();
        let show_symrefs = args.contains_key("symrefs");
        let show_peeled = args.contains_key("peel");

        // Filter by ref-prefix if specified
        let prefixes: Vec<&str> = args.keys().filter_map(|k| k.strip_prefix("ref-prefix ")).collect();

        for reference in refs {
            let (name, target, peeled) = reference.unpack();

            // Apply ref-prefix filtering
            if !prefixes.is_empty() {
                let name_str = name.to_str_lossy();
                if !prefixes.iter().any(|prefix| name_str.starts_with(prefix)) {
                    continue;
                }
            }

            if let Some(target_oid) = target {
                let mut line = format!("{} {}", target_oid.to_hex(), name.to_str_lossy());

                // Add symref info if requested and this is HEAD
                if show_symrefs && name == "HEAD" {
                    if let Ok(head) = self.repository.head() {
                        if let gix::head::Kind::Symbolic(target_ref) = head.kind {
                            line.push_str(&format!(" symref-target:{}", target_ref.name.as_bstr().to_str_lossy()));
                        }
                    }
                }

                lines.push(line);

                // Add peeled info if requested and available
                if show_peeled {
                    if let Some(peeled_oid) = peeled {
                        lines.push(format!("{} {}^{{}}", peeled_oid.to_hex(), name.to_str_lossy()));
                    }
                }
            }
        }

        Ok(lines)
    }
}
