//! One sync pass, start to finish.
//!
//! Reads both sides, asks [`plan`](crate::plan) what to do, does it, moves the
//! history, and only then records what was agreed. The order matters and is the
//! whole of the design:
//!
//! 1. **Objects before files.** Content is addressed by hash and referred to by
//!    the ledger. Sending a ledger entry whose content has not arrived leaves
//!    the other side holding a history it cannot act on — every restore fails
//!    with "content is not in the object store". Objects first means the worst
//!    case is content nothing refers to yet, which is harmless and is cleaned up
//!    by nothing at all.
//! 2. **Files, one at a time**, each one reported.
//! 3. **Ledgers merged**, in both directions, by key.
//! 4. **The base written last**, and only if nothing failed. It is a claim that
//!    both sides really do hold this state; writing it after a partial pass
//!    would record an agreement that never happened, and the next pass would
//!    take that fiction as ground truth and skip the files that never arrived.
//!
//! Conflicts stop nothing. They are collected and returned, the rest of the
//! pass completes, and a person settles them afterwards — a sync that refused
//! to move forty untouched files because one needed a decision would be a sync
//! nobody runs.

use std::path::Path;

use crate::client::Hub;
use crate::plan::Action;
use crate::{base, manifest, Result};

/// What a pass did.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReport {
    pub pushed: usize,
    pub pulled: usize,
    pub deleted_here: usize,
    pub deleted_there: usize,
    pub objects_sent: usize,
    pub objects_received: usize,
    pub history_merged: usize,
    /// Left for a person. Nothing about them has been changed on either side.
    pub conflicts: Vec<ConflictReport>,
    /// Non-fatal failures, named. A pass that hit one of these did not write a
    /// base, so the next pass will try again.
    pub problems: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictReport {
    pub path: String,
    /// `both-edited`, `both-created`, `local-deleted-remote-edited`,
    /// `local-edited-remote-deleted`.
    pub kind: String,
}

impl SyncReport {
    pub fn moved_anything(&self) -> bool {
        self.pushed + self.pulled + self.deleted_here + self.deleted_there > 0
    }
    /// A pass is only clean if it can honestly record what both sides hold.
    pub fn clean(&self) -> bool {
        self.problems.is_empty()
    }
}

/// What a pass needs the vault to do for it.
///
/// A trait rather than direct filesystem calls so a pass goes through the same
/// ledgered, indexed write path the editor uses. Applying an incoming change by
/// writing bytes behind the API's back would leave the search index stale and
/// the open windows unaware — the class of bug that comes from a feature having
/// its own private door into the vault.
pub trait Local {
    fn root(&self) -> &Path;
    fn read(&self, path: &str) -> Result<Vec<u8>>;
    fn write(&self, path: &str, bytes: &[u8]) -> Result<()>;
    fn delete(&self, path: &str) -> Result<()>;
    fn ledger_keys(&self) -> Result<Vec<String>>;
    fn read_ledger(&self, key: &str) -> Result<String>;
    fn merge_ledger(&self, key: &str, jsonl: &str) -> Result<usize>;
    fn read_object(&self, hash: &str) -> Result<String>;
    fn write_object(&self, content: &str) -> Result<()>;
}

/// Work out what a pass would do, without doing any of it.
///
/// The preview behind "what would this change?", and it is genuinely read-only:
/// it takes both manifests and returns the plan. Nothing here writes.
pub fn preview(local: &dyn Local, hub: &Hub) -> Result<Vec<Action>> {
    let here = manifest::of(local.root())?;
    let there = hub.manifest()?;
    let agreed = base::load(local.root(), hub_key(hub));
    Ok(crate::plan(&agreed, &here, &there.files))
}

/// Run a full pass.
pub fn run(local: &dyn Local, hub: &Hub) -> Result<SyncReport> {
    let root = local.root().to_path_buf();
    let here = manifest::of(&root)?;
    let there = hub.manifest()?;
    let agreed = base::load(&root, hub_key(hub));

    let actions = crate::plan(&agreed, &here, &there.files);
    let mut report = SyncReport::default();

    // History first — see the module note. Objects travel inside this step,
    // before the entries that name them.
    match sync_history(local, hub, &mut report) {
        Ok(()) => {}
        Err(e) => report.problems.push(format!("history: {e}")),
    }

    for action in &actions {
        let outcome = apply(local, hub, &there.generation, action, &mut report);
        if let Err(e) = outcome {
            report.problems.push(format!("{}: {e}", action.path()));
        }
    }

    // Only now, and only if everything landed. A base written after a partial
    // pass is a lie the next pass believes.
    if report.clean() {
        let settled = manifest::of(&root)?;
        base::save(&root, hub_key(hub), &settled)?;
    }
    Ok(report)
}

fn apply(
    local: &dyn Local,
    hub: &Hub,
    generation: &str,
    action: &Action,
    report: &mut SyncReport,
) -> Result<()> {
    match action {
        Action::Push(path) => {
            hub.write(path, &local.read(path)?, generation)?;
            report.pushed += 1;
        }
        Action::Pull(path) => {
            local.write(path, &hub.read(path)?)?;
            report.pulled += 1;
        }
        Action::PushDelete(path) => {
            hub.delete(path, generation)?;
            report.deleted_there += 1;
        }
        Action::PullDelete(path) => {
            local.delete(path)?;
            report.deleted_here += 1;
        }
        Action::Conflict { path, kind } => {
            // Recorded and left alone. Neither copy is touched.
            report.conflicts.push(ConflictReport {
                path: path.clone(),
                kind: kind.name().into(),
            });
        }
    }
    Ok(())
}

/// Move history both ways, and the content it refers to with it.
fn sync_history(local: &dyn Local, hub: &Hub, report: &mut SyncReport) -> Result<()> {
    let mut keys = local.ledger_keys()?;
    keys.extend(hub.ledger_keys()?);
    keys.sort();
    keys.dedup();

    for key in keys {
        let ours = local.read_ledger(&key)?;
        let theirs = hub.read_ledger(&key)?;
        if ours == theirs {
            continue;
        }

        // Objects before entries, in both directions. An entry whose content is
        // missing is a history the other side cannot act on.
        send_objects(local, hub, &ours, report)?;
        fetch_objects(local, hub, &theirs, report)?;

        report.history_merged += local.merge_ledger(&key, &theirs)?;
        hub.merge_ledger(&key, &ours)?;
    }
    Ok(())
}

fn send_objects(local: &dyn Local, hub: &Hub, jsonl: &str, report: &mut SyncReport) -> Result<()> {
    let hashes = hashes_in(jsonl);
    if hashes.is_empty() {
        return Ok(());
    }
    for hash in hub.missing_objects(&hashes)? {
        // A hash the ledger names but this side does not hold is not a failure
        // to stop for: an older vault may reference content from before the
        // object store existed. Skip it and keep the history.
        if let Ok(content) = local.read_object(&hash) {
            hub.write_object(&content)?;
            report.objects_sent += 1;
        }
    }
    Ok(())
}

fn fetch_objects(local: &dyn Local, hub: &Hub, jsonl: &str, report: &mut SyncReport) -> Result<()> {
    for hash in hashes_in(jsonl) {
        if local.read_object(&hash).is_ok() {
            continue;
        }
        if let Ok(content) = hub.read_object(&hash) {
            local.write_object(&content)?;
            report.objects_received += 1;
        }
    }
    Ok(())
}

/// Every content hash a stretch of history refers to.
///
/// Read out of the raw JSON rather than by deserialising `Entry`, so a ledger
/// line written by a newer build still yields its objects. Losing content
/// because this build did not recognise the entry that named it would be the
/// worst kind of upgrade bug: silent, and only visible much later when a
/// restore fails.
pub fn hashes_in(jsonl: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in jsonl.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        for field in ["before", "after"] {
            if let Some(h) = value.get(field).and_then(|v| v.as_str()) {
                out.push(h.to_string());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// The key a base manifest is filed under: the hub's own URL.
fn hub_key(hub: &Hub) -> &str {
    hub.base()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_hash_a_history_names_is_found() {
        let jsonl = concat!(
            r#"{"ts":"2026-01-01T00:00:00Z","op":"create","after":"blake3:aaa"}"#,
            "\n",
            r#"{"ts":"2026-01-02T00:00:00Z","op":"edit","before":"blake3:aaa","after":"blake3:bbb"}"#,
            "\n",
        );
        assert_eq!(hashes_in(jsonl), vec!["blake3:aaa", "blake3:bbb"]);
    }

    /// The upgrade case. A line this build cannot fully understand must still
    /// give up the content it names, or a restore fails later with no clue why.
    #[test]
    fn an_unfamiliar_entry_still_yields_its_objects() {
        let jsonl = r#"{"ts":"2030-01-01T00:00:00Z","op":"something-new","after":"blake3:ccc","extra":{"a":1}}"#;
        assert_eq!(hashes_in(jsonl), vec!["blake3:ccc"]);
    }

    #[test]
    fn a_history_with_no_content_names_no_objects() {
        let jsonl = r#"{"ts":"2026-01-01T00:00:00Z","op":"delete"}"#;
        assert!(hashes_in(jsonl).is_empty());
    }

    #[test]
    fn junk_lines_do_not_break_the_scan() {
        assert!(hashes_in("not json\n\n").is_empty());
    }
}
