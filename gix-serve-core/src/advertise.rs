//! Shared advertisement configuration and helpers.

use crate::visibility::{HiddenRefPredicate, RefRecord};
use std::sync::Arc;

/// Configuration for shaping advertisements.
#[derive(Clone)]
pub struct AdvertConfig {
    /// Optional agent string (e.g., `gix-upload-pack/0.1.0`).
    pub agent: Option<String>,
    /// Predicate to hide references.
    pub hidden: Arc<HiddenRefPredicate>,
    /// Whether to include symref hints.
    pub symref_hints: bool,
}

impl AdvertConfig {
    /// Reasonable defaults for modern servers.
    pub fn modern_defaults() -> Self {
        Self {
            agent: None,
            hidden: Arc::new(|_r: &RefRecord| false),
            symref_hints: true,
        }
    }

    /// Set agent string.
    pub fn with_agent(mut self, agent: Option<String>) -> Self {
        self.agent = agent;
        self
    }

    /// Set hidden predicate.
    pub fn with_hidden(mut self, hidden: Arc<HiddenRefPredicate>) -> Self {
        self.hidden = hidden;
        self
    }

    /// Enable/disable symref hints.
    pub fn with_symref_hints(mut self, enabled: bool) -> Self {
        self.symref_hints = enabled;
        self
    }
}


