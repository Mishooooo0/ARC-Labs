//! The vault: a root plus the read operations every shell needs.
//!
//! # Phase 0 has no write path, structurally
//!
//! There is no `write_note` here, and there is no private one either. The Phase
//! 0 acceptance criterion is that `git status` inside a real vault stays clean
//! after an hour of browsing, and the way to guarantee that is not care — it is
//! that the capability does not exist yet. Phase 1 adds writing together with
//! the atomic-save machinery and the fidelity re-application that make it safe.
//!
//! When that happens, writes arrive as a separate type that must be constructed
//! deliberately, so "did this code path write?" stays answerable by looking at
//! the signature.

use crate::error::{Error, Result};
use crate::fidelity::NoteText;
use crate::markdown::{render, RenderedNote};
use crate::path::{VaultPath, VaultRoot};
use crate::tree::{self, Tree};

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

    #[test]
    fn vault_name_is_the_directory_name() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("My Notes");
        std::fs::create_dir(&dir).unwrap();
        assert_eq!(Vault::open(&dir).unwrap().name(), "My Notes");
    }
}
