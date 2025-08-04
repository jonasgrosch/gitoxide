//! Configuration management for upload-pack server

use crate::{Error, Result, ServerCapabilities};
use bstr::{BString, ByteSlice};
use std::path::PathBuf;
use std::time::Duration;

/// Configuration options for the upload-pack server
#[derive(Debug, Clone)]
pub struct ServerOptions {
    /// Whether to advertise refs (for stateless protocols)
    pub advertise_refs: bool,

    /// Whether this is a stateless RPC connection
    pub stateless_rpc: bool,

    /// Timeout for client operations
    pub timeout: Option<Duration>,

    /// Whether to enable strict mode
    pub strict: bool,

    /// Server capabilities to advertise
    pub capabilities: ServerCapabilities,

    /// Maximum pack size to generate (in bytes)
    pub max_pack_size: Option<u64>,

    /// Enable keep-alive packets
    pub keepalive: Option<Duration>,

    /// Custom upload-pack hook path
    pub upload_pack_hook: Option<PathBuf>,

    /// Custom pack-objects hook path  
    pub pack_objects_hook: Option<PathBuf>,

    /// Pre-receive hook path
    pub pre_upload_pack_hook: Option<PathBuf>,

    /// Post-upload hook path
    pub post_upload_pack_hook: Option<PathBuf>,

    /// Hidden refs patterns
    pub hidden_refs: Vec<BString>,

    /// Allowed filter specs
    pub allowed_filters: Vec<BString>,

    /// Maximum tree filter depth
    pub max_tree_filter_depth: Option<u32>,

    /// Enable shallow clone support
    pub allow_shallow: bool,

    /// Enable filter support
    pub allow_filter: bool,

    /// Allow any SHA1 in want (dangerous)
    pub allow_any_sha1_in_want: bool,

    /// Allow reachable SHA1 in want
    pub allow_reachable_sha1_in_want: bool,

    /// Allow tip SHA1 in want
    pub allow_tip_sha1_in_want: bool,

    /// Allow deepen-relative
    pub allow_deepen_relative: bool,

    /// Allow packfile URIs (protocol v2)
    pub allow_packfile_uris: bool,

    /// Enable session ID support
    pub enable_session_id: bool,

    /// Enable SHA-256 support
    pub enable_sha256: bool,

    /// Enable object-info command (protocol v2)
    pub enable_object_info: bool,

    /// Allow blob filtering
    pub allow_blob_filter: bool,

    /// Allow tree filtering
    pub allow_tree_filter: bool,

    /// Allow sparse filtering
    pub allow_sparse_filter: bool,

    /// Maximum shallow depth
    pub max_shallow_depth: Option<u32>,

    /// Enable sideband-all support
    pub allow_sideband_all: bool,

    /// Custom user agent string
    pub user_agent: Option<BString>,

    /// Supported hash algorithms
    pub hash_algorithms: Vec<gix_hash::Kind>,

    /// Enable tracing/logging
    pub enable_tracing: bool,

    /// Custom configuration values
    pub custom_config: std::collections::HashMap<String, String>,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            advertise_refs: false,
            stateless_rpc: false,
            timeout: Some(Duration::from_secs(900)), // 15 minutes
            strict: false,
            capabilities: ServerCapabilities::default(),
            max_pack_size: None,
            keepalive: Some(Duration::from_secs(5)),
            upload_pack_hook: None,
            pack_objects_hook: None,
            pre_upload_pack_hook: None,
            post_upload_pack_hook: None,
            hidden_refs: Vec::new(),
            allowed_filters: vec![
                "blob:none".into(),
                "blob:limit=1k".into(),
                "tree:0".into(),
                "sparse:oid=".into(),
            ],
            max_tree_filter_depth: Some(u32::MAX),
            allow_shallow: true,
            allow_filter: true,
            allow_any_sha1_in_want: false,
            allow_reachable_sha1_in_want: false,
            allow_tip_sha1_in_want: false,
            allow_deepen_relative: true,
            allow_packfile_uris: false,
            enable_session_id: true,
            enable_sha256: false,
            enable_object_info: false,
            allow_blob_filter: true,
            allow_tree_filter: true,
            allow_sparse_filter: false,
            max_shallow_depth: None,
            allow_sideband_all: true,
            user_agent: None,
            hash_algorithms: vec![gix_hash::Kind::Sha1],
            enable_tracing: false,
            custom_config: std::collections::HashMap::new(),
        }
    }
}

impl ServerOptions {
    /// Create new server options with defaults
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable or disable ref advertisement
    pub fn with_advertise_refs(mut self, advertise: bool) -> Self {
        self.advertise_refs = advertise;
        self
    }

    /// Set stateless RPC mode
    pub fn with_stateless_rpc(mut self, stateless: bool) -> Self {
        self.stateless_rpc = stateless;
        self
    }

    /// Set timeout duration
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Set strict mode
    pub fn with_strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }

    /// Set server capabilities
    pub fn with_capabilities(mut self, capabilities: ServerCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// Set maximum pack size
    pub fn with_max_pack_size(mut self, max_size: u64) -> Self {
        self.max_pack_size = Some(max_size);
        self
    }

    /// Set keepalive interval
    pub fn with_keepalive(mut self, keepalive: Duration) -> Self {
        self.keepalive = Some(keepalive);
        self
    }

    /// Add hidden ref pattern
    pub fn with_hidden_ref(mut self, pattern: impl Into<BString>) -> Self {
        self.hidden_refs.push(pattern.into());
        self
    }

    /// Set allowed filters
    pub fn with_allowed_filters(mut self, filters: Vec<BString>) -> Self {
        self.allowed_filters = filters;
        self
    }

    /// Enable/disable shallow support
    pub fn with_shallow_support(mut self, allow: bool) -> Self {
        self.allow_shallow = allow;
        self
    }

    /// Enable/disable filter support
    pub fn with_filter_support(mut self, allow: bool) -> Self {
        self.allow_filter = allow;
        self
    }

    /// Set custom user agent
    pub fn with_user_agent(mut self, agent: impl Into<BString>) -> Self {
        self.user_agent = Some(agent.into());
        self
    }

    /// Add custom configuration
    pub fn with_config(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.custom_config.insert(key.into(), value.into());
        self
    }

    /// Load configuration from a Git repository
    pub fn from_repository(repo: &gix::Repository) -> Result<Self> {
        let mut options = Self::default();

        // Load configuration values from git config
        let config = repo.config_snapshot();

        // uploadpack.* configuration
        if let Some(value) = config.boolean("uploadpack.allowAnySHA1InWant") {
            options.allow_any_sha1_in_want = value;
        }

        if let Some(value) = config.boolean("uploadpack.allowReachableSHA1InWant") {
            options.allow_reachable_sha1_in_want = value;
        }

        if let Some(value) = config.boolean("uploadpack.allowTipSHA1InWant") {
            options.allow_tip_sha1_in_want = value;
        }

        if let Some(value) = config.boolean("uploadpack.allowFilter") {
            options.allow_filter = value;
        }

        if let Some(value) = config.integer("uploadpack.keepAlive") {
            if value > 0 {
                options.keepalive = Some(Duration::from_secs(value as u64));
            } else {
                options.keepalive = None;
            }
        }

        if let Some(value) = config.string("uploadpack.packObjectsHook") {
            options.pack_objects_hook = Some(PathBuf::from(value.to_string()));
        }

        // transfer.* configuration
        if let Some(value) = config.string("transfer.hideRefs") {
            options.hidden_refs.push(BString::from(value.into_owned()));
        }

        // Load hidden refs from multiple values - use strings() method instead
        if let Some(values) = config.strings("transfer.hideRefs") {
            for value in values {
                options.hidden_refs.push(BString::from(value.into_owned()));
            }
        }

        // Check if object info command is enabled via transfer.advertiseobjectinfo
        if let Some(value) = config.boolean("transfer.advertiseObjectInfo") {
            options.enable_object_info = value;
        }

        Ok(options)
    }

    /// Validate configuration for consistency
    pub fn validate(&self) -> Result<()> {
        // Validate hook paths exist if specified
        if let Some(hook_path) = &self.upload_pack_hook {
            if !hook_path.exists() {
                return Err(Error::Hook {
                    hook: "upload-pack".to_string(),
                    path: hook_path.clone(),
                });
            }
        }

        if let Some(hook_path) = &self.pack_objects_hook {
            if !hook_path.exists() {
                return Err(Error::Hook {
                    hook: "pack-objects".to_string(),
                    path: hook_path.clone(),
                });
            }
        }

        // Validate filter specs
        for filter in &self.allowed_filters {
            if filter.is_empty() {
                return Err(Error::Config {
                    message: "Empty filter specification not allowed".to_string(),
                });
            }
        }

        // Validate timeout
        if let Some(timeout) = self.timeout {
            if timeout.as_secs() == 0 {
                return Err(Error::Config {
                    message: "Timeout cannot be zero".to_string(),
                });
            }
        }

        Ok(())
    }

    /// Check if a reference should be hidden
    pub fn is_ref_hidden(&self, ref_name: &str) -> bool {
        // Check user-configured hidden refs
        for pattern in &self.hidden_refs {
            if let Ok(pattern) = gix_pathspec::Pattern::from_bytes(pattern, gix_pathspec::Defaults::default()) {
                // Simple pattern matching for now - just check if it matches
                // Simple string matching for now - just check if pattern bytes are contained in ref_name
                if let Ok(pattern_str) = std::str::from_utf8(pattern.path()) {
                    if ref_name.contains(pattern_str) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Check if a filter is allowed
    pub fn is_filter_allowed(&self, filter_spec: &str) -> bool {
        if !self.allow_filter {
            return false;
        }

        // Check against allowed filter patterns
        for allowed in &self.allowed_filters {
            if filter_spec.starts_with(&*allowed.to_str_lossy()) {
                return true;
            }
        }

        false
    }
}
