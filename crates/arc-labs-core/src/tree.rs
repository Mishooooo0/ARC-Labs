//! The vault file tree.
//!
//! Returned as a **flat list with parent indices**, not a nested structure.
//! Three reasons, in order of how much they matter:
//!
//! 1. It serialises to JSON without recursion, which the browser shell needs and
//!    a nested tree makes needlessly expensive.
//! 2. The UI renders only expanded nodes, so it wants random access by index,
//!    not a walk.
//! 3. A 5,000-note vault is one allocation instead of a few thousand.
//!
//! Entries are ordered the way Obsidian orders them — directories first, then
//! files, each case-insensitively — so a user switching between the two apps
//! sees the same vault in the same order.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::path::{VaultPath, VaultRoot};

/// Directories that are infrastructure, not notes.
///
/// `.obsidian` is skipped rather than hidden: it is Obsidian's own state, the
/// user did not write it, and showing it invites editing it. `.arc` is ours and
/// gets the same treatment. Everything else beginning with `.` is skipped for
/// the same reason a file manager hides dotfiles.
const SKIP_DIRS: &[&str] = &[
    ".arc",
    ".obsidian",
    ".git",
    ".trash",
    ".stfolder",
    ".stversions",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeEntry {
    pub path: VaultPath,
    /// The display name — the last path component. Sent alongside `path` so the
    /// UI never has to parse a path to render a row.
    pub name: String,
    pub is_dir: bool,
    /// Index into [`Tree::entries`], or `None` for a top-level entry.
    pub parent: Option<usize>,
    /// Bytes on disk. `0` for a directory.
    ///
    /// Here rather than in the index because the library view draws a note's
    /// spine width from its length, and it must be able to draw a vault that
    /// has never been indexed — a fresh one, or one whose index is still
    /// building. The walk already reads each entry's file type; this is one
    /// more `metadata()` on the same handle, which Windows answers from the
    /// directory listing it already had.
    pub size: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tree {
    pub entries: Vec<TreeEntry>,
    pub note_count: usize,
    pub canvas_count: usize,
    /// Entries the scan could not use: unreadable directories, names that are
    /// not valid vault paths. Surfaced rather than swallowed — a vault that is
    /// half-visible should say so.
    pub skipped: Vec<Skipped>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skipped {
    pub path: String,
    pub reason: String,
}

/// Walk the vault.
///
/// Symlinks are not followed. A symlinked directory could point at `/` and turn
/// a tree scan into a filesystem crawl, and a symlinked file that escapes the
/// vault is rejected at read time by [`VaultRoot::resolve_existing`] anyway.
pub fn scan(root: &VaultRoot) -> Result<Tree> {
    let mut tree = Tree::default();
    // Maps a directory's vault path to its index, so children can find a parent
    // without searching. walkdir yields parents before children, so the lookup
    // is always populated by the time it is needed.
    let mut index_of: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    let walker = walkdir::WalkDir::new(root.path())
        .follow_links(false)
        .sort_by(|a, b| {
            let a_dir = a.file_type().is_dir();
            let b_dir = b.file_type().is_dir();
            b_dir.cmp(&a_dir).then_with(|| {
                let (an, bn) = (
                    a.file_name().to_string_lossy(),
                    b.file_name().to_string_lossy(),
                );
                an.to_lowercase()
                    .cmp(&bn.to_lowercase())
                    .then_with(|| an.cmp(&bn))
            })
        })
        .into_iter()
        .filter_entry(|e| {
            if e.depth() == 0 {
                return true;
            }
            let name = e.file_name().to_string_lossy();
            if e.file_type().is_dir() {
                return !(name.starts_with('.') || SKIP_DIRS.contains(&name.as_ref()));
            }
            true
        });

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                tree.skipped.push(Skipped {
                    path: e
                        .path()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                    reason: "could not be read".into(),
                });
                continue;
            }
        };
        if entry.depth() == 0 {
            continue;
        }

        let vp = match root.relativize(entry.path()) {
            Ok(v) => v,
            Err(Error::InvalidVaultPath { reason, .. }) => {
                // A real case, not theoretical: a Linux-authored vault can hold
                // `aux.md`, which Windows cannot open. Naming it beats hiding it.
                tree.skipped.push(Skipped {
                    path: entry.file_name().to_string_lossy().into_owned(),
                    reason: reason.to_string(),
                });
                continue;
            }
            Err(_) => continue,
        };

        let is_dir = entry.file_type().is_dir();
        // A file whose metadata cannot be read still belongs in the tree: the
        // size is a display detail, and dropping the note because of it would
        // hide something that exists.
        let size = if is_dir {
            0
        } else {
            entry.metadata().map(|m| m.len()).unwrap_or(0)
        };
        if !is_dir {
            if vp.is_markdown() {
                tree.note_count += 1;
            } else if vp.is_canvas() {
                tree.canvas_count += 1;
            }
        }

        let parent = vp.parent().and_then(|p| index_of.get(p.as_str()).copied());
        let idx = tree.entries.len();
        if is_dir {
            index_of.insert(vp.as_str().to_string(), idx);
        }
        tree.entries.push(TreeEntry {
            name: vp.file_name().to_string(),
            path: vp,
            is_dir,
            parent,
            size,
        });
    }

    Ok(tree)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(files: &[&str]) -> (tempfile::TempDir, VaultRoot) {
        let tmp = tempfile::tempdir().unwrap();
        for f in files {
            let p = tmp.path().join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, b"# note\n").unwrap();
        }
        let root = VaultRoot::open(tmp.path()).unwrap();
        (tmp, root)
    }

    /// The library draws a spine's width from this, so a wrong size is a wrong
    /// picture — and a zero-byte note is a real thing the fixtures contain.
    #[test]
    fn entries_carry_their_size_and_directories_carry_none() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("Folder")).unwrap();
        let body = b"# a longer note\n\nwith body\n";
        std::fs::write(tmp.path().join("Folder/Long.md"), body).unwrap();
        std::fs::write(tmp.path().join("Empty.md"), b"").unwrap();
        let root = VaultRoot::open(tmp.path()).unwrap();

        let tree = scan(&root).unwrap();
        let of = |name: &str| {
            tree.entries
                .iter()
                .find(|e| e.name == name)
                .unwrap_or_else(|| panic!("no entry {name}"))
        };

        assert_eq!(of("Long.md").size, body.len() as u64);
        assert_eq!(of("Empty.md").size, 0, "a zero-byte note is still a note");
        assert_eq!(of("Folder").size, 0, "a directory has no size of its own");
        assert!(of("Folder").is_dir);
    }

    #[test]
    fn scans_a_nested_vault_and_links_parents() {
        let (_t, root) = build(&[
            "a.md",
            "Daily/2026-09-02.md",
            "Daily/Sub/deep.md",
            "board.canvas",
        ]);
        let tree = scan(&root).unwrap();

        assert_eq!(tree.note_count, 3);
        assert_eq!(tree.canvas_count, 1);

        let find = |p: &str| {
            tree.entries
                .iter()
                .position(|e| e.path.as_str() == p)
                .unwrap()
        };
        let daily = find("Daily");
        assert_eq!(
            tree.entries[find("Daily/2026-09-02.md")].parent,
            Some(daily)
        );
        assert_eq!(
            tree.entries[find("Daily/Sub/deep.md")].parent,
            Some(find("Daily/Sub"))
        );
        assert_eq!(tree.entries[find("a.md")].parent, None);

        // Every parent index must be a real directory earlier in the list —
        // the invariant the UI relies on to render without a second pass.
        for (i, e) in tree.entries.iter().enumerate() {
            if let Some(p) = e.parent {
                assert!(p < i, "parent of {} comes after it", e.path);
                assert!(tree.entries[p].is_dir);
            }
        }
    }

    #[test]
    fn skips_infrastructure_directories() {
        let (_t, root) = build(&[
            "note.md",
            ".obsidian/app.json",
            ".git/config",
            ".arc/index.db",
            ".hidden/x.md",
        ]);
        let tree = scan(&root).unwrap();
        for e in &tree.entries {
            let p = e.path.as_str();
            assert!(!p.starts_with('.'), "leaked infrastructure entry: {p}");
        }
        assert_eq!(tree.note_count, 1);
    }

    #[test]
    fn orders_directories_before_files_case_insensitively() {
        let (_t, root) = build(&["zebra.md", "Apple.md", "beta/x.md", "Alpha/y.md"]);
        let tree = scan(&root).unwrap();
        let top: Vec<&str> = tree
            .entries
            .iter()
            .filter(|e| e.parent.is_none())
            .map(|e| e.path.as_str())
            .collect();
        assert_eq!(top, ["Alpha", "beta", "Apple.md", "zebra.md"]);
    }

    #[test]
    fn an_empty_vault_scans_cleanly() {
        let tmp = tempfile::tempdir().unwrap();
        let tree = scan(&VaultRoot::open(tmp.path()).unwrap()).unwrap();
        assert!(tree.entries.is_empty());
        assert_eq!(tree.note_count, 0);
        assert!(tree.skipped.is_empty());
    }

    #[test]
    fn reports_names_it_cannot_represent_instead_of_hiding_them() {
        // Only creatable on Linux; on Windows the OS refuses the name outright,
        // which is exactly the portability problem being reported.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("ok.md"), b"x").unwrap();
        let hostile = tmp.path().join("aux.md");
        if std::fs::write(&hostile, b"x").is_err() {
            eprintln!("skipping: this platform will not create a reserved name");
            return;
        }
        let tree = scan(&VaultRoot::open(tmp.path()).unwrap()).unwrap();
        if cfg!(unix) {
            assert_eq!(tree.skipped.len(), 1, "a reserved name should be reported");
            assert_eq!(tree.skipped[0].path, "aux.md");
        }
        assert_eq!(tree.note_count, 1);
    }
}
