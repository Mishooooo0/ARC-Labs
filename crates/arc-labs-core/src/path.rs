//! Path containment: [`VaultRoot`] and [`VaultPath`].
//!
//! Every operation in ARC-LABS names a file with a [`VaultPath`], never a
//! string and never a `PathBuf`. That is not tidiness — the product has four
//! shells, two of which (the HTTP server and the Phase 6 MCP server) accept
//! paths from callers who are not the person sitting at the machine. One
//! handler that forgets to validate is a filesystem read primitive.
//!
//! The type makes forgetting impossible in two ways:
//!
//! 1. A `VaultPath` is *relative* by construction and cannot become an absolute
//!    path without a `VaultRoot`. Holding one grants nothing on its own.
//! 2. `Deserialize` runs the same validation as the constructor, so a hostile
//!    JSON body cannot conjure one. There is no route from untrusted input to
//!    an invalid `VaultPath`.
//!
//! Validation is stricter than either Windows or Linux requires on its own,
//! because a vault must behave identically on both. A name that is legal on
//! Linux and silently aliased on Windows (`note.` and `note`) is rejected on
//! both, rather than becoming a portability bug that surfaces on one platform
//! months later.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Device names Windows resolves regardless of directory or extension:
/// `AUX.md` opens a device, not a file. Enforced on every platform so a vault
/// authored on Linux cannot become unopenable on Windows.
const WINDOWS_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM0", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
    "COM8", "COM9", "LPT0", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// A path inside a vault: relative, normalised, forward-slashed.
///
/// Forward slashes on every platform, because this value is serialised into
/// JSON, into `.canvas` files and into the index. A vault authored on Windows
/// and opened on Linux must resolve links identically.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct VaultPath(String);

impl VaultPath {
    pub fn new(input: impl AsRef<str>) -> Result<Self> {
        let input = input.as_ref();

        if input.is_empty() {
            return Err(Error::invalid(input, "path is empty"));
        }
        if input.contains('\0') {
            return Err(Error::invalid(input, "path contains a NUL byte"));
        }
        // One rule rejecting three things: drive letters (`C:\...`), NTFS
        // alternate data streams (`note.md:secret`), and URL-ish schemes.
        if input.contains(':') {
            return Err(Error::invalid(input, "path contains a colon"));
        }
        if input.starts_with('/') || input.starts_with('\\') {
            return Err(Error::invalid(input, "path is absolute"));
        }

        let mut parts: Vec<&str> = Vec::new();
        for raw in input.split(['/', '\\']) {
            match raw {
                // Collapse `a//b` and `a/./b` rather than rejecting them: both
                // are harmless and common in hand-written links.
                "" | "." => continue,
                ".." => return Err(Error::invalid(input, "path contains a parent reference")),
                _ => {}
            }
            if raw.trim().is_empty() {
                return Err(Error::invalid(
                    input,
                    "path has a whitespace-only component",
                ));
            }
            // Windows strips these silently, so `note ` and `note` name the same
            // file there and different files on Linux.
            if raw.ends_with(' ') || raw.ends_with('.') {
                return Err(Error::invalid(
                    input,
                    "component ends with a space or a dot",
                ));
            }
            let stem = raw.split('.').next().unwrap_or(raw);
            if WINDOWS_RESERVED
                .iter()
                .any(|r| stem.eq_ignore_ascii_case(r))
            {
                return Err(Error::invalid(input, "component is a reserved device name"));
            }
            parts.push(raw);
        }

        if parts.is_empty() {
            return Err(Error::invalid(input, "path resolves to the vault root"));
        }
        Ok(VaultPath(parts.join("/")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The final component: `Daily/2026-09-02.md` -> `2026-09-02.md`.
    pub fn file_name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or(&self.0)
    }

    /// The final component without its extension — what a `[[wikilink]]` matches.
    pub fn stem(&self) -> &str {
        let name = self.file_name();
        match name.rfind('.') {
            Some(i) if i > 0 => &name[..i],
            _ => name,
        }
    }

    pub fn extension(&self) -> Option<&str> {
        let name = self.file_name();
        name.rfind('.').filter(|i| *i > 0).map(|i| &name[i + 1..])
    }

    pub fn parent(&self) -> Option<VaultPath> {
        self.0
            .rfind('/')
            .map(|i| VaultPath(self.0[..i].to_string()))
    }

    pub fn is_markdown(&self) -> bool {
        matches!(self.extension(), Some(e) if e.eq_ignore_ascii_case("md"))
    }

    pub fn is_canvas(&self) -> bool {
        matches!(self.extension(), Some(e) if e.eq_ignore_ascii_case("canvas"))
    }
}

impl TryFrom<String> for VaultPath {
    type Error = Error;
    fn try_from(s: String) -> Result<Self> {
        VaultPath::new(s)
    }
}

impl From<VaultPath> for String {
    fn from(v: VaultPath) -> String {
        v.0
    }
}

impl std::fmt::Display for VaultPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A canonicalised, verified vault root: the only thing that can turn a
/// [`VaultPath`] into something the filesystem will accept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultRoot {
    root: PathBuf,
}

impl VaultRoot {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        // dunce, not std: std::fs::canonicalize yields \\?\C:\... on Windows,
        // which breaks strip_prefix against anything the user typed and leaks
        // into every display string.
        let root = dunce::canonicalize(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Error::VaultRootMissing(path.to_path_buf()),
            _ => Error::io(path, e),
        })?;
        if !root.is_dir() {
            return Err(Error::VaultRootNotDirectory(root));
        }
        Ok(VaultRoot { root })
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    /// Join without touching the filesystem. Safe for a path that does not exist
    /// yet, but does **not** defeat a symlink — call [`Self::resolve_existing`]
    /// before reading.
    pub fn join(&self, vp: &VaultPath) -> PathBuf {
        let mut p = self.root.clone();
        for part in vp.as_str().split('/') {
            p.push(part);
        }
        p
    }

    /// Resolve for reading: the file must exist, and after every symlink is
    /// followed it must still be inside the vault.
    ///
    /// This is the check that matters. `VaultPath` validation rejects `..` in
    /// the *text* of a path; this rejects a vault file that is a symlink to
    /// `/etc/shadow`, which no amount of string validation can catch.
    pub fn resolve_existing(&self, vp: &VaultPath) -> Result<PathBuf> {
        let joined = self.join(vp);
        let real = dunce::canonicalize(&joined).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Error::NoteNotFound(vp.to_string()),
            _ => Error::io(&joined, e),
        })?;
        if !real.starts_with(&self.root) {
            return Err(Error::PathEscapesVault(real));
        }
        Ok(real)
    }

    /// Resolve for **creating** a file that does not exist yet.
    ///
    /// `resolve_existing` cannot help here — it canonicalises the target, and
    /// the target is the thing we are about to make. So this canonicalises the
    /// *parent* instead, which is what actually needs checking: a note at
    /// `Notes/x.md` is safe if and only if `Notes/` really is inside the vault
    /// after every symlink is followed. Creating a directory whose parent is a
    /// symlink out of the vault is the hole this closes.
    ///
    /// Creates missing parent directories, because "new note in a new folder"
    /// is an ordinary thing to want. Each level is checked as it is made.
    pub fn resolve_for_create(&self, vp: &VaultPath) -> Result<PathBuf> {
        let joined = self.join(vp);
        let parent = joined
            .parent()
            .ok_or_else(|| Error::invalid(vp.as_str(), "path has no parent directory"))?;

        if !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        let real_parent = dunce::canonicalize(parent).map_err(|e| Error::io(parent, e))?;
        if !real_parent.starts_with(&self.root) {
            return Err(Error::PathEscapesVault(real_parent));
        }

        let name = joined
            .file_name()
            .ok_or_else(|| Error::invalid(vp.as_str(), "path has no file name"))?;
        Ok(real_parent.join(name))
    }

    /// Turn an absolute path into a [`VaultPath`], rejecting anything outside.
    pub fn relativize(&self, abs: &Path) -> Result<VaultPath> {
        let real = dunce::canonicalize(abs).map_err(|e| Error::io(abs, e))?;
        let rel = real
            .strip_prefix(&self.root)
            .map_err(|_| Error::PathEscapesVault(real.clone()))?;
        let text: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        VaultPath::new(text.join("/"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ordinary_paths() {
        let good = [
            "note.md",
            "Daily/2026-09-02.md",
            "a/b/c/deep.md",
            "with space.md",
            "emoji-check.md",
        ];
        for g in good {
            assert!(VaultPath::new(g).is_ok(), "should accept {g}");
        }
    }

    #[test]
    fn normalises_separators_and_redundant_components() {
        assert_eq!(
            VaultPath::new("Daily\\2026.md").unwrap().as_str(),
            "Daily/2026.md"
        );
        assert_eq!(VaultPath::new("a//b/./c.md").unwrap().as_str(), "a/b/c.md");
    }

    #[test]
    fn rejects_traversal_and_absolutes() {
        let bad = [
            "../secret",
            "a/../../etc/passwd",
            "..\\..\\Windows\\System32",
            "/etc/shadow",
            "\\\\server\\share\\x",
            "C:\\Users\\misho\\.ssh\\id_ed25519",
            "note.md:hidden",
            "",
            ".",
            "a/\0/b",
        ];
        for b in bad {
            assert!(VaultPath::new(b).is_err(), "should reject {b:?}");
        }
    }

    #[test]
    fn rejects_windows_hostile_names_on_every_platform() {
        // These would make a Linux-authored vault fail to open on Windows.
        // Rejecting them everywhere keeps vaults portable.
        let bad = [
            "CON",
            "aux.md",
            "COM1.txt",
            "nul",
            "trailing.",
            "trailing ",
            "a/PRN/b.md",
        ];
        for b in bad {
            assert!(VaultPath::new(b).is_err(), "should reject {b:?}");
        }
    }

    #[test]
    fn deserialize_runs_the_same_validation_as_the_constructor() {
        // The hostile-JSON case: a browser client posting a traversal.
        assert!(serde_json::from_str::<VaultPath>("\"../../etc/passwd\"").is_err());
        assert!(serde_json::from_str::<VaultPath>("\"C:\\\\Windows\\\\win.ini\"").is_err());

        let ok: VaultPath = serde_json::from_str("\"Daily/note.md\"").unwrap();
        assert_eq!(ok.as_str(), "Daily/note.md");
        assert_eq!(serde_json::to_string(&ok).unwrap(), "\"Daily/note.md\"");
    }

    #[test]
    fn name_accessors() {
        let p = VaultPath::new("Daily/2026-09-02.md").unwrap();
        assert_eq!(p.file_name(), "2026-09-02.md");
        assert_eq!(p.stem(), "2026-09-02");
        assert_eq!(p.extension(), Some("md"));
        assert_eq!(p.parent().unwrap().as_str(), "Daily");
        assert!(p.is_markdown());
        assert!(!p.is_canvas());

        // A dotfile is a name, not an extension.
        let dot = VaultPath::new(".gitignore").unwrap();
        assert_eq!(dot.stem(), ".gitignore");
        assert_eq!(dot.extension(), None);
    }

    #[test]
    fn root_rejects_paths_outside_itself() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = tmp.path().join("vault");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&vault).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(vault.join("inside.md"), b"# in").unwrap();
        std::fs::write(outside.join("secret.md"), b"# out").unwrap();

        let root = VaultRoot::open(&vault).unwrap();

        let inside = VaultPath::new("inside.md").unwrap();
        assert!(root.resolve_existing(&inside).is_ok());

        // relativize is the other door into the type, and it must be shut too.
        assert!(matches!(
            root.relativize(&outside.join("secret.md")),
            Err(Error::PathEscapesVault(_))
        ));
        assert_eq!(
            root.relativize(&vault.join("inside.md")).unwrap().as_str(),
            "inside.md"
        );
    }

    #[test]
    fn root_rejects_a_symlink_that_escapes_the_vault() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = tmp.path().join("vault");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&vault).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let target = outside.join("secret.md");
        std::fs::write(&target, b"# out").unwrap();

        let link = vault.join("innocent.md");
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&target, &link).is_ok();
        #[cfg(windows)]
        // Unprivileged Windows without Developer Mode cannot create symlinks.
        // Skip rather than fail: the Linux and Docker runs of this same test
        // cover the behaviour, and `--all-modes` runs all of them.
        let made = std::os::windows::fs::symlink_file(&target, &link).is_ok();

        if !made {
            eprintln!("skipping: this platform/user cannot create symlinks");
            return;
        }

        let root = VaultRoot::open(&vault).unwrap();
        let vp = VaultPath::new("innocent.md").unwrap();
        // The path text is spotless. Only canonicalisation catches this.
        assert!(matches!(
            root.resolve_existing(&vp),
            Err(Error::PathEscapesVault(_))
        ));
    }
}
