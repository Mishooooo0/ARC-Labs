//! The client half of sync: what a machine does to keep itself in step with a
//! hub, and the endpoint that asks it to.
//!
//! `arc-labs-sync` decides and moves; this connects it to a real vault. The
//! [`Vault`](arc_labs_sync::pass::Local) implementation here is deliberately
//! thin, and every write goes through the ordinary `Api` path rather than
//! straight to disk — so an incoming note is indexed, announced to open windows
//! and trashed on delete exactly like one typed by hand. A feature with its own
//! private door into the vault is how the index goes stale and the UI stops
//! matching the disk.
//!
//! ## Applying an incoming change writes no ledger entry
//!
//! The same rule as the hub side, for the same reason: the change already
//! happened on another machine and already has an entry there, which arrives
//! through the ledger merge carrying its real author. Writing a second one here
//! would file someone else's edit under this machine's name.

use std::path::{Path, PathBuf};

use arc_labs_core::VaultPath;
use arc_labs_sync::client::Hub;
use arc_labs_sync::pass::{self, SyncReport};

use crate::{ApiError, ApiResult, ErrorCode};

/// Where a client stands with its hub.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    /// `standalone`, `client` or `hub`.
    pub role: String,
    pub hub: String,
    /// Whether a token is actually available, without ever saying what it is.
    pub has_token: bool,
    /// `manual`, `daily`, `weekly` or `monthly`.
    pub cadence: String,
    pub hour: u32,
    pub minute: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_sync_at: Option<String>,
    /// Whether this instance is set up well enough to sync at all.
    pub ready: bool,
    /// Why not, when it is not. Plain enough to act on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked: Option<String>,
}

impl crate::Api {
    /// Where this machine stands. Never includes the token itself.
    pub fn sync_status(&self) -> SyncStatus {
        let config = self.config();
        let s = &config.sync;
        let role = config.resolved_role();
        let has_token = s.token().is_some();

        let blocked = match role {
            arc_labs_core::Role::Hub => {
                Some("this instance is a hub; other machines sync to it".to_string())
            }
            arc_labs_core::Role::Standalone => {
                Some("this vault is on disk only. Connect it to a vault server to sync.".into())
            }
            arc_labs_core::Role::Client if s.hub.trim().is_empty() => {
                Some("no vault server is set".into())
            }
            arc_labs_core::Role::Client if !has_token => Some(format!(
                "no token: set {} in the environment this app starts in",
                s.token_env
            )),
            arc_labs_core::Role::Client => None,
        };

        SyncStatus {
            role: match role {
                arc_labs_core::Role::Standalone => "standalone",
                arc_labs_core::Role::Client => "client",
                arc_labs_core::Role::Hub => "hub",
            }
            .into(),
            hub: s.hub.clone(),
            has_token,
            cadence: match s.cadence {
                arc_labs_core::Cadence::Manual => "manual",
                arc_labs_core::Cadence::Daily => "daily",
                arc_labs_core::Cadence::Weekly => "weekly",
                arc_labs_core::Cadence::Monthly => "monthly",
            }
            .into(),
            hour: s.hour,
            minute: s.minute,
            last_sync_at: s.last_sync_at.clone(),
            ready: blocked.is_none(),
            blocked,
        }
    }

    /// The hub this vault syncs to, if it is set up to.
    fn hub(&self) -> ApiResult<Hub> {
        let config = self.config();
        if config.resolved_role() != arc_labs_core::Role::Client {
            return Err(ApiError::new(
                ErrorCode::NotPermitted,
                "this vault is not connected to a vault server",
            ));
        }
        let url = config.sync.hub.trim();
        if url.is_empty() {
            return Err(ApiError::new(ErrorCode::Config, "no vault server is set"));
        }
        let token = config.sync.token().ok_or_else(|| {
            ApiError::new(
                ErrorCode::Config,
                format!(
                    "no token: set {} in the environment this app starts in",
                    config.sync.token_env
                ),
            )
        })?;
        Ok(Hub::new(url, &token))
    }

    /// What a sync would do, changing nothing.
    ///
    /// Worth having as its own operation rather than a flag on `sync_now`: the
    /// honest way to offer "sync" to someone who has not done it before is to
    /// show them what it will move first, and a preview that shared a code path
    /// with the real thing would eventually stop being a preview.
    pub fn sync_preview(&self) -> ApiResult<Vec<PreviewItem>> {
        let hub = self.hub()?;
        let local = self.local_side()?;
        let actions = pass::preview(&local, &hub).map_err(sync_err)?;
        Ok(actions.iter().map(PreviewItem::from_action).collect())
    }

    /// Run a pass now.
    pub fn sync_now(&self) -> ApiResult<SyncReport> {
        let hub = self.hub()?;
        let local = self.local_side()?;
        let report = pass::run(&local, &hub).map_err(sync_err)?;

        // Only a pass that could honestly record what both sides hold advances
        // the clock. A partial one leaves `last_sync_at` alone so the next
        // scheduled window comes round again rather than being skipped.
        if report.clean() {
            self.mark_synced()?;
        }
        self.publish_synced();
        Ok(report)
    }

    /// Settle one conflict, by choosing which side wins.
    ///
    /// `keep_local` sends this machine's copy up; otherwise the hub's comes
    /// down. Nothing is merged — there is no automatic reconciliation of two
    /// people's prose here and there should not be.
    ///
    /// ## The version that loses is written down first
    ///
    /// Resolving destroys a real state of a real note, and a notebook whose
    /// whole premise is that no version is lost cannot quietly drop one because
    /// someone clicked a button. So before anything is overwritten, the losing
    /// content is recorded as its own timeline entry — restoring to it gives
    /// that version back exactly.
    ///
    /// This is deliberately unlike `hub_write`, which records nothing: that is
    /// applying a change made elsewhere, this is a decision made **here** by the
    /// person standing in front of it. That is precisely what the ledger is for.
    ///
    /// A note that is not valid UTF-8 — an image, a PDF — cannot go through the
    /// object store, so it gets no entry and says so in the log rather than
    /// pretending it was recorded.
    pub fn sync_resolve(&self, path: &VaultPath, keep_local: bool) -> ApiResult<()> {
        let hub = self.hub()?;
        let local = self.local_side()?;
        let p = path.as_str();

        let ours = self.hub_read(path).unwrap_or_default();
        let theirs = hub.read(p).map_err(sync_err)?;
        let (winner, loser) = if keep_local {
            (&ours, &theirs)
        } else {
            (&theirs, &ours)
        };

        self.record_resolution(path, loser, winner, keep_local);

        if keep_local {
            let generation = hub.manifest().map_err(sync_err)?.generation;
            hub.write(p, winner, &generation).map_err(sync_err)?;
        } else {
            pass::Local::write(&local, p, winner)
                .map_err(|e| ApiError::new(ErrorCode::Io, e.to_string()))?;
        }

        // The base is deliberately left alone. The next pass re-reads both
        // sides, finds them agreeing and advances it honestly; writing it here
        // would claim an agreement about every *other* path too, and this call
        // knows nothing about those.
        self.publish_synced();
        Ok(())
    }

    /// Two entries: the version being dropped, then the choice that dropped it.
    ///
    /// Two rather than one because `restore` replays an entry's *after* state.
    /// Recorded as a single entry the loser would only ever be a `before` hash —
    /// present in the object store, but not something the timeline can put back
    /// in one click, which is the whole point of writing it down.
    fn record_resolution(&self, path: &VaultPath, loser: &[u8], winner: &[u8], kept_local: bool) {
        let (Ok(loser), Ok(winner)) = (std::str::from_utf8(loser), std::str::from_utf8(winner))
        else {
            tracing::warn!(
                path = %path,
                "resolving a conflict on a file that is not text: the version not kept                  cannot be recorded in the ledger, and is gone once this is written"
            );
            return;
        };
        if loser == winner {
            return;
        }

        let kept = if kept_local { "mine" } else { "theirs" };
        let record = self.ledger().and_then(|l| {
            l.record_change(
                path,
                self.human(),
                arc_labs_ledger::Op::Edit,
                "the version this sync did not keep, recorded before it was replaced",
                None,
                Some(loser),
            )
            .and_then(|_| {
                l.record_change(
                    path,
                    self.human(),
                    arc_labs_ledger::Op::Edit,
                    format!("resolved a sync conflict, kept {kept}"),
                    Some(loser),
                    Some(winner),
                )
            })
            .map_err(crate::ledger_err)
        });
        if let Err(e) = record {
            tracing::warn!(error = %e.message, path = %path, "could not record the resolution");
        }
    }

    fn mark_synced(&self) -> ApiResult<()> {
        let (mut config, path) = {
            let state = self.state.read().expect("state lock poisoned");
            (state.config.clone(), state.config_path.clone())
        };
        config.sync.last_sync_at = Some(arc_labs_ledger::now_rfc3339());
        if let Some(p) = &path {
            config
                .save(p)
                .map_err(|e| ApiError::new(ErrorCode::Config, e.to_string()))?;
        }
        self.state.write().expect("state lock poisoned").config = config;
        Ok(())
    }

    /// This machine's schedule, or `None` when nothing is scheduled.
    ///
    /// `None` for a standalone vault, a hub, or a client set to sync only when
    /// asked — three different reasons that all mean "the daemon has nothing to
    /// do", which is why they collapse to one answer here.
    pub fn schedule(&self) -> Option<arc_labs_sync::schedule::Schedule> {
        use arc_labs_sync::schedule::{Cadence, Schedule};
        let config = self.config();
        if config.resolved_role() != arc_labs_core::Role::Client {
            return None;
        }
        let cadence = match config.sync.cadence {
            arc_labs_core::Cadence::Manual => return None,
            arc_labs_core::Cadence::Daily => Cadence::Daily,
            arc_labs_core::Cadence::Weekly => Cadence::Weekly,
            arc_labs_core::Cadence::Monthly => Cadence::Monthly,
        };
        Some(Schedule {
            cadence,
            hour: config.sync.hour.min(23),
            minute: config.sync.minute.min(59),
            utc_offset_secs: i64::from(config.sync.utc_offset_minutes) * 60,
        })
    }

    fn local_side(&self) -> ApiResult<VaultSide<'_>> {
        let root = self.with_vault(|v| Ok(v.root().path().to_path_buf()))?;
        Ok(VaultSide { api: self, root })
    }
}

/// One line of a preview.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewItem {
    pub path: String,
    /// `push`, `pull`, `push-delete`, `pull-delete`, or a conflict kind.
    pub action: String,
    pub conflict: bool,
}

impl PreviewItem {
    fn from_action(a: &arc_labs_sync::Action) -> PreviewItem {
        use arc_labs_sync::Action::*;
        let action = match a {
            Push(_) => "push".to_string(),
            Pull(_) => "pull".to_string(),
            PushDelete(_) => "push-delete".to_string(),
            PullDelete(_) => "pull-delete".to_string(),
            Conflict { kind, .. } => kind.name().to_string(),
        };
        PreviewItem {
            path: a.path().to_string(),
            action,
            conflict: a.is_conflict(),
        }
    }
}

/// This vault, as a sync pass sees it.
struct VaultSide<'a> {
    api: &'a crate::Api,
    root: PathBuf,
}

impl VaultSide<'_> {
    fn vp(path: &str) -> arc_labs_sync::Result<VaultPath> {
        // A path from a hub is untrusted input, and `VaultPath` is the thing
        // that has always stood between a crafted name and the filesystem. It
        // gets to do that job here too rather than a second check being written.
        VaultPath::new(path).map_err(|e| arc_labs_sync::SyncError::Wire(e.to_string()))
    }
}

impl pass::Local for VaultSide<'_> {
    fn root(&self) -> &Path {
        &self.root
    }

    fn read(&self, path: &str) -> arc_labs_sync::Result<Vec<u8>> {
        self.api.hub_read(&Self::vp(path)?).map_err(api_err)
    }

    fn write(&self, path: &str, bytes: &[u8]) -> arc_labs_sync::Result<()> {
        // `hub_write` is the right call on this side too: it is "apply a change
        // made elsewhere", which is the same operation whichever end is doing
        // it. No generation, because a local vault has no other writer racing
        // it through this path.
        self.api
            .hub_write(&Self::vp(path)?, bytes, None)
            .map(|_| ())
            .map_err(api_err)
    }

    fn delete(&self, path: &str) -> arc_labs_sync::Result<()> {
        // The generation is the hub's answer to "did anything else change".
        // A local vault has no other writer coming through this path, so there
        // is nothing to carry forward and it is dropped here.
        self.api
            .hub_delete(&Self::vp(path)?, None)
            .map(|_| ())
            .map_err(api_err)
    }

    fn ledger_keys(&self) -> arc_labs_sync::Result<Vec<String>> {
        self.api.hub_ledger_keys().map_err(api_err)
    }

    fn read_ledger(&self, key: &str) -> arc_labs_sync::Result<String> {
        self.api.hub_read_ledger(key).map_err(api_err)
    }

    fn merge_ledger(&self, key: &str, jsonl: &str) -> arc_labs_sync::Result<usize> {
        self.api.hub_merge_ledger(key, jsonl).map_err(api_err)
    }

    fn read_object(&self, hash: &str) -> arc_labs_sync::Result<String> {
        self.api.hub_read_object(hash).map_err(api_err)
    }

    fn write_object(&self, content: &str) -> arc_labs_sync::Result<()> {
        self.api
            .hub_write_object(content)
            .map(|_| ())
            .map_err(api_err)
    }
}

fn api_err(e: ApiError) -> arc_labs_sync::SyncError {
    arc_labs_sync::SyncError::Hub(e.message)
}

fn sync_err(e: arc_labs_sync::SyncError) -> ApiError {
    // The two failures that need different actions get different codes: a hub
    // that cannot be reached is a box to turn on, a hub that said no is usually
    // a setting to change.
    let code = match e {
        arc_labs_sync::SyncError::Unreachable { .. } => ErrorCode::VaultNotFound,
        _ => ErrorCode::Io,
    };
    ApiError::new(code, e.to_string())
}

// ── The daemon ──────────────────────────────────────────────────────────────

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A running sync daemon. Dropping it stops the thread.
///
/// The same shape as [`crate::weave::WeaveDaemon`], deliberately: one background
/// pattern in this product, not two. Stop flag, short sleep slices, `Drop` that
/// joins.
pub struct SyncDaemon {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl SyncDaemon {
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for SyncDaemon {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Start the scheduled-sync daemon.
///
/// Wakes every 30 seconds to ask a cheap question — is a pass due? — and does
/// nothing the rest of the time. That is much more often than any cadence
/// fires, and deliberately: a machine that was asleep through its window should
/// sync shortly after waking, not at the same time tomorrow.
///
/// It sleeps in 250 ms slices so quitting the app does not wait out the poll,
/// which is the difference between a clean exit and something that looks like a
/// hang.
pub fn spawn(api: Arc<crate::Api>) -> SyncDaemon {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();

    let handle = std::thread::Builder::new()
        .name("arc-sync".into())
        .spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                if let Some(schedule) = api.schedule() {
                    let last = api.config().sync.last_sync_at;
                    if schedule.due(last.as_deref(), crate::now_secs() as i64) {
                        match api.sync_now() {
                            Ok(r) => tracing::info!(
                                pushed = r.pushed,
                                pulled = r.pulled,
                                conflicts = r.conflicts.len(),
                                problems = r.problems.len(),
                                "scheduled sync"
                            ),
                            // Never fatal to the daemon. A hub that is down
                            // now may be up in half an hour, and a daemon that
                            // exited on the first failure would need the app
                            // restarted to ever try again.
                            Err(e) => tracing::warn!(error = %e.message, "scheduled sync failed"),
                        }
                    }
                }
                for _ in 0..120 {
                    if flag.load(Ordering::Relaxed) {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
            }
        })
        .expect("spawning the sync thread");

    SyncDaemon {
        stop,
        handle: Some(handle),
    }
}
