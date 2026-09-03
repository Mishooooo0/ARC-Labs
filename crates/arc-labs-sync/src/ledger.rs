//! Merging two copies of a note's history.
//!
//! This is the part of sync that cannot go wrong, and it is worth saying why.
//!
//! A ledger file is append-only JSONL: one entry per line, never rewritten,
//! never reordered in place. Two machines that each appended to their own copy
//! did not overwrite anything — they wrote *different lines*. So the merge is a
//! set union, and the result contains everything both sides know. There is no
//! case where one history wins and another is lost, because there is no case
//! where they disagree about a line they both hold.
//!
//! Compare that with the note itself, where two edits to the same paragraph
//! genuinely conflict and a person has to choose. The ledger is immune by
//! construction, which is exactly why history is the thing that survives a
//! two-machine setup intact.
//!
//! ## Dedup is by the whole line
//!
//! Not by timestamp, and not by any invented id. Entries have no id, and the
//! same second can legitimately hold several — `record_change` stamps to the
//! second and a fast sequence of edits shares one. The line itself is the
//! identity: serialisation is deterministic, so the same entry produced twice
//! is byte-identical, and two entries that differ in any field are different
//! lines and both survive. Guessing that two same-second entries "must" be the
//! same would silently drop real history.

use std::collections::BTreeSet;

/// Merge two ledger files, given their raw text.
///
/// Returns the merged text, ordered by timestamp, with duplicates collapsed.
/// Blank lines are dropped; a line that is not valid JSON is **kept** — see
/// below.
pub fn merge(ours: &str, theirs: &str) -> String {
    let mut lines: BTreeSet<Ordered> = BTreeSet::new();

    for raw in ours.lines().chain(theirs.lines()) {
        let line = raw.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        lines.insert(Ordered {
            ts: timestamp_of(line).unwrap_or_default(),
            line: line.to_string(),
        });
    }

    lines.into_iter().map(|o| o.line + "\n").collect()
}

/// A line, ordered by its timestamp and then by itself.
///
/// The second key is what makes the merge deterministic: entries sharing a
/// second must still come out in the same order on both machines, or the two
/// ledgers differ byte-for-byte after a merge that agreed on content, and the
/// next sync sees a change that is not one.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Ordered {
    ts: String,
    line: String,
}

/// Pull `ts` out without parsing the whole entry.
///
/// A line that will not parse keeps a timestamp of `""` and sorts first. It is
/// **not discarded**: a ledger line this build does not understand is far more
/// likely to be from a newer build than to be corrupt, and a merge that quietly
/// dropped what it could not read would delete history during an upgrade — the
/// one moment it is least likely to be noticed.
fn timestamp_of(line: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    value.get("ts")?.as_str().map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(ts: &str, reason: &str) -> String {
        format!(r#"{{"ts":"{ts}","op":"edit","reason":"{reason}"}}"#)
    }

    #[test]
    fn a_merge_keeps_everything_both_sides_know() {
        let ours = format!(
            "{}\n{}\n",
            entry("2026-09-01T10:00:00Z", "a"),
            entry("2026-09-03T10:00:00Z", "c")
        );
        let theirs = format!(
            "{}\n{}\n",
            entry("2026-09-02T10:00:00Z", "b"),
            entry("2026-09-04T10:00:00Z", "d")
        );

        let merged = merge(&ours, &theirs);
        let lines: Vec<&str> = merged.lines().collect();

        assert_eq!(lines.len(), 4, "no entry may be lost in a merge");
        assert!(lines[0].contains("\"a\""));
        assert!(lines[1].contains("\"b\""));
        assert!(lines[2].contains("\"c\""));
        assert!(lines[3].contains("\"d\""));
    }

    /// The common case: both machines already have most of the history.
    #[test]
    fn entries_both_sides_already_had_appear_once() {
        let shared = entry("2026-09-01T10:00:00Z", "shared");
        let ours = format!("{shared}\n{}\n", entry("2026-09-02T10:00:00Z", "mine"));
        let theirs = format!("{shared}\n{}\n", entry("2026-09-03T10:00:00Z", "yours"));

        let merged = merge(&ours, &theirs);
        assert_eq!(merged.lines().count(), 3);
        assert_eq!(merged.matches("shared").count(), 1);
    }

    /// Merging is order-independent and idempotent, or two machines end up with
    /// byte-different ledgers that hold identical history — and the next sync
    /// sees a change that is not one.
    #[test]
    fn merging_is_symmetric_and_settles() {
        let a = format!("{}\n", entry("2026-09-02T10:00:00Z", "b"));
        let b = format!("{}\n", entry("2026-09-01T10:00:00Z", "a"));

        let ab = merge(&a, &b);
        let ba = merge(&b, &a);
        assert_eq!(ab, ba, "the two machines must reach the same bytes");
        assert_eq!(merge(&ab, &ba), ab, "merging again changes nothing");
        assert_eq!(merge(&ab, ""), ab, "merging with nothing changes nothing");
    }

    /// Two entries in the same second are two entries. The clock is not an id.
    #[test]
    fn two_entries_sharing_a_second_both_survive() {
        let ts = "2026-09-01T10:00:00Z";
        let ours = format!("{}\n", entry(ts, "first"));
        let theirs = format!("{}\n", entry(ts, "second"));

        let merged = merge(&ours, &theirs);
        assert_eq!(
            merged.lines().count(),
            2,
            "a shared second is not a duplicate"
        );
    }

    /// An upgrade must not eat history it cannot parse.
    #[test]
    fn a_line_this_build_cannot_read_is_kept_not_dropped() {
        let ours = format!("{}\n", entry("2026-09-01T10:00:00Z", "known"));
        let theirs = "{\"from\":\"a newer build\",\"shape\":[1,2,3]}\nnot json at all\n";

        let merged = merge(&ours, theirs);
        assert_eq!(merged.lines().count(), 3);
        assert!(merged.contains("a newer build"));
        assert!(merged.contains("not json at all"));
    }

    #[test]
    fn blank_lines_and_trailing_newlines_do_not_accumulate() {
        let ours = format!("\n{}\n\n\n", entry("2026-09-01T10:00:00Z", "a"));
        let merged = merge(&ours, "\n\n");

        assert_eq!(merged, format!("{}\n", entry("2026-09-01T10:00:00Z", "a")));
    }

    /// A CRLF copy and an LF copy hold the same history and must merge to one.
    #[test]
    fn line_endings_do_not_make_an_entry_look_new() {
        let e = entry("2026-09-01T10:00:00Z", "a");
        let merged = merge(&format!("{e}\r\n"), &format!("{e}\n"));
        assert_eq!(merged.lines().count(), 1);
    }

    #[test]
    fn merging_two_empty_ledgers_is_empty() {
        assert_eq!(merge("", ""), "");
    }
}
