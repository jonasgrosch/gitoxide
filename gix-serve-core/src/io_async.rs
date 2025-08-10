//! Async I/O helpers for server flows.

use crate::pktline::PktWriter;
use futures_io::AsyncWrite;

/// Create a pkt-line writer over an async `AsyncWrite`.
pub fn pkt_writer<W: AsyncWrite + Unpin>(w: W) -> PktWriter<W> {
    PktWriter::new(w)
}


