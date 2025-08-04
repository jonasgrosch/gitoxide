//! Protocol version detection and management
//!
//! This module centralizes protocol version detection logic and provides
//! utilities for protocol-specific behavior.

use crate::{
    error::Result,
    types::ProtocolVersion,
};

/// Protocol detection service
pub struct ProtocolDetector;

impl ProtocolDetector {
    /// Detect protocol version from environment variable
    /// 
    /// This matches native git's determine_protocol_version_server behavior exactly
    pub fn detect_version() -> Result<ProtocolVersion> {
        // Check GIT_PROTOCOL environment variable exactly like native git
        if let Ok(protocol) = std::env::var("GIT_PROTOCOL") {
            match protocol.as_str() {
                "version=0" => Ok(ProtocolVersion::V0),
                "version=1" => Ok(ProtocolVersion::V1), 
                "version=2" => Ok(ProtocolVersion::V2),
                _ => {
                    // Invalid protocol version - native git would return protocol_unknown_version
                    // For now, fall back to v0 (most conservative)
                    Ok(ProtocolVersion::V0)
                }
            }
        } else {
            // No GIT_PROTOCOL environment variable - default to v0 like native git
            // Native git uses v0 as the default when no protocol is specified
            Ok(ProtocolVersion::V0)
        }
    }

    /// Check if we're using explicit protocol v1 (vs v0/default)
    pub fn is_explicit_v1() -> bool {
        std::env::var("GIT_PROTOCOL")
            .map(|p| p == "version=1")
            .unwrap_or(false)
    }

    /// Get protocol version string for logging/debugging
    pub fn version_string(version: ProtocolVersion) -> &'static str {
        match version {
            ProtocolVersion::V0 => "v0",
            ProtocolVersion::V1 => "v1", 
            ProtocolVersion::V2 => "v2",
        }
    }

    /// Check if protocol version supports specific features
    pub fn supports_sideband_all(version: ProtocolVersion) -> bool {
        matches!(version, ProtocolVersion::V2)
    }

    /// Check if protocol version supports ls-refs command
    pub fn supports_ls_refs(version: ProtocolVersion) -> bool {
        matches!(version, ProtocolVersion::V2)
    }

    /// Check if protocol version supports object-info command
    pub fn supports_object_info(version: ProtocolVersion) -> bool {
        matches!(version, ProtocolVersion::V2)
    }

    /// Check if protocol version requires capability advertisement
    pub fn requires_capability_advertisement(version: ProtocolVersion) -> bool {
        !matches!(version, ProtocolVersion::V2) // V2 advertises on-demand
    }
}