//! Blocking I/O helpers for server flows.

use crate::pktline::{PktWriter, write_flush, write_delimiter, write_response_end};

/// Create a pkt-line writer over a blocking `Write`.
pub fn pkt_writer<W: std::io::Write>(w: W) -> PktWriter<W> {
    PktWriter::new(w)
}

/// Write a typical advertisement trailer: delimiter then flush.
pub fn write_advert_trailer<W: std::io::Write>(w: &mut PktWriter<W>) -> std::io::Result<()> {
    write_delimiter(w)?;
    write_flush(w)
}

/// Finish a multi-response exchange with a response-end pkt.
pub fn write_end<W: std::io::Write>(w: &mut PktWriter<W>) -> std::io::Result<()> {
    write_response_end(w)
}


