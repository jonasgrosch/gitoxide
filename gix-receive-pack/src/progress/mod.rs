// M3: Progress adapters and sideband integration (blocking-first).
//
// This module provides a SidebandProgressWriter for emitting progress over sideband
// channel 2, along with a minimal bridge that can mirror prodash progress messages
// to the sideband writer. KEEPALIVE policy is intentionally minimal and will evolve
// with later milestones.
//
// Notes
// - We use gix-packetline-blocking encoders to avoid duplicating pkt-line semantics.
// - Progress remains strictly on sideband channel 2 and never interferes with report-status.

use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gix_features::progress::{DynNestedProgress, Id, MessageLevel, Progress, Step, Unit};
use gix_packetline_blocking as pkt;
use gix_packetline_blocking::encode as enc;

/// Keepalive emission policy. See upstream receive-pack for reference behavior.
///
/// Mapping to upstream:
/// - KEEPALIVE_NEVER
/// - KEEPALIVE_AFTER_NUL
/// - KEEPALIVE_ALWAYS
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepalivePolicy {
    Never,
    AfterNul,
    Always,
}

/// A blocking sideband progress writer that emits progress on channel 2 exclusively.
///
/// It writes pkt-lines via gix-packetline-blocking and can emit a keepalive frame
/// (a single NUL) depending on the configured policy. By default, it starts with
/// KeepalivePolicy::AfterNul to emulate upstream behavior for receive-pack.
#[derive(Debug)]
pub struct SidebandProgressWriter<W: Write> {
    out: W,
    policy: KeepalivePolicy,
    last_keepalive: Option<Instant>,
    keepalive_interval: Option<Duration>,
}

impl<W: Write> SidebandProgressWriter<W> {
    /// Create a new sideband progress writer with KEEPALIVE_AFTER_NUL policy and no interval timer.
    pub fn new(out: W) -> Self {
        Self {
            out,
            policy: KeepalivePolicy::AfterNul,
            last_keepalive: None,
            keepalive_interval: None,
        }
    }

    /// Set the keepalive policy.
    pub fn set_policy(&mut self, policy: KeepalivePolicy) {
        self.policy = policy;
    }

    /// Set an optional keepalive interval. If set, [`keepalive_tick()`] will emit
    /// a keepalive at the given cadence depending on policy.
    pub fn set_keepalive_interval(&mut self, interval: Option<Duration>) {
        self.keepalive_interval = interval;
    }

    /// Emit a progress payload over sideband channel 2.
    ///
    /// The payload is transmitted verbatim. Callers control formatting, e.g. adding
    /// trailing newlines or carriage returns if desired by clients.
    pub fn emit_progress(&mut self, message: &[u8]) -> io::Result<()> {
        // Upstream expects sideband '2' for progress.
        let _ = enc::band_to_write(pkt::Channel::Progress, message, &mut self.out)?;
        self.flush()
    }

    /// Emit a keepalive frame depending on policy. For now we use a single NUL byte
    /// over channel 2, matching common client expectations to ignore it.
    pub fn emit_keepalive(&mut self) -> io::Result<()> {
        let _ = enc::band_to_write(pkt::Channel::Progress, b"\0", &mut self.out)?;
        self.last_keepalive = Some(Instant::now());
        self.flush()
    }

    /// Periodic tick to maintain keepalives based on configured policy and interval.
    ///
    /// Behavior:
    /// - Never: no-op
    /// - AfterNul: emit keepalive only after the first logical NUL has been observed by higher layers (not tracked here);
    ///             since we don't see the input, treat this as disabled until policy is set to Always.
    /// - Always: if `keepalive_interval` is set and elapsed, emit a keepalive.
    pub fn keepalive_tick(&mut self) -> io::Result<()> {
        match self.policy {
            KeepalivePolicy::Never => Ok(()),
            KeepalivePolicy::AfterNul => {
                // Without input context, we can't tell if a NUL was seen; defer to later policy switch.
                Ok(())
            }
            KeepalivePolicy::Always => {
                if let Some(interval) = self.keepalive_interval {
                    let due = match self.last_keepalive {
                        Some(t) => t.elapsed() >= interval,
                        None => true,
                    };
                    if due {
                        self.emit_keepalive()?;
                    }
                }
                Ok(())
            }
        }
    }

    /// Flush the underlying writer.
    pub fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }

    /// Access the underlying writer mutably.
    pub fn inner_mut(&mut self) -> &mut W {
        &mut self.out
    }

    /// Access the underlying writer by reference.
    pub fn inner(&self) -> &W {
        &self.out
    }
}

/// A DynNestedProgress bridge that mirrors progress messages to a shared sideband writer.
///
/// It delegates all counting and structure to an inner DynNestedProgress while writing
/// informational messages to sideband channel 2. This preserves the separation rule:
/// status/report packets are not emitted here.
pub struct SidebandDynProgress<W: Write + Send> {
    inner: Box<dyn DynNestedProgress>,
    writer: Arc<Mutex<SidebandProgressWriter<W>>>,
    name: Option<String>,
    id: Id,
}

impl<W: Write + Send> SidebandDynProgress<W> {
    /// Create a new sideband-progress bridge.
    pub fn new(inner: Box<dyn DynNestedProgress>, writer: W) -> Self {
        let id = inner.id();
        Self {
            inner,
            writer: Arc::new(Mutex::new(SidebandProgressWriter::new(writer))),
            name: None,
            id,
        }
    }

    /// Create a child clone sharing the same sideband writer.
    fn with_shared_writer(inner: Box<dyn DynNestedProgress>, shared: Arc<Mutex<SidebandProgressWriter<W>>>, name: Option<String>) -> Self {
        let id = inner.id();
        Self {
            inner,
            writer: shared,
            name,
            id,
        }
    }

    /// Write a message to sideband channel 2, prefixing with the current progress name if present.
    fn sideband_message(&self, _level: MessageLevel, mut message: String) {
        // We don't encode the level textual marker by default; keep it simple for now.
        if let Some(name) = &self.name {
            // Prefix "name: " similar to gix RemoteProgress translator
            message = format!("{}: {}", name.split_once(':').map_or(name.as_str(), |x| x.0), message);
        }
        // Best-effort emission; ignore IO errors as progress must not affect protocol correctness.
        if let Ok(mut w) = self.writer.lock() {
            let _ = w.emit_progress(message.as_bytes());
        }
    }

    /// Access the shared writer, e.g. to toggle policy to Always once ingestion starts.
    pub fn writer(&self) -> Arc<Mutex<SidebandProgressWriter<W>>> {
        self.writer.clone()
    }
}

// Delegation of Count to the inner progress
impl<W: Write + Send> gix_features::progress::Count for SidebandDynProgress<W> {
    fn set(&self, step: Step) {
        self.inner.set(step)
    }
    fn step(&self) -> Step {
        self.inner.step()
    }
    fn inc_by(&self, step: Step) {
        self.inner.inc_by(step)
    }
    fn counter(&self) -> gix_features::progress::StepShared {
        self.inner.counter()
    }
}

// Implement core Progress trait, mirroring message() to sideband channel 2.
impl<W: Write + Send> Progress for SidebandDynProgress<W> {
    fn init(&mut self, max: Option<Step>, unit: Option<Unit>) {
        self.inner.init(max, unit)
    }
    fn unit(&self) -> Option<Unit> {
        self.inner.unit()
    }
    fn max(&self) -> Option<Step> {
        self.inner.max()
    }
    fn set_max(&mut self, max: Option<Step>) -> Option<Step> {
        self.inner.set_max(max)
    }
    fn set_name(&mut self, name: String) {
        self.name = Some(name.clone());
        self.inner.set_name(name);
    }
    fn name(&self) -> Option<String> {
        self.inner.name()
    }
    fn id(&self) -> Id {
        self.id
    }
    fn message(&self, level: MessageLevel, message: String) {
        // forward to inner
        self.inner.message(level, message.clone());
        // mirror to sideband
        self.sideband_message(level, message);
    }
}

// NestedProgress implementation - create child nodes that share the same writer.
impl<W: Write + Send> gix_features::progress::NestedProgress for SidebandDynProgress<W> {
    type SubProgress = Self;

    fn add_child(&mut self, name: impl Into<String>) -> Self::SubProgress {
        let name = name.into();
        let child = self.inner.add_child(name.clone());
        Self::with_shared_writer(Box::new(child), self.writer.clone(), Some(name))
    }

    fn add_child_with_id(&mut self, name: impl Into<String>, id: Id) -> Self::SubProgress {
        let name = name.into();
        let child = self.inner.add_child_with_id(name.clone(), id);
        let mut out = Self::with_shared_writer(Box::new(child), self.writer.clone(), Some(name));
        out.id = id;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gix_features::progress::DynNestedProgress;

    // A minimal inner progress that supports DynNestedProgress via Discard root.
    struct DummyDyn;
    impl gix_features::progress::Count for DummyDyn {
        fn set(&self, _step: Step) {}
        fn step(&self) -> Step { 0 }
        fn inc_by(&self, _step: Step) {}
        fn counter(&self) -> gix_features::progress::StepShared { gix_features::progress::StepShared::default() }
    }
    impl Progress for DummyDyn {
        fn init(&mut self, _max: Option<Step>, _unit: Option<Unit>) {}
        fn set_name(&mut self, _name: String) {}
        fn name(&self) -> Option<String> { None }
        fn id(&self) -> Id { *b"SBND" }
        fn message(&self, _level: MessageLevel, _message: String) {}
    }
    impl gix_features::progress::NestedProgress for DummyDyn {
        type SubProgress = DummyDyn;
        fn add_child(&mut self, _name: impl Into<String>) -> Self::SubProgress { DummyDyn }
        fn add_child_with_id(&mut self, _name: impl Into<String>, _id: Id) -> Self::SubProgress { DummyDyn }
    }

    #[test]
    fn sideband_writer_emits_data_and_keepalive() {
        let mut buf = Vec::<u8>::new();
        {
            let mut sb = SidebandProgressWriter::new(&mut buf);
            sb.emit_progress(b"indexing objects") .unwrap();
            sb.emit_keepalive().unwrap();
            sb.flush().unwrap();
        }
        // There should be two pkt-lines in the buffer, we don't parse fully here.
        assert!(!buf.is_empty());
    }

    #[test]
    fn dyn_bridge_writes_messages() {
        let inner: Box<dyn DynNestedProgress> = Box::new(DummyDyn);
        let mut out = Vec::<u8>::new();
        {
            let bridge = SidebandDynProgress::new(inner, &mut out);
            // Call message() via Progress::message by using the trait bound
            bridge.message(MessageLevel::Info, "hello".to_string());
        }
        assert!(!out.is_empty());
    }
}