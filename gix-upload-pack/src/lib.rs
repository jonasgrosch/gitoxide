//! Git upload-pack server implementation for gitoxide
//!
//! This crate provides a complete implementation of Git's upload-pack service,
//! which handles client requests for fetching objects from a Git repository.
//! It supports both protocol v1 and v2, with full feature parity with Git's
//! native upload-pack implementation.
//!
//! # Features
//!
//! - Full protocol v1 and v2 support
//! - Shallow clone and partial clone support
//! - Object filtering (blob size, tree depth, etc.)
//! - Sideband communication
//! - Multi-ack negotiation algorithms
//! - Ref advertisement and filtering
//! - Hook support for customization
//! - Comprehensive capability management
//! - Drop-in replacement for git-upload-pack
//!
//! # Example Usage
//!
//! ```no_run
//! use gix_upload_pack::{Server, ServerOptions};
//! use std::io::{stdin, stdout};
//!
//! // Create a server instance
//! let options = ServerOptions::default()
//!     .with_stateless_rpc(false)
//!     .with_advertise_refs(false);
//!     
//! let mut server = Server::new("/path/to/repo", options)?;
//!
//! // Handle upload-pack protocol (now synchronous)
//! let stdin = stdin();
//! let stdout = stdout();
//! server.serve(stdin.lock(), stdout.lock())?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![deny(rust_2018_idioms)]
// #![warn(missing_docs, clippy::all, clippy::pedantic)]

pub mod config;
pub mod error;
pub mod protocol;
pub mod server;
pub mod services;
mod types;

pub use config::ServerOptions;
pub use error::{Error, Result};
pub use server::Server;
pub use types::*;

/// The version of this crate
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
