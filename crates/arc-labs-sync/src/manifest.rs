//! What a vault looks like, as one comparable value.
//!
//! Size and content hash per file, and **no mtime**. Two machines do not agree
//! about the time and never will: clocks drift, timezones lie, filesystems
//! round to two seconds, and a file copied by any tool arrives with a fresh
//! one. A hash disagrees only when the bytes disagree, which is the only
//! question sync is actually asking.
//!
//! The shape matches `manifest_of` in xtask, which has been the fidelity oracle
//! since Phase 0 — deliberately, so the thing that proves a vault did not
//! change and the thing that decides what to send cannot drift apart.

use std::collections::BTreeMap;
use std::path::Path;

use crate::{Result, SyncError};

/// One file, as a manifest sees it.
///
/// `len` is not redundant with `hash`. It is free, it makes a mismatch
/// readable in a log, and it turns a truncated transfer into an obvious error
/// rather than a hash that merely fails to match for no stated reason.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileState {
    pub len: u64,
    /// blake3, hex.
    pub hash: String,
}

/// Every syncable file in a vault, keyed by forward-slash relative path.
///
/// A `BTreeMap` rather than a `HashMap` so a plan comes out in a stable order.
/// A sync that lists its work differently on each run is one nobody can read a
/// diff of, and the tests would have to sort before every assertion.
pub type Manifest = BTreeMap<String, FileState>;

/// Take a manifest of `root`.
///
/// **`.arc/` is excluded.** The index is derived and rebuilt locally, the trash
/// is per-machine undo state, and the ledger and object store are append-only
/// and content-addressed — they merge instead of reconciling, so putting them
/// through the three-way plan would invent conflicts that cannot exist. They
/// travel by their own routes; see `ledger` and `objects`.
pub fn of(root: &Path) -> Result<Manifest> {
    let mut out = Manifest::new();

    let walk = walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        // `.arc` is pruned rather than filtered per-entry, so a large object
        // store is never walked at all.
        .filter_entry(|e| e.file_name() != ".arc");

    for entry in walk {
        let entry = entry.map_err(|e| SyncError::Walk(e.to_string()))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let abs = entry.path();
        let Some(rel) = relative(root, abs) else {
            continue;
        };
        let bytes = std::fs::read(abs).map_err(|e| SyncError::io(abs, e))?;
        out.insert(
            rel,
            FileState {
                len: bytes.len() as u64,
                hash: blake3::hash(&bytes).to_hex().to_string(),
            },
        );
    }
    Ok(out)
}

/// A vault-relative path with forward slashes, on every platform.
///
/// A manifest crosses machines, so `Notes\A.md` from Windows and `Notes/A.md`
/// from Linux have to be the same key — otherwise every file in the vault looks
/// like it was created on one side and deleted on the other, and the first sync
/// between a laptop and the hub duplicates the entire vault.
fn relative(root: &Path, abs: &Path) -> Option<String> {
    let rel = abs.strip_prefix(root).ok()?;
    let mut parts = Vec::new();
    for part in rel.components() {
        parts.push(part.as_os_str().to_str()?.to_string());
    }
    Some(parts.join("/")).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault(files: &[(&str, &[u8])]) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        for (name, body) in files {
            let p = tmp.path().join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, body).unwrap();
        }
        tmp
    }

    #[test]
    fn a_manifest_names_every_file_by_hash_and_size() {
        let t = vault(&[("A.md", b"# A\n"), ("Notes/B.md", b"# B\n")]);
        let m = of(t.path()).unwrap();

        assert_eq!(m.len(), 2);
        assert_eq!(m["A.md"].len, 4);
        assert_eq!(m["A.md"].hash, blake3::hash(b"# A\n").to_hex().to_string());
        assert!(m.contains_key("Notes/B.md"));
    }

    /// The key that crosses machines has to be spelled the same on both.
    #[test]
    fn nested_paths_use_forward_slashes_on_every_platform() {
        let t = vault(&[("a/b/c/D.md", b"deep\n")]);
        let m = of(t.path()).unwrap();

        assert!(
            m.contains_key("a/b/c/D.md"),
            "got {:?}",
            m.keys().collect::<Vec<_>>()
        );
        assert!(
            !m.keys().any(|k| k.contains('\\')),
            "a backslash key would make every note look new on the other side"
        );
    }

    /// `.arc/` merges rather than reconciles, so it must not appear here.
    #[test]
    fn the_arc_directory_is_not_in_the_manifest() {
        let t = vault(&[
            ("A.md", b"# A\n"),
            (".arc/index.db", b"derived\n"),
            (".arc/ledger/abc.jsonl", b"{}\n"),
            (".arc/objects/ab/cdef", b"content\n"),
            (".arc/trash/ab/123-A.md", b"deleted\n"),
        ]);
        let m = of(t.path()).unwrap();

        assert_eq!(m.keys().collect::<Vec<_>>(), vec!["A.md"]);
    }

    #[test]
    fn identical_bytes_hash_identically_regardless_of_name() {
        let t = vault(&[("One.md", b"same\n"), ("Two.md", b"same\n")]);
        let m = of(t.path()).unwrap();
        assert_eq!(m["One.md"], m["Two.md"]);
    }

    #[test]
    fn an_empty_vault_is_an_empty_manifest_not_an_error() {
        let t = tempfile::tempdir().unwrap();
        assert!(of(t.path()).unwrap().is_empty());
    }

    /// Zero-byte notes are real — the Phase 0 fixture has two — and a manifest
    /// that skipped them would delete them on the other machine.
    #[test]
    fn a_zero_byte_note_is_still_a_file() {
        let t = vault(&[("Empty.md", b"")]);
        let m = of(t.path()).unwrap();
        assert_eq!(m["Empty.md"].len, 0);
    }
}
