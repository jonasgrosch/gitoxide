//! Main server implementation for upload-pack

use crate::{
    config::ServerOptions,
    error::{Error, Result},
    protocol::{v1, v2, ProtocolHandler},
    types::*,
};
use gix::Repository;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub mod capabilities;
pub mod handshake;
pub mod negotiation;
pub mod pack_generation;
pub mod response;

/// The main upload-pack server implementation
#[derive(Debug)]
pub struct Server {
    /// Repository being served
    repository: Repository,
    
    /// Server configuration options
    options: ServerOptions,
    
    /// Repository path
    repository_path: PathBuf,
}

impl Server {
    /// Create a new server instance for the given repository
    pub fn new<P: AsRef<Path>>(repository_path: P, options: ServerOptions) -> Result<Self> {
        let repository_path = repository_path.as_ref().to_path_buf();
        
        // Validate and open the repository
        let repository = gix::open(&repository_path)
            .map_err(|e| Error::Repository(e))?;
            
        // Validate configuration
        options.validate()?;
        
        Ok(Self {
            repository,
            options,
            repository_path,
        })
    }
    
    /// Create a server with configuration loaded from the repository
    pub fn from_repository<P: AsRef<Path>>(repository_path: P) -> Result<Self> {
        let repository_path = repository_path.as_ref().to_path_buf();
        let repository = gix::open(&repository_path)?;
        let options = ServerOptions::from_repository(&repository)?;
        
        Ok(Self {
            repository,
            options,
            repository_path,
        })
    }
    
    /// Serve upload-pack protocol over the given input/output streams
    pub fn serve<R: Read, W: Write>(&mut self, input: R, output: W) -> Result<()> {
        let mut session = SessionContext::new(&self.repository_path);
        session.stateless_rpc = self.options.stateless_rpc;
        
        // Determine protocol version using environment variable or default for now
        session.protocol_version = self.detect_protocol_version()?;
        eprintln!("Debug: Using protocol version: {:?}", session.protocol_version);
        
        match session.protocol_version {
            ProtocolVersion::V0 | ProtocolVersion::V1 => {
                self.serve_v1(input, output, session)
            }
            ProtocolVersion::V2 => {
                self.serve_v2(input, output, session)
            }
        }
    }

    /// Serve using protocol version 1
    fn serve_v1<R: Read, W: Write>(
        &mut self,
        input: R,
        output: W,
        mut session: SessionContext,
    ) -> Result<()> {
        let mut handler = v1::Handler::new(&self.repository, &self.options);
        handler.handle_session(input, output, &mut session)
    }
    
    /// Serve using protocol version 2
    fn serve_v2<R: Read, W: Write>(
        &mut self,
        input: R,
        output: W,
        mut session: SessionContext,
    ) -> Result<()> {
        let mut handler = v2::Handler::new(&self.repository, &self.options);
        handler.handle_session(input, output, &mut session)
    }
    
    /// Detect protocol version from environment or default
    fn detect_protocol_version(&self) -> Result<ProtocolVersion> {
        // Check GIT_PROTOCOL environment variable
        if let Ok(protocol) = std::env::var("GIT_PROTOCOL") {
            match protocol.as_str() {
                "version=0" => Ok(ProtocolVersion::V0),
                "version=1" => Ok(ProtocolVersion::V1),
                "version=2" => Ok(ProtocolVersion::V2),
                _ => Ok(ProtocolVersion::V1), // Default fallback
            }
        } else {
            // For our testing, default to V2 since we want to test the fetch functionality
            Ok(ProtocolVersion::V2)
        }
    }
    
    /// Get repository reference
    pub fn repository(&self) -> &Repository {
        &self.repository
    }
    
    /// Get mutable repository reference
    pub fn repository_mut(&mut self) -> &mut Repository {
        &mut self.repository
    }
    
    /// Get server options
    pub fn options(&self) -> &ServerOptions {
        &self.options
    }
    
    /// Update server options
    pub fn set_options(&mut self, options: ServerOptions) -> Result<()> {
        options.validate()?;
        self.options = options;
        Ok(())
    }
    
    /// Get repository path
    pub fn repository_path(&self) -> &Path {
        &self.repository_path
    }
    
    /// Set stateless RPC mode
    pub fn stateless_rpc(mut self, stateless: bool) -> Self {
        self.options.stateless_rpc = stateless;
        self
    }
}

/// Builder for creating and configuring a Server
#[derive(Debug, Default)]
pub struct ServerBuilder {
    options: ServerOptions,
}

impl ServerBuilder {
    /// Create a new server builder
    pub fn new() -> Self {
        Self::default()
    }
    
    /// Set advertise refs mode
    pub fn advertise_refs(mut self, advertise: bool) -> Self {
        self.options.advertise_refs = advertise;
        self
    }
    
    /// Set stateless RPC mode
    pub fn stateless_rpc(mut self, stateless: bool) -> Self {
        self.options.stateless_rpc = stateless;
        self
    }
    
    /// Set timeout
    pub fn timeout(mut self, timeout: std::time::Duration) -> Self {
        self.options.timeout = Some(timeout);
        self
    }
    
    /// Set strict mode
    pub fn strict(mut self, strict: bool) -> Self {
        self.options.strict = strict;
        self
    }
    
    /// Add hidden ref pattern
    pub fn hide_ref<S: Into<bstr::BString>>(mut self, pattern: S) -> Self {
        self.options.hidden_refs.push(pattern.into());
        self
    }
    
    /// Enable shallow support
    pub fn allow_shallow(mut self, allow: bool) -> Self {
        self.options.allow_shallow = allow;
        self
    }
    
    /// Enable filter support
    pub fn allow_filter(mut self, allow: bool) -> Self {
        self.options.allow_filter = allow;
        self
    }
    
    /// Build the server
    pub fn build<P: AsRef<Path>>(self, repository_path: P) -> Result<Server> {
        Server::new(repository_path, self.options)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_server_creation() {
        // This test would require a proper git repository
        // For now, just test that the API compiles
        let _builder = ServerBuilder::new()
            .advertise_refs(true)
            .stateless_rpc(false)
            .timeout(std::time::Duration::from_secs(300))
            .strict(true)
            .hide_ref("refs/internal/*")
            .allow_shallow(true)
            .allow_filter(true);
    }
}
