//! Shared capability set used by services to shape advertisements.

/// A set of capabilities for advertisement and negotiation.
#[derive(Clone, Default, Debug)]
pub struct CapabilitySet {
    /// Internal representation; keep simple for now.
    extras: Vec<String>,
}

impl CapabilitySet {
    /// Modern default capability set.
    pub fn modern_defaults() -> Self {
        Self { extras: Vec::new() }
    }

    /// Return `true` if the named capability is present.
    pub fn contains(&self, name: &str) -> bool {
        self.extras.iter().any(|n| n == name)
    }

    /// Add an extra capability.
    pub fn push_extra(&mut self, name: impl Into<String>) {
        self.extras.push(name.into());
    }
}

/// Controls ordering of capabilities during advertisement.
#[derive(Clone, Default, Debug)]
pub struct CapabilityOrdering;


