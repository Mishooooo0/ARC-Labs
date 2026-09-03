//! The vault: a root plus the operations every shell needs.
//!
//! # Writing
//!
//! Phase 0 deliberately had no write path at all — the guarantee that browsing
//! leaves a vault untouched came from the capability not existing, not from
//! care. Phase 1 adds one, and it arrives with the two things that make it safe
//! rather than as a bare `fs::write`:
//!
//! - **Atomic replacement** ([`crate::atomic`]), so a note is never observed
//!   truncated and a crash mid-save cannot lose it.
//! - **Fidelity re-application** ([`NoteText::encode`]), so a note saved with no
//!   net change is byte-identical to what was read — not merely equivalent.
//!
//! [`Vault::write_note`] takes the [`NoteText`] that was *read*, not just a
//! string. That is what carries the file's own conventions to the write, and it
//! makes "save without having read" impossible to express.

use crate::atomic;
use crate::error::{Error, Result};
use crate::fidelity::NoteText;
use crate::markdown::{render, RenderedNote};
use crate::path::{VaultPath, VaultRoot};
use crate::tree::{self, Tree};

/// What a save actually did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Saved {
    /// Bytes were written.
    Written { bytes: usize },
    /// The encoded bytes matched what is already on disk, so nothing was
    /// written and the file's mtime is untouched.
    ///
    /// This is not just an optimisation. Phase 3 requires that a proposal which
    /// is never accepted leaves mtime alone, and Phase 6 audits a week of git
    /// history for file changes with no matching ledger entry. A save path that
    /// rewrites identical bytes would put noise into both.
    Unchanged,
}

#[derive(Debug, Clone)]
pub struct Vault {
    root: VaultRoot,
}

impl Vault {
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Vault> {
        let root = VaultRoot::open(path)?;
        // Clear anything a process that died mid-write left behind. The note
        // itself always survives such a kill; this is about not leaving
        // `.arc-write-*` litter in the user's notes folder.
        let swept = atomic::sweep_temp_files(root.path());
        if swept > 0 {
            tracing::info!(count = swept, "swept temp files from an interrupted write");
        }
        Ok(Vault { root })
    }

    pub fn root(&self) -> &VaultRoot {
        &self.root
    }

    /// The vault's own name, for display. The directory name, which is what
    /// Obsidian shows too.
    pub fn name(&self) -> String {
        self.root
            .path()
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.root.path().display().to_string())
    }

    pub fn tree(&self) -> Result<Tree> {
        tree::scan(&self.root)
    }

    /// Read a note, preserving how it was written.
    pub fn read_note(&self, path: &VaultPath) -> Result<NoteText> {
        let abs = self.root.resolve_existing(path)?;
        let bytes = std::fs::read(&abs).map_err(|e| Error::io(&abs, e))?;
        NoteText::decode(&bytes).ok_or_else(|| Error::NotUtf8 {
            path: path.to_string(),
        })
    }

    /// Read raw bytes — for `.canvas` and anything else that is not prose.
    pub fn read_bytes(&self, path: &VaultPath) -> Result<Vec<u8>> {
        let abs = self.root.resolve_existing(path)?;
        std::fs::read(&abs).map_err(|e| Error::io(&abs, e))
    }

    /// Read and render in one step, which is what every shell actually wants.
    pub fn render_note(&self, path: &VaultPath) -> Result<RenderedNote> {
        Ok(render(self.read_note(path)?.text()))
    }

    /// Can this process actually write here?
    ///
    /// Asked by *attempting* a write rather than by reading permission bits,
    /// because the bits are not the whole answer: a container running as a
    /// different uid than the volume's owner, a read-only mount, a full disk and
    /// an ACL all look fine to a stat and fail on the first save.
    ///
    /// Found on a real deployment, where the app came up reporting VAULT ONLINE
    /// with a silently broken index because the container's uid could not create
    /// `.arc/`. A notebook that cannot save must say so before you type into it,
    /// not after.
    pub fn is_writable(&self) -> bool {
        let probe = self.root.path().join(".arc-write-probe");
        match std::fs::write(&probe, b"") {
            Ok(()) => {
                let _ = std::fs::remove_file(&probe);
                true
            }
            Err(_) => false,
        }
    }

    pub fn exists(&self, path: &VaultPath) -> bool {
        self.root.resolve_existing(path).is_ok()
    }

    /// Save `new_text` to `path`, preserving how the file was written.
    ///
    /// `original` is the [`NoteText`] this edit started from. It supplies the
    /// line endings, BOM and — when the text comes back unchanged — the exact
    /// original bytes. Requiring it is what makes a fidelity-losing save
    /// impossible to write by accident.
    pub fn write_note(
        &self,
        path: &VaultPath,
        original: &NoteText,
        new_text: &str,
    ) -> Result<Saved> {
        // Resolve through the existing file so a symlink escaping the vault is
        // caught on the write path exactly as it is on the read path.
        let abs = self.root.resolve_existing(path)?;
        let bytes = original.encode(new_text);

        // Compare against what is actually on disk, not against what we think is
        // there. If the two already match there is nothing to do, and touching
        // mtime would be a lie about when the note last changed.
        if let Ok(current) = std::fs::read(&abs) {
            if current == bytes {
                return Ok(Saved::Unchanged);
            }
        }

        atomic::replace(&abs, &bytes)?;
        Ok(Saved::Written { bytes: bytes.len() })
    }

    /// Create a new note.
    ///
    /// **Refuses to overwrite.** A create that silently replaced an existing
    /// note because the names collided would be exactly the data loss this whole
    /// product exists to prevent — and it would be invisible, because the
    /// clobbered note's content simply would not be there any more. So the
    /// collision is an error the caller has to handle, and the UI turns it into
    /// "Untitled 2" rather than a lost note.
    ///
    /// New files are written LF with a trailing newline: the convention every
    /// tool in this space uses, and the one that makes a fresh vault diff
    /// cleanly. Existing files keep whatever they already had — that is
    /// [`Self::write_note`]'s job, not this one's.
    pub fn create_note(&self, path: &VaultPath, text: &str) -> Result<usize> {
        let abs = self.root.resolve_for_create(path)?;
        if abs.exists() {
            return Err(Error::AlreadyExists(path.to_string()));
        }

        let mut body = text.replace("\r\n", "\n");
        if !body.is_empty() && !body.ends_with('\n') {
            body.push('\n');
        }
        let bytes = body.into_bytes();

        // Atomic, like every other write: a create interrupted halfway should
        // leave no file at all rather than half a note.
        atomic::replace(&abs, &bytes)?;
        Ok(bytes.len())
    }

    /// Create a folder.
    ///
    /// Refuses an existing path for the same reason every other create does,
    /// with one wrinkle worth stating: an existing *directory* is refused too.
    /// Silently succeeding on a folder that is already there sounds harmless
    /// until the caller treats it as proof the folder is theirs and writes into
    /// someone else's.
    ///
    /// Not ledgered, and that is deliberate — see `Api::create_folder`.
    pub fn create_folder(&self, path: &VaultPath) -> Result<()> {
        // Resolve as if creating a file inside it, so the same containment and
        // symlink-escape checks run. A folder is a write to the vault and gets
        // the same scrutiny as a note.
        let abs = self.root.resolve_for_create(path)?;
        if abs.exists() {
            return Err(Error::AlreadyExists(path.to_string()));
        }
        std::fs::create_dir_all(&abs).map_err(|e| Error::io(&abs, e))
    }

    /// Create a file from bytes the caller has already formatted.
    ///
    /// Deliberately format-agnostic. A canvas is created by handing this the
    /// output of `arc-labs-canvas`'s own emitter — core does not know what a
    /// canvas looks like and must not learn, because the fan-out runs the other
    /// way and the byte-exact format has exactly one owner.
    ///
    /// Refuses to overwrite, like every other create here.
    pub fn create_file(&self, path: &VaultPath, bytes: &[u8]) -> Result<usize> {
        let abs = self.root.resolve_for_create(path)?;
        if abs.exists() {
            return Err(Error::AlreadyExists(path.to_string()));
        }
        atomic::replace(&abs, bytes)?;
        Ok(bytes.len())
    }

    /// Write bytes to a path, creating it or replacing what is there.
    ///
    /// **The one write here that does overwrite**, and the only caller is sync
    /// applying a change another machine already made. It is sound because of
    /// what happens before it rather than anything it checks itself: the
    /// content was reconciled against a base manifest, this path came back as
    /// "only one side moved", and a path where both sides moved never reaches
    /// here — it became a conflict for a person to settle.
    ///
    /// Bytes rather than text because a vault holds images and PDFs next to its
    /// notes, and a sync that could only carry UTF-8 would quietly drop them.
    ///
    /// No `FileFidelity` round-trip, deliberately: the bytes are already exactly
    /// what the other machine holds, and re-encoding them to this machine's
    /// conventions would make the two copies differ and the next pass see a
    /// change that nobody made.
    pub fn write_bytes(&self, path: &VaultPath, bytes: &[u8]) -> Result<usize> {
        let abs = self.root.resolve_for_create(path)?;
        atomic::replace(&abs, bytes)?;
        Ok(bytes.len())
    }

    /// Move a note to a new path.
    ///
    /// Refuses to overwrite the destination, for the same reason `create_note`
    /// does. The caller is responsible for moving the note's ledger alongside
    /// it — see `Ledger::record_rename`.
    pub fn rename_note(&self, from: &VaultPath, to: &VaultPath) -> Result<()> {
        if from == to {
            return Ok(());
        }
        let src = self.root.resolve_existing(from)?;
        let dst = self.root.resolve_for_create(to)?;
        if dst.exists() {
            return Err(Error::AlreadyExists(to.to_string()));
        }

        std::fs::rename(&src, &dst).map_err(|e| Error::io(&src, e))?;
        Ok(())
    }

    /// Delete a note — into the vault's trash, not into nothing.
    ///
    /// The ledger keeps the content and can restore it, so this could unlink.
    /// It does not, because those are different guarantees: the ledger protects
    /// you from a bad *edit*, and a copy on disk protects you from a bad
    /// *click*, a corrupt ledger, or a version of this app that has a bug in its
    /// restore path. Trash is cheap and the failure it prevents is permanent.
    ///
    /// Returns where the file went, so the caller can say so.
    pub fn delete_note(&self, path: &VaultPath) -> Result<std::path::PathBuf> {
        let abs = self.root.resolve_existing(path)?;

        // Keyed by a hash of the relative path, matching how the ledger names
        // its own files.
        let key = blake3::hash(path.as_str().as_bytes()).to_hex();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let dir = self.root.path().join(".arc").join("trash").join(&key[..16]);
        std::fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;

        let name = abs
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "note.md".into());

        // A second stamp is not unique: create-then-delete twice inside one
        // second is a normal thing to do while tidying up, and the second copy
        // would land on the first. Walk a counter until the name is free rather
        // than trusting the clock — the whole point of the trash is that nothing
        // in it gets overwritten.
        let mut grave = dir.join(format!("{stamp}-{name}"));
        let mut n = 1;
        while grave.exists() {
            grave = dir.join(format!("{stamp}-{n}-{name}"));
            n += 1;
        }

        // Copy-then-remove rather than rename: the trash may be on a different
        // volume from the note on some setups, and rename fails across volumes.
        std::fs::copy(&abs, &grave).map_err(|e| Error::io(&abs, e))?;
        std::fs::remove_file(&abs).map_err(|e| Error::io(&abs, e))?;
        Ok(grave)
    }

    /// Drop trashed copies older than `retention_days`. Returns how many went.
    ///
    /// **The expiry clock is already in the filename.** `delete_note` names
    /// every grave `<unix_secs>-<name>`, so this needs no sidecar file, no
    /// index, and no second answer to "when was this deleted" that could
    /// disagree with the first.
    ///
    /// `now_secs` is passed in rather than read from the clock so a test can
    /// age a file without waiting a week.
    ///
    /// This is not losing the note. Restore replays content from the ledger's
    /// object store by hash and never from here; what expires is the second
    /// copy that exists in case the first mechanism is the thing that failed.
    pub fn purge_trash(&self, retention_days: u32, now_secs: u64) -> Result<usize> {
        // Keeping for ever is a real answer, and it is the one that does
        // nothing rather than the one that deletes everything.
        if retention_days == 0 {
            return Ok(0);
        }
        let trash = self.root.path().join(".arc").join("trash");
        if !trash.is_dir() {
            return Ok(0);
        }

        let cutoff = now_secs.saturating_sub(u64::from(retention_days) * 86_400);
        let mut purged = 0;

        for bucket in std::fs::read_dir(&trash)
            .map_err(|e| Error::io(&trash, e))?
            .flatten()
        {
            let dir = bucket.path();
            if !dir.is_dir() {
                continue;
            }
            for grave in std::fs::read_dir(&dir)
                .map_err(|e| Error::io(&dir, e))?
                .flatten()
            {
                let file = grave.path();
                let Some(name) = file.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                // Anything whose name this does not understand is left alone.
                // Deleting a file because it did not match an expected shape is
                // how a cleanup routine becomes the bug it was meant to prevent.
                let Some(stamp) = name
                    .split_once('-')
                    .and_then(|(s, _)| s.parse::<u64>().ok())
                else {
                    continue;
                };
                if stamp < cutoff && std::fs::remove_file(&file).is_ok() {
                    purged += 1;
                }
            }
            // Tidy the bucket if it emptied. Failure is not an error: an empty
            // directory is untidy, not broken.
            let _ = std::fs::remove_dir(&dir);
        }
        Ok(purged)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault_with(files: &[(&str, &[u8])]) -> (tempfile::TempDir, Vault) {
        let tmp = tempfile::tempdir().unwrap();
        for (name, body) in files {
            let p = tmp.path().join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, body).unwrap();
        }
        let v = Vault::open(tmp.path()).unwrap();
        (tmp, v)
    }

    fn vp(s: &str) -> VaultPath {
        VaultPath::new(s).unwrap()
    }

    // ── create / rename / delete ────────────────────────────────────────────

    #[test]
    fn a_normal_vault_reports_itself_writable_and_leaves_no_litter() {
        let (t, v) = vault_with(&[(
            "a.md", b"# A
",
        )]);
        assert!(v.is_writable());
        // The probe must clean up after itself, or every open leaves a file in
        // someone's notes folder.
        let leftovers: Vec<_> = std::fs::read_dir(t.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("probe"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "the writability probe left {leftovers:?}"
        );
    }

    #[test]
    fn creating_a_note_writes_it_and_it_reads_back() {
        let (_t, v) = vault_with(&[]);
        let n = v.create_note(&vp("Notes/New.md"), "# New\n\nbody").unwrap();
        assert!(n > 0);
        assert_eq!(
            v.read_note(&vp("Notes/New.md")).unwrap().text(),
            "# New\n\nbody\n"
        );
    }

    #[test]
    fn creating_a_note_makes_the_folders_it_needs() {
        // "New note in a new folder" is an ordinary thing to want.
        let (_t, v) = vault_with(&[]);
        v.create_note(&vp("a/b/c/Deep.md"), "x").unwrap();
        assert!(v.exists(&vp("a/b/c/Deep.md")));
    }

    /// **The guard that matters.** Overwriting on create is silent data loss.
    #[test]
    fn creating_over_an_existing_note_is_refused_and_leaves_it_alone() {
        let (_t, v) = vault_with(&[("Keep.md", b"# Precious\n")]);
        let err = v
            .create_note(&vp("Keep.md"), "# Replacement\n")
            .unwrap_err();
        assert!(matches!(err, Error::AlreadyExists(_)), "got {err:?}");
        assert_eq!(v.read_note(&vp("Keep.md")).unwrap().text(), "# Precious\n");
    }

    #[test]
    fn a_new_note_is_lf_with_a_trailing_newline() {
        // A fresh vault should diff cleanly whatever platform made it.
        let (t, v) = vault_with(&[]);
        v.create_note(&vp("A.md"), "one\r\ntwo").unwrap();
        let raw = std::fs::read(t.path().join("A.md")).unwrap();
        assert_eq!(raw, b"one\ntwo\n");
    }

    #[test]
    fn an_empty_new_note_stays_empty() {
        // Zero-byte notes are real — the E-Tron vault has two. Do not invent a
        // newline for a note the user deliberately left blank.
        let (t, v) = vault_with(&[]);
        v.create_note(&vp("Blank.md"), "").unwrap();
        assert_eq!(std::fs::read(t.path().join("Blank.md")).unwrap(), b"");
    }

    #[test]
    fn a_create_cannot_escape_the_vault() {
        // The textual forms never become a VaultPath at all, so they cannot
        // reach `create_note` — the type is the guard, on the create path as
        // much as the read path.
        for bad in ["../escape.md", "/etc/passwd", "C:\\Windows\\win.ini"] {
            assert!(VaultPath::new(bad).is_err(), "accepted {bad}");
        }

        // And what does get created lands inside the root, parent directories
        // included — `resolve_for_create` canonicalises the parent precisely so
        // a deep create cannot be walked out of the vault.
        let (t, v) = vault_with(&[]);
        v.create_note(&vp("deep/deeper/x.md"), "x").unwrap();
        let real = dunce::canonicalize(t.path().join("deep/deeper/x.md")).unwrap();
        assert!(real.starts_with(dunce::canonicalize(t.path()).unwrap()));
    }

    #[test]
    fn renaming_moves_the_note_and_keeps_the_bytes() {
        let (_t, v) = vault_with(&[("Old.md", b"# Same\r\nbody\r\n")]);
        let before = v.read_bytes(&vp("Old.md")).unwrap();
        v.rename_note(&vp("Old.md"), &vp("Sub/New.md")).unwrap();

        assert!(!v.exists(&vp("Old.md")));
        // Byte-for-byte: a rename is not an edit, so CRLF stays CRLF.
        assert_eq!(v.read_bytes(&vp("Sub/New.md")).unwrap(), before);
    }

    #[test]
    fn renaming_onto_an_existing_note_is_refused() {
        let (_t, v) = vault_with(&[("A.md", b"# A\n"), ("B.md", b"# B\n")]);
        let err = v.rename_note(&vp("A.md"), &vp("B.md")).unwrap_err();
        assert!(matches!(err, Error::AlreadyExists(_)), "got {err:?}");
        // Both survive.
        assert_eq!(v.read_note(&vp("A.md")).unwrap().text(), "# A\n");
        assert_eq!(v.read_note(&vp("B.md")).unwrap().text(), "# B\n");
    }

    #[test]
    fn renaming_a_note_to_itself_does_nothing() {
        let (_t, v) = vault_with(&[("A.md", b"# A\n")]);
        v.rename_note(&vp("A.md"), &vp("A.md")).unwrap();
        assert_eq!(v.read_note(&vp("A.md")).unwrap().text(), "# A\n");
    }

    /// Deleting keeps the bytes. The ledger protects you from a bad edit; a copy
    /// on disk protects you from a bad click.
    #[test]
    fn deleting_a_note_moves_it_to_the_trash() {
        let (_t, v) = vault_with(&[("Gone.md", b"# Gone\nbut not lost\n")]);
        let grave = v.delete_note(&vp("Gone.md")).unwrap();

        assert!(!v.exists(&vp("Gone.md")));
        assert!(grave.exists(), "nothing landed in the trash");
        assert_eq!(std::fs::read(&grave).unwrap(), b"# Gone\nbut not lost\n");
        assert!(grave.to_string_lossy().contains("trash"));
    }

    /// The whole point of a retention window: recent deletes survive it.
    #[test]
    fn a_fresh_delete_is_not_swept_up() {
        let (_t, v) = vault_with(&[("Keep.md", b"# Keep\n")]);
        let grave = v.delete_note(&vp("Keep.md")).unwrap();

        let now = now_secs();
        assert_eq!(v.purge_trash(7, now).unwrap(), 0);
        assert!(
            grave.exists(),
            "a note deleted seconds ago must still be there"
        );
    }

    #[test]
    fn a_trashed_note_past_the_window_goes() {
        let (_t, v) = vault_with(&[("Old.md", b"# Old\n")]);
        let grave = v.delete_note(&vp("Old.md")).unwrap();

        // Eight days later, with a seven-day window.
        let now = now_secs() + 8 * 86_400;
        assert_eq!(v.purge_trash(7, now).unwrap(), 1);
        assert!(!grave.exists());
        // And the bucket it lived in does not linger as an empty directory.
        assert!(!grave.parent().unwrap().exists());
    }

    /// `0` is "keep for ever", and it is deliberately the value that does
    /// nothing rather than the value that deletes everything.
    #[test]
    fn zero_days_keeps_everything_for_ever() {
        let (_t, v) = vault_with(&[("Forever.md", b"# Forever\n")]);
        let grave = v.delete_note(&vp("Forever.md")).unwrap();

        // Ten years on.
        let now = now_secs() + 3650 * 86_400;
        assert_eq!(v.purge_trash(0, now).unwrap(), 0);
        assert!(grave.exists());
    }

    /// A cleanup routine that deletes what it cannot parse is the bug it was
    /// written to prevent.
    #[test]
    fn a_filename_it_does_not_understand_is_left_alone() {
        let (t, v) = vault_with(&[("A.md", b"# A\n")]);
        v.delete_note(&vp("A.md")).unwrap();

        let bucket = std::fs::read_dir(t.path().join(".arc").join("trash"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let stranger = bucket.join("not-a-timestamp.md");
        std::fs::write(&stranger, b"someone else put this here\n").unwrap();

        let now = now_secs() + 3650 * 86_400;
        // The real grave goes; the file with no timestamp stays.
        assert_eq!(v.purge_trash(7, now).unwrap(), 1);
        assert!(stranger.exists());
    }

    #[test]
    fn purging_a_vault_that_has_never_deleted_anything_is_fine() {
        let (_t, v) = vault_with(&[("A.md", b"# A\n")]);
        assert_eq!(v.purge_trash(7, now_secs()).unwrap(), 0);
    }

    fn now_secs() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    #[test]
    fn deleting_twice_does_not_clobber_the_first_copy() {
        let (_t, v) = vault_with(&[("X.md", b"first\n")]);
        let a = v.delete_note(&vp("X.md")).unwrap();
        v.create_note(&vp("X.md"), "second").unwrap();
        // Same second, so the timestamp alone would collide.
        let b = v.delete_note(&vp("X.md")).unwrap();

        assert_eq!(std::fs::read(&a).unwrap(), b"first\n");
        assert_eq!(std::fs::read(&b).unwrap(), b"second\n");
    }

    #[test]
    fn deleting_a_note_that_is_not_there_is_a_clean_error() {
        let (_t, v) = vault_with(&[]);
        assert!(v.delete_note(&vp("Nope.md")).is_err());
    }

    #[test]
    fn reads_and_renders_a_note() {
        let (_t, v) = vault_with(&[("n.md", b"---\ntitle: T\n---\n# H\n\nSee [[Other]] #tag\n")]);
        let p = VaultPath::new("n.md").unwrap();

        let r = v.render_note(&p).unwrap();
        assert_eq!(r.frontmatter.as_deref(), Some("title: T\n"));
        assert!(r.html.contains("<h1>H</h1>"));
        assert_eq!(r.links[0].target, "Other");
        assert_eq!(r.tags, ["tag"]);
    }

    #[test]
    fn reading_preserves_line_endings_for_the_phase_1_gate() {
        let (_t, v) = vault_with(&[("crlf.md", b"# T\r\nbody\r\n")]);
        let note = v.read_note(&VaultPath::new("crlf.md").unwrap()).unwrap();
        assert_eq!(note.fidelity().line_ending(), crate::LineEnding::Crlf);
        assert_eq!(note.encode(note.text()), b"# T\r\nbody\r\n");
    }

    #[test]
    fn handles_a_zero_byte_note() {
        // His real vault has two, and one is referenced from a canvas.
        let (_t, v) = vault_with(&[("empty.md", b"")]);
        let r = v.render_note(&VaultPath::new("empty.md").unwrap()).unwrap();
        assert!(r.html.is_empty() || r.html.trim().is_empty());
    }

    #[test]
    fn a_missing_note_is_not_found_rather_than_an_io_error() {
        let (_t, v) = vault_with(&[("a.md", b"x")]);
        let err = v
            .read_note(&VaultPath::new("nope.md").unwrap())
            .unwrap_err();
        assert!(matches!(err, Error::NoteNotFound(_)), "got {err:?}");
        assert!(!v.exists(&VaultPath::new("nope.md").unwrap()));
    }

    #[test]
    fn non_utf8_is_reported_not_corrupted() {
        let (_t, v) = vault_with(&[("latin1.md", b"caf\xE9\n")]);
        let err = v
            .read_note(&VaultPath::new("latin1.md").unwrap())
            .unwrap_err();
        assert!(matches!(err, Error::NotUtf8 { .. }), "got {err:?}");
    }

    /// **The Phase 1 acceptance gate, in miniature.**
    ///
    /// Open a note, type a character, undo it, save. The file must be
    /// byte-identical — not equivalent, identical — whatever conventions it was
    /// written with.
    #[test]
    fn edit_then_undo_then_save_is_byte_identical() {
        let cases: &[(&str, &[u8])] = &[
            ("lf.md", b"# Title\n\nbody\n"),
            ("crlf.md", b"# Title\r\n\r\nbody\r\n"),
            ("bom.md", b"\xEF\xBB\xBF# Title\n\nbody\n"),
            ("bom-crlf.md", b"\xEF\xBB\xBF# Title\r\nbody\r\n"),
            ("mixed.md", b"# Title\r\nsecond\nthird\r\n"),
            ("no-trailing.md", b"# no newline at end"),
            ("empty.md", b""),
            (
                "frontmatter.md",
                b"---\nzeta: 1\nalpha: 'q'  # comment\n---\n\n# Body\n",
            ),
        ];

        let tmp = tempfile::tempdir().unwrap();
        for (name, bytes) in cases {
            std::fs::write(tmp.path().join(name), bytes).unwrap();
        }
        let v = Vault::open(tmp.path()).unwrap();

        for (name, original_bytes) in cases {
            let p = VaultPath::new(*name).unwrap();
            let note = v.read_note(&p).unwrap();

            // Type a character, then undo it. The editor's round trip.
            let typed = format!("{}x", note.text());
            let undone = typed[..typed.len() - 1].to_string();
            assert_eq!(undone, note.text(), "the undo itself was lossy for {name}");

            let saved = v.write_note(&p, &note, &undone).unwrap();
            assert_eq!(
                saved,
                Saved::Unchanged,
                "{name} was rewritten despite no net change"
            );

            let after = std::fs::read(tmp.path().join(name)).unwrap();
            assert_eq!(&after, original_bytes, "{name} changed on disk");
        }
    }

    #[test]
    fn a_real_edit_keeps_the_files_own_conventions() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("crlf.md"), b"\xEF\xBB\xBF# T\r\nbody\r\n").unwrap();
        let v = Vault::open(tmp.path()).unwrap();
        let p = VaultPath::new("crlf.md").unwrap();

        let note = v.read_note(&p).unwrap();
        let edited = format!("{}added line\n", note.text());
        assert!(matches!(
            v.write_note(&p, &note, &edited).unwrap(),
            Saved::Written { .. }
        ));

        // The BOM survives and the new line uses CRLF like its neighbours.
        assert_eq!(
            std::fs::read(tmp.path().join("crlf.md")).unwrap(),
            b"\xEF\xBB\xBF# T\r\nbody\r\nadded line\r\n"
        );
    }

    /// Mixed line endings: what survives, and what does not.
    ///
    /// Pinned as a test because it is the one place where fidelity is not
    /// perfect, and the boundary needs to be a decision rather than a surprise:
    ///
    /// - A **no-op save** leaves a mixed file exactly as it was. This is the
    ///   Phase 1 acceptance criterion, and it holds.
    /// - A **real edit** re-encodes the document with the dominant ending, so
    ///   the mix is lost. Preserving an arbitrary mix through an arbitrary edit
    ///   has no coherent answer — which line ending should an inserted line get?
    ///   VS Code and Obsidian both normalise here too.
    ///
    /// Because it is lossy, it is surfaced: `NoteView.line_endings_mixed` puts a
    /// warning in the note's detail strip, so the user learns before the edit
    /// rather than from a 200-line diff afterwards.
    #[test]
    fn mixed_line_endings_survive_a_no_op_but_normalise_on_a_real_edit() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("mixed.md");
        let original: &[u8] = b"# mixed\r\nsecond\nthird\r\n";
        std::fs::write(&file, original).unwrap();
        let v = Vault::open(tmp.path()).unwrap();
        let p = VaultPath::new("mixed.md").unwrap();

        let note = v.read_note(&p).unwrap();
        assert!(
            note.fidelity().is_mixed(),
            "the mix must be detected and reportable"
        );

        // No-op: byte-identical, nothing written.
        assert_eq!(
            v.write_note(&p, &note, note.text()).unwrap(),
            Saved::Unchanged
        );
        assert_eq!(std::fs::read(&file).unwrap(), original);

        // Real edit: normalised to the dominant ending, which here is CRLF.
        let edited = format!("{}fourth\n", note.text());
        assert!(matches!(
            v.write_note(&p, &note, &edited).unwrap(),
            Saved::Written { .. }
        ));
        assert_eq!(
            std::fs::read(&file).unwrap(),
            b"# mixed\r\nsecond\r\nthird\r\nfourth\r\n",
            "a real edit should normalise to the dominant ending"
        );
    }

    #[test]
    fn an_unchanged_save_does_not_touch_mtime() {
        // Phase 3 needs this: an unaccepted proposal must leave mtime alone, and
        // Phase 6 audits git history for changes with no ledger entry.
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("n.md");
        std::fs::write(&file, b"# T\nbody\n").unwrap();
        let v = Vault::open(tmp.path()).unwrap();
        let p = VaultPath::new("n.md").unwrap();

        let before = std::fs::metadata(&file).unwrap().modified().unwrap();
        let note = v.read_note(&p).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));

        assert_eq!(
            v.write_note(&p, &note, note.text()).unwrap(),
            Saved::Unchanged
        );
        assert_eq!(
            std::fs::metadata(&file).unwrap().modified().unwrap(),
            before
        );
    }

    #[test]
    fn writing_through_an_escaping_symlink_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = tmp.path().join("vault");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&vault).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let target = outside.join("secret.md");
        std::fs::write(&target, b"# untouched\n").unwrap();

        let link = vault.join("innocent.md");
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&target, &link).is_ok();
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(&target, &link).is_ok();
        if !made {
            eprintln!("skipping: this platform/user cannot create symlinks");
            return;
        }

        let v = Vault::open(&vault).unwrap();
        let p = VaultPath::new("innocent.md").unwrap();
        // Reading is already refused, so construct the NoteText directly to
        // prove the write path checks independently rather than relying on the
        // read having happened first.
        let note = NoteText::decode(b"# untouched\n").unwrap();
        assert!(matches!(
            v.write_note(&p, &note, "# OVERWRITTEN\n"),
            Err(Error::PathEscapesVault(_))
        ));
        assert_eq!(std::fs::read(&target).unwrap(), b"# untouched\n");
    }

    #[test]
    fn vault_name_is_the_directory_name() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("My Notes");
        std::fs::create_dir(&dir).unwrap();
        assert_eq!(Vault::open(&dir).unwrap().name(), "My Notes");
    }
}
