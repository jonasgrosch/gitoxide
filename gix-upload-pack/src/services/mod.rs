//! Shared business logic services
//!
//! This module contains the core business logic services that are used by
//! both protocol v1 and v2 handlers. These services are designed to be
//! dependency-injected into protocol handlers for better testability and
//! separation of concerns.

pub mod capabilities;
pub mod command_parser;
pub mod pack;
pub mod packet_io;
pub mod references;

// Re-export commonly used types for convenience
pub use capabilities::CapabilityManager;
pub use command_parser::CommandParser;
pub use pack::{PackGenerator, ProgressReporter};
pub use packet_io::PacketIOFactory;
pub use references::ReferenceManager;