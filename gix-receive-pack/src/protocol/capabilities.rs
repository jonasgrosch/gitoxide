use std::fmt;

/// How to order emitted capability tokens when building the advertisement line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityOrdering {
    /// Keep the crate's idiomatic deterministic order.
    PreserveIdiomatic,
    /// Emit tokens lexicographically. This is useful for golden tests.
    Lexicographic,
}

/// A minimal, typed capability set for receive-pack advertisements.
///
/// This structure focuses on frequently used capabilities needed in M1 and allows
/// attaching additional raw tokens for forward-compatibility.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CapabilitySet {
    pub report_status: bool,
    pub report_status_v2: bool,
    pub side_band_64k: bool,
    pub quiet: bool,
    pub delete_refs: bool,
    pub ofs_delta: bool,
    /// An optional agent string. When present, it is emitted as `agent=<value>`.
    pub agent: Option<String>,
    /// Additional opaque capability tokens (e.g., "no-thin", "atomic").
    pub extra: Vec<String>,
}

impl CapabilitySet {
    /// Construct a set with opinionated defaults suitable for modern receive-pack servers.
    ///
    /// Defaults are chosen as per ROADMAP M1 acceptance criteria and may evolve in later milestones.
    pub fn modern_defaults() -> Self {
        Self {
            report_status: true,
            report_status_v2: true,
            side_band_64k: false, // optional; can be enabled explicitly
            quiet: true,
            delete_refs: true,
            ofs_delta: true,
            agent: None,
            extra: Vec::new(),
        }
    }

    /// Enable or disable the `agent` token.
    pub fn with_agent(mut self, agent: Option<String>) -> Self {
        self.agent = agent;
        self
    }

    /// Push an additional raw capability token.
    pub fn push_extra<S: Into<String>>(&mut self, token: S) {
        self.extra.push(token.into());
    }

    /// Return the capability tokens as strings, in idiomatic deterministic order.
    fn tokens_idiomatic(&self) -> Vec<String> {
        let mut tokens = Vec::with_capacity(8 + self.extra.len());
        if self.report_status {
            tokens.push("report-status".to_string());
        }
        if self.report_status_v2 {
            tokens.push("report-status-v2".to_string());
        }
        if self.side_band_64k {
            tokens.push("side-band-64k".to_string());
        }
        if self.quiet {
            tokens.push("quiet".to_string());
        }
        if self.delete_refs {
            tokens.push("delete-refs".to_string());
        }
        if self.ofs_delta {
            tokens.push("ofs-delta".to_string());
        }
        if let Some(a) = &self.agent {
            // agent value must not contain spaces; we don't enforce here, tests will.
            tokens.push(format!("agent={}", a));
        }
        tokens.extend(self.extra.iter().cloned());
        tokens
    }

    /// Build a single space-separated capability line according to the selected ordering.
    pub fn encode(&self, ordering: CapabilityOrdering) -> String {
        let mut tokens = self.tokens_idiomatic();
        if ordering == CapabilityOrdering::Lexicographic {
            tokens.sort_unstable_by(|a, b| a.as_str().cmp(b.as_str()));
        }
        tokens.join(" ")
    }
}

impl fmt::Display for CapabilitySet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Default display uses idiomatic ordering.
        f.write_str(&self.encode(CapabilityOrdering::PreserveIdiomatic))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_preserve_vs_lexicographic() {
        let mut caps = CapabilitySet::modern_defaults().with_agent(Some("gix/1.0".into()));
        caps.push_extra("atomic");
        caps.side_band_64k = true;

        let idiomatic = caps.encode(CapabilityOrdering::PreserveIdiomatic);
        let lex = caps.encode(CapabilityOrdering::Lexicographic);

        // The two orders should not be equal once there are enough tokens.
        assert_ne!(idiomatic, lex);

        // Lexicographic should be sorted.
        let mut parts = lex.split(' ').collect::<Vec<_>>();
        let mut sorted = parts.clone();
        sorted.sort_unstable();
        assert_eq!(parts, sorted);

        // Containment sanity
        for t in ["report-status", "report-status-v2", "side-band-64k", "quiet", "delete-refs", "ofs-delta", "agent=gix/1.0", "atomic"] {
            assert!(idiomatic.contains(t));
            assert!(lex.contains(t));
        }
    }
}