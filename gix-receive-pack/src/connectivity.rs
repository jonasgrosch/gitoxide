// M4: Connectivity checking
//
// This module provides a pluggable ConnectivityChecker trait and a default, minimal
// implementation that honors hidden refs by design (callers pass only visible refs),
// supports a deferral policy, and can optionally emit progress when the "progress"
// feature is enabled. Parallel execution is modeled via a configuration flag but not
// implemented yet to keep the surface stable and compilable.

use crate::protocol::{CommandUpdate, RefRecord};
use crate::Error;

/// Configuration for connectivity checking.
#[derive(Debug, Clone)]
pub struct ConnectivityConfig {
    /// If true and the "parallel" feature is enabled, an implementation may use a thread-pool.
    pub parallel: bool,
    /// Rate limit for progress emission (milliseconds); None disables rate limiting.
    pub progress_rate_limit_ms: Option<u64>,
    /// If true, allow per-ref deferred reachability checks based on workload.
    pub defer_per_ref: bool,
    /// Maximum number of refs to check in this pass when deferral is enabled.
    /// Remaining refs will be returned in `ConnectivityOutcome::deferred_refs`.
    pub defer_limit: Option<usize>,
}

impl Default for ConnectivityConfig {
    fn default() -> Self {
        Self {
            parallel: false,
            progress_rate_limit_ms: Some(100),
            defer_per_ref: false,
            defer_limit: None,
        }
    }
}

/// Result of a connectivity check.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConnectivityOutcome {
    /// Total number of refs that were considered for checking.
    pub total_refs: usize,
    /// Number of refs that were actually checked in this pass.
    pub checked_refs: usize,
    /// Ref names that were deferred for later checking (deferral policy).
    pub deferred_refs: Vec<String>,
    /// True if the connectivity check completed without detecting unreachable objects.
    /// The default implementation here does not perform real reachability and thus
    /// always sets this to true.
    pub ok: bool,
}

/// Trait for checking object connectivity after receiving a pack.
///
/// Implementations are expected to:
/// - Exclude hidden refs by design (callers should pass only visible refs).
/// - Optionally emit progress messages when the "progress" feature is enabled.
/// - Support a deferral policy to avoid long pauses on heavily loaded servers.
pub trait ConnectivityChecker {
    /// Check connectivity for the provided updates against the visible reference set.
    ///
    /// Parameters
    /// - `updates`: Command updates parsed from head-info (create, update, delete).
    /// - `visible_refs`: Refs that are visible (hidden refs are excluded by the caller).
    ///
    /// Returns a ConnectivityOutcome describing what was checked and what was deferred.
    fn check(
        &mut self,
        updates: &[CommandUpdate],
        visible_refs: &[RefRecord],
    ) -> Result<ConnectivityOutcome, Error>;

    /// Whether this checker is configured to attempt parallel execution.
    fn is_parallel(&self) -> bool;
}

/// A default, minimal connectivity checker that provides configuration knobs but
/// does not yet perform real graph traversal.
///
/// It models:
/// - Hidden ref exclusion implicitly by operating only on `visible_refs`.
/// - Deferral policy: when enabled, it will only "check" up to `defer_limit` refs
///   (or a simple default) and mark the rest as deferred.
/// - Optional progress emission under the "progress" feature.
#[derive(Debug, Clone)]
pub struct DefaultConnectivityChecker {
    config: ConnectivityConfig,
}

impl Default for DefaultConnectivityChecker {
    fn default() -> Self {
        Self {
            config: ConnectivityConfig::default(),
        }
    }
}

impl DefaultConnectivityChecker {
    pub fn new(config: ConnectivityConfig) -> Self {
        Self { config }
    }

    /// Update configuration.
    pub fn set_config(&mut self, config: ConnectivityConfig) {
        self.config = config;
    }

    /// Access configuration.
    pub fn config(&self) -> &ConnectivityConfig {
        &self.config
    }

    #[cfg(feature = "progress")]
    fn maybe_progress_message(
        &self,
        progress: &mut dyn gix_features::progress::DynNestedProgress,
        msg: &str,
    ) {
        use gix_features::progress::Progress;
        progress.message(gix_features::progress::MessageLevel::Info, msg.to_string());
    }

    /// Internal helper to apply deferral policy and compute outcome shell.
    fn plan_outcome(&self, total_refs: usize, names: &[String]) -> ConnectivityOutcome {
        if self.config.defer_per_ref {
            let limit = self.config.defer_limit.unwrap_or_else(|| total_refs.saturating_div(2).max(1));
            let (checked, deferred) = if total_refs > limit {
                (limit, names[limit..].to_vec())
            } else {
                (total_refs, Vec::new())
            };
            ConnectivityOutcome {
                total_refs,
                checked_refs: checked,
                deferred_refs: deferred,
                ok: true,
            }
        } else {
            ConnectivityOutcome {
                total_refs,
                checked_refs: total_refs,
                deferred_refs: Vec::new(),
                ok: true,
            }
        }
    }
}

impl ConnectivityChecker for DefaultConnectivityChecker {
    fn check(
        &mut self,
        updates: &[CommandUpdate],
        visible_refs: &[RefRecord],
    ) -> Result<ConnectivityOutcome, Error> {
        // Collect the set of refnames relevant for connectivity. In a full implementation,
        // this would derive tips from both updates (new target commits) and repository refs,
        // excluding hidden ones that are not provided here.
        let mut names: Vec<String> = Vec::with_capacity(visible_refs.len());
        for r in visible_refs {
            names.push(r.name.clone());
        }

        // Include target refnames of updates as points of interest (dedup by simple filter).
        for u in updates {
            let candidate = u.name().to_owned();
            if !names.iter().any(|n| n == &candidate) {
                names.push(candidate);
            }
        }

        let total = names.len();
        let outcome = self.plan_outcome(total, &names);

        // No real graph traversal yet; this is a milestone M4 scaffold.
        Ok(outcome)
    }

    fn is_parallel(&self) -> bool {
        self.config.parallel
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gix_hash::ObjectId;

    fn oid(hex40: &str) -> ObjectId {
        ObjectId::from_hex(hex40.as_bytes()).expect("valid hex")
    }

    fn rr(hex40: &str, name: &str) -> RefRecord {
        RefRecord::new(oid(hex40), name)
    }

    #[test]
    fn default_checker_no_deferral() {
        let mut checker = DefaultConnectivityChecker::default();
        let updates = vec![
            CommandUpdate::Create {
                new: oid("1111111111111111111111111111111111111111"),
                name: "refs/heads/main".to_string(),
            },
        ];
        let refs = vec![rr("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "refs/heads/other")];

        let out = checker.check(&updates, &refs).unwrap();
        assert_eq!(out.total_refs, 2); // one visible + one from updates
        assert_eq!(out.checked_refs, 2);
        assert!(out.deferred_refs.is_empty());
        assert!(out.ok);
    }

    #[test]
    fn deferral_applies_and_returns_rest() {
        let mut cfg = ConnectivityConfig::default();
        cfg.defer_per_ref = true;
        cfg.defer_limit = Some(1);

        let mut checker = DefaultConnectivityChecker::new(cfg);
        let updates = vec![
            CommandUpdate::Create {
                new: oid("1111111111111111111111111111111111111111"),
                name: "refs/heads/main".to_string(),
            },
        ];
        let refs = vec![
            rr("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "refs/heads/one"),
            rr("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "refs/heads/two"),
        ];

        let out = checker.check(&updates, &refs).unwrap();
        assert_eq!(out.total_refs, 3);
        assert_eq!(out.checked_refs, 1);
        assert_eq!(out.deferred_refs.len(), 2);
        assert!(out.ok);
    }

    #[test]
    fn parallel_flag_exposed() {
        let mut cfg = ConnectivityConfig::default();
        cfg.parallel = true;
        let checker = DefaultConnectivityChecker::new(cfg);
        assert!(checker.is_parallel());
    }
}