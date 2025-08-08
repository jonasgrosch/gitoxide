use std::io::{self, Write as _};

use super::{HiddenRefPredicate, RefRecord};
use crate::protocol::capabilities::{CapabilityOrdering, CapabilitySet};
use gix_packetline_blocking as pkt;

/// Writes v0/v1-style advertisements for receive-pack (blocking).
///
/// Format (first line):
///   <oid> <refname>\0<capabilities space-separated>
/// Subsequent lines:
///   <oid> <refname>
/// Finalize with a FLUSH pkt-line.
///
/// Notes
/// - For empty repositories, a special first line is emitted using a zero OID and the refname `capabilities^{}`.
/// - Capability ordering is controllable; strict-compat details will be added in later milestones.
pub struct Advertiser<W: io::Write> {
    out: pkt::Writer<W>,
    ordering: CapabilityOrdering,
}

impl<W: io::Write> Advertiser<W> {
    /// Create a new advertiser over the given writer, in text mode.
    pub fn new(write: W) -> Self {
        let mut out = pkt::Writer::new(write);
        out.enable_text_mode(); // ensure newline-terminated text pkt-lines
        Self {
            out,
            ordering: CapabilityOrdering::PreserveIdiomatic,
        }
    }

    /// Set the capability ordering policy.
    pub fn with_ordering(mut self, ordering: CapabilityOrdering) -> Self {
        self.ordering = ordering;
        self
    }

    /// Write the advertisement for the provided refs and capabilities, applying an optional hidden predicate.
    ///
    /// Returns Ok on success. IO errors are currently mapped to crate::Error::Unimplemented as error taxonomy
    /// wiring will be added in a later milestone.
    pub fn write_advertisement(
        &mut self,
        refs: &[RefRecord],
        caps: &CapabilitySet,
        hidden: Option<&HiddenRefPredicate>,
    ) -> Result<(), crate::Error> {
        let mut visible: Vec<&RefRecord> = match hidden {
            Some(pred) => refs.iter().filter(|r| !(pred)(r)).collect(),
            None => refs.iter().collect(),
        };

        let caps_line = caps.encode(self.ordering);

        if visible.is_empty() {
            // Empty repository: emit a special capabilities line with a zero OID and 'capabilities^{}'
            let zeros = "0".repeat(40); // SHA-1 default; object-format enforcement is added in M2.
            let first = format!("{zeros} capabilities^{{}}\0{caps_line}");
            self.out
                .write_all(first.as_bytes())
                .map_err(|_| crate::Error::Unimplemented)?;
            pkt::encode::flush_to_write(self.out.inner_mut()).map_err(|_| crate::Error::Unimplemented)?;
            self.out.flush().map_err(|_| crate::Error::Unimplemented)?;
            return Ok(());
        }

        // First visible ref line carries capabilities after a NUL
        let first_ref = visible.remove(0);
        let first = format!("{} {}\0{caps_line}", first_ref.oid.to_string(), first_ref.name);
        self.out
            .write_all(first.as_bytes())
            .map_err(|_| crate::Error::Unimplemented)?;

        // Remaining refs as standard lines
        for r in visible {
            let line = format!("{} {}", r.oid.to_string(), r.name);
            self.out.write_all(line.as_bytes()).map_err(|_| crate::Error::Unimplemented)?;
        }

        // Final flush
        pkt::encode::flush_to_write(self.out.inner_mut()).map_err(|_| crate::Error::Unimplemented)?;
        self.out.flush().map_err(|_| crate::Error::Unimplemented)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gix_packetline_blocking::{PacketLineRef, StreamingPeekableIter};

    fn oid(hex40: &str) -> gix_hash::ObjectId {
        gix_hash::ObjectId::from_hex(hex40.as_bytes()).expect("valid hex")
    }

    fn collect_data_lines(buf: &[u8]) -> Vec<Vec<u8>> {
        let mut rd = StreamingPeekableIter::new(std::io::Cursor::new(buf), &[PacketLineRef::Flush], false);
        let mut out = Vec::new();
        while let Some(next) = rd.read_line() {
            match next.expect("io ok").expect("decode ok") {
                PacketLineRef::Data(d) => out.push(d.to_vec()),
                PacketLineRef::Flush | PacketLineRef::Delimiter | PacketLineRef::ResponseEnd => break,
            }
        }
        out
    }

    #[test]
    fn non_empty_repo_first_line_carries_caps_after_nul() {
        let refs = vec![
            RefRecord::new(oid("1111111111111111111111111111111111111111"), "refs/heads/main"),
            RefRecord::new(oid("2222222222222222222222222222222222222222"), "refs/tags/v1"),
        ];
        let caps = CapabilitySet::modern_defaults().with_agent(Some("gix/1.0".into()));
        let mut buf = Vec::new();
        let mut adv = Advertiser::new(&mut buf).with_ordering(CapabilityOrdering::Lexicographic);
        adv.write_advertisement(&refs, &caps, None).unwrap();

        let lines = collect_data_lines(&buf);
        assert_eq!(lines.len(), 2);

        let first = &lines[0];
        let nul = first.iter().position(|b| *b == 0).expect("nul present");
        let before = &first[..nul];
        let after = &first[nul + 1..]; // includes trailing '\n' from text mode

        assert!(before.starts_with(b"1111111111111111111111111111111111111111 refs/heads/main"));
        let after_str = std::str::from_utf8(after).unwrap();
        assert!(after_str.contains("report-status"));
        assert!(after_str.contains("report-status-v2"));
        assert!(after_str.contains("quiet"));
        assert!(after_str.contains("delete-refs"));
        assert!(after_str.contains("ofs-delta"));
        assert!(after_str.contains("agent=gix/1.0"));
    }

    #[test]
    fn hidden_refs_filtered_out() {
        let refs = vec![
            RefRecord::new(oid("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"), "refs/heads/main"),
            RefRecord::new(oid("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"), "refs/hidden/secret"),
        ];
        let caps = CapabilitySet::modern_defaults();
        let mut buf = Vec::new();
        let mut adv = Advertiser::new(&mut buf);
        let hide = |r: &RefRecord| r.name.starts_with("refs/hidden/");
        adv.write_advertisement(&refs, &caps, Some(&hide)).unwrap();

        let lines = collect_data_lines(&buf);
        assert_eq!(lines.len(), 1);
        let first = &lines[0];
        let nul = first.iter().position(|b| *b == 0).expect("nul present");
        let before = &first[..nul];
        assert!(before.starts_with(b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa refs/heads/main"));
    }

    #[test]
    fn empty_repo_emits_capabilities_ref() {
        let refs: Vec<RefRecord> = Vec::new();
        let caps = CapabilitySet::modern_defaults();
        let mut buf = Vec::new();
        let mut adv = Advertiser::new(&mut buf);
        adv.write_advertisement(&refs, &caps, None).unwrap();

        let lines = collect_data_lines(&buf);
        assert_eq!(lines.len(), 1);
        let first = &lines[0];
        let nul = first.iter().position(|b| *b == 0).expect("nul present");
        let before = &first[..nul];
        // 40 zeros for SHA-1 default in M1.
        assert!(before.starts_with(b"0000000000000000000000000000000000000000 capabilities^{}"));
    }
}