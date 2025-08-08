// M2: Options and negotiation parsing (blocking-first). Async parity will be added behind feature flags.
//
// This module parses and validates client-negotiated capabilities, push-options and shallow lines
// from the receive-pack head-info phase, as outlined in ROADMAP M2 and INTERFACES section 4.

use gix_hash::ObjectId;

use crate::Error;
use crate::protocol::capabilities::{CapabilityOrdering, CapabilitySet};

/// Parsed options negotiated during head-info parsing.
///
/// Contains a subset of tokens negotiated by the client (capabilities),
/// push-options lines and shallow/unshallow OIDs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Options {
    /// Space-separated capability tokens as negotiated from the first command line after the `\0`.
    ///
    /// Examples: "report-status", "report-status-v2", "side-band-64k", "agent=gix/1.0", "atomic"
    pub negotiated: Vec<String>,
    /// Push options provided by the client as additional pkt-lines of the form `push-option=<value>`.
    pub push_options: Vec<String>,
    /// OIDs from `shallow <oid>` lines.
    pub shallow: Vec<ObjectId>,
    /// OIDs from `unshallow <oid>` lines.
    pub unshallow: Vec<ObjectId>,
}

impl Options {
    /// Parse capability tokens from a space-separated string (part after the NUL on the first command line).
    ///
    /// - Unknown tokens are accepted here and later rejected by `validate_against()`.
    /// - Token order is preserved.
    pub fn parse(tokens: &str) -> Self {
        let mut out = Options::default();
        for t in tokens.split(' ').filter(|t| !t.is_empty()) {
            out.negotiated.push(t.to_string());
        }
        out
    }

    /// Add a push-option value (the string after `push-option=`).
    pub fn add_push_option<S: Into<String>>(&mut self, value: S) {
        self.push_options.push(value.into());
    }

    /// Add a shallow OID parsed from a `shallow <oid>` line.
    pub fn add_shallow_oid(&mut self, oid: ObjectId) {
        self.shallow.push(oid);
    }

    /// Add an unshallow OID parsed from an `unshallow <oid>` line.
    pub fn add_unshallow_oid(&mut self, oid: ObjectId) {
        self.unshallow.push(oid);
    }

    /// Check if a capability token was negotiated.
    pub fn has(&self, token: &str) -> bool {
        self.negotiated.iter().any(|t| t == token || (t.starts_with(token) && t.get(token.len()..token.len() + 1) == Some("=")))
    }

    /// Validate negotiated capability tokens against the set we advertised.
    ///
    /// - Reject tokens that weren't advertised.
    /// - Agent is validated only syntactically here (no spaces). More rules can be added later.
    pub fn validate_against(&self, advertised: &CapabilitySet) -> Result<(), Error> {
        let advertised_tokens = Self::advertised_token_set(advertised);

        for tok in &self.negotiated {
            // 'agent=' is special: if we didn't advertise agent at all, it's not allowed.
            if tok.starts_with("agent=") {
                // only allowed if our advertised set contains "agent=" prefix (i.e., agent was present)
                if !advertised_tokens.iter().any(|t| t == "agent" || t.starts_with("agent=")) {
                    return Err(Error::Validation(format!("capability '{}' not advertised (agent disabled)", tok)));
                }
                // basic syntax check: no spaces
                if tok.split_once('=').map_or(true, |(_, v)| v.contains(' ')) {
                    return Err(Error::Validation(format!("invalid agent token '{}': must not contain spaces", tok)));
                }
                continue;
            }

            // Standard tokens or key=value pairs must appear in the advertised token set.
            // We check both exact matches and key matches for key=value forms.
            if let Some((key, _value)) = tok.split_once('=') {
                if !advertised_tokens.contains(key) && !advertised_tokens.contains(tok) {
                    return Err(Error::Validation(format!(
                        "capability '{}' not advertised (neither '{}' nor exact match allowed)",
                        tok, key
                    )));
                }
            } else if !advertised_tokens.contains(tok) {
                return Err(Error::Validation(format!("capability '{}' not advertised", tok)));
            }
        }
        Ok(())
    }

    fn advertised_token_set(advertised: &CapabilitySet) -> std::collections::HashSet<String> {
        // Build a set of tokens we advertised. For key=value tokens like `agent=…` we also include the key
        // to allow any value (subject to basic validation) as upstream does.
        let mut set = std::collections::HashSet::new();
        for tok in advertised
            .encode(CapabilityOrdering::PreserveIdiomatic)
            .split(' ')
            .filter(|t| !t.is_empty())
        {
            set.insert(tok.to_string());
            if let Some((key, _)) = tok.split_once('=') {
                set.insert(key.to_string());
            }
        }
        set
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::capabilities::{CapabilityOrdering, CapabilitySet};

    #[test]
    fn parse_tokens_splits_and_preserves_order() {
        let opts = Options::parse("report-status report-status-v2 side-band-64k agent=gix/1.0 atomic");
        assert_eq!(
            opts.negotiated,
            vec![
                "report-status",
                "report-status-v2",
                "side-band-64k",
                "agent=gix/1.0",
                "atomic"
            ]
        );
        assert!(opts.has("report-status"));
        assert!(opts.has("agent"));
        assert!(opts.has("agent=whatever") == false); // only exact string present; `has` checks key-prefix presence too
    }

    #[test]
    fn validate_against_advertised_ok() {
        let adv = CapabilitySet::modern_defaults().with_agent(Some("gix/1.0".into()));
        let opts = Options::parse("report-status report-status-v2 quiet delete-refs ofs-delta agent=gix/2.0");
        assert!(opts.validate_against(&adv).is_ok());
    }

    #[test]
    fn validate_rejects_unadvertised() {
        let adv = CapabilitySet::modern_defaults(); // no agent
        let opts = Options::parse("report-status no-such-cap");
        let err = opts.validate_against(&adv).unwrap_err();
        assert!(matches!(err, Error::Validation(_)));
        assert!(format!("{err}").contains("no-such-cap"));
    }

    #[test]
    fn validate_rejects_agent_when_not_advertised() {
        let adv = CapabilitySet::modern_defaults(); // no agent
        let opts = Options::parse("agent=gix/1.0");
        let err = opts.validate_against(&adv).unwrap_err();
        assert!(matches!(err, Error::Validation(_)));
        assert!(format!("{err}").contains("agent"));
    }

    #[test]
    fn add_push_option_and_shallow() {
        let mut opts = Options::default();
        opts.add_push_option("ci-skip=true");
        opts.add_push_option("notify=team");
        let oid = oid("1111111111111111111111111111111111111111");
        opts.add_shallow_oid(oid);
        assert_eq!(opts.push_options, vec!["ci-skip=true", "notify=team"]);
        assert_eq!(opts.shallow.len(), 1);
        assert_eq!(opts.shallow[0], oid);
    }

    #[test]
    fn add_unshallow() {
        let mut opts = Options::default();
        let o1 = oid("2222222222222222222222222222222222222222");
        let o2 = oid("3333333333333333333333333333333333333333");
        opts.add_unshallow_oid(o1);
        opts.add_unshallow_oid(o2);
        assert_eq!(opts.unshallow, vec![o1, o2]);
    }

    fn oid(hex40: &str) -> gix_hash::ObjectId {
        gix_hash::ObjectId::from_hex(hex40.as_bytes()).expect("valid hex")
    }

    #[test]
    fn validate_rejects_agent_with_spaces() {
        let adv = CapabilitySet::modern_defaults().with_agent(Some("gix/1.0".into()));
        // Create an Options with a manually constructed agent token that contains spaces
        let mut opts = Options::default();
        opts.negotiated.push("agent=gix with spaces".to_string());
        let err = opts.validate_against(&adv).unwrap_err();
        assert!(matches!(err, Error::Validation(_)));
        let err_msg = format!("{err}");
        assert!(err_msg.contains("must not contain spaces"));
    }

    // Quick sanity to ensure advertised token building includes keys for key=value items.
    #[test]
    fn advertised_token_set_includes_keys() {
        let adv = CapabilitySet::modern_defaults().with_agent(Some("gix/1.0".into()));
        let set = super::Options::advertised_token_set(&adv);
        assert!(set.contains("report-status"));
        assert!(set.contains("agent"));
        // Depending on defaults, ensure ofs-delta is included as well
        assert!(set.contains("ofs-delta"));
        // The exact advertised agent value is present too.
        assert!(set.contains(&adv.encode(CapabilityOrdering::PreserveIdiomatic).split(' ').find(|t| t.starts_with("agent=")).unwrap().to_string()));
    }
}