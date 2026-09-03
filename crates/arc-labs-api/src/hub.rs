//! The hub half of sync: what an always-on vault answers to the machines
//! syncing against it.
//!
//! There is no separate hub binary. A hub is this program, with a vault open,
//! told that other machines sync to it — so it is the same engine, the same
//! ledger and the same guarantees as the desktop app, and a bug fixed in one is
//! fixed in both. That is the same reason the four shells are thin wrappers
//! over this crate.
//!
//! ## Files move one at a time
//!
//! Not batched into one archive. A batch that fails halfway leaves the caller
//! guessing which half landed, and the natural fix — staging the whole vault
//! before swapping — means holding a second copy of someone's notebook in
//! memory. One file per request is slower on a first sync and unambiguous
//! always, and the client's plan already lists exactly which ones to ask for.
//!
//! ## The generation, and why a counter and not a hash
//!
//! Two clients pushing at once must not interleave into a state neither of them
//! planned against. So a manifest comes stamped with a generation, a push
//! quotes the one it planned against, and a push quoting a stale one is refused
//! rather than applied — the same optimistic-concurrency shape as the
//! content-hash check that already guards `save_note`.
//!
//! It is a counter and not a hash of the manifest because a hash costs a full
//! vault walk on every single request, and this is on the path of every file in
//! a first sync. The counter is seeded randomly at startup, so a generation from
//! a previous process cannot be mistaken for a current one after a restart.

use std::sync::atomic::Ordering;

use arc_labs_core::VaultPath;

use crate::{ApiError, ApiResult, ErrorCode};

/// A hub's view of its vault, and the state it was in when it looked.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HubManifest {
    /// Opaque to the client. It quotes this back on a push; it never parses it.
    pub generation: String,
    pub files: arc_labs_sync::Manifest,
}

impl crate::Api {
    /// The current generation stamp.
    fn generation(&self) -> String {
        format!(
            "{}-{}",
            self.session,
            self.mutations.load(Ordering::Relaxed)
        )
    }

    /// Refuse a write planned against a vault that has since moved.
    ///
    /// `None` means the caller did not quote one, which is allowed: a first
    /// push, a script, or a client that predates this check. It is not silently
    /// unsafe — it is the same exposure as any client that does not read before
    /// writing, and refusing it outright would break the simplest useful case.
    fn check_generation(&self, quoted: Option<&str>) -> ApiResult<()> {
        let Some(quoted) = quoted.map(str::trim).filter(|q| !q.is_empty()) else {
            return Ok(());
        };
        let now = self.generation();
        if quoted == now {
            return Ok(());
        }
        Err(ApiError::new(
            ErrorCode::Conflict,
            format!(
                "the vault moved since this was planned (planned against {quoted}, now {now}); \
                 ask for the manifest again and re-plan"
            ),
        ))
    }

    /// What this hub holds, and the generation it held it at.
    pub fn hub_manifest(&self) -> ApiResult<HubManifest> {
        let root = self.with_vault(|v| Ok(v.root().path().to_path_buf()))?;
        let files = arc_labs_sync::manifest::of(&root).map_err(sync_err)?;
        Ok(HubManifest {
            // Read *after* the walk. A generation taken first could be stamped
            // on a manifest that already reflects a later write, which is the
            // one direction of error that lets a stale push through.
            generation: self.generation(),
            files,
        })
    }

    /// Raw bytes of one file.
    ///
    /// Bytes rather than text: a vault holds images and PDFs alongside its
    /// notes, and a sync that could only carry UTF-8 would quietly drop them.
    pub fn hub_read(&self, path: &VaultPath) -> ApiResult<Vec<u8>> {
        self.with_vault(|v| Ok(v.read_bytes(path)?))
    }

    /// Write one file, creating or replacing it.
    ///
    /// This is the one place in the product where an incoming write replaces
    /// existing content without a hash check on that specific file, and it is
    /// sound only because of what happened before it: the client planned
    /// against a manifest, this path came back as "only one side moved", and
    /// `generation` proves the vault has not moved since. A path where both
    /// sides moved never reaches here — it became a conflict and a person
    /// decides.
    ///
    /// ## No ledger entry, and that is the point
    ///
    /// Every other write in this product records who made it. This one does
    /// not, because it did not make a change — it is applying one that already
    /// happened somewhere else, and that change already has a ledger entry on
    /// the machine where it happened. The entry arrives by its own route, in
    /// `hub_merge_ledger`, carrying the actor, timestamp and reason the *real*
    /// author wrote.
    ///
    /// Synthesising one here would file someone else's edit under the hub's
    /// name, and then the merge would deliver the true entry alongside it: two
    /// records of one change, one of them wrong. A ledger that logs things
    /// inaccurately is worse than one that stays silent about what it does not
    /// itself know — the same reasoning that keeps drafting out of the egress
    /// log in `draft.rs`.
    /// Returns the generation the vault is at **after** this write.
    ///
    /// A write of its own bumps the generation, so a client pushing several
    /// files would invalidate itself between the first and the second and get a
    /// 409 for everything after file one. Handing the new value back lets the
    /// caller carry it forward, which is the same thing an ETag does and the
    /// reason this is not simply re-read: re-reading cannot tell "my write" from
    /// "someone else's", and that distinction is the whole point of the check.
    pub fn hub_write(
        &self,
        path: &VaultPath,
        bytes: &[u8],
        generation: Option<&str>,
    ) -> ApiResult<String> {
        self.check_generation(generation)?;
        self.with_vault(|v| Ok(v.write_bytes(path, bytes)?))?;
        let _ = self.reindex_note(path);
        self.publish(crate::EventKind::Edited, Some(path), None);
        Ok(self.generation())
    }

    /// Delete one file, on behalf of a machine that deleted it.
    ///
    /// The bytes still go to the trash — a deletion arriving over the wire
    /// deserves the same second chance as one made here, and it is the case
    /// where a mistake is *most* likely, because the person who made it was not
    /// looking at this machine.
    ///
    /// But it is deliberately **not** `delete_note`, for the same reason
    /// `hub_write` writes no entry: the deletion already happened elsewhere and
    /// already has a ledger entry there. Recording a second one attributed to
    /// the hub would put a change in the log that the hub did not make, and the
    /// merge would then deliver the true one beside it.
    pub fn hub_delete(&self, path: &VaultPath, generation: Option<&str>) -> ApiResult<String> {
        self.check_generation(generation)?;

        match self.with_vault(|v| Ok(v.delete_note(path)?)) {
            Ok(_) => {}
            // Already gone is the goal, not a failure. Two clients that both
            // deleted the same note must not turn the second one into an error.
            Err(e) if e.code == ErrorCode::NoteNotFound => return Ok(self.generation()),
            Err(e) => return Err(e),
        }

        self.forget_indexed(path);
        let days = self.config().trash.retention_days;
        let _ = self.with_vault(|v| Ok(v.purge_trash(days, crate::now_secs())));
        self.publish(crate::EventKind::Deleted, Some(path), None);
        Ok(self.generation())
    }

    /// Which of these object hashes this hub does not have.
    ///
    /// Asked before sending, so a sync moves only content the other side is
    /// genuinely missing. Objects are content-addressed, so this is a set
    /// question with no ambiguity — and it is why the object store costs almost
    /// nothing to keep in step even though it is the largest thing travelling.
    pub fn hub_missing_objects(&self, hashes: &[String]) -> ApiResult<Vec<String>> {
        let ledger = self.ledger()?;
        Ok(hashes
            .iter()
            .filter(|h| !ledger.objects().contains(h))
            .cloned()
            .collect())
    }

    /// Read one object by hash.
    pub fn hub_read_object(&self, hash: &str) -> ApiResult<String> {
        self.ledger()?
            .objects()
            .get(hash)
            .map_err(crate::ledger_err)
    }

    /// Store one object.
    ///
    /// No generation check, and none is possible or needed: the store is keyed
    /// by the hash of the content, so writing an object either puts back exactly
    /// what is already there or adds one nothing yet refers to. There is no
    /// state to race.
    pub fn hub_write_object(&self, content: &str) -> ApiResult<String> {
        self.ledger()?
            .objects()
            .put(content)
            .map_err(crate::ledger_err)
    }

    /// Every ledger this hub holds, by key.
    ///
    /// **By key, not by path.** A ledger file is named after a hash of its
    /// note's path, so a path cannot be recovered from it — but both machines
    /// derive the same key from the same path, so two copies of one history
    /// line up without anyone needing to. It also covers what a manifest
    /// cannot: a deleted note still has a history worth carrying, and its path
    /// is in nobody's file list.
    pub fn hub_ledger_keys(&self) -> ApiResult<Vec<String>> {
        self.ledger()?.keys().map_err(crate::ledger_err)
    }

    /// One history, verbatim.
    pub fn hub_read_ledger(&self, key: &str) -> ApiResult<String> {
        self.ledger()?.raw_by_key(key).map_err(crate::ledger_err)
    }

    /// Merge incoming history into the one held here. Returns lines gained.
    ///
    /// A union, so this cannot lose an entry and cannot conflict — see
    /// `arc_labs_sync::ledger`. Deliberately **not** guarded by the generation:
    /// a merge is safe against any concurrent state, and refusing history
    /// because the note changed while it was in flight would drop the one thing
    /// that merges cleanly no matter what else happened.
    pub fn hub_merge_ledger(&self, key: &str, incoming: &str) -> ApiResult<usize> {
        let ledger = self.ledger()?;
        let ours = ledger.raw_by_key(key).map_err(crate::ledger_err)?;
        let merged = arc_labs_sync::ledger::merge(&ours, incoming);
        if merged == ours {
            return Ok(0);
        }
        let added = merged.lines().count().saturating_sub(ours.lines().count());
        ledger
            .replace_by_key(key, &merged)
            .map_err(crate::ledger_err)?;
        Ok(added)
    }
}

fn sync_err(e: arc_labs_sync::SyncError) -> ApiError {
    ApiError::new(ErrorCode::Io, e.to_string())
}

#[cfg(test)]
mod tests {
    use crate::Capabilities;
    use arc_labs_core::{Config, VaultPath};

    fn hub() -> (tempfile::TempDir, crate::Api) {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("A.md"), b"# A\n").unwrap();
        let api = crate::Api::new(Config::default(), None, Capabilities::desktop());
        api.open_vault(tmp.path()).unwrap();
        (tmp, api)
    }

    fn vp(s: &str) -> VaultPath {
        VaultPath::new(s).unwrap()
    }

    #[test]
    fn a_manifest_carries_the_files_and_a_generation() {
        let (_t, api) = hub();
        let m = api.hub_manifest().unwrap();

        assert!(m.files.contains_key("A.md"));
        assert!(!m.generation.is_empty());
        // `.arc` is derived or merges; it must never appear in a file manifest.
        assert!(!m.files.keys().any(|k| k.starts_with(".arc")));
    }

    /// Two clients planning against the same state, one of them landing first.
    /// The second must be told to re-plan rather than applying a change it
    /// worked out against a vault that no longer exists.
    #[test]
    fn a_push_planned_against_a_stale_vault_is_refused() {
        let (_t, api) = hub();
        let planned = api.hub_manifest().unwrap().generation;

        // Somebody else got there first.
        api.hub_write(&vp("Other.md"), b"# Other\n", None).unwrap();

        let err = api
            .hub_write(&vp("Mine.md"), b"# Mine\n", Some(&planned))
            .unwrap_err();
        assert_eq!(err.code, crate::ErrorCode::Conflict);
        assert!(err.message.contains("re-plan"), "got {}", err.message);

        // And the refused write did not happen.
        assert!(api.hub_read(&vp("Mine.md")).is_err());
    }

    #[test]
    fn a_push_quoting_the_current_generation_lands() {
        let (_t, api) = hub();
        let gen = api.hub_manifest().unwrap().generation;

        api.hub_write(&vp("New.md"), b"# New\n", Some(&gen))
            .unwrap();
        assert_eq!(api.hub_read(&vp("New.md")).unwrap(), b"# New\n");
    }

    /// The provenance rule. A hub applying someone else's change must not sign
    /// it — the true entry arrives by the ledger merge, and a synthetic one
    /// would be a second record of the same change, attributed to the wrong
    /// machine.
    #[test]
    fn applying_a_change_does_not_forge_a_ledger_entry() {
        let (_t, api) = hub();

        api.hub_write(&vp("Synced.md"), b"# Synced\n", None)
            .unwrap();
        assert!(
            api.timeline(&vp("Synced.md")).unwrap().is_empty(),
            "the hub must not claim authorship of a change made elsewhere"
        );

        api.hub_delete(&vp("Synced.md"), None).unwrap();
        assert!(
            api.timeline(&vp("Synced.md")).unwrap().is_empty(),
            "nor of a deletion made elsewhere"
        );
    }

    /// A deletion arriving over the wire is the one most likely to be a
    /// mistake, because the person who made it was not looking at this machine.
    #[test]
    fn a_synced_delete_still_leaves_the_bytes_in_the_trash() {
        let (t, api) = hub();
        api.hub_delete(&vp("A.md"), None).unwrap();

        assert!(
            api.hub_read(&vp("A.md")).is_err(),
            "the note should be gone"
        );
        let trash = t.path().join(".arc").join("trash");
        let copies: Vec<_> = walk(&trash);
        assert_eq!(copies.len(), 1, "the bytes should be recoverable");
        assert_eq!(std::fs::read(&copies[0]).unwrap(), b"# A\n");
    }

    fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                if e.path().is_dir() {
                    out.extend(walk(&e.path()));
                } else {
                    out.push(e.path());
                }
            }
        }
        out
    }

    /// Two machines both deleting the same note is agreement, not an error.
    #[test]
    fn deleting_something_already_gone_is_success() {
        let (_t, api) = hub();
        api.hub_delete(&vp("A.md"), None).unwrap();
        api.hub_delete(&vp("A.md"), None).unwrap();
    }

    #[test]
    fn only_genuinely_missing_objects_are_asked_for() {
        let (_t, api) = hub();
        let have = api.hub_write_object("stored content").unwrap();
        let absent = "blake3:".to_string() + &"0".repeat(64);

        let missing = api
            .hub_missing_objects(&[have.clone(), absent.clone()])
            .unwrap();
        assert_eq!(missing, vec![absent]);
        assert_eq!(api.hub_read_object(&have).unwrap(), "stored content");
    }

    /// A ledger key becomes a filename, and it arrives over the network.
    #[test]
    fn a_ledger_key_that_is_not_a_key_is_refused_before_it_becomes_a_path() {
        let (_t, api) = hub();
        for attack in ["../../../etc/passwd", "..", "", "not-hex", "/etc/shadow"] {
            let err = api.hub_read_ledger(attack).unwrap_err();
            assert_eq!(
                err.code,
                crate::ErrorCode::InvalidPath,
                "{attack:?} should be refused as a bad key, got {err:?}"
            );
        }
    }

    /// History merges rather than reconciles, so a merge can only ever add.
    #[test]
    fn merging_history_adds_and_never_removes() {
        let (_t, api) = hub();
        let p = vp("Hist.md");
        api.create_note(&p, "# Hist\n").unwrap();

        let key = arc_labs_ledger::Ledger::key_for(&p);
        let ours = api.hub_read_ledger(&key).unwrap();
        assert_eq!(ours.lines().count(), 1);

        let theirs = r#"{"ts":"2020-01-01T00:00:00Z","actor":{"kind":"human","id":"laptop"},"op":"edit","reason":"from another machine"}"#;
        let added = api.hub_merge_ledger(&key, theirs).unwrap();
        assert_eq!(added, 1);

        let merged = api.hub_read_ledger(&key).unwrap();
        assert_eq!(merged.lines().count(), 2);
        assert!(merged.contains("from another machine"));
        assert!(merged.contains("created"), "our own entry must survive");

        // Merging the same history again changes nothing.
        assert_eq!(api.hub_merge_ledger(&key, theirs).unwrap(), 0);
    }
}
