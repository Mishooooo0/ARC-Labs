//! What a ledger entry is.
//!
//! The shape comes straight from the spec, and every field earns its place:
//! *who* changed a note, *when*, *what kind* of change, *why*, and the content
//! hashes needed to prove it and to put it back.

use serde::{Deserialize, Serialize};

/// Who made a change.
///
/// `kind` is the field the entire product hangs off. It decides the colour a
/// change is drawn in — amber for human, blue for agent — which is constraint 6:
/// authorship must be readable from a screenshot without reading a word.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Actor {
    pub kind: ActorKind,
    /// The person, or the agent's name. From config, falling back to `$USER`.
    pub id: String,
    /// The model, for an agent. Absent for a human.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The run this belongs to, so a whole agent session can be reviewed or
    /// undone as a unit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActorKind {
    Human,
    Agent,
}

impl Actor {
    pub fn human(id: impl Into<String>) -> Actor {
        Actor { kind: ActorKind::Human, id: id.into(), model: None, session: None }
    }

    pub fn agent(id: impl Into<String>, model: impl Into<String>, session: impl Into<String>) -> Actor {
        Actor {
            kind: ActorKind::Agent,
            id: id.into(),
            model: Some(model.into()),
            session: Some(session.into()),
        }
    }

    pub fn is_agent(&self) -> bool {
        self.kind == ActorKind::Agent
    }
}

/// What happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Op {
    Create,
    Edit,
    Rename,
    Delete,
    /// An agent suggested a change. **The file is not touched.**
    Propose,
    /// A proposal was applied. This is the entry that changes the file.
    Accept,
    /// A proposal was discarded. The file was never touched, and this records
    /// that it was considered — a rejected suggestion is history too.
    Reject,
    /// Vault bytes left this machine. Phase 5 writes these for a remote model;
    /// the server shell writes them when serving a non-loopback client.
    Egress,
}

impl Op {
    /// Whether this operation changes the file on disk.
    ///
    /// `propose` and `reject` never do. That is the whole of constraint 4, and
    /// having it as a function means no code path has to remember it.
    pub fn touches_file(self) -> bool {
        matches!(self, Op::Create | Op::Edit | Op::Rename | Op::Delete | Op::Accept)
    }
}

/// One line of the ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    /// RFC 3339, UTC.
    pub ts: String,
    pub actor: Actor,
    pub op: Op,
    /// Why, in the actor's words. "manual edit" for a person; for an agent, the
    /// prompt node or tool that produced it.
    pub reason: String,

    /// Content hash before. Absent for `create`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    /// Content hash after. Absent for `delete` and `reject`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,

    /// A unified diff, for display only.
    ///
    /// **Restore never uses this.** Restoring replays content from the object
    /// store by hash, so it is exact by construction rather than dependent on a
    /// patch applying cleanly to whatever is on disk now. A diff that is only
    /// ever shown to a human cannot corrupt anything.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,

    /// The path before a rename.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_path: Option<String>,

    /// Where bytes went, for an `egress` entry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
}

impl Entry {
    pub fn new(actor: Actor, op: Op, reason: impl Into<String>) -> Entry {
        Entry {
            ts: now_rfc3339(),
            actor,
            op,
            reason: reason.into(),
            before: None,
            after: None,
            patch: None,
            from_path: None,
            destination: None,
            bytes: None,
        }
    }

    pub fn with_hashes(mut self, before: Option<String>, after: Option<String>) -> Entry {
        self.before = before;
        self.after = after;
        self
    }

    pub fn with_patch(mut self, patch: Option<String>) -> Entry {
        self.patch = patch;
        self
    }
}

/// RFC 3339 in UTC, to the second.
///
/// Hand-rolled from `SystemTime` rather than pulling in a date-time crate. The
/// need is one format, in one timezone, with no parsing, no locales and no
/// arithmetic — `chrono` would be a large dependency and a supply-chain surface
/// for thirty lines of civil-calendar conversion.
pub fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    format_rfc3339(secs)
}

pub fn format_rfc3339(unix_secs: i64) -> String {
    let days = unix_secs.div_euclid(86_400);
    let rem = unix_secs.rem_euclid(86_400);
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

/// Howard Hinnant's `civil_from_days`: days since the Unix epoch to a calendar
/// date. Handles leap years and the 400-year Gregorian cycle correctly, which is
/// the part that is easy to get subtly wrong by hand.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_round_trip_through_json() {
        let e = Entry::new(
            Actor::agent("weave", "qwen3.5:0.8b", "run-7"),
            Op::Propose,
            "summarise the note",
        )
        .with_hashes(Some("blake3:aa".into()), Some("blake3:bb".into()));

        let line = serde_json::to_string(&e).unwrap();
        assert_eq!(serde_json::from_str::<Entry>(&line).unwrap(), e);

        // The wire form matches the shape the spec writes out.
        assert!(line.contains("\"kind\":\"agent\""));
        assert!(line.contains("\"op\":\"propose\""));
        assert!(line.contains("\"model\":\"qwen3.5:0.8b\""));
    }

    #[test]
    fn a_human_entry_carries_no_model_or_session() {
        let line = serde_json::to_string(&Entry::new(
            Actor::human("mishal"),
            Op::Edit,
            "manual edit",
        ))
        .unwrap();
        assert!(!line.contains("model"));
        assert!(!line.contains("session"));
    }

    #[test]
    fn propose_and_reject_never_touch_the_file() {
        // Constraint 4, as a function rather than a rule every call site has to
        // remember.
        assert!(!Op::Propose.touches_file());
        assert!(!Op::Reject.touches_file());
        assert!(!Op::Egress.touches_file());
        assert!(Op::Accept.touches_file());
        assert!(Op::Edit.touches_file());
    }

    #[test]
    fn timestamps_are_correct_rfc3339() {
        assert_eq!(format_rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_rfc3339(1_000_000_000), "2001-09-09T01:46:40Z");
        // A leap day, which is where hand-rolled calendars usually break.
        assert_eq!(format_rfc3339(1_709_164_800), "2024-02-29T00:00:00Z");
        // End of a century that is not a leap year.
        assert_eq!(format_rfc3339(4_107_542_400), "2100-03-01T00:00:00Z");
        // The date this was written.
        assert_eq!(format_rfc3339(1_788_307_200), "2026-09-02T00:00:00Z");
    }

    #[test]
    fn now_is_sane_and_sorts_lexicographically() {
        let a = now_rfc3339();
        assert_eq!(a.len(), 20);
        assert!(a.ends_with('Z'));
        // RFC 3339 UTC sorts as text, which is why the timeline can order
        // entries without parsing a single one.
        assert!(format_rfc3339(1_000) < format_rfc3339(2_000));
    }
}
