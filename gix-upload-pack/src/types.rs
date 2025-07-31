//! Common types and structures used throughout the upload-pack implementation

use bstr::BString;
use gix_hash::ObjectId;
use smallvec::SmallVec;
use std::collections::HashSet;

/// Protocol version supported by the server
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProtocolVersion {
    /// Version 0 (legacy, no explicit version)
    V0 = 0,
    /// Version 1 (stateful)
    V1 = 1,
    /// Version 2 (stateless, preferred)
    V2 = 2,
}

impl Default for ProtocolVersion {
    fn default() -> Self {
        Self::V2
    }
}

impl From<ProtocolVersion> for gix_transport::Protocol {
    fn from(version: ProtocolVersion) -> Self {
        match version {
            ProtocolVersion::V0 => Self::V0,
            ProtocolVersion::V1 => Self::V1,
            ProtocolVersion::V2 => Self::V2,
        }
    }
}

impl TryFrom<u8> for ProtocolVersion {
    type Error = crate::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::V0),
            1 => Ok(Self::V1),
            2 => Ok(Self::V2),
            version => Err(crate::Error::InvalidProtocolVersion { version }),
        }
    }
}

/// Represents a Git reference with its target
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    /// The full reference name (e.g., "refs/heads/main")
    pub name: BString,
    /// The object ID this reference points to
    pub target: ObjectId,
    /// Whether this is a peeled reference (annotated tag -> commit)
    pub peeled: Option<ObjectId>,
}

/// Client capabilities and requested features
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

/// Side-band modes for multiplexed communication
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

/// Server capabilities that can be advertised to clients
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
    /// Object info support (protocol v2)
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
            no_progress: true,
            filter: true,
            allow_tip_sha1_in_want: false,
            allow_reachable_sha1_in_want: false,
            allow_any_sha1_in_want: false,
            no_done: true,
            agent: format!("git/gitoxide-{}", crate::VERSION).into(),
            object_format: smallvec::smallvec![gix_hash::Kind::Sha1],
            session_id: None,
            packfile_uris: false,
            wait_for_done: true,
            object_info: false,
        }
    }
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
    /// Acknowledge client has object
    Ack {
        /// Object ID being acknowledged
        oid: ObjectId,
        /// Status of the acknowledgment
        status: AckStatus,
    },
    /// Negative acknowledgment
    Nak,
    /// Server is ready to send pack
    Ready,
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

/// Status of acknowledgment during negotiation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AckStatus {
    /// Simple acknowledgment
    Common,
    /// Ready to receive more
    Continue,
    /// Ready to send pack
    Ready,
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
