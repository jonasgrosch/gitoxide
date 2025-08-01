//! Protocol version implementations

pub mod v1;
pub mod v2;

use crate::{error::Result, types::SessionContext};
use std::io::{Read, Write};

#[cfg(feature = "async")]
use futures_lite::io::{AsyncRead, AsyncWrite};

/// Common trait for protocol handlers
#[cfg(feature = "async")]
pub trait ProtocolHandler {
    /// Handle a complete upload-pack session
    async fn handle_session<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
        &mut self,
        input: R,
        output: W,
        session: &mut SessionContext,
    ) -> Result<()>;
}

/// Common trait for protocol handlers (sync version)
#[cfg(not(feature = "async"))]
pub trait ProtocolHandler {
    /// Handle a complete upload-pack session
    fn handle_session<R: Read, W: Write>(
        &mut self,
        input: R,
        output: W,
        session: &mut SessionContext,
    ) -> Result<()>;
}
