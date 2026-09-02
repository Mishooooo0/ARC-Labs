//! Content-addressed storage for every version a note has had.
//!
//! # Why content, and not patches
//!
//! The obvious design is to store a diff per entry and replay them to restore.
//! It is also the one that fails the gate. Patch application is *fuzzy*: it
//! depends on context lines matching, on line endings, and on the file being in
//! the state the patch expected. "Restore to state #17 exactly" then becomes
//! "restore to state #17 unless something drifted", which is not the same
//! promise at all.
//!
//! Storing content addressed by its own hash makes restore a lookup. It cannot
//! drift, cannot half-apply, and cannot silently produce something almost right.
//! The diff in the ledger stays, for a human to read; it is never the mechanism.
//!
//! # Why it is not as expensive as it sounds
//!
//! Content is keyed by hash, so identical states are stored once no matter how
//! many notes or how many entries reach them. Typing a character and undoing it
//! costs nothing. A note edited fifty times stores fifty versions — but a 4 KB
//! note is 200 KB, and the whole store lives under `.arc/`, which is derived-ish
//! and gitignored.
//!
//! Two-character directory fanout, like git: a hundred thousand objects in one
//! directory is slow on every filesystem, and painful on NTFS in particular.

use std::path::{Path, PathBuf};

use crate::{LedgerError, Result};

pub struct ObjectStore {
    root: PathBuf,
}

impl ObjectStore {
    pub fn new(arc_dir: &Path) -> ObjectStore {
        ObjectStore { root: arc_dir.join("objects") }
    }

    /// `blake3:abcdef…` -> `<root>/ab/cdef…`
    fn path_for(&self, hash: &str) -> Result<PathBuf> {
        let hex = hash.strip_prefix("blake3:").unwrap_or(hash);
        if hex.len() < 4 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(LedgerError::BadHash(hash.to_string()));
        }
        Ok(self.root.join(&hex[..2]).join(&hex[2..]))
    }

    /// Store `content`, returning its hash. Idempotent.
    pub fn put(&self, content: &str) -> Result<String> {
        let hash = hash_of(content);
        let path = self.path_for(&hash)?;

        // Already stored. Content addressing means identical bytes are the same
        // object, so this is the common case for an undo or a revert.
        if path.exists() {
            return Ok(hash);
        }

        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| LedgerError::io(dir, e))?;
        }
        // Written atomically: a half-written object would be a version that
        // hashes to something it is not, and restore would hand back corruption
        // while believing it had verified the content.
        arc_labs_core::atomic::replace(&path, content.as_bytes())
            .map_err(|e| LedgerError::Core(Box::new(e)))?;
        Ok(hash)
    }

    /// Retrieve content by hash, verifying it on the way out.
    ///
    /// The verification is not paranoia: it is what makes a restore trustworthy.
    /// If the file on disk no longer hashes to its own name, something outside
    /// this program changed it, and handing it back as though it were the
    /// recorded version would be worse than failing.
    pub fn get(&self, hash: &str) -> Result<String> {
        let path = self.path_for(hash)?;
        let bytes = std::fs::read(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => LedgerError::MissingObject(hash.to_string()),
            _ => LedgerError::io(&path, e),
        })?;
        let content =
            String::from_utf8(bytes).map_err(|_| LedgerError::CorruptObject(hash.to_string()))?;

        if hash_of(&content) != normalise(hash) {
            return Err(LedgerError::CorruptObject(hash.to_string()));
        }
        Ok(content)
    }

    pub fn contains(&self, hash: &str) -> bool {
        self.path_for(hash).map(|p| p.exists()).unwrap_or(false)
    }

    /// Total bytes held. For `doctor`, and for deciding when pruning is worth it.
    pub fn size_on_disk(&self) -> u64 {
        fn walk(dir: &Path) -> u64 {
            let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
            entries
                .flatten()
                .map(|e| match e.file_type() {
                    Ok(t) if t.is_dir() => walk(&e.path()),
                    Ok(_) => e.metadata().map(|m| m.len()).unwrap_or(0),
                    Err(_) => 0,
                })
                .sum()
        }
        walk(&self.root)
    }
}

pub fn hash_of(content: &str) -> String {
    format!("blake3:{}", blake3::hash(content.as_bytes()).to_hex())
}

fn normalise(hash: &str) -> String {
    if hash.starts_with("blake3:") {
        hash.to_string()
    } else {
        format!("blake3:{hash}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, ObjectStore) {
        let tmp = tempfile::tempdir().unwrap();
        let s = ObjectStore::new(tmp.path());
        (tmp, s)
    }

    #[test]
    fn round_trips_content() {
        let (_t, s) = store();
        let h = s.put("# A note\n\nwith content\n").unwrap();
        assert_eq!(s.get(&h).unwrap(), "# A note\n\nwith content\n");
        assert!(s.contains(&h));
    }

    #[test]
    fn identical_content_is_stored_once() {
        let (_t, s) = store();
        let a = s.put("same").unwrap();
        let b = s.put("same").unwrap();
        assert_eq!(a, b);
        // An undo returns to a state already stored, so it costs nothing.
        assert_eq!(s.size_on_disk(), 4);
    }

    #[test]
    fn handles_the_awkward_content_a_vault_actually_holds() {
        let (_t, s) = store();
        for content in ["", "\n", "no trailing newline", "unicode café ☕ 日本語", "\r\n\r\n"] {
            let h = s.put(content).unwrap();
            assert_eq!(s.get(&h).unwrap(), content, "round trip failed for {content:?}");
        }
    }

    #[test]
    fn a_missing_object_is_reported_not_guessed_at() {
        let (_t, s) = store();
        let err = s.get("blake3:0000000000000000").unwrap_err();
        assert!(matches!(err, LedgerError::MissingObject(_)));
    }

    #[test]
    fn a_tampered_object_is_refused_rather_than_returned() {
        // The property that makes a restore trustworthy: if the stored bytes no
        // longer hash to their own name, something outside this program changed
        // them, and returning them as the recorded version would be worse than
        // failing.
        let (_t, s) = store();
        let h = s.put("original content").unwrap();

        let hex = h.strip_prefix("blake3:").unwrap();
        let path = s.root.join(&hex[..2]).join(&hex[2..]);
        std::fs::write(&path, b"tampered content").unwrap();

        assert!(matches!(s.get(&h), Err(LedgerError::CorruptObject(_))));
    }

    #[test]
    fn a_malformed_hash_is_rejected_before_touching_the_filesystem() {
        let (_t, s) = store();
        // Each of these would be a path-traversal primitive if the hash were
        // used to build a path without checking.
        for bad in ["", "ab", "../../etc/passwd", "blake3:../../x", "zz zz", "blake3:g0g0g0g0"] {
            assert!(matches!(s.get(bad), Err(LedgerError::BadHash(_))), "accepted {bad:?}");
        }
    }

    #[test]
    fn objects_fan_out_into_subdirectories() {
        let (_t, s) = store();
        let h = s.put("x").unwrap();
        let hex = h.strip_prefix("blake3:").unwrap();
        assert!(s.root.join(&hex[..2]).is_dir(), "expected two-character fanout");
    }
}
