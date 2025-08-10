//! Thin progress adapters for server-side sideband emission.
//! Keep progress semantics in service crates; this module only provides wiring.

use crate::pktline::{write_sideband_progress, PktWriter};

/// A minimal sink for progress messages.
pub trait ProgressSink {
    /// Emit a user-visible progress message.
    fn info(&mut self, message: &[u8]);
}

/// Progress sink that writes messages to sideband channel 2 using pkt-line writer.
#[cfg(feature = "blocking-io")]
pub struct SidebandProgressWriter<'a, W: std::io::Write> {
    writer: &'a mut PktWriter<W>,
}

#[cfg(feature = "blocking-io")]
impl<'a, W: std::io::Write> SidebandProgressWriter<'a, W> {
    /// Create a new progress writer over a pkt-line writer.
    pub fn new(writer: &'a mut PktWriter<W>) -> Self {
        Self { writer }
    }
}

#[cfg(feature = "blocking-io")]
impl<'a, W: std::io::Write> ProgressSink for SidebandProgressWriter<'a, W> {
    fn info(&mut self, message: &[u8]) {
        let _ = write_sideband_progress(self.writer, message);
    }
}

/// Optional bridge to gix-features progress API.
#[cfg(all(feature = "blocking-io", feature = "progress"))]
pub fn bridge_to_gix_features<'a, W: std::io::Write + 'a>(
    sink: &'a mut dyn ProgressSink,
) -> impl gix_features::progress::Progress + 'a {
    use gix_features::progress::Progress;
    struct Bridge<'a> {
        sink: &'a mut dyn ProgressSink,
    }
    impl<'a> Progress for Bridge<'a> {
        fn init(&mut self, _max: Option<u64>, _unit: gix_features::progress::Unit) {}
        fn set(&self, _value: u64) {}
        fn inc_by(&self, _delta: usize) {}
        fn info(&self, msg: String) {
            self.sink.info(msg.as_bytes())
        }
        fn message(&self, _level: gix_features::progress::MessageLevel, msg: String) {
            self.sink.info(msg.as_bytes())
        }
        fn done(&self) {}
    }
    Bridge { sink }
}


