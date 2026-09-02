//! The ledger: who changed what, when, why — and enough to put it back.
//!
//! This is the phase that makes ARC-LABS not a prettier Obsidian, so it is worth
//! being precise about what it guarantees.
//!
//! # Append-only, one file per note
//!
//! `.arc/ledger/<blake3-of-relpath>.jsonl`, one JSON object per line. Appending
//! is the only write. Nothing is ever rewritten in place, so a crash can lose at
//! most the entry being written, never the history before it — and a partly
//! written last line is detected and skipped on read rather than corrupting the
//! file.
//!
//! One file per note, rather than one for the vault: a 5,000-note vault would
//! otherwise be one enormous file that every read has to scan, and every
//! concurrent write has to contend for.
//!
//! # Restore is exact, and does not use the diff
//!
//! Each entry names the content hash before and after. The content lives in a
//! content-addressed store, so restoring is a lookup by hash — not a patch
//! replay. See [`objects`] for why that distinction is the whole gate.
//!
//! # A proposal never touches the file
//!
//! Constraint 4. [`Ledger::propose`] writes an entry and stores the proposed
//! content as an object; it does not go near the note. Only
//! [`Ledger::record_accept`] does, and only after a human said so.

pub mod entry;
pub mod objects;

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use arc_labs_core::VaultPath;

pub use entry::{format_rfc3339, now_rfc3339, Actor, ActorKind, Entry, Op};
pub use objects::{hash_of, ObjectStore};

#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("not a valid content hash: {0}")]
    BadHash(String),
    #[error("content {0} is not in the object store")]
    MissingObject(String),
    #[error("content {0} does not match its own hash")]
    CorruptObject(String),
    #[error("no entry at index {index}; this note has {len}")]
    NoSuchEntry { index: usize, len: usize },
    #[error("entry {index} has no recorded content to restore")]
    NothingToRestore { index: usize },
    #[error(transparent)]
    Core(#[from] Box<arc_labs_core::Error>),
}

impl LedgerError {
    fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        LedgerError::Io { path: path.into(), source }
    }

    /// A message safe to send to a caller that may be remote. Same rule as the
    /// core: never name an absolute host path.
    pub fn public(&self) -> String {
        match self {
            LedgerError::Io { source, .. } => format!("ledger io error: {}", source.kind()),
            LedgerError::BadHash(_) => "not a valid content hash".into(),
            LedgerError::MissingObject(_) => "that version is no longer stored".into(),
            LedgerError::CorruptObject(_) => "that version failed its integrity check".into(),
            LedgerError::NoSuchEntry { index, len } => {
                format!("no entry {index}; this note has {len}")
            }
            LedgerError::NothingToRestore { index } => {
                format!("entry {index} has no content to restore")
            }
            LedgerError::Core(e) => e.public(),
        }
    }
}

pub type Result<T> = std::result::Result<T, LedgerError>;

/// The ledger for one vault.
pub struct Ledger {
    dir: PathBuf,
    objects: ObjectStore,
}

impl Ledger {
    /// Open (creating if needed) the ledger under a vault's `.arc` directory.
    pub fn open(vault_root: &Path) -> Result<Ledger> {
        let arc = vault_root.join(".arc");
        let dir = arc.join("ledger");
        std::fs::create_dir_all(&dir).map_err(|e| LedgerError::io(&dir, e))?;
        Ok(Ledger { dir, objects: ObjectStore::new(&arc) })
    }

    pub fn objects(&self) -> &ObjectStore {
        &self.objects
    }

    /// The ledger file for a note.
    ///
    /// Keyed on a hash of the relative path rather than the path itself: vault
    /// paths contain slashes, spaces, unicode and characters that are legal in a
    /// note name and illegal in a filename. A hash is always a valid filename on
    /// every platform, and it is the same length for every note.
    fn file_for(&self, path: &VaultPath) -> PathBuf {
        let key = blake3::hash(path.as_str().as_bytes()).to_hex();
        self.dir.join(format!("{key}.jsonl"))
    }

    /// Every entry for a note, oldest first.
    ///
    /// A truncated final line — the signature of a crash mid-append — is skipped
    /// rather than treated as corruption. Losing the entry that was being
    /// written is the expected worst case; losing the file is not.
    pub fn read(&self, path: &VaultPath) -> Result<Vec<Entry>> {
        let file = self.file_for(path);
        let handle = match std::fs::File::open(&file) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(LedgerError::io(&file, e)),
        };

        let mut out = Vec::new();
        for (n, line) in std::io::BufReader::new(handle).lines().enumerate() {
            let line = line.map_err(|e| LedgerError::io(&file, e))?;
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Entry>(&line) {
                Ok(e) => out.push(e),
                Err(e) => {
                    tracing::warn!(line = n + 1, error = %e, "skipping unreadable ledger entry");
                }
            }
        }
        Ok(out)
    }

    /// Append one entry. The only write this type performs.
    pub fn append(&self, path: &VaultPath, entry: &Entry) -> Result<()> {
        let file = self.file_for(path);
        let mut line = serde_json::to_string(entry).expect("an Entry always serialises");
        line.push('\n');

        let mut handle = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file)
            .map_err(|e| LedgerError::io(&file, e))?;
        handle.write_all(line.as_bytes()).map_err(|e| LedgerError::io(&file, e))?;
        // Durable before the caller is told it happened. The ledger is the thing
        // that makes agent activity auditable; an entry that is "probably on
        // disk" is not an audit trail.
        handle.sync_all().map_err(|e| LedgerError::io(&file, e))?;
        Ok(())
    }

    /// Record a change that has been made to a note.
    ///
    /// Stores both versions in the object store, so any point can be restored
    /// exactly, and computes a diff for display.
    pub fn record_change(
        &self,
        path: &VaultPath,
        actor: Actor,
        op: Op,
        reason: impl Into<String>,
        before: Option<&str>,
        after: Option<&str>,
    ) -> Result<Entry> {
        let before_hash = before.map(|c| self.objects.put(c)).transpose()?;
        let after_hash = after.map(|c| self.objects.put(c)).transpose()?;

        let patch = match (before, after) {
            (Some(b), Some(a)) if b != a => Some(diff(b, a)),
            (None, Some(a)) => Some(diff("", a)),
            _ => None,
        };

        let entry = Entry::new(actor, op, reason)
            .with_hashes(before_hash, after_hash)
            .with_patch(patch);
        self.append(path, &entry)?;
        Ok(entry)
    }

    /// Record an agent's proposal. **Does not touch the note.**
    ///
    /// The proposed content goes into the object store so it can be shown,
    /// diffed and later applied — but the file on disk is untouched, and its
    /// mtime is unchanged. That is constraint 4, and it is a Phase 3 gate.
    pub fn propose(
        &self,
        path: &VaultPath,
        actor: Actor,
        reason: impl Into<String>,
        current: &str,
        proposed: &str,
    ) -> Result<Entry> {
        debug_assert!(actor.is_agent() || cfg!(test), "proposals come from agents");
        let before = self.objects.put(current)?;
        let after = self.objects.put(proposed)?;

        let entry = Entry::new(actor, Op::Propose, reason)
            .with_hashes(Some(before), Some(after))
            .with_patch(Some(diff(current, proposed)));
        self.append(path, &entry)?;
        Ok(entry)
    }

    /// Record that a proposal was applied. The caller writes the file.
    pub fn record_accept(
        &self,
        path: &VaultPath,
        actor: Actor,
        reason: impl Into<String>,
        before: &str,
        after: &str,
    ) -> Result<Entry> {
        self.record_change(path, actor, Op::Accept, reason, Some(before), Some(after))
    }

    /// Record that a proposal was discarded. Nothing is written to the note.
    ///
    /// Kept rather than forgotten: that a suggestion was considered and refused
    /// is part of the history of a note, and an audit that shows only accepted
    /// changes tells you what an agent did but not what it wanted to do.
    pub fn record_reject(
        &self,
        path: &VaultPath,
        actor: Actor,
        reason: impl Into<String>,
        proposed: &str,
    ) -> Result<Entry> {
        let after = self.objects.put(proposed)?;
        let entry = Entry::new(actor, Op::Reject, reason).with_hashes(None, Some(after));
        self.append(path, &entry)?;
        Ok(entry)
    }

    /// Record that vault bytes left this machine.
    pub fn record_egress(
        &self,
        path: &VaultPath,
        actor: Actor,
        destination: impl Into<String>,
        bytes: u64,
    ) -> Result<Entry> {
        let mut entry = Entry::new(actor, Op::Egress, "vault content sent off this machine");
        entry.destination = Some(destination.into());
        entry.bytes = Some(bytes);
        self.append(path, &entry)?;
        Ok(entry)
    }

    /// The content of a note as of entry `index`.
    ///
    /// A lookup by hash, not a patch replay. Exact, or an error — never
    /// approximately right.
    pub fn content_at(&self, path: &VaultPath, index: usize) -> Result<String> {
        let entries = self.read(path)?;
        let entry = entries
            .get(index)
            .ok_or(LedgerError::NoSuchEntry { index, len: entries.len() })?;

        // `reject` records what was refused, not a state the note was ever in;
        // restoring "to" one means restoring to the state before it.
        let hash = match entry.op {
            Op::Reject => entries[..index]
                .iter()
                .rev()
                .find_map(|e| e.after.clone())
                .ok_or(LedgerError::NothingToRestore { index })?,
            _ => entry.after.clone().ok_or(LedgerError::NothingToRestore { index })?,
        };
        self.objects.get(&hash)
    }

    /// Move a note's history to a new path, and record the move.
    ///
    /// The ledger file is keyed on the path, so a rename has to carry it — or
    /// the note's history vanishes exactly when someone tidies their folders.
    /// The old path is recorded on the entry, so the history stays continuous
    /// and readable across the rename.
    pub fn record_rename(
        &self,
        from: &VaultPath,
        to: &VaultPath,
        actor: Actor,
        content: &str,
    ) -> Result<Entry> {
        let old = self.file_for(from);
        let new = self.file_for(to);

        if old.exists() {
            if new.exists() {
                // The destination already has history — a note was renamed onto
                // a path that has been used before. Concatenate rather than
                // clobber: both are real history, and losing either is worse
                // than an interleaved timeline.
                let existing = std::fs::read(&old).map_err(|e| LedgerError::io(&old, e))?;
                let mut handle = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&new)
                    .map_err(|e| LedgerError::io(&new, e))?;
                handle.write_all(&existing).map_err(|e| LedgerError::io(&new, e))?;
                handle.sync_all().map_err(|e| LedgerError::io(&new, e))?;
                std::fs::remove_file(&old).map_err(|e| LedgerError::io(&old, e))?;
            } else {
                std::fs::rename(&old, &new).map_err(|e| LedgerError::io(&old, e))?;
            }
        }

        let hash = self.objects.put(content)?;
        let mut entry = Entry::new(actor, Op::Rename, format!("renamed from {from}"))
            .with_hashes(Some(hash.clone()), Some(hash));
        entry.from_path = Some(from.to_string());
        self.append(to, &entry)?;
        Ok(entry)
    }

    /// Drop a note's ledger. Only for a vault being reset — never as part of
    /// deleting a note, whose history is the whole point.
    pub fn forget(&self, path: &VaultPath) -> Result<()> {
        let file = self.file_for(path);
        if file.exists() {
            std::fs::remove_file(&file).map_err(|e| LedgerError::io(&file, e))?;
        }
        Ok(())
    }
}

/// A unified diff, for display only.
pub fn diff(before: &str, after: &str) -> String {
    use similar::TextDiff;
    TextDiff::from_lines(before, after)
        .unified_diff()
        .context_radius(3)
        .header("before", "after")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger() -> (tempfile::TempDir, Ledger, VaultPath) {
        let tmp = tempfile::tempdir().unwrap();
        let l = Ledger::open(tmp.path()).unwrap();
        (tmp, l, VaultPath::new("note.md").unwrap())
    }

    #[test]
    fn a_note_with_no_history_reads_as_empty() {
        let (_t, l, p) = ledger();
        assert!(l.read(&p).unwrap().is_empty());
    }

    #[test]
    fn entries_append_and_read_back_in_order() {
        let (_t, l, p) = ledger();
        l.record_change(&p, Actor::human("mishal"), Op::Create, "new note", None, Some("v1"))
            .unwrap();
        l.record_change(&p, Actor::human("mishal"), Op::Edit, "manual edit", Some("v1"), Some("v2"))
            .unwrap();

        let entries = l.read(&p).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].op, Op::Create);
        assert_eq!(entries[1].op, Op::Edit);
        assert!(entries[1].patch.as_ref().unwrap().contains("+v2"));
    }

    /// **The Phase 3 acceptance gate.**
    ///
    /// Fifty mixed human and agent mutations, then restore to state #17 exactly.
    #[test]
    fn fifty_mixed_mutations_then_restore_to_state_seventeen_exactly() {
        let (_t, l, p) = ledger();

        let mut content = String::from("# Note\n\nline 0\n");
        let mut states = vec![content.clone()];
        l.record_change(&p, Actor::human("mishal"), Op::Create, "new note", None, Some(&content))
            .unwrap();

        for i in 1..50 {
            let before = content.clone();
            content.push_str(&format!("line {i}\n"));

            // Alternate human and agent, so the timeline has both colours and
            // the restore has to work across a mixed history.
            let (actor, op, reason) = if i % 3 == 0 {
                (
                    Actor::agent("weave", "qwen3.5:0.8b", format!("run-{i}")),
                    Op::Accept,
                    format!("accepted suggestion {i}"),
                )
            } else {
                (Actor::human("mishal"), Op::Edit, "manual edit".to_string())
            };
            l.record_change(&p, actor, op, reason, Some(&before), Some(&content)).unwrap();
            states.push(content.clone());
        }

        let entries = l.read(&p).unwrap();
        assert_eq!(entries.len(), 50);

        // The gate.
        assert_eq!(l.content_at(&p, 17).unwrap(), states[17]);

        // And every other point, because "any prior state" is the actual claim.
        for (i, expected) in states.iter().enumerate() {
            assert_eq!(&l.content_at(&p, i).unwrap(), expected, "state {i} did not restore");
        }

        // Both actors are present and distinguishable — constraint 6 has
        // something to colour.
        assert!(entries.iter().any(|e| e.actor.is_agent()));
        assert!(entries.iter().any(|e| !e.actor.is_agent()));
        let agent = entries.iter().find(|e| e.actor.is_agent()).unwrap();
        assert_eq!(agent.actor.model.as_deref(), Some("qwen3.5:0.8b"));
    }

    /// **The constraint-4 gate:** a proposal that is never accepted leaves the
    /// file's mtime untouched.
    #[test]
    fn an_unaccepted_proposal_never_touches_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let note = tmp.path().join("note.md");
        std::fs::write(&note, b"# Original\n").unwrap();
        let before_meta = std::fs::metadata(&note).unwrap();
        let before_mtime = before_meta.modified().unwrap();

        let l = Ledger::open(tmp.path()).unwrap();
        let p = VaultPath::new("note.md").unwrap();

        std::thread::sleep(std::time::Duration::from_millis(20));
        l.propose(
            &p,
            Actor::agent("weave", "qwen3.5:0.8b", "run-1"),
            "rewrite the opening",
            "# Original\n",
            "# Rewritten by an agent\n",
        )
        .unwrap();

        let after_meta = std::fs::metadata(&note).unwrap();
        assert_eq!(after_meta.modified().unwrap(), before_mtime, "mtime moved");
        assert_eq!(after_meta.len(), before_meta.len());
        assert_eq!(std::fs::read(&note).unwrap(), b"# Original\n", "the file changed");

        // The proposal is recorded, and its content is retrievable for review.
        let entries = l.read(&p).unwrap();
        assert_eq!(entries[0].op, Op::Propose);
        assert!(!entries[0].op.touches_file());
        assert_eq!(
            l.objects().get(entries[0].after.as_ref().unwrap()).unwrap(),
            "# Rewritten by an agent\n"
        );
    }

    #[test]
    fn a_rejected_proposal_is_kept_as_history() {
        let (_t, l, p) = ledger();
        l.record_change(&p, Actor::human("m"), Op::Create, "new", None, Some("v1")).unwrap();
        l.record_reject(
            &p,
            Actor::agent("weave", "m", "s"),
            "user rejected the rewrite",
            "a rewrite nobody wanted",
        )
        .unwrap();

        let entries = l.read(&p).unwrap();
        assert_eq!(entries[1].op, Op::Reject);
        // Restoring "to" a rejection gives the state the note was actually in.
        assert_eq!(l.content_at(&p, 1).unwrap(), "v1");
    }

    #[test]
    fn history_survives_a_rename() {
        let (_t, l, _) = ledger();
        let from = VaultPath::new("old-name.md").unwrap();
        let to = VaultPath::new("Archive/new-name.md").unwrap();

        l.record_change(&from, Actor::human("m"), Op::Create, "new", None, Some("v1")).unwrap();
        l.record_change(&from, Actor::human("m"), Op::Edit, "edit", Some("v1"), Some("v2"))
            .unwrap();

        l.record_rename(&from, &to, Actor::human("m"), "v2").unwrap();

        // The old path has nothing left; the new path has everything.
        assert!(l.read(&from).unwrap().is_empty());
        let entries = l.read(&to).unwrap();
        assert_eq!(entries.len(), 3, "{entries:#?}");
        assert_eq!(entries[2].op, Op::Rename);
        assert_eq!(entries[2].from_path.as_deref(), Some("old-name.md"));

        // And the pre-rename states still restore.
        assert_eq!(l.content_at(&to, 0).unwrap(), "v1");
        assert_eq!(l.content_at(&to, 1).unwrap(), "v2");
    }

    #[test]
    fn renaming_onto_a_path_with_history_keeps_both() {
        let (_t, l, _) = ledger();
        let a = VaultPath::new("a.md").unwrap();
        let b = VaultPath::new("b.md").unwrap();

        l.record_change(&a, Actor::human("m"), Op::Create, "a", None, Some("a1")).unwrap();
        l.record_change(&b, Actor::human("m"), Op::Create, "b", None, Some("b1")).unwrap();

        l.record_rename(&a, &b, Actor::human("m"), "a1").unwrap();

        // b's own history, a's history, and the rename: nothing discarded.
        assert_eq!(l.read(&b).unwrap().len(), 3);
        assert!(l.read(&a).unwrap().is_empty());
    }

    #[test]
    fn a_truncated_last_line_costs_one_entry_not_the_file() {
        let (_t, l, p) = ledger();
        for i in 0..3 {
            l.record_change(&p, Actor::human("m"), Op::Edit, "e", None, Some(&format!("v{i}")))
                .unwrap();
        }

        // Simulate a crash mid-append.
        let file = l.file_for(&p);
        let mut raw = std::fs::read_to_string(&file).unwrap();
        raw.push_str("{\"ts\":\"2026-09-02T00:00:00Z\",\"actor\":{\"kin");
        std::fs::write(&file, raw).unwrap();

        let entries = l.read(&p).unwrap();
        assert_eq!(entries.len(), 3, "the complete entries should still be readable");
    }

    #[test]
    fn egress_records_where_bytes_went_and_how_many() {
        let (_t, l, p) = ledger();
        l.record_egress(
            &p,
            Actor::agent("runtime", "qwen3.5:4b", "run-9"),
            "http://workstation.local:11434",
            4096,
        )
        .unwrap();

        let e = &l.read(&p).unwrap()[0];
        assert_eq!(e.op, Op::Egress);
        assert!(!e.op.touches_file());
        assert_eq!(e.destination.as_deref(), Some("http://workstation.local:11434"));
        assert_eq!(e.bytes, Some(4096));
    }

    #[test]
    fn a_note_name_that_is_not_a_legal_filename_still_gets_a_ledger() {
        // Keyed on a hash of the path, so folders, spaces and unicode are fine.
        let (_t, l, _) = ledger();
        for name in ["Daily/2026-09-02.md", "with space.md", "unicode-café.md", "a/b/c/deep.md"] {
            let p = VaultPath::new(name).unwrap();
            l.record_change(&p, Actor::human("m"), Op::Create, "new", None, Some("v")).unwrap();
            assert_eq!(l.read(&p).unwrap().len(), 1, "failed for {name}");
        }
    }

    #[test]
    fn restoring_a_missing_index_says_so() {
        let (_t, l, p) = ledger();
        l.record_change(&p, Actor::human("m"), Op::Create, "new", None, Some("v1")).unwrap();
        assert!(matches!(
            l.content_at(&p, 9),
            Err(LedgerError::NoSuchEntry { index: 9, len: 1 })
        ));
    }
}
