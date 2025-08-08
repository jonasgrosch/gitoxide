// M2: Command parsing (blocking-first).
//
// This module parses receive-pack head-info command lines of the form
//   "<old-oid> <new-oid> <refname>[\\0<capabilities>]"
//
// It also accepts additional lines that may appear in head-info like
//   "push-option=<value>"
//   "shallow <oid>"
//
// Capability tokens (after NUL on the first command line) are parsed into Options,
// while 'push-option=' and 'shallow ' lines are added to it as they occur.
//
// Wire IO integration (pkt-line iteration) can be added later; this file focuses on
// robust, typed parsing independent of IO.

use crate::protocol::options::Options;
use crate::Error;
use gix_hash::ObjectId;

/// A single update command as sent by the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandUpdate {
    /// Create a new reference with `new` object.
    Create { new: ObjectId, name: String },
    /// Update an existing reference from `old` to `new`.
    Update { old: ObjectId, new: ObjectId, name: String },
    /// Delete an existing reference which had `old` object.
    Delete { old: ObjectId, name: String },
}

impl CommandUpdate {
    /// The refname targeted by this command.
    pub fn name(&self) -> &str {
        match self {
            CommandUpdate::Create { name, .. } => name,
            CommandUpdate::Update { name, .. } => name,
            CommandUpdate::Delete { name, .. } => name,
        }
    }
}

/// A list of parsed update commands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandList {
    commands: Vec<CommandUpdate>,
}

impl CommandList {
    pub fn new() -> Self {
        Self { commands: Vec::new() }
    }

    pub fn push(&mut self, cmd: CommandUpdate) {
        self.commands.push(cmd);
    }

    pub fn iter(&self) -> impl Iterator<Item = &CommandUpdate> {
        self.commands.iter()
    }

    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Parse head-info from text, one logical line per `\n`.
    ///
    /// - Command lines: "<old> <new> <ref>[\\0caps]"
    /// - Additional recognized lines:
    ///   - "push-option=<value>" → recorded in Options::push_options
    ///   - "shallow <oid>" → recorded in Options::shallow
    ///
    /// Returns the list of commands and the parsed Options.
    ///
    /// Notes
    /// - Object-format enforcement is minimal: we accept both 40 (SHA-1) and 64 (SHA-256) hex lengths via ObjectId::from_hex().
    /// - Invariants enforced:
    ///   - Create: old is zero, new is non-zero
    ///   - Delete: new is zero, old is non-zero
    ///   - Update: old and new are non-zero
    ///   - Both zero → invalid
    pub fn parse_from_text(text: &str) -> Result<(Self, Options), Error> {
        let mut list = CommandList::new();
        let mut opts = Options::default();
        let mut caps_seen = false;

        for raw_line in text.lines() {
            let line = raw_line.trim_end_matches('\r');
            if line.is_empty() {
                continue;
            }

            // push-option support
            if let Some(value) = line.strip_prefix("push-option=") {
                opts.add_push_option(value.to_string());
                continue;
            }

            // shallow support
            if let Some(rest) = line.strip_prefix("shallow ") {
                let oid = parse_oid(rest)
                    .map_err(|e| Error::Protocol(format!("invalid shallow oid '{}': {}", rest, e)))?;
                opts.add_shallow_oid(oid);
                continue;
            }

            // unshallow support
            if let Some(rest) = line.strip_prefix("unshallow ") {
                let oid = parse_oid(rest)
                    .map_err(|e| Error::Protocol(format!("invalid unshallow oid '{}': {}", rest, e)))?;
                opts.add_unshallow_oid(oid);
                continue;
            }

            // Command line possibly with capabilities after NUL
            let (cmd_part, caps_part) = split_once_nul(line);

            if !caps_seen {
                if let Some(caps) = caps_part {
                    // Only the first command line must carry capabilities; if we see it later, we still accept but override.
                    let parsed = Options::parse(caps);
                    // only assign negotiated tokens; keep possibly gathered push-options/shallow
                    opts.negotiated = parsed.negotiated;
                    caps_seen = true;
                }
            }

            let cmd = parse_command_before_nul(cmd_part)?;
            list.push(cmd);
        }

        Ok((list, opts))
    }
}

/// Parse a command line (before any NUL) into a CommandUpdate.
fn parse_command_before_nul(cmd_part: &str) -> Result<CommandUpdate, Error> {
    // Expect three parts: <old> <new> <refname>
    let mut it = cmd_part.split_whitespace();
    let old_hex = it
        .next()
        .ok_or_else(|| Error::Protocol("missing <old> oid".into()))?;
    let new_hex = it
        .next()
        .ok_or_else(|| Error::Protocol("missing <new> oid".into()))?;
    let name = it
        .next()
        .ok_or_else(|| Error::Protocol("missing <refname>".into()))?;

    // Extra tokens would be invalid; refnames can't contain spaces.
    if it.next().is_some() {
        return Err(Error::Protocol("unexpected tokens after <refname>".into()));
    }

    let old_is_zero = is_all_zeros(old_hex);
    let new_is_zero = is_all_zeros(new_hex);

    let old_oid = if !old_is_zero {
        Some(parse_oid(old_hex).map_err(|e| Error::Protocol(format!("invalid old oid '{}': {}", old_hex, e)))?)
    } else {
        None
    };

    let new_oid = if !new_is_zero {
        Some(parse_oid(new_hex).map_err(|e| Error::Protocol(format!("invalid new oid '{}': {}", new_hex, e)))?)
    } else {
        None
    };

    if old_is_zero && new_is_zero {
        return Err(Error::Validation("both old and new are zero (invalid command)".into()));
    }

    if old_is_zero {
        // Create
        let new = new_oid.expect("non-zero new for create");
        return Ok(CommandUpdate::Create {
            new,
            name: name.to_owned(),
        });
    }

    if new_is_zero {
        // Delete
        let old = old_oid.expect("non-zero old for delete");
        return Ok(CommandUpdate::Delete {
            old,
            name: name.to_owned(),
        });
    }

    // Update
    Ok(CommandUpdate::Update {
        old: old_oid.expect("present"),
        new: new_oid.expect("present"),
        name: name.to_owned(),
    })
}

/// Try to decode a hex string into an ObjectId using gix-hash utilities.
fn parse_oid(hex: &str) -> Result<ObjectId, String> {
    ObjectId::from_hex(hex.as_bytes()).map_err(|e| e.to_string())
}

/// Split once at the first NUL byte; return (before, Option<after>).
fn split_once_nul(s: &str) -> (&str, Option<&str>) {
    match s.find('\0') {
        Some(pos) => (&s[..pos], Some(&s[pos + 1..])),
        None => (s, None),
    }
}

/// Return true if all chars are ASCII '0'.
fn is_all_zeros(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b == b'0')
}

#[cfg(test)]
mod tests {
    use super::*;
    use gix_hash::ObjectId;

    fn oid(hex40: &str) -> ObjectId {
        ObjectId::from_hex(hex40.as_bytes()).expect("valid hex")
    }

    #[test]
    fn create_update_delete_parsing_and_caps() {
        let text = concat!(
            // first command carries capabilities after NUL
            "0000000000000000000000000000000000000000 1111111111111111111111111111111111111111 refs/heads/main\0report-status report-status-v2 quiet delete-refs ofs-delta agent=gix/1.0\n",
            // subsequent command lines
            "1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 refs/heads/main\n",
            "2222222222222222222222222222222222222222 0000000000000000000000000000000000000000 refs/tags/v1\n",
            // additional recognized lines
            "push-option=notify=team\n",
            "shallow 3333333333333333333333333333333333333333\n",
        );

        let (list, opts) = CommandList::parse_from_text(text).unwrap();
        assert_eq!(list.len(), 3);

        // Create
        match &list.commands[0] {
            CommandUpdate::Create { new, name } => {
                assert_eq!(*new, oid("1111111111111111111111111111111111111111"));
                assert_eq!(name, "refs/heads/main");
            }
            _ => panic!("expected Create"),
        }

        // Update
        match &list.commands[1] {
            CommandUpdate::Update { old, new, name } => {
                assert_eq!(*old, oid("1111111111111111111111111111111111111111"));
                assert_eq!(*new, oid("2222222222222222222222222222222222222222"));
                assert_eq!(name, "refs/heads/main");
            }
            _ => panic!("expected Update"),
        }

        // Delete
        match &list.commands[2] {
            CommandUpdate::Delete { old, name } => {
                assert_eq!(*old, oid("2222222222222222222222222222222222222222"));
                assert_eq!(name, "refs/tags/v1");
            }
            _ => panic!("expected Delete"),
        }

        // Options negotiated on first line
        assert!(opts.has("report-status"));
        assert!(opts.has("report-status-v2"));
        assert!(opts.has("quiet"));
        assert!(opts.has("delete-refs"));
        assert!(opts.has("ofs-delta"));
        assert!(opts.has("agent"));
        // additional recognized lines recorded
        assert_eq!(opts.push_options, vec!["notify=team"]);
        assert_eq!(opts.shallow.len(), 1);
        assert_eq!(opts.shallow[0], oid("3333333333333333333333333333333333333333"));
    }

    #[test]
    fn invalid_both_zero_is_validation_error() {
        let text = "0000000000000000000000000000000000000000 0000000000000000000000000000000000000000 refs/heads/main\n";
        let err = CommandList::parse_from_text(text).unwrap_err();
        match err {
            Error::Validation(_) => {}
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn invalid_oid_is_protocol_error() {
        let text = "zzzz000000000000000000000000000000000000 1111111111111111111111111111111111111111 refs/heads/main\n";
        let err = CommandList::parse_from_text(text).unwrap_err();
        match err {
            Error::Protocol(_) => {}
            other => panic!("expected Protocol, got {other:?}"),
        }
    }

    #[test]
    fn extra_tokens_after_refname_is_protocol_error() {
        let text = "0000000000000000000000000000000000000000 1111111111111111111111111111111111111111 refs/heads/main extra-token\n";
        let err = CommandList::parse_from_text(text).unwrap_err();
        match err {
            Error::Protocol(_) => {}
            other => panic!("expected Protocol, got {other:?}"),
        }
    }

    #[test]
    fn shallow_invalid_oid_is_protocol_error() {
        let text = concat!(
            "0000000000000000000000000000000000000000 1111111111111111111111111111111111111111 refs/heads/main\n",
            "shallow zzzz000000000000000000000000000000000000\n",
        );
        let err = CommandList::parse_from_text(text).unwrap_err();
        match err {
            Error::Protocol(_) => {}
            other => panic!("expected Protocol, got {other:?}"),
        }
    }

    #[test]
    fn unshallow_parsing_and_validation() {
        let text = concat!(
            "0000000000000000000000000000000000000000 1111111111111111111111111111111111111111 refs/heads/main\n",
            "unshallow 4444444444444444444444444444444444444444\n",
        );
        let (_list, opts) = CommandList::parse_from_text(text).unwrap();
        assert_eq!(opts.unshallow.len(), 1);
        assert_eq!(opts.unshallow[0], oid("4444444444444444444444444444444444444444"));
    }
}