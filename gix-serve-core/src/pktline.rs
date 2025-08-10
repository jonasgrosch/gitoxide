//! pkt-line re-exports and helpers for server usage.

#[cfg(feature = "blocking-io")]
use gix_packetline_blocking as pkt;
#[cfg(feature = "async-io")]
use gix_packetline as pkt;

#[cfg(all(feature = "blocking-io", feature = "async-io"))]
compile_error!("Cannot enable both 'blocking-io' and 'async-io' in gix-serve-core");

pub use pkt::{
    Channel as SidebandChannel, PacketLineRef, StreamingPeekableIter as PktIter, Writer as PktWriter,
};

/// Write a flush packet.
#[cfg(feature = "blocking-io")]
pub fn write_flush<W: std::io::Write>(w: &mut PktWriter<W>) -> std::io::Result<()> {
    pkt::encode::flush_to_write(w.inner_mut()).map(|_| ())
}

/// Write a delimiter packet.
#[cfg(feature = "blocking-io")]
pub fn write_delimiter<W: std::io::Write>(w: &mut PktWriter<W>) -> std::io::Result<()> {
    pkt::encode::delim_to_write(w.inner_mut()).map(|_| ())
}

/// Write a response-end packet.
#[cfg(feature = "blocking-io")]
pub fn write_response_end<W: std::io::Write>(w: &mut PktWriter<W>) -> std::io::Result<()> {
    pkt::encode::response_end_to_write(w.inner_mut()).map(|_| ())
}

/// Write a sideband progress message (channel 2).
#[cfg(feature = "blocking-io")]
pub fn write_sideband_progress<W: std::io::Write>(w: &mut PktWriter<W>, msg: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    w.write_all(msg).map(|_| ())
}

/// Write a sideband error message (channel 3).
#[cfg(feature = "blocking-io")]
pub fn write_sideband_error<W: std::io::Write>(w: &mut PktWriter<W>, msg: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    w.write_all(msg).map(|_| ())
}
