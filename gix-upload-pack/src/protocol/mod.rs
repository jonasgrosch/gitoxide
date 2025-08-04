//! Protocol version implementations

pub mod v1;
pub mod v2;

use crate::{error::Result, types::SessionContext};

use std::io::{Read, Write};

/// Common trait for protocol handlers
pub trait ProtocolHandler {
    /// Handle a complete upload-pack session
    fn handle_session<R: Read, W: Write>(&mut self, reader: R, writer: W, session: &mut SessionContext) -> Result<()>;
}
