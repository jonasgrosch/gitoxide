//! Centralized command parsing for upload-pack protocol
//!
//! This module consolidates the parsing logic for want, have, done, shallow,
//! and deepen commands that was previously duplicated between v1 and v2 protocols.

use crate::{
    error::{Error, Result},
    types::*,
};

use gix::Repository;
use gix_pack::Find;

/// Centralized command parser for protocol commands
pub struct CommandParser<'a> {
    repository: &'a Repository,
}

impl<'a> CommandParser<'a> {
    /// Create a new command parser
    pub fn new(repository: &'a Repository) -> Self {
        Self { repository }
    }

    /// Parse a want line and add to session (centralized from v1 and v2)
    pub fn parse_want_line(&self, line: &[u8], session: &mut SessionContext) -> Result<()> {
        let line_str =
            std::str::from_utf8(line.trim_ascii()).map_err(|_| Error::custom("Invalid UTF-8 in want line"))?;

        // Extract object ID (first 40 characters after "want ")
        if line_str.len() < 40 {
            return Err(Error::custom("Want line too short"));
        }

        let oid_str = &line_str[..40];
        let oid = gix_hash::ObjectId::from_hex(oid_str.as_bytes()).map_err(|_| Error::InvalidObjectId {
            oid: oid_str.to_string(),
        })?;

        // Validate that the object exists
        if !self.repository.objects.contains(&oid) {
            return Err(Error::ObjectNotFound { oid });
        }

        session.negotiation.wants.insert(oid);

        Ok(())
    }

    /// Parse a have line and process it (centralized from v1 and v2)
    pub fn parse_have_line(&self, line: &[u8], session: &mut SessionContext) -> Result<bool> {
        let line_str =
            std::str::from_utf8(line.trim_ascii()).map_err(|_| Error::custom("Invalid UTF-8 in have line"))?;

        let oid = gix_hash::ObjectId::from_hex(line_str.as_bytes()).map_err(|_| Error::InvalidObjectId {
            oid: line_str.to_string(),
        })?;

        // Check if we have this object and add to appropriate set
        if self.repository.objects.contains(&oid) {
            session.negotiation.common.insert(oid);
            Ok(true) // Common object found
        } else {
            session.negotiation.haves.insert(oid);
            Ok(false) // Not a common object
        }
    }

    /// Parse a done line (centralized from v1 and v2)
    pub fn parse_done_line(&self, session: &mut SessionContext) -> Result<()> {
        session.negotiation.done = true;
        Ok(())
    }

    /// Parse a shallow line (centralized from v1 and v2)
    pub fn parse_shallow_line(&self, line: &[u8], session: &mut SessionContext) -> Result<()> {
        let line_str =
            std::str::from_utf8(line.trim_ascii()).map_err(|_| Error::custom("Invalid UTF-8 in shallow line"))?;

        let oid = gix_hash::ObjectId::from_hex(line_str.as_bytes()).map_err(|_| Error::InvalidObjectId {
            oid: line_str.to_string(),
        })?;

        session.negotiation.shallow.insert(oid);
        Ok(())
    }

    /// Parse a deepen line (centralized from v1 and v2)
    pub fn parse_deepen_line(&self, line: &[u8], session: &mut SessionContext) -> Result<()> {
        let line_str =
            std::str::from_utf8(line.trim_ascii()).map_err(|_| Error::custom("Invalid UTF-8 in deepen line"))?;

        let depth: u32 = line_str.parse().map_err(|_| Error::custom("Invalid depth value"))?;

        session.negotiation.deepen = Some(DeepenSpec::Depth(depth));
        Ok(())
    }

    /// Parse a deepen-since line (centralized from v1 and v2)
    pub fn parse_deepen_since_line(&self, line: &[u8], session: &mut SessionContext) -> Result<()> {
        let line_str =
            std::str::from_utf8(line.trim_ascii()).map_err(|_| Error::custom("Invalid UTF-8 in deepen-since line"))?;

        let timestamp: i64 = line_str
            .parse()
            .map_err(|_| Error::custom("Invalid timestamp in deepen-since"))?;

        let time = gix_date::Time::new(timestamp, 0);
        session.negotiation.deepen = Some(DeepenSpec::Since(time));
        Ok(())
    }

    /// Parse a deepen-not line (centralized from v1 and v2)
    pub fn parse_deepen_not_line(&self, line: &[u8], session: &mut SessionContext) -> Result<()> {
        let ref_str =
            std::str::from_utf8(line.trim_ascii()).map_err(|_| Error::custom("Invalid UTF-8 in deepen-not line"))?;

        if let Some(DeepenSpec::Not(ref mut refs)) = session.negotiation.deepen {
            refs.push(ref_str.into());
        } else {
            session.negotiation.deepen = Some(DeepenSpec::Not(vec![ref_str.into()]));
        }
        Ok(())
    }
}
