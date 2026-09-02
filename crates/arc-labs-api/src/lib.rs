//! Every ARC-LABS operation, defined exactly once.
//!
//! This crate is the reason four shells cost one implementation. A Tauri
//! command, an HTTP handler and (from Phase 6) an MCP tool are each a ten-line
//! wrapper around a method here. An operation added once appears in all of them;
//! an operation that forgets to go through the ledger cannot exist in one shell
//! and not another, because there is only one place it could have been written.
//!
//! Nothing here knows about Tauri, HTTP, WebSockets or MCP. That is what keeps
//! the whole API surface testable with `cargo test` and no socket.
//!
//! # Phase 0 is read-only
//!
//! There is no write operation on [`Api`], not even a private one. The Phase 0
//! gate is that a real vault is byte-identical after an hour of browsing, and
//! the way to guarantee that is for the capability not to exist yet.

pub mod error;
pub mod runs;
pub mod types;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use arc_labs_core::{Config, LineEnding, Vault, VaultPath};
use arc_labs_index::{query, Index};

pub use error::{ApiError, ApiResult, ErrorCode};
pub use types::*;

/// What this deployment is allowed to do beyond reading an open vault.
///
/// Filesystem browsing is the interesting one. The browser shell needs it —
/// there is no native folder picker in a browser, so picking a vault means
/// listing directories. But on a server bound past loopback, that same operation
/// is a remote directory-listing primitive for anyone who can reach the port. So
/// it is a capability the shell grants, not a method that always exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub shell: Shell,
    pub browse_filesystem: bool,
    pub native_folder_picker: bool,
    /// Whether absolute paths may be shown to the caller. False when serving a
    /// client that has no business learning the server's layout.
    pub expose_paths: bool,
}

impl Capabilities {
    /// Desktop: the user is at the machine, so everything is theirs to see.
    pub fn desktop() -> Self {
        Capabilities {
            shell: Shell::Desktop,
            browse_filesystem: true,
            native_folder_picker: true,
            expose_paths: true,
        }
    }

    /// A server on loopback: same machine, same person, but no native dialogs.
    pub fn local_server() -> Self {
        Capabilities {
            shell: Shell::Server,
            browse_filesystem: true,
            native_folder_picker: false,
            expose_paths: true,
        }
    }

    /// A server reachable from elsewhere. The vault must be chosen at launch
    /// (`--vault` or `ARC_LABS_VAULT`), and the filesystem stays invisible.
    pub fn remote_server() -> Self {
        Capabilities {
            shell: Shell::Server,
            browse_filesystem: false,
            native_folder_picker: false,
            expose_paths: false,
        }
    }
}

struct State {
    config: Config,
    config_path: Option<PathBuf>,
    vault: Option<Vault>,
    status: VaultStatus,
}

pub struct Api {
    caps: Capabilities,
    state: RwLock<State>,
    /// The derived index.
    ///
    /// A `Mutex` rather than an `RwLock`: the underlying store serialises access
    /// anyway, so one lock is honest about what is actually happening. It is
    /// separate from `state` so a long index build never blocks a status call,
    /// which is what keeps the indicator moving while indexing runs.
    index: Mutex<Option<Index>>,
    /// Runs in flight, and recently finished ones.
    ///
    /// Its own lock, because a run holds it for the length of a pipeline and
    /// nothing else should wait on that.
    runs: Arc<runs::Runs>,
}

impl Api {
    pub fn new(config: Config, config_path: Option<PathBuf>, caps: Capabilities) -> Api {
        Api {
            caps,
            state: RwLock::new(State {
                config,
                config_path,
                vault: None,
                status: VaultStatus::Offline,
            }),
            index: Mutex::new(None),
            runs: runs::Runs::new(),
        }
    }

    pub fn capabilities(&self) -> Capabilities {
        self.caps
    }

    /// Resolve the vault to open at startup: explicit flag, then environment,
    /// then the last one used. The same order on every platform, so a Docker
    /// user setting `ARC_LABS_VAULT` and a desktop user who opened one last week
    /// both get what they expect.
    pub fn resolve_startup_vault(&self, explicit: Option<PathBuf>) -> Option<PathBuf> {
        explicit
            .or_else(Config::vault_from_env)
            .or_else(|| self.state.read().ok()?.config.vault.clone())
    }

    pub fn status(&self) -> Status {
        let state = self.state.read().expect("state lock poisoned");
        Status {
            status: state.status,
            shell: self.caps.shell,
            version: env!("CARGO_PKG_VERSION").to_string(),
            vault: state.vault.as_ref().map(|v| self.vault_info(v)),
            can_browse: self.caps.browse_filesystem,
            can_pick_folder: self.caps.native_folder_picker,
        }
    }

    fn vault_info(&self, vault: &Vault) -> VaultInfo {
        // Counts come from a tree walk, which is cheap enough at Phase 0 scale
        // and becomes an index lookup in Phase 2.
        let (notes, canvases) = vault.tree().map(|t| (t.note_count, t.canvas_count)).unwrap_or((0, 0));
        VaultInfo {
            name: vault.name(),
            path: self.caps.expose_paths.then(|| vault.root().path().display().to_string()),
            note_count: notes,
            canvas_count: canvases,
        }
    }

    /// Open a vault and remember it. Persisting is best-effort: a read-only
    /// config directory (common in Docker) should not stop the vault opening.
    pub fn open_vault(&self, path: &Path) -> ApiResult<VaultInfo> {
        let vault = Vault::open(path)?;
        let info = self.vault_info(&vault);

        let mut state = self.state.write().expect("state lock poisoned");
        state.config.vault = Some(vault.root().path().to_path_buf());
        state.vault = Some(vault);
        state.status = VaultStatus::Online;

        if let (Some(p), cfg) = (state.config_path.clone(), state.config.clone()) {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(e) = std::fs::write(&p, cfg.to_toml()) {
                tracing::warn!(path = %p.display(), error = %e, "could not persist config");
            }
        }
        Ok(info)
    }

    pub fn close_vault(&self) {
        let mut state = self.state.write().expect("state lock poisoned");
        state.vault = None;
        state.status = VaultStatus::Offline;
    }

    fn with_vault<T>(&self, f: impl FnOnce(&Vault) -> ApiResult<T>) -> ApiResult<T> {
        let state = self.state.read().expect("state lock poisoned");
        f(state.vault.as_ref().ok_or_else(ApiError::no_vault)?)
    }

    pub fn tree(&self) -> ApiResult<TreeView> {
        self.with_vault(|v| Ok(TreeView { tree: v.tree()? }))
    }

    pub fn read_note(&self, path: &VaultPath) -> ApiResult<NoteView> {
        self.read_note_inner(path, false)
    }

    /// Read a note including its raw markdown, for the editor.
    ///
    /// Separate from [`Self::read_note`] so a plain render never ships the
    /// source over the wire twice.
    pub fn read_note_for_edit(&self, path: &VaultPath) -> ApiResult<NoteView> {
        self.read_note_inner(path, true)
    }

    fn read_note_inner(&self, path: &VaultPath, with_text: bool) -> ApiResult<NoteView> {
        self.with_vault(|v| {
            let note = v.read_note(path)?;
            let rendered = arc_labs_core::render(note.text());
            Ok(NoteView {
                name: path.file_name().to_string(),
                path: path.clone(),
                html: rendered.html,
                frontmatter: rendered.frontmatter,
                links: rendered.links.iter().map(Into::into).collect(),
                embeds: rendered.embeds.iter().map(Into::into).collect(),
                tags: rendered.tags,
                size: note.text().len(),
                line_ending: match note.fidelity().line_ending() {
                    LineEnding::Lf => "LF",
                    LineEnding::Crlf => "CRLF",
                }
                .into(),
                line_endings_mixed: note.fidelity().is_mixed(),
                text: with_text.then(|| note.text().to_string()),
                hash: note.content_hash(),
            })
        })
    }

    /// Save a note.
    ///
    /// `base_hash` is the hash the editor started from. When it does not match
    /// what is on disk, the save is refused rather than applied: a vault is
    /// frequently open in Obsidian, syncing over Syncthing, or being changed by
    /// git at the same time, and silently discarding someone else's write is the
    /// one failure mode a notebook must not have.
    ///
    /// Fidelity comes from the file as it is *now*, so line endings and BOM stay
    /// the file's own even across an external change.
    pub fn write_note(
        &self,
        path: &VaultPath,
        text: &str,
        base_hash: Option<&str>,
    ) -> ApiResult<SaveResult> {
        // Captured before the write, for the ledger entry afterwards.
        let before_text = self.with_vault(|v| Ok(v.read_note(path)?)).ok().map(|n| n.text().to_string());

        let result = self.with_vault(|v| {
            let current = v.read_note(path)?;
            if let Some(base) = base_hash {
                if current.content_hash() != base {
                    return Err(ApiError::conflict());
                }
            }

            let saved = v.write_note(path, &current, text)?;
            // Hash the text we just committed to, so the editor's next save has
            // the right base whether or not bytes were actually written.
            let hash = arc_labs_core::NoteText::decode(text.as_bytes())
                .map(|n| n.content_hash())
                .unwrap_or_else(|| current.content_hash());

            Ok(match saved {
                arc_labs_core::Saved::Written { bytes } => {
                    SaveResult { written: true, bytes, hash }
                }
                arc_labs_core::Saved::Unchanged => {
                    SaveResult { written: false, bytes: 0, hash }
                }
            })
        })?;

        if result.written {
            // Record the change. Constraint 5 says *every* mutation is
            // ledgered, human included — a ledger that only saw agents would
            // show one colour and answer none of the questions the product
            // exists to answer.
            if let Some(before) = before_text.as_deref() {
                if let Err(e) = self.ledger().and_then(|l| {
                    l.record_change(
                        path,
                        self.human(),
                        arc_labs_ledger::Op::Edit,
                        "manual edit",
                        Some(before),
                        Some(text),
                    )
                    .map_err(ledger_err)
                }) {
                    tracing::warn!(error = %e, path = %path, "could not record the edit");
                }
            }

            // Refresh this note's index rows. Without it, search, backlinks and
            // the graph go stale the moment anything is edited — and a search
            // index that disagrees with the vault is worse than none.
            if let Err(e) = self.reindex_note(path) {
                tracing::warn!(error = %e, path = %path, "could not reindex after save");
            }
        }
        Ok(result)
    }

    pub fn config(&self) -> Config {
        self.state.read().expect("state lock poisoned").config.clone()
    }

    // ── Runtime ─────────────────────────────────────────────────────────────

    /// Whether a canvas can run, and which cards are executable.
    ///
    /// Cheap enough to call on every canvas edit — that is the point. The gate
    /// says a cycle is marked within 100 ms, so the user finds out while they
    /// are drawing the loop rather than when they press Run.
    pub fn canvas_runnability(&self, path: &VaultPath) -> ApiResult<CanvasRunnability> {
        let source = self.with_vault(|v| Ok(v.read_note(path)?))?;
        let canvas = arc_labs_canvas::Canvas::parse(source.text()).map_err(canvas_err)?;
        let graph = arc_labs_runtime::Graph::from_canvas(&canvas);

        let mut runnable: Vec<String> = graph.nodes.keys().cloned().collect();
        runnable.sort();
        Ok(CanvasRunnability {
            cycle: arc_labs_runtime::find_cycle(&graph).unwrap_or_default(),
            runnable,
        })
    }

    /// Start a run on a background thread and return its id immediately.
    ///
    /// Non-blocking because a pipeline takes seconds to minutes on this
    /// hardware, and an HTTP request or an IPC call that blocked for that long
    /// would freeze the surface that is meant to be showing its progress.
    pub fn start_run(
        &self,
        canvas: &VaultPath,
        target: &str,
        approve_egress: bool,
    ) -> ApiResult<String> {
        // Fail fast on the things that can be known before any thread starts —
        // a cycle, or a missing node — so the caller gets a real error rather
        // than a run id that immediately fails.
        let check = self.canvas_runnability(canvas)?;
        if !check.cycle.is_empty() {
            return Err(ApiError::new(
                ErrorCode::Config,
                format!("this canvas has a cycle: {}", check.cycle.join(", ")),
            ));
        }
        if !check.runnable.iter().any(|id| id == target) {
            return Err(ApiError::new(
                ErrorCode::NoteNotFound,
                format!("no runnable node {target} on this canvas"),
            ));
        }

        let vault_root = self.with_vault(|v| Ok(v.root().path().to_path_buf()))?;
        let config = self.config();
        let id = runs::next_run_id();
        let cancel = arc_labs_runtime::Cancel::new();
        self.runs.start(&id, canvas.as_str(), target, cancel.clone());

        let runs = Arc::clone(&self.runs);
        let canvas_path = canvas.clone();
        let target = target.to_string();
        let run_id = id.clone();

        std::thread::spawn(move || {
            let started = std::time::Instant::now();

            // The thread opens its own handles rather than sharing the API's.
            // A run must not hold the index lock for minutes while the rest of
            // the app tries to search.
            let outcome = (|| -> Result<arc_labs_runtime::RunReport, String> {
                let vault = Vault::open(&vault_root).map_err(|e| e.public())?;
                let ledger =
                    arc_labs_ledger::Ledger::open(&vault_root).map_err(|e| e.public())?;
                let index = arc_labs_index::Index::open_for_vault(&vault_root).ok();

                let ollama = arc_labs_runtime::Ollama::new(config.model.endpoint.clone());
                let runner = arc_labs_runtime::Runner {
                    vault: &vault,
                    index: index.as_ref(),
                    ledger: &ledger,
                    llm: &ollama,
                    config: &config,
                    session: run_id.clone(),
                };

                runner
                    .run(&canvas_path, &target, approve_egress, &cancel, &mut |event| {
                        use arc_labs_runtime::Event;
                        match event {
                            Event::NodeStarted { id, kind } => {
                                runs.node_started(&run_id, &id, &kind)
                            }
                            Event::Token { id, text } => runs.token(&run_id, &id, &text),
                            Event::NodeFinished { id, output, cost } => {
                                runs.node_finished(&run_id, &id, &output, cost)
                            }
                            Event::Egress { destination, bytes } => {
                                runs.egress(&run_id, &destination, bytes)
                            }
                        }
                    })
                    .map_err(|e| e.to_string())
            })();

            let elapsed = started.elapsed().as_millis();
            match outcome {
                Ok(report) => {
                    for r in &report.results {
                        if let Some(path) = &r.proposed_to {
                            runs.set_proposed(&run_id, &r.id, path);
                        }
                    }
                    runs.finish(&run_id, RunState::Done, None, elapsed);
                }
                Err(message) => {
                    // "Needs approval" is a decision waiting on a person, not a
                    // failure, and the surface has to tell them apart to know
                    // whether to show an error or a prompt.
                    let state = if message.contains("needs approval") {
                        RunState::NeedsEgressApproval
                    } else if message.contains("cancelled") {
                        RunState::Cancelled
                    } else {
                        RunState::Failed
                    };
                    runs.finish(&run_id, state, Some(message), elapsed);
                }
            }
        });

        Ok(id)
    }

    pub fn run_status(&self, id: &str) -> ApiResult<RunStatus> {
        self.runs
            .get(id)
            .ok_or_else(|| ApiError::new(ErrorCode::NoteNotFound, format!("no run {id}")))
    }

    pub fn runs(&self) -> Vec<RunStatus> {
        self.runs.list()
    }

    pub fn cancel_run(&self, id: &str) -> ApiResult<()> {
        if self.runs.cancel(id) {
            Ok(())
        } else {
            Err(ApiError::new(ErrorCode::NoteNotFound, format!("no run {id}")))
        }
    }

    // ── Canvas ──────────────────────────────────────────────────────────────

    /// Read a canvas, with each card's authorship resolved from the ledger.
    pub fn read_canvas(&self, path: &VaultPath) -> ApiResult<CanvasView> {
        let source = self.with_vault(|v| Ok(v.read_note(path)?))?;
        let canvas =
            arc_labs_canvas::Canvas::parse(source.text()).map_err(canvas_err)?;
        let ledger = self.ledger().ok();

        let nodes = canvas
            .nodes
            .iter()
            .map(|n| {
                // Constraint 6 on the canvas. A `file` card shows a note, so its
                // authorship is that note's; a text card belongs to the canvas,
                // so its authorship is the canvas's. Both are read from real
                // history — a card with no record gets no border rather than a
                // default one, because inventing authorship is worse than
                // omitting it.
                let source_path = match n.file().and_then(|f| VaultPath::new(f).ok()) {
                    Some(p) => p,
                    None => path.clone(),
                };
                let (author, author_model) = ledger
                    .as_ref()
                    .and_then(|l| l.read(&source_path).ok())
                    .and_then(|entries| {
                        entries.iter().rev().find(|e| e.op.touches_file()).map(|e| {
                            (
                                match e.actor.kind {
                                    arc_labs_ledger::ActorKind::Human => "human".to_string(),
                                    arc_labs_ledger::ActorKind::Agent => "agent".to_string(),
                                },
                                e.actor.model.clone(),
                            )
                        })
                    })
                    .map_or((None, None), |(k, m)| (Some(k), m));

                CanvasNode {
                    id: n.id().to_string(),
                    kind: format!("{:?}", n.kind()).to_lowercase(),
                    arc_kind: n.arc_kind().map(|k| k.as_str().to_string()),
                    x: n.x(),
                    y: n.y(),
                    width: n.width(),
                    height: n.height(),
                    file: n.file().map(str::to_string),
                    text: n.text().map(str::to_string),
                    url: n.url().map(str::to_string),
                    color: n.color().map(str::to_string),
                    author,
                    author_model,
                }
            })
            .collect();

        let edges = canvas
            .edges
            .iter()
            .map(|e| CanvasEdge {
                id: e.id().to_string(),
                from_node: e.from_node().to_string(),
                to_node: e.to_node().to_string(),
                from_side: e.from_side().map(str::to_string),
                to_side: e.to_side().map(str::to_string),
                label: e.label().map(str::to_string),
                color: e.as_map().get("color").and_then(|v| v.as_str()).map(str::to_string),
            })
            .collect();

        Ok(CanvasView {
            name: path.file_name().to_string(),
            path: path.clone(),
            nodes,
            edges,
        })
    }

    /// Apply node moves and resizes, and save.
    ///
    /// Goes through the canvas parser rather than rewriting the file, so
    /// everything the parser preserves — per-node key order, unknown keys, the
    /// file's own formatting — survives a drag. Moving one card changes one line.
    pub fn move_canvas_nodes(
        &self,
        path: &VaultPath,
        moves: &[NodeGeometry],
    ) -> ApiResult<SaveResult> {
        let source = self.with_vault(|v| Ok(v.read_note(path)?))?;
        let mut canvas =
            arc_labs_canvas::Canvas::parse(source.text()).map_err(canvas_err)?;

        for m in moves {
            let Some(node) = canvas.node_mut(&m.id) else { continue };
            node.set_position(m.x, m.y);
            if let (Some(w), Some(h)) = (m.width, m.height) {
                node.set_size(w, h);
            }
        }

        let out = canvas.serialize();
        let saved = self.with_vault(|v| Ok(v.write_note(path, &source, &out)?))?;

        if matches!(saved, arc_labs_core::Saved::Written { .. }) {
            if let Ok(l) = self.ledger() {
                let _ = l.record_change(
                    path,
                    self.human(),
                    arc_labs_ledger::Op::Edit,
                    format!("moved {} card(s)", moves.len()),
                    Some(source.text()),
                    Some(&out),
                );
            }
            let _ = self.reindex_note(path);
        }
        Ok(save_result(saved, &out))
    }

    // ── Ledger ──────────────────────────────────────────────────────────────

    fn ledger(&self) -> ApiResult<arc_labs_ledger::Ledger> {
        let state = self.state.read().expect("state lock poisoned");
        let vault = state.vault.as_ref().ok_or_else(ApiError::no_vault)?;
        arc_labs_ledger::Ledger::open(vault.root().path()).map_err(ledger_err)
    }

    /// Who the current user is, for attribution.
    fn human(&self) -> arc_labs_ledger::Actor {
        arc_labs_ledger::Actor::human(
            self.state.read().expect("state lock poisoned").config.resolved_actor_id(),
        )
    }

    /// A note's history, oldest first.
    pub fn timeline(&self, path: &VaultPath) -> ApiResult<Vec<TimelineEntry>> {
        let entries = self.ledger()?.read(path).map_err(ledger_err)?;
        Ok(entries.iter().enumerate().map(|(i, e)| to_timeline(i, e)).collect())
    }

    /// Proposals on a note that have not been accepted or rejected.
    ///
    /// Worked out by walking the history rather than stored as a separate list:
    /// the ledger is the only record, and a second list of "pending" state could
    /// disagree with it.
    pub fn proposals(&self, path: &VaultPath) -> ApiResult<Vec<Proposal>> {
        use arc_labs_ledger::Op;
        let entries = self.ledger()?.read(path).map_err(ledger_err)?;

        let mut open: Vec<Proposal> = Vec::new();
        for (i, e) in entries.iter().enumerate() {
            match e.op {
                Op::Propose => {
                    let (added, removed) = diff_counts(e.patch.as_deref());
                    open.push(Proposal {
                        index: i,
                        ts: e.ts.clone(),
                        actor_id: e.actor.id.clone(),
                        model: e.actor.model.clone(),
                        reason: e.reason.clone(),
                        patch: e.patch.clone().unwrap_or_default(),
                        added,
                        removed,
                    });
                }
                // An accept or reject settles the oldest outstanding proposal.
                Op::Accept | Op::Reject if !open.is_empty() => {
                    open.remove(0);
                }
                _ => {}
            }
        }
        Ok(open)
    }

    /// The diff for one entry, and optionally the content it restores to.
    pub fn entry_diff(&self, path: &VaultPath, index: usize) -> ApiResult<EntryDiff> {
        let ledger = self.ledger()?;
        let entries = ledger.read(path).map_err(ledger_err)?;
        let entry = entries.get(index).ok_or_else(|| {
            ApiError::new(ErrorCode::NoteNotFound, format!("no entry {index}"))
        })?;
        Ok(EntryDiff {
            index,
            patch: entry.patch.clone().unwrap_or_default(),
            content: ledger.content_at(path, index).ok(),
        })
    }

    /// Restore a note to the state it was in at `index`.
    ///
    /// This is itself a change, so it is recorded as one — attributed to the
    /// person who asked for it, with the entry it came from as the reason. A
    /// restore that erased its own trace would be the one hole in the audit.
    pub fn restore(&self, path: &VaultPath, index: usize) -> ApiResult<SaveResult> {
        let ledger = self.ledger()?;
        let target = ledger.content_at(path, index).map_err(ledger_err)?;
        let current = self.with_vault(|v| Ok(v.read_note(path)?))?;

        let saved = self.with_vault(|v| Ok(v.write_note(path, &current, &target)?))?;
        ledger
            .record_change(
                path,
                self.human(),
                arc_labs_ledger::Op::Edit,
                format!("restored to entry {index}"),
                Some(current.text()),
                Some(&target),
            )
            .map_err(ledger_err)?;

        let _ = self.reindex_note(path);
        Ok(save_result(saved, &target))
    }

    /// Record an agent's proposal. **Does not touch the note.**
    pub fn propose(
        &self,
        path: &VaultPath,
        agent: &str,
        model: &str,
        session: &str,
        reason: &str,
        proposed: &str,
    ) -> ApiResult<Proposal> {
        let ledger = self.ledger()?;
        let current = self.with_vault(|v| Ok(v.read_note(path)?))?;
        let entry = ledger
            .propose(
                path,
                arc_labs_ledger::Actor::agent(agent, model, session),
                reason,
                current.text(),
                proposed,
            )
            .map_err(ledger_err)?;

        let index = ledger.read(path).map_err(ledger_err)?.len() - 1;
        let (added, removed) = diff_counts(entry.patch.as_deref());
        Ok(Proposal {
            index,
            ts: entry.ts,
            actor_id: entry.actor.id,
            model: entry.actor.model,
            reason: entry.reason,
            patch: entry.patch.unwrap_or_default(),
            added,
            removed,
        })
    }

    /// Apply a proposal. This is the only path by which agent output reaches a
    /// file, and it runs because a person said so.
    pub fn accept(&self, path: &VaultPath, index: usize) -> ApiResult<SaveResult> {
        let ledger = self.ledger()?;
        let entries = ledger.read(path).map_err(ledger_err)?;
        let entry = entries.get(index).filter(|e| e.op == arc_labs_ledger::Op::Propose).ok_or_else(
            || ApiError::new(ErrorCode::NoteNotFound, format!("no proposal at {index}")),
        )?;
        let proposed = ledger
            .objects()
            .get(entry.after.as_deref().unwrap_or_default())
            .map_err(ledger_err)?;

        let current = self.with_vault(|v| Ok(v.read_note(path)?))?;
        let saved = self.with_vault(|v| Ok(v.write_note(path, &current, &proposed)?))?;

        // Attributed to the agent that proposed it, because that is who wrote
        // the words — with the acceptance itself recorded as the reason.
        let mut actor = entry.actor.clone();
        actor.session = entry.actor.session.clone();
        ledger
            .record_accept(
                path,
                actor,
                format!("accepted proposal {index}: {}", entry.reason),
                current.text(),
                &proposed,
            )
            .map_err(ledger_err)?;

        let _ = self.reindex_note(path);
        Ok(save_result(saved, &proposed))
    }

    /// Discard a proposal. The note is never touched.
    pub fn reject(&self, path: &VaultPath, index: usize) -> ApiResult<()> {
        let ledger = self.ledger()?;
        let entries = ledger.read(path).map_err(ledger_err)?;
        let entry = entries.get(index).filter(|e| e.op == arc_labs_ledger::Op::Propose).ok_or_else(
            || ApiError::new(ErrorCode::NoteNotFound, format!("no proposal at {index}")),
        )?;
        let proposed = ledger
            .objects()
            .get(entry.after.as_deref().unwrap_or_default())
            .map_err(ledger_err)?;

        ledger
            .record_reject(
                path,
                entry.actor.clone(),
                format!("rejected proposal {index}: {}", entry.reason),
                &proposed,
            )
            .map_err(ledger_err)?;
        Ok(())
    }

    // ── Index ───────────────────────────────────────────────────────────────

    /// Open (or create) the index for the current vault and bring it up to date.
    ///
    /// Called after opening a vault. Blocking: the caller decides whether to run
    /// it on a background thread, because the desktop shell and the server want
    /// different answers.
    ///
    /// A corrupt or stale-schema index is **deleted and rebuilt** rather than
    /// migrated. That is the whole point of a derived cache: the recovery path
    /// for anything going wrong is to throw it away, and exercising that path
    /// routinely is what keeps it working.
    pub fn open_index(&self, force: bool) -> ApiResult<arc_labs_index::BuildStats> {
        let root = {
            let state = self.state.read().expect("state lock poisoned");
            let vault = state.vault.as_ref().ok_or_else(ApiError::no_vault)?;
            vault.root().path().to_path_buf()
        };
        let mut index = Index::open_for_vault(&root).map_err(index_err)?;

        self.set_status(VaultStatus::Indexing);
        let result = {
            let state = self.state.read().expect("state lock poisoned");
            let vault = state.vault.as_ref().ok_or_else(ApiError::no_vault)?;
            index.build(vault, force, |_| {})
        };
        self.set_status(VaultStatus::Online);

        let stats = result.map_err(index_err)?;
        *self.index.lock().expect("index lock poisoned") = Some(index);
        Ok(stats)
    }

    fn set_status(&self, status: VaultStatus) {
        self.state.write().expect("state lock poisoned").status = status;
    }

    fn with_index<T>(
        &self,
        f: impl FnOnce(&Index) -> std::result::Result<T, arc_labs_index::IndexError>,
    ) -> ApiResult<T> {
        let guard = self.index.lock().expect("index lock poisoned");
        let index = guard
            .as_ref()
            .ok_or_else(|| ApiError::new(ErrorCode::NoVault, "the index is not ready yet"))?;
        f(index).map_err(index_err)
    }

    pub fn search(&self, q: &str, limit: usize) -> ApiResult<Vec<query::SearchHit>> {
        self.with_index(|i| i.search(q, limit.min(200)))
    }

    pub fn quick_open(&self, q: &str, limit: usize) -> ApiResult<Vec<query::NoteRef>> {
        self.with_index(|i| i.quick_open(q, limit.min(200)))
    }

    pub fn backlinks(&self, path: &VaultPath) -> ApiResult<Vec<query::Backlink>> {
        let p = path.as_str().to_string();
        self.with_index(move |i| i.backlinks(&p))
    }

    pub fn outgoing(&self, path: &VaultPath) -> ApiResult<Vec<query::OutgoingLink>> {
        let p = path.as_str().to_string();
        self.with_index(move |i| i.outgoing(&p))
    }

    pub fn unresolved(&self, limit: usize) -> ApiResult<Vec<query::UnresolvedLink>> {
        self.with_index(|i| i.unresolved(limit.min(500)))
    }

    pub fn tags(&self) -> ApiResult<Vec<query::TagCount>> {
        self.with_index(|i| i.tag_counts())
    }

    pub fn notes_with_tag(&self, tag: &str) -> ApiResult<Vec<query::NoteRef>> {
        let t = tag.to_string();
        self.with_index(move |i| i.notes_with_tag(&t))
    }

    pub fn graph(&self) -> ApiResult<query::Graph> {
        self.with_index(|i| i.graph())
    }

    pub fn index_stats(&self) -> ApiResult<query::IndexStats> {
        self.with_index(|i| i.stats())
    }

    pub fn recent(&self, limit: usize) -> ApiResult<Vec<query::NoteRef>> {
        self.with_index(|i| i.recent(limit.min(100)))
    }

    /// Re-index one note after it changed. Cheap enough to call on every save.
    pub fn reindex_note(&self, path: &VaultPath) -> ApiResult<()> {
        let state = self.state.read().expect("state lock poisoned");
        let vault = state.vault.as_ref().ok_or_else(ApiError::no_vault)?;
        let mut guard = self.index.lock().expect("index lock poisoned");
        if let Some(index) = guard.as_mut() {
            index.reindex_note(vault, path).map_err(index_err)?;
        }
        Ok(())
    }

    /// List directories under `path` so a browser client can choose a vault.
    ///
    /// Directories only, and gated on [`Capabilities::browse_filesystem`].
    pub fn browse(&self, path: Option<&Path>) -> ApiResult<DirListing> {
        if !self.caps.browse_filesystem {
            return Err(ApiError::not_permitted("browsing the filesystem"));
        }

        let dir = match path {
            Some(p) => p.to_path_buf(),
            None => home_dir().unwrap_or_else(|| PathBuf::from("/")),
        };
        let dir = dunce_canonicalize(&dir)?;

        let mut entries = Vec::new();
        let read = std::fs::read_dir(&dir)
            .map_err(|e| ApiError::new(ErrorCode::Io, format!("cannot list directory: {}", e.kind())))?;
        for entry in read.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            // Hidden directories are noise when picking a notes folder.
            if name.starts_with('.') {
                continue;
            }
            let p = entry.path();
            entries.push(DirEntry {
                is_vault: p.join(".obsidian").is_dir() || p.join(".arc").is_dir(),
                name,
                path: p.display().to_string(),
            });
        }
        entries.sort_by_key(|e| e.name.to_lowercase());

        Ok(DirListing {
            parent: dir.parent().map(|p| p.display().to_string()),
            path: dir.display().to_string(),
            entries,
        })
    }
}

fn canvas_err(e: arc_labs_canvas::CanvasError) -> ApiError {
    // A malformed canvas is the user's file, not an internal fault, so the
    // message says what is wrong with it rather than hiding behind "io error".
    ApiError::new(ErrorCode::NotUtf8, format!("this canvas could not be read: {e}"))
}

fn ledger_err(e: arc_labs_ledger::LedgerError) -> ApiError {
    tracing::debug!(error = %e, "ledger error");
    ApiError::new(ErrorCode::Io, e.public())
}

fn save_result(saved: arc_labs_core::Saved, content: &str) -> SaveResult {
    let hash = arc_labs_ledger::hash_of(content);
    match saved {
        arc_labs_core::Saved::Written { bytes } => SaveResult { written: true, bytes, hash },
        arc_labs_core::Saved::Unchanged => SaveResult { written: false, bytes: 0, hash },
    }
}

/// Added and removed line counts from a unified diff.
///
/// Counted here rather than in the browser so the timeline can size a bar
/// without shipping and parsing every patch in a note's history.
fn diff_counts(patch: Option<&str>) -> (usize, usize) {
    let Some(p) = patch else { return (0, 0) };
    let mut added = 0;
    let mut removed = 0;
    for line in p.lines() {
        // `+++`/`---` are the file headers, not content.
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        match line.as_bytes().first() {
            Some(b'+') => added += 1,
            Some(b'-') => removed += 1,
            _ => {}
        }
    }
    (added, removed)
}

fn to_timeline(index: usize, e: &arc_labs_ledger::Entry) -> TimelineEntry {
    let (added, removed) = diff_counts(e.patch.as_deref());
    TimelineEntry {
        index,
        ts: e.ts.clone(),
        actor_kind: match e.actor.kind {
            arc_labs_ledger::ActorKind::Human => "human",
            arc_labs_ledger::ActorKind::Agent => "agent",
        }
        .into(),
        actor_id: e.actor.id.clone(),
        model: e.actor.model.clone(),
        session: e.actor.session.clone(),
        op: format!("{:?}", e.op).to_lowercase(),
        reason: e.reason.clone(),
        touched_file: e.op.touches_file(),
        added,
        removed,
        from_path: e.from_path.clone(),
        destination: e.destination.clone(),
        bytes: e.bytes,
    }
}

fn index_err(e: arc_labs_index::IndexError) -> ApiError {
    // The full form can name the database path; the public form must not.
    tracing::debug!(error = %e, "index error");
    match e {
        arc_labs_index::IndexError::SchemaMismatch { .. } => {
            ApiError::new(ErrorCode::Config, "the index was built by a different version")
        }
        _ => ApiError::new(ErrorCode::Io, "the index could not be read"),
    }
}

fn dunce_canonicalize(p: &Path) -> ApiResult<PathBuf> {
    dunce::canonicalize(p).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => ApiError::new(ErrorCode::VaultNotFound, "no such directory"),
        _ => ApiError::new(ErrorCode::Io, format!("cannot open directory: {}", e.kind())),
    })
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api_with_vault(caps: Capabilities) -> (tempfile::TempDir, Api) {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("Daily")).unwrap();
        std::fs::write(tmp.path().join("a.md"), b"# A\n\nlink to [[B]] #tag\n").unwrap();
        std::fs::write(tmp.path().join("Daily/b.md"), b"# B\n").unwrap();
        std::fs::write(tmp.path().join("board.canvas"), b"{}").unwrap();

        let api = Api::new(Config::default(), None, caps);
        api.open_vault(tmp.path()).unwrap();
        (tmp, api)
    }

    #[test]
    fn status_reflects_whether_a_vault_is_open() {
        let api = Api::new(Config::default(), None, Capabilities::desktop());
        assert_eq!(api.status().status, VaultStatus::Offline);
        assert!(api.status().vault.is_none());

        let (_t, api) = api_with_vault(Capabilities::desktop());
        let s = api.status();
        assert_eq!(s.status, VaultStatus::Online);
        assert_eq!(s.vault.as_ref().unwrap().note_count, 2);
        assert_eq!(s.vault.as_ref().unwrap().canvas_count, 1);
    }

    #[test]
    fn operations_without_a_vault_say_so_rather_than_panicking() {
        let api = Api::new(Config::default(), None, Capabilities::desktop());
        assert_eq!(api.tree().unwrap_err().code, ErrorCode::NoVault);
        let p = VaultPath::new("a.md").unwrap();
        assert_eq!(api.read_note(&p).unwrap_err().code, ErrorCode::NoVault);
    }

    #[test]
    fn reads_and_renders_a_note() {
        let (_t, api) = api_with_vault(Capabilities::desktop());
        let n = api.read_note(&VaultPath::new("a.md").unwrap()).unwrap();
        assert_eq!(n.name, "a.md");
        assert!(n.html.contains("<h1>A</h1>"));
        assert_eq!(n.links[0].target, "B");
        assert_eq!(n.tags, ["tag"]);
        assert_eq!(n.line_ending, "LF");
        // Phase 0 has no index, so resolution is unknown — not guessed.
        assert_eq!(n.links[0].resolved, None);
    }

    #[test]
    fn editing_returns_the_source_and_a_base_hash() {
        let (_t, api) = api_with_vault(Capabilities::desktop());
        let p = VaultPath::new("a.md").unwrap();

        assert!(api.read_note(&p).unwrap().text.is_none(), "a render should not ship the source");

        let edit = api.read_note_for_edit(&p).unwrap();
        assert_eq!(edit.text.as_deref(), Some("# A\n\nlink to [[B]] #tag\n"));
        assert!(edit.hash.starts_with("blake3:"));
    }

    #[test]
    fn saving_unchanged_text_writes_nothing() {
        let (_t, api) = api_with_vault(Capabilities::desktop());
        let p = VaultPath::new("a.md").unwrap();
        let note = api.read_note_for_edit(&p).unwrap();

        let r = api.write_note(&p, note.text.as_deref().unwrap(), Some(&note.hash)).unwrap();
        assert!(!r.written);
        assert_eq!(r.hash, note.hash);
    }

    #[test]
    fn saving_changed_text_writes_and_returns_a_new_base() {
        let (_t, api) = api_with_vault(Capabilities::desktop());
        let p = VaultPath::new("a.md").unwrap();
        let note = api.read_note_for_edit(&p).unwrap();

        let r = api.write_note(&p, "# A changed\n", Some(&note.hash)).unwrap();
        assert!(r.written && r.bytes > 0);
        assert_ne!(r.hash, note.hash);

        // The returned hash is the right base for the next save.
        assert!(api.write_note(&p, "# A changed again\n", Some(&r.hash)).is_ok());
    }

    #[test]
    fn a_stale_base_hash_is_refused_rather_than_clobbering_the_other_writer() {
        let (tmp, api) = api_with_vault(Capabilities::desktop());
        let p = VaultPath::new("a.md").unwrap();
        let note = api.read_note_for_edit(&p).unwrap();

        // Someone else — Obsidian, Syncthing, git — writes to the same file.
        std::fs::write(tmp.path().join("a.md"), b"# written by someone else\n").unwrap();

        let err = api.write_note(&p, "# my version\n", Some(&note.hash)).unwrap_err();
        assert_eq!(err.code, ErrorCode::Conflict);
        // Their work is still there.
        assert_eq!(
            std::fs::read(tmp.path().join("a.md")).unwrap(),
            b"# written by someone else\n"
        );
    }

    #[test]
    fn every_human_save_lands_on_the_timeline() {
        // Constraint 5: *every* mutation is ledgered, human included. A ledger
        // that only recorded agents would show one colour and answer none of
        // the questions the product exists to answer.
        let (_t, api) = api_with_vault(Capabilities::desktop());
        let p = VaultPath::new("a.md").unwrap();

        let n = api.read_note_for_edit(&p).unwrap();
        api.write_note(&p, "# A changed\n", Some(&n.hash)).unwrap();

        let timeline = api.timeline(&p).unwrap();
        assert_eq!(timeline.len(), 1);
        assert_eq!(timeline[0].actor_kind, "human");
        assert_eq!(timeline[0].op, "edit");
        assert!(timeline[0].touched_file);
        assert!(timeline[0].added > 0);
    }

    /// **The constraint-4 gate, end to end through the API.**
    #[test]
    fn a_proposal_leaves_the_file_alone_until_a_person_accepts_it() {
        let (tmp, api) = api_with_vault(Capabilities::desktop());
        let p = VaultPath::new("a.md").unwrap();
        let file = tmp.path().join("a.md");

        let original = std::fs::read(&file).unwrap();
        let mtime = std::fs::metadata(&file).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));

        let proposal = api
            .propose(&p, "weave", "qwen3.5:0.8b", "run-1", "rewrite the opening", "# Rewritten\n")
            .unwrap();

        // Nothing on disk moved.
        assert_eq!(std::fs::read(&file).unwrap(), original);
        assert_eq!(std::fs::metadata(&file).unwrap().modified().unwrap(), mtime);

        // But it is visible, reviewable, and attributed.
        let open = api.proposals(&p).unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].model.as_deref(), Some("qwen3.5:0.8b"));
        assert!(open[0].patch.contains("+# Rewritten"));

        // The timeline shows it as an agent entry that did not touch the file.
        let t = api.timeline(&p).unwrap();
        assert_eq!(t[0].actor_kind, "agent");
        assert!(!t[0].touched_file);

        // Accepting is the only thing that writes, and it is still attributed
        // to the agent that wrote the words.
        api.accept(&p, proposal.index).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"# Rewritten\n");
        assert!(api.proposals(&p).unwrap().is_empty(), "accepting should settle it");

        let t = api.timeline(&p).unwrap();
        assert_eq!(t[1].op, "accept");
        assert_eq!(t[1].actor_kind, "agent");
        assert!(t[1].touched_file);
    }

    #[test]
    fn rejecting_a_proposal_never_writes_and_is_kept_as_history() {
        let (tmp, api) = api_with_vault(Capabilities::desktop());
        let p = VaultPath::new("a.md").unwrap();
        let file = tmp.path().join("a.md");
        let original = std::fs::read(&file).unwrap();

        let proposal =
            api.propose(&p, "weave", "m", "s", "a rewrite", "# Nobody wanted this\n").unwrap();
        api.reject(&p, proposal.index).unwrap();

        assert_eq!(std::fs::read(&file).unwrap(), original, "reject must not write");
        assert!(api.proposals(&p).unwrap().is_empty());

        // The refusal is history too: an audit that shows only accepted changes
        // tells you what an agent did but not what it wanted to do.
        let t = api.timeline(&p).unwrap();
        assert_eq!(t[1].op, "reject");
        assert!(!t[1].touched_file);
    }

    #[test]
    fn restore_puts_a_note_back_and_records_that_it_did() {
        let (_t, api) = api_with_vault(Capabilities::desktop());
        let p = VaultPath::new("a.md").unwrap();

        let v0 = api.read_note_for_edit(&p).unwrap();
        let original = v0.text.clone().unwrap();

        let r1 = api.write_note(&p, "# second\n", Some(&v0.hash)).unwrap();
        api.write_note(&p, "# third\n", Some(&r1.hash)).unwrap();
        assert_eq!(api.read_note_for_edit(&p).unwrap().text.as_deref(), Some("# third\n"));

        // Entry 0 is the first edit, whose `after` is "# second\n".
        api.restore(&p, 0).unwrap();
        assert_eq!(api.read_note_for_edit(&p).unwrap().text.as_deref(), Some("# second\n"));

        // The restore is itself on the timeline — an undo that erased its own
        // trace would be the one hole in the audit.
        let t = api.timeline(&p).unwrap();
        assert_eq!(t.len(), 3);
        assert!(t[2].reason.contains("restored to entry 0"));
        assert_eq!(t[2].actor_kind, "human");

        // The original is still reachable, because nothing is ever discarded.
        let diff = api.entry_diff(&p, 0).unwrap();
        assert!(!diff.patch.is_empty());
        assert!(original.starts_with("# A"));
    }

    #[test]
    fn a_remote_server_hides_the_filesystem_and_the_vault_path() {
        let (_t, api) = api_with_vault(Capabilities::remote_server());

        // The directory-listing primitive simply is not available.
        assert_eq!(api.browse(None).unwrap_err().code, ErrorCode::NotPermitted);

        // And the client is not told where on the server the vault lives.
        let s = api.status();
        assert!(s.vault.unwrap().path.is_none());
        assert!(!s.can_browse && !s.can_pick_folder);
    }

    #[test]
    fn a_local_shell_can_browse_but_only_sees_directories() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("Notes")).unwrap();
        std::fs::create_dir(tmp.path().join(".hidden")).unwrap();
        std::fs::write(tmp.path().join("file.txt"), b"x").unwrap();

        let api = Api::new(Config::default(), None, Capabilities::local_server());
        let listing = api.browse(Some(tmp.path())).unwrap();

        let names: Vec<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Notes"], "should list directories only, and no dotfiles");
        assert!(listing.parent.is_some());
    }

    #[test]
    fn browse_marks_folders_that_are_already_vaults() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("MyVault/.obsidian")).unwrap();
        std::fs::create_dir(tmp.path().join("Plain")).unwrap();

        let api = Api::new(Config::default(), None, Capabilities::desktop());
        let listing = api.browse(Some(tmp.path())).unwrap();
        let vault = listing.entries.iter().find(|e| e.name == "MyVault").unwrap();
        let plain = listing.entries.iter().find(|e| e.name == "Plain").unwrap();
        assert!(vault.is_vault);
        assert!(!plain.is_vault);
    }

    #[test]
    fn startup_vault_resolution_prefers_the_explicit_flag() {
        let api = Api::new(
            Config { vault: Some(PathBuf::from("/from-config")), ..Default::default() },
            None,
            Capabilities::desktop(),
        );
        assert_eq!(
            api.resolve_startup_vault(Some(PathBuf::from("/from-flag"))),
            Some(PathBuf::from("/from-flag"))
        );
        assert_eq!(api.resolve_startup_vault(None), Some(PathBuf::from("/from-config")));
    }

    #[test]
    fn opening_a_missing_vault_is_a_clean_error() {
        let api = Api::new(Config::default(), None, Capabilities::desktop());
        let err = api.open_vault(Path::new("/definitely/not/here")).unwrap_err();
        assert_eq!(err.code, ErrorCode::VaultNotFound);
        assert!(!err.message.contains("definitely"), "echoed the path back: {}", err.message);
    }
}
