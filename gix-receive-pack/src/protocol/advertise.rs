use std::io;

use super::{HiddenRefPredicate, RefRecord};
use crate::protocol::capabilities::{CapabilityFormatter, CapabilityOrdering, CapabilitySet, IdiomaticFormatter};

// Blocking implementation
#[cfg(feature = "blocking-io")]
mod blocking {
    use super::*;
    use std::io::Write;
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
    /// - Capability formatting is controllable via CapabilityFormatter implementations.
    pub struct Advertiser<W: io::Write> {
        out: pkt::Writer<W>,
        formatter: Box<dyn CapabilityFormatter + Send + Sync>,
    }

    impl<W: io::Write> Advertiser<W> {
        /// Create a new advertiser over the given writer, in text mode.
        pub fn new(write: W) -> Self {
            let mut out = pkt::Writer::new(write);
            out.enable_text_mode(); // ensure newline-terminated text pkt-lines
            Self {
                out,
                formatter: Box::new(IdiomaticFormatter::new(CapabilityOrdering::PreserveIdiomatic)),
            }
        }

        /// Set the capability ordering policy (using idiomatic formatter).
        pub fn with_ordering(mut self, ordering: CapabilityOrdering) -> Self {
            self.formatter = Box::new(IdiomaticFormatter::new(ordering));
            self
        }

        /// Set a custom capability formatter.
        pub fn with_formatter(mut self, formatter: Box<dyn CapabilityFormatter + Send + Sync>) -> Self {
            self.formatter = formatter;
            self
        }

        /// Create a new advertiser with strict compatibility formatting.
        /// This method is only available when the "strict-compat" feature is enabled.
        #[cfg(feature = "strict-compat")]
        pub fn with_strict_compat(write: W) -> Self {
            let mut out = pkt::Writer::new(write);
            out.enable_text_mode();
            Self {
                out,
                formatter: Box::new(crate::protocol::capabilities::StrictCompatFormatter::new()),
            }
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

            let caps_line = self.formatter.format_capabilities(caps);

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
}

// Async implementation (stub for compilation)
#[cfg(all(feature = "async-io", not(feature = "blocking-io")))]
mod async_impl {
    use super::*;

    /// Async advertiser stub for compilation.
    /// 
    /// This is a minimal stub to ensure the crate compiles with async-io feature.
    /// Full async implementation will be added in M8.
    pub struct Advertiser<W: io::Write> {
        _write: std::marker::PhantomData<W>,
        formatter: Box<dyn CapabilityFormatter + Send + Sync>,
    }

    impl<W: io::Write> Advertiser<W> {
        /// Create a new advertiser over the given writer, in text mode.
        pub fn new(_write: W) -> Self {
            Self {
                _write: std::marker::PhantomData,
                formatter: Box::new(IdiomaticFormatter::new(CapabilityOrdering::PreserveIdiomatic)),
            }
        }

        /// Set the capability ordering policy (using idiomatic formatter).
        pub fn with_ordering(mut self, ordering: CapabilityOrdering) -> Self {
            self.formatter = Box::new(IdiomaticFormatter::new(ordering));
            self
        }

        /// Set a custom capability formatter.
        pub fn with_formatter(mut self, formatter: Box<dyn CapabilityFormatter + Send + Sync>) -> Self {
            self.formatter = formatter;
            self
        }

        /// Create a new advertiser with strict compatibility formatting.
        /// This method is only available when the "strict-compat" feature is enabled.
        #[cfg(feature = "strict-compat")]
        pub fn with_strict_compat(_write: W) -> Self {
            Self {
                _write: std::marker::PhantomData,
                formatter: Box::new(crate::protocol::capabilities::StrictCompatFormatter::new()),
            }
        }

        /// Write the advertisement for the provided refs and capabilities, applying an optional hidden predicate.
        ///
        /// This is a stub implementation that returns Unimplemented for async-io builds.
        /// Full async implementation will be added in M8.
        pub fn write_advertisement(
            &mut self,
            _refs: &[RefRecord],
            _caps: &CapabilitySet,
            _hidden: Option<&HiddenRefPredicate>,
        ) -> Result<(), crate::Error> {
            Err(crate::Error::Unimplemented)
        }
    }
}

// Re-export the appropriate implementation
#[cfg(feature = "blocking-io")]
pub use blocking::Advertiser;

#[cfg(all(feature = "async-io", not(feature = "blocking-io")))]
pub use async_impl::Advertiser;



#[cfg(all(test, feature = "blocking-io"))]
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

    #[cfg(feature = "strict-compat")]
    #[test]
    fn strict_compat_formatter_ordering() {
        use crate::protocol::capabilities::StrictCompatFormatter;
        
        let mut caps = CapabilitySet::modern_defaults().with_agent(Some("gix/1.0".into()));
        caps.push_extra("atomic");
        caps.side_band_64k = true;

        let formatter = StrictCompatFormatter::new();
        let strict_output = formatter.format_capabilities(&caps);

        // Verify upstream ordering: report-status, report-status-v2, delete-refs, quiet, atomic, ofs-delta, side-band-64k, agent
        let tokens: Vec<&str> = strict_output.split(' ').collect();
        
        // Find positions of key tokens
        let report_status_pos = tokens.iter().position(|&t| t == "report-status").unwrap();
        let report_status_v2_pos = tokens.iter().position(|&t| t == "report-status-v2").unwrap();
        let delete_refs_pos = tokens.iter().position(|&t| t == "delete-refs").unwrap();
        let quiet_pos = tokens.iter().position(|&t| t == "quiet").unwrap();
        let atomic_pos = tokens.iter().position(|&t| t == "atomic").unwrap();
        let ofs_delta_pos = tokens.iter().position(|&t| t == "ofs-delta").unwrap();
        let side_band_pos = tokens.iter().position(|&t| t == "side-band-64k").unwrap();
        let agent_pos = tokens.iter().position(|&t| t.starts_with("agent=")).unwrap();

        // Verify upstream ordering
        assert!(report_status_pos < report_status_v2_pos);
        assert!(report_status_v2_pos < delete_refs_pos);
        assert!(delete_refs_pos < quiet_pos);
        assert!(quiet_pos < atomic_pos);
        assert!(atomic_pos < ofs_delta_pos);
        assert!(ofs_delta_pos < side_band_pos);
        assert!(side_band_pos < agent_pos);
    }

    #[cfg(feature = "strict-compat")]
    #[test]
    fn advertiser_with_strict_compat() {
        let refs = vec![
            RefRecord::new(oid("1111111111111111111111111111111111111111"), "refs/heads/main"),
        ];
        let mut caps = CapabilitySet::modern_defaults().with_agent(Some("gix/1.0".into()));
        caps.push_extra("atomic");
        caps.side_band_64k = true;

        let mut buf = Vec::new();
        let mut adv = Advertiser::with_strict_compat(&mut buf);
        adv.write_advertisement(&refs, &caps, None).unwrap();

        let lines = collect_data_lines(&buf);
        assert_eq!(lines.len(), 1);

        let first = &lines[0];
        let nul = first.iter().position(|b| *b == 0).expect("nul present");
        let after = &first[nul + 1..];
        let after_str = std::str::from_utf8(after).unwrap();

        // Verify that capabilities are in strict upstream order
        let tokens: Vec<&str> = after_str.trim().split(' ').collect();
        let report_status_pos = tokens.iter().position(|&t| t == "report-status").unwrap();
        let atomic_pos = tokens.iter().position(|&t| t == "atomic").unwrap();
        let agent_pos = tokens.iter().position(|&t| t.starts_with("agent=")).unwrap();

        // In strict mode, agent should come last
        assert!(report_status_pos < atomic_pos);
        assert!(atomic_pos < agent_pos);
    }
}