//! Common types and structures used throughout the upload-pack implementation

use bstr::BString;
use gix_hash::ObjectId;
use smallvec::SmallVec;
use std::collections::HashSet;

// Re-export transport types
pub use gix_transport::client::Capabilities;
pub use gix_transport::Protocol as ProtocolVersion;

// Re-export protocol types for gradual migration
pub use gix_protocol::handshake::Ref as ProtocolRef;
pub use gix_protocol::Command;
pub use gix_protocol::fetch::response::Acknowledgement;
pub use gix_packetline::Channel as SideBandChannel;
pub use gix_shallow::Update as ShallowUpdate;

// Use ProtocolRef directly as our Reference type
pub type Reference = ProtocolRef;

/// Multi-ack modes for negotiation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultiAckMode {
    /// No multi-ack support
    None,
    /// Basic multi-ack
    Basic,
    /// Detailed multi-ack with more granular responses
    Detailed,
}

impl Default for MultiAckMode {
    fn default() -> Self {
        Self::None
    }
}

/// Side-band capability levels for protocol negotiation.
/// This represents the negotiated side-band capability level, not the actual channel types
/// (which are represented by `gix_packetline::Channel`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideBandMode {
    /// No side-band support
    None,
    /// Basic side-band (up to 1000 bytes)
    Basic,
    /// Side-band 64k (up to 65520 bytes)
    SideBand64k,
}

impl Default for SideBandMode {
    fn default() -> Self {
        Self::None
    }
}

impl SideBandMode {
    /// Returns the maximum data size per packet for this side-band mode
    pub fn max_data_size(self) -> Option<usize> {
        match self {
            SideBandMode::None => None,
            SideBandMode::Basic => Some(999),      // 1000 - 1 byte for channel
            SideBandMode::SideBand64k => Some(65519), // 65520 - 1 byte for channel
        }
    }

    /// Returns true if this mode supports the given channel type
    pub fn supports_channel(self, channel: SideBandChannel) -> bool {
        match self {
            SideBandMode::None => false,
            SideBandMode::Basic => matches!(channel, SideBandChannel::Progress),
            SideBandMode::SideBand64k => true, // Supports all channels
        }
    }

    /// Convert from capability strings as they appear in the protocol
    pub fn from_capability_string(capability: &str) -> Option<Self> {
        match capability {
            "side-band" => Some(SideBandMode::Basic),
            "side-band-64k" => Some(SideBandMode::SideBand64k),
            _ => None,
        }
    }

    /// Convert to protocol v1 capability strings
    pub fn to_capability_strings(&self) -> Vec<&'static str> {
        match self {
            SideBandMode::None => vec![],
            SideBandMode::Basic => vec!["side-band"],
            SideBandMode::SideBand64k => vec!["side-band", "side-band-64k"],
        }
    }
    
    /// Convert to protocol v2 capability strings
    pub fn to_v2_capability_strings(&self) -> Vec<&'static str> {
        match self {
            SideBandMode::None => vec![],
            SideBandMode::Basic => vec!["sideband"],
            SideBandMode::SideBand64k => vec!["sideband-all"],
        }
    }
}

/// Server configuration for capability management
#[derive(Debug, Clone)]
pub struct ServerCapabilities {
    /// Multi-ack support level
    pub multi_ack: MultiAckMode,
    /// Thin-pack support
    pub thin_pack: bool,
    /// Side-band support
    pub side_band: SideBandMode,
    /// Offset delta support
    pub ofs_delta: bool,
    /// Include tag support
    pub include_tag: bool,
    /// Shallow support
    pub shallow: bool,
    /// Deepen-since support
    pub deepen_since: bool,
    /// Deepen-not support
    pub deepen_not: bool,
    /// Deepen-relative support
    pub deepen_relative: bool,
    /// No-progress support
    pub no_progress: bool,
    /// Filter support
    pub filter: bool,
    /// Allow tip SHA1 in want
    pub allow_tip_sha1_in_want: bool,
    /// Allow reachable SHA1 in want
    pub allow_reachable_sha1_in_want: bool,
    /// Allow any SHA1 in want
    pub allow_any_sha1_in_want: bool,
    /// No-done support (protocol v2)
    pub no_done: bool,
    /// Agent string
    pub agent: BString,
    /// Supported object formats
    pub object_format: SmallVec<[gix_hash::Kind; 2]>,
    /// Session ID for tracing
    pub session_id: Option<BString>,
    /// Packfile URIs support (protocol v2)
    pub packfile_uris: bool,
    /// Wait for done support (protocol v2)
    pub wait_for_done: bool,
    /// Object info support (protocol v2) - disabled by default
    pub object_info: bool,
}

impl Default for ServerCapabilities {
    fn default() -> Self {
        Self {
            multi_ack: MultiAckMode::Detailed,
            thin_pack: true,
            side_band: SideBandMode::SideBand64k,
            ofs_delta: true,
            include_tag: true,
            shallow: true,
            deepen_since: true,
            deepen_not: true,
            deepen_relative: true,
            no_progress: false,
            filter: false,
            allow_tip_sha1_in_want: false,
            allow_reachable_sha1_in_want: false,
            allow_any_sha1_in_want: false,
            no_done: true,
            agent: format!("git/gitoxide-{}", crate::VERSION).into(),
            object_format: smallvec::smallvec![gix_hash::Kind::Sha1],
            session_id: None,
            packfile_uris: false,
            wait_for_done: true,
            object_info: false, // Disabled by default
        }
    }
}

/// Client capabilities parsed from the wire protocol
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientCapabilities {
    /// Multi-ack capability level
    pub multi_ack: MultiAckMode,
    /// Support for thin-pack
    pub thin_pack: bool,
    /// Support for side-band
    pub side_band: SideBandMode,
    /// Support for offset deltas
    pub ofs_delta: bool,
    /// Include tags in pack
    pub include_tag: bool,
    /// Suppress progress information
    pub no_progress: bool,
    /// Allow tip SHA1 in want
    pub allow_tip_sha1_in_want: bool,
    /// Allow reachable SHA1 in want
    pub allow_reachable_sha1_in_want: bool,
    /// Deepen capability
    pub deepen_relative: bool,
    /// Shallow capability
    pub shallow: bool,
    /// Filter capability with spec
    pub filter: Option<BString>,
    /// Session ID for tracing
    pub session_id: Option<BString>,
    /// Agent string
    pub agent: Option<BString>,
    /// Object format (hash algorithm)
    pub object_format: Option<gix_hash::Kind>,
}

/// Request from client during negotiation
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientRequest {
    /// Client wants this object
    Want {
        /// Object ID requested
        oid: ObjectId,
        /// Capabilities (only on first want)
        capabilities: Option<ClientCapabilities>,
    },
    /// Client has this object
    Have {
        /// Object ID the client has
        oid: ObjectId,
    },
    /// Client indicates end of negotiation
    Done,
    /// Client requests deepen by count
    Deepen {
        /// Depth to deepen to
        depth: u32,
    },
    /// Client requests deepen since timestamp
    DeepenSince {
        /// Timestamp to deepen since
        timestamp: gix_date::Time,
    },
    /// Client requests deepen not from refs
    DeepenNot {
        /// Reference patterns to exclude
        refs: Vec<BString>,
    },
    /// Client sends shallow commits
    Shallow {
        /// Shallow commit OID
        oid: ObjectId,
    },
    /// Custom extension for protocol v2
    Extension {
        /// Extension name
        name: BString,
        /// Extension value
        value: Option<BString>,
    },
}

/// Server response during negotiation
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerResponse {
    /// Acknowledgment response (includes Common, Ready, Nak variants)
    Ack(Acknowledgement),
    /// Shallow commit information
    Shallow {
        /// Shallow commit OID
        oid: ObjectId,
    },
    /// Unshallow commit information
    Unshallow {
        /// Unshallowed commit OID
        oid: ObjectId,
    },
    /// Error message
    Error {
        /// Error message to client
        message: BString,
    },
}

/// Status of acknowledgment during negotiation (server-side perspective)
/// This provides more granular control than the client-side Acknowledgement enum
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AckStatus {
    /// Simple acknowledgment
    Common,
    /// Ready to receive more (multi-ack mode)
    Continue,
    /// Ready to send pack
    Ready,
}

impl AckStatus {
    /// Convert to client-side Acknowledgement when possible
    pub fn to_acknowledgement(self, oid: ObjectId) -> Option<Acknowledgement> {
        match self {
            AckStatus::Common => Some(Acknowledgement::Common(oid)),
            AckStatus::Ready => Some(Acknowledgement::Ready),
            AckStatus::Continue => None, // Continue doesn't map to client-side enum
        }
    }
}

/// Negotiation state tracking
#[derive(Debug, Default)]
pub struct NegotiationState {
    /// Objects the client wants
    pub wants: HashSet<ObjectId>,
    /// Objects the client has
    pub haves: HashSet<ObjectId>,
    /// Common objects found
    pub common: HashSet<ObjectId>,
    /// Shallow commits
    pub shallow: HashSet<ObjectId>,
    /// Whether negotiation is complete
    pub done: bool,
    /// Deepen specification
    pub deepen: Option<DeepenSpec>,
    /// Filter specification
    pub filter: Option<BString>,
}

/// Specification for deepening shallow clones
#[derive(Debug, Clone)]
pub enum DeepenSpec {
    /// Deepen by commit count
    Depth(u32),
    /// Deepen since timestamp
    Since(gix_date::Time),
    /// Deepen excluding refs
    Not(Vec<BString>),
}

/// Statistics about pack generation
#[derive(Debug, Default)]
pub struct PackStats {
    /// Number of objects in pack
    pub objects: u32,
    /// Total pack size in bytes
    pub size: u64,
    /// Number of deltified objects
    pub deltas: u32,
    /// Time taken to generate pack
    pub generation_time: std::time::Duration,
}

/// Upload pack session context
#[derive(Debug)]
pub struct SessionContext {
    /// Client capabilities
    pub capabilities: ClientCapabilities,
    /// Server capabilities
    pub server_capabilities: Option<ServerCapabilities>,
    /// Negotiation state
    pub negotiation: NegotiationState,
    /// Protocol version being used
    pub protocol_version: ProtocolVersion,
    /// Whether this is a stateless RPC session
    pub stateless_rpc: bool,
    /// Session start time
    pub start_time: std::time::Instant,
    /// Repository being served
    pub repository_path: std::path::PathBuf,
}

impl SessionContext {
    /// Create a new session context
    pub fn new(repository_path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            capabilities: ClientCapabilities::default(),
            server_capabilities: None,
            negotiation: NegotiationState::default(),
            protocol_version: ProtocolVersion::default(),
            stateless_rpc: false,
            start_time: std::time::Instant::now(),
            repository_path: repository_path.into(),
        }
    }

    /// Get session duration
    pub fn duration(&self) -> std::time::Duration {
        self.start_time.elapsed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gix_packetline::Channel as SideBandChannel;

    #[test]
    fn test_sideband_mode_max_data_size() {
        assert_eq!(SideBandMode::None.max_data_size(), None);
        assert_eq!(SideBandMode::Basic.max_data_size(), Some(999));
        assert_eq!(SideBandMode::SideBand64k.max_data_size(), Some(65519));
    }

    #[test]
    fn test_sideband_mode_supports_channel() {
        // None supports no channels
        assert!(!SideBandMode::None.supports_channel(SideBandChannel::Data));
        assert!(!SideBandMode::None.supports_channel(SideBandChannel::Progress));
        assert!(!SideBandMode::None.supports_channel(SideBandChannel::Error));
        
        // Basic only supports Progress
        assert!(!SideBandMode::Basic.supports_channel(SideBandChannel::Data));
        assert!(SideBandMode::Basic.supports_channel(SideBandChannel::Progress));
        assert!(!SideBandMode::Basic.supports_channel(SideBandChannel::Error));
        
        // SideBand64k supports all channels
        assert!(SideBandMode::SideBand64k.supports_channel(SideBandChannel::Data));
        assert!(SideBandMode::SideBand64k.supports_channel(SideBandChannel::Progress));
        assert!(SideBandMode::SideBand64k.supports_channel(SideBandChannel::Error));
    }

    #[test]
    fn test_sideband_mode_from_capability_string() {
        assert_eq!(SideBandMode::from_capability_string("side-band"), Some(SideBandMode::Basic));
        assert_eq!(SideBandMode::from_capability_string("side-band-64k"), Some(SideBandMode::SideBand64k));
        assert_eq!(SideBandMode::from_capability_string("unknown"), None);
        assert_eq!(SideBandMode::from_capability_string(""), None);
    }

    #[test]
    fn test_sideband_mode_to_capability_strings() {
        assert_eq!(SideBandMode::None.to_capability_strings(), Vec::<&str>::new());
        assert_eq!(SideBandMode::Basic.to_capability_strings(), vec!["side-band"]);
        assert_eq!(SideBandMode::SideBand64k.to_capability_strings(), vec!["side-band", "side-band-64k"]);
    }

    #[test]
    fn test_sideband_mode_to_v2_capability_strings() {
        assert_eq!(SideBandMode::None.to_v2_capability_strings(), Vec::<&str>::new());
        assert_eq!(SideBandMode::Basic.to_v2_capability_strings(), vec!["sideband"]);
        assert_eq!(SideBandMode::SideBand64k.to_v2_capability_strings(), vec!["sideband-all"]);
    }

    #[test]
    fn test_capability_roundtrip() {
        // Test v1 capabilities
        assert_eq!(SideBandMode::from_capability_string("side-band"), Some(SideBandMode::Basic));
        assert_eq!(SideBandMode::from_capability_string("side-band-64k"), Some(SideBandMode::SideBand64k));
        
        // Test that the first capability from SideBand64k mode can roundtrip to Basic
        // (This is expected behavior since "side-band" appears first)
        let sideband64k_caps = SideBandMode::SideBand64k.to_capability_strings();
        assert_eq!(sideband64k_caps, vec!["side-band", "side-band-64k"]);
        assert_eq!(SideBandMode::from_capability_string(sideband64k_caps[0]), Some(SideBandMode::Basic));
        assert_eq!(SideBandMode::from_capability_string(sideband64k_caps[1]), Some(SideBandMode::SideBand64k));
    }
}
