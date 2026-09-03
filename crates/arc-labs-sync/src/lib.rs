//! Reconciliation between a vault and a hub.
//!
//! The hub is an always-on ARC-LABS holding a shared vault; devices sync to it
//! and back up against it. This crate is the part that **decides**, and it
//! deliberately contains no client, no socket and no HTTP: given three
//! manifests it says what should happen, and something above it does the
//! moving. That split is what makes the interesting cases — a delete racing an
//! edit between two machines — testable as values rather than as a distributed
//! system.
//!
//! ## Three things travel, by three different routes
//!
//! **Notes, canvases and attachments** reconcile. Two people can edit the same
//! paragraph, so this is the only part that can conflict, and when it does a
//! person decides. See [`plan`].
//!
//! **Ledgers** merge. Append-only JSONL unions line by line and cannot
//! conflict, which is why a note's history survives a two-machine setup fully
//! intact. See [`ledger`].
//!
//! **Objects** copy. `.arc/objects/` is content-addressed, so a hash either is
//! present or is not; there is nothing to reconcile and nothing to overwrite.
//!
//! Objects are not optional. Restore replays content from the object store by
//! hash, so a vault that received a ledger without them holds a history it
//! cannot act on — every restore fails with "content is not in the object
//! store". They are the reason "everything but trash" is the payload.
//!
//! **The index does not travel.** `.arc/index.db` is derived from the notes and
//! is rebuilt locally. Sending it would move the largest file in the vault to
//! reproduce something the receiver can compute.
//!
//! **The trash does not travel either.** It is per-machine undo state with its
//! own expiry, and a deletion you made here should not refill from another
//! machine's bin.

pub mod base;
pub mod client;
pub mod ledger;
pub mod manifest;
pub mod pass;
pub mod plan;
pub mod schedule;

pub use manifest::{FileState, Manifest};
pub use plan::{plan, Action, Conflict};

use std::path::Path;

pub type Result<T> = std::result::Result<T, SyncError>;

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("{path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("could not walk the vault: {0}")]
    Walk(String),
    /// The hub was reached and said no.
    #[error("{0}")]
    Hub(String),
    /// The hub was not reached at all. Kept separate from `Hub` because the two
    /// need different actions: one is a setting to change, the other is a box
    /// to turn on or a network to fix.
    #[error("could not reach {hub}: {why}")]
    Unreachable { hub: String, why: String },
    /// A reply that did not make sense.
    #[error("unexpected reply from the hub: {0}")]
    Wire(String),
}

impl SyncError {
    fn io(path: &Path, source: std::io::Error) -> SyncError {
        SyncError::Io {
            path: path.display().to_string(),
            source,
        }
    }
}
