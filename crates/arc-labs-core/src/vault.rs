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
        Ok(Vault { root: VaultRoot::open(path)? })
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
        NoteText::decode(&bytes).ok_or_else(|| Error::NotUtf8 { path: path.to_string() })
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
        let err = v.read_note(&VaultPath::new("nope.md").unwrap()).unwrap_err();
        assert!(matches!(err, Error::NoteNotFound(_)), "got {err:?}");
        assert!(!v.exists(&VaultPath::new("nope.md").unwrap()));
    }

    #[test]
    fn non_utf8_is_reported_not_corrupted() {
        let (_t, v) = vault_with(&[("latin1.md", b"caf\xE9\n")]);
        let err = v.read_note(&VaultPath::new("latin1.md").unwrap()).unwrap_err();
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
            ("frontmatter.md", b"---\nzeta: 1\nalpha: 'q'  # comment\n---\n\n# Body\n"),
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
            assert_eq!(saved, Saved::Unchanged, "{name} was rewritten despite no net change");

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
        assert!(matches!(v.write_note(&p, &note, &edited).unwrap(), Saved::Written { .. }));

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
        assert!(note.fidelity().is_mixed(), "the mix must be detected and reportable");

        // No-op: byte-identical, nothing written.
        assert_eq!(v.write_note(&p, &note, note.text()).unwrap(), Saved::Unchanged);
        assert_eq!(std::fs::read(&file).unwrap(), original);

        // Real edit: normalised to the dominant ending, which here is CRLF.
        let edited = format!("{}fourth\n", note.text());
        assert!(matches!(v.write_note(&p, &note, &edited).unwrap(), Saved::Written { .. }));
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

        assert_eq!(v.write_note(&p, &note, note.text()).unwrap(), Saved::Unchanged);
        assert_eq!(std::fs::metadata(&file).unwrap().modified().unwrap(), before);
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
        assert!(matches!(v.write_note(&p, &note, "# OVERWRITTEN\n"), Err(Error::PathEscapesVault(_))));
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
