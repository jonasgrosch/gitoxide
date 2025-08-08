use std::fmt;

/// How to order emitted capability tokens when building the advertisement line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityOrdering {
    /// Keep the crate's idiomatic deterministic order.
    PreserveIdiomatic,
    /// Emit tokens lexicographically. This is useful for golden tests.
    Lexicographic,
}

impl Default for CapabilityOrdering {
    fn default() -> Self {
        Self::PreserveIdiomatic
    }
}

/// Trait for formatting capability strings with different compatibility modes.
pub trait CapabilityFormatter {
    /// Format the capability set into a space-separated string.
    fn format_capabilities(&self, caps: &CapabilitySet) -> String;
}

/// Idiomatic capability formatter that uses the crate's default ordering and spacing.
#[derive(Debug, Clone, Copy, Default)]
pub struct IdiomaticFormatter {
    ordering: CapabilityOrdering,
}

impl IdiomaticFormatter {
    /// Create a new idiomatic formatter with the specified ordering.
    pub fn new(ordering: CapabilityOrdering) -> Self {
        Self { ordering }
    }
}

impl CapabilityFormatter for IdiomaticFormatter {
    fn format_capabilities(&self, caps: &CapabilitySet) -> String {
        caps.encode(self.ordering)
    }
}

/// Strict compatibility formatter that matches upstream git-receive-pack byte-for-byte.
/// This formatter is only available when the "strict-compat" feature is enabled.
#[cfg(feature = "strict-compat")]
#[derive(Debug, Clone, Copy, Default)]
pub struct StrictCompatFormatter;

#[cfg(feature = "strict-compat")]
impl StrictCompatFormatter {
    /// Create a new strict compatibility formatter.
    pub fn new() -> Self {
        Self
    }
}

#[cfg(feature = "strict-compat")]
impl CapabilityFormatter for StrictCompatFormatter {
    fn format_capabilities(&self, caps: &CapabilitySet) -> String {
        // Implement upstream git-receive-pack capability ordering and spacing.
        // Based on git's builtin/receive-pack.c, capabilities are typically emitted in this order:
        // report-status, report-status-v2, delete-refs, quiet, atomic, ofs-delta, side-band-64k, agent
        let mut tokens = Vec::with_capacity(8 + caps.extra.len());
        
        // Follow upstream ordering from git's receive-pack.c
        if caps.report_status {
            tokens.push("report-status".to_string());
        }
        if caps.report_status_v2 {
            tokens.push("report-status-v2".to_string());
        }
        if caps.delete_refs {
            tokens.push("delete-refs".to_string());
        }
        if caps.quiet {
            tokens.push("quiet".to_string());
        }
        
        // Add extra capabilities in their original order (atomic, etc.)
        tokens.extend(caps.extra.iter().cloned());
        
        if caps.ofs_delta {
            tokens.push("ofs-delta".to_string());
        }
        if caps.side_band_64k {
            tokens.push("side-band-64k".to_string());
        }
        if let Some(a) = &caps.agent {
            tokens.push(format!("agent={}", a));
        }
        
        tokens.join(" ")
    }
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
        let parts = lex.split(' ').collect::<Vec<_>>();
        let mut sorted = parts.clone();
        sorted.sort_unstable();
        assert_eq!(parts, sorted);

        // Containment sanity
        for t in ["report-status", "report-status-v2", "side-band-64k", "quiet", "delete-refs", "ofs-delta", "agent=gix/1.0", "atomic"] {
            assert!(idiomatic.contains(t));
            assert!(lex.contains(t));
        }
    }

    #[test]
    fn formatter_trait_idiomatic() {
        let mut caps = CapabilitySet::modern_defaults().with_agent(Some("gix/1.0".into()));
        caps.push_extra("atomic");

        let formatter = IdiomaticFormatter::new(CapabilityOrdering::PreserveIdiomatic);
        let output = formatter.format_capabilities(&caps);
        let expected = caps.encode(CapabilityOrdering::PreserveIdiomatic);
        assert_eq!(output, expected);
    }

    #[test]
    fn formatter_trait_lexicographic() {
        let mut caps = CapabilitySet::modern_defaults().with_agent(Some("gix/1.0".into()));
        caps.push_extra("atomic");

        let formatter = IdiomaticFormatter::new(CapabilityOrdering::Lexicographic);
        let output = formatter.format_capabilities(&caps);
        let expected = caps.encode(CapabilityOrdering::Lexicographic);
        assert_eq!(output, expected);
    }

    #[cfg(feature = "strict-compat")]
    #[test]
    fn strict_compat_formatter_vs_idiomatic() {
        let mut caps = CapabilitySet::modern_defaults().with_agent(Some("gix/1.0".into()));
        caps.push_extra("atomic");
        caps.side_band_64k = true;

        let idiomatic_formatter = IdiomaticFormatter::new(CapabilityOrdering::PreserveIdiomatic);
        let strict_formatter = StrictCompatFormatter::new();

        let idiomatic_output = idiomatic_formatter.format_capabilities(&caps);
        let strict_output = strict_formatter.format_capabilities(&caps);

        // They should contain the same tokens but potentially in different order
        let idiomatic_tokens: std::collections::HashSet<&str> = idiomatic_output.split(' ').collect();
        let strict_tokens: std::collections::HashSet<&str> = strict_output.split(' ').collect();
        assert_eq!(idiomatic_tokens, strict_tokens);

        // But the order should be different (unless they happen to match)
        // We can't assert they're different because they might coincidentally be the same
        // for this particular set of capabilities, but we can verify the strict ordering
        let strict_parts: Vec<&str> = strict_output.split(' ').collect();
        
        // Verify some key ordering constraints from upstream git
        if let (Some(report_pos), Some(agent_pos)) = (
            strict_parts.iter().position(|&t| t == "report-status"),
            strict_parts.iter().position(|&t| t.starts_with("agent="))
        ) {
            assert!(report_pos < agent_pos, "report-status should come before agent in strict mode");
        }
    }
}