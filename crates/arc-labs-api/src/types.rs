//! Wire types: what crosses between the core and whichever shell is running.
//!
//! All `camelCase` on the wire, because the consumer is always TypeScript —
//! whether it arrives via Tauri's IPC or an HTTP body. Naming it once here keeps
//! every shell from having to remember.

use arc_labs_core::{Tree, VaultPath, WikiLink};
use serde::{Deserialize, Serialize};

/// The vault indicator in the top bar. Load-bearing from Phase 0 onward, so it
/// is a real state machine from the start rather than a string that gets
/// stringly-typed into a corner by Phase 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VaultStatus {
    /// No vault open — the first-run surface.
    Offline,
    /// Walking the file tree.
    Scanning,
    Online,
    /// Phase 2 onward: building or updating the derived index.
    Indexing,
}

/// Which shell the UI is talking to. The UI adapts to this rather than sniffing
/// for `window.__TAURI__` a second time — one source of truth for "can I open a
/// native folder picker".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Shell {
    Desktop,
    Server,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultInfo {
    pub name: String,
    /// Display-only. Absent when the shell is serving a client that should not
    /// learn the server's filesystem layout.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub note_count: usize,
    pub canvas_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub status: VaultStatus,
    pub shell: Shell,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vault: Option<VaultInfo>,
    /// Whether this deployment allows browsing the filesystem to pick a vault.
    /// False on a server bound past loopback — see `Api::new`.
    pub can_browse: bool,
    /// Whether the shell can open a native folder picker.
    pub can_pick_folder: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteView {
    pub path: VaultPath,
    pub name: String,
    pub html: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frontmatter: Option<String>,
    pub links: Vec<Link>,
    pub embeds: Vec<Link>,
    pub tags: Vec<String>,
    /// Bytes on disk — shown in the note's detail strip, in monospace, because
    /// it is data.
    pub size: usize,
    pub line_ending: String,
    /// True when the file mixes CRLF and LF.
    ///
    /// Surfaced rather than swallowed. A no-op save leaves such a file exactly
    /// as it was, but a *real* edit re-encodes the whole document with the
    /// dominant ending — the same thing VS Code and Obsidian do, because there
    /// is no sane way to preserve an arbitrary mix through an arbitrary edit.
    /// Silently rewriting every line of a file because the user changed one word
    /// is the kind of thing a notebook has to say out loud.
    pub line_endings_mixed: bool,
    /// The note's raw markdown, for the editor. Absent from a read that only
    /// needs to render.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Content hash of the text as read. The editor sends it back on save, and
    /// a mismatch means someone else wrote to the file in the meantime.
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveResult {
    /// False when the encoded bytes already matched what was on disk, so
    /// nothing was written and mtime is untouched.
    pub written: bool,
    pub bytes: usize,
    /// The note's hash after saving — the editor's new base for the next save.
    pub hash: String,
}

/// A wikilink, flattened for the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Link {
    pub target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    pub display: String,
    /// Phase 0 cannot answer this without an index, so it is `None` rather than
    /// a guess. Constraint 7: never present an inference as an observation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved: Option<bool>,
}

impl From<&WikiLink> for Link {
    fn from(w: &WikiLink) -> Self {
        Link {
            target: w.target.clone(),
            anchor: w.anchor.clone(),
            alias: w.alias.clone(),
            display: w.display().to_string(),
            resolved: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeView {
    #[serde(flatten)]
    pub tree: Tree,
}

/// One entry when browsing the filesystem for a vault to open. Only directories
/// are listed: the user is choosing a folder, and listing every file would leak
/// far more about the machine than the task needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirEntry {
    pub name: String,
    pub path: String,
    /// True if it looks like an Obsidian or ARC vault already, so the picker can
    /// point at the right folder instead of making the user recognise it.
    pub is_vault: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirListing {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub entries: Vec<DirEntry>,
}

// ── Ledger (Phase 3) ─────────────────────────────────────────────────────────

/// One entry on the timeline, with the index needed to restore to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineEntry {
    /// Position in the note's history. What `restore` takes.
    pub index: usize,
    pub ts: String,
    /// "human" or "agent". **The field the whole surface hangs off** — it
    /// decides whether an entry is drawn amber or blue, which is constraint 6.
    pub actor_kind: String,
    pub actor_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    pub op: String,
    pub reason: String,
    /// Whether this operation changed the file. A proposal did not.
    pub touched_file: bool,
    /// Lines added and removed, for sizing the timeline bar without parsing the
    /// diff in the browser.
    pub added: usize,
    pub removed: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
}

/// A proposal awaiting a decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Proposal {
    pub index: usize,
    pub ts: String,
    pub actor_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub reason: String,
    /// The unified diff, for review.
    pub patch: String,
    pub added: usize,
    pub removed: usize,
}

/// The full diff for one timeline entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryDiff {
    pub index: usize,
    pub patch: String,
    /// Content as of this entry, so the UI can preview a restore before doing it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}
