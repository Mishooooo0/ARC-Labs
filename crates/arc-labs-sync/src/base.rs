//! The state both sides last agreed on, kept between passes.
//!
//! [`plan`](crate::plan) needs a base to tell "I made this" from "they deleted
//! this", and a base only exists if the previous pass wrote one down. This is
//! that file: `.arc/sync/base.json`, per hub, alongside the index and the
//! ledger in the derived-state directory.
//!
//! ## Losing it is safe, and that is deliberate
//!
//! A missing or unreadable base is treated as an empty one, which makes every
//! differing path a conflict rather than a guess. Noisy, and never wrong: with
//! no record of what was agreed, nothing justifies choosing one side. This is
//! why the file is read with a fallback rather than a `?` — a corrupt base must
//! degrade to caution, not to a sync that refuses to run at all.
//!
//! ## Written only after the pass succeeded
//!
//! The base is a claim that both sides really do hold this state. Writing it
//! before the transfer, or after a partial one, would record an agreement that
//! never happened — and the next pass would take that fiction as ground truth
//! and skip the files that never arrived. It is the last thing a pass does.

use std::path::{Path, PathBuf};

use crate::manifest::Manifest;
use crate::{Result, SyncError};

/// Where the base for `hub` lives inside a vault.
///
/// Keyed by hub so a vault can sync to more than one without either forgetting
/// what the other agreed. The key is hashed rather than used raw because a URL
/// contains `/` and `:`, neither of which belongs in a filename.
pub fn path_for(vault: &Path, hub: &str) -> PathBuf {
    let key = blake3::hash(hub.trim().trim_end_matches('/').as_bytes()).to_hex();
    vault
        .join(".arc")
        .join("sync")
        .join(format!("{}.json", &key[..16]))
}

/// Read the last agreed state. A first run, a missing file or an unreadable one
/// all give an empty manifest — see the module note on why that is the safe
/// answer rather than an error.
pub fn load(vault: &Path, hub: &str) -> Manifest {
    let path = path_for(vault, hub);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Manifest::new();
    };
    match serde_json::from_str(&text) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "the sync base is unreadable; treating this as a first sync, so \
                 anything that differs will be raised as a conflict rather than guessed"
            );
            Manifest::new()
        }
    }
}

/// Record a new agreed state. Call this **only** after a pass fully succeeded.
pub fn save(vault: &Path, hub: &str, manifest: &Manifest) -> Result<()> {
    let path = path_for(vault, hub);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| SyncError::io(dir, e))?;
    }

    // Written beside and renamed, like every other write in this product: a
    // crash mid-write must not leave a half-parsed base, because the fallback
    // for that is "conflict on everything" and the user would never know why.
    let tmp = path.with_extension("json.tmp");
    let text =
        serde_json::to_string_pretty(manifest).map_err(|e| SyncError::Walk(e.to_string()))?;
    std::fs::write(&tmp, text.as_bytes()).map_err(|e| SyncError::io(&tmp, e))?;
    std::fs::rename(&tmp, &path).map_err(|e| SyncError::io(&path, e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::FileState;

    fn manifest(entries: &[(&str, &str)]) -> Manifest {
        entries
            .iter()
            .map(|(p, tag)| {
                (
                    (*p).to_string(),
                    FileState {
                        len: tag.len() as u64,
                        hash: blake3::hash(tag.as_bytes()).to_hex().to_string(),
                    },
                )
            })
            .collect()
    }

    #[test]
    fn a_base_round_trips() {
        let t = tempfile::tempdir().unwrap();
        let m = manifest(&[("A.md", "v1"), ("Notes/B.md", "v2")]);

        save(t.path(), "https://hub.example", &m).unwrap();
        assert_eq!(load(t.path(), "https://hub.example"), m);
    }

    #[test]
    fn a_first_run_has_no_base_and_that_is_not_an_error() {
        let t = tempfile::tempdir().unwrap();
        assert!(load(t.path(), "https://hub.example").is_empty());
    }

    /// Corruption must degrade to caution, never to a refusal to sync.
    #[test]
    fn an_unreadable_base_reads_as_empty() {
        let t = tempfile::tempdir().unwrap();
        let p = path_for(t.path(), "https://hub.example");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"{ this is not json").unwrap();

        assert!(load(t.path(), "https://hub.example").is_empty());
    }

    /// Two hubs must not share one memory of what was agreed.
    #[test]
    fn each_hub_gets_its_own_base() {
        let t = tempfile::tempdir().unwrap();
        let a = manifest(&[("A.md", "v1")]);
        let b = manifest(&[("B.md", "v2")]);

        save(t.path(), "https://one.example", &a).unwrap();
        save(t.path(), "https://two.example", &b).unwrap();

        assert_eq!(load(t.path(), "https://one.example"), a);
        assert_eq!(load(t.path(), "https://two.example"), b);
    }

    /// A trailing slash is the same hub. Otherwise typing the URL slightly
    /// differently in Settings silently starts the whole sync over.
    #[test]
    fn a_trailing_slash_is_the_same_hub() {
        let t = tempfile::tempdir().unwrap();
        let m = manifest(&[("A.md", "v1")]);

        save(t.path(), "https://hub.example", &m).unwrap();
        assert_eq!(load(t.path(), "https://hub.example/"), m);
        assert_eq!(load(t.path(), "  https://hub.example  "), m);
    }

    #[test]
    fn saving_twice_replaces_rather_than_appends() {
        let t = tempfile::tempdir().unwrap();
        save(t.path(), "h", &manifest(&[("A.md", "v1")])).unwrap();
        let second = manifest(&[("B.md", "v2")]);
        save(t.path(), "h", &second).unwrap();

        assert_eq!(load(t.path(), "h"), second);
        // And no temp file is left behind.
        let dir = t.path().join(".arc").join("sync");
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "tmp"))
            .collect();
        assert!(leftovers.is_empty(), "a .tmp file survived the write");
    }
}
