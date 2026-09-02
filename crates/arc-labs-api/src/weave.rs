//! Weave, wired into the API.
//!
//! The daemon itself lives in `arc-labs-weave`. What is here is the part that
//! has to know about the rest of the application: which embedder the config
//! asks for, where the vault and index are, and what happens when a person
//! accepts a suggestion.
//!
//! # Accepting a suggestion is two ledger entries, not one
//!
//! Weave proposed the link; the user accepted it. Recording that as a single
//! human edit would be a small lie — the text came from a model — and constraint
//! 6 says a stranger looking at the timeline should be able to see where an
//! agent has worked. So [`Api::accept_suggestion`] writes a `propose` entry
//! attributed to Weave and then an `accept` entry attributed to the person. Two
//! entries, one blue and one amber, which is exactly what happened.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use arc_labs_core::VaultPath;
use arc_labs_weave::{Budget, Embedder, MockEmbedder, OllamaEmbedder, PassReport, Weave};

use crate::{ApiError, ApiResult, ErrorCode, LinkSuggestion, SaveResult, WeaveStatus};

/// The index, locked for the length of one statement and no longer.
///
/// This is the fix for a real freeze. Weave used to be handed the index guard
/// for a whole pass, which meant every save queued behind an HTTP round trip to
/// Ollama — and because saves run on the async runtime's worker threads, enough
/// of them blocked the entire server, not just the editor. Each method here
/// takes the mutex, runs one statement, and drops it, so the longest a save can
/// wait on Weave is one SQL write.
struct LockedIndex<'a> {
    index: &'a Mutex<Option<arc_labs_index::Index>>,
}

impl LockedIndex<'_> {
    fn with<T>(
        &self,
        f: impl FnOnce(&arc_labs_index::Index) -> arc_labs_weave::Result<T>,
    ) -> arc_labs_weave::Result<T> {
        let guard = self.index.lock().expect("index lock poisoned");
        let index = guard
            .as_ref()
            .ok_or_else(|| arc_labs_weave::WeaveError::Index("no index".into()))?;
        f(index)
    }
}

impl arc_labs_weave::Store for LockedIndex<'_> {
    fn pending(
        &self,
        model: &str,
        dimensions: usize,
    ) -> arc_labs_weave::Result<Vec<arc_labs_index::vectors::Pending>> {
        self.with(|i| arc_labs_weave::Store::pending(i, model, dimensions))
    }
    fn store_embedding(
        &self,
        note_id: i64,
        vector: &[f32],
        hash: &str,
        model: &str,
        dimensions: usize,
    ) -> arc_labs_weave::Result<()> {
        self.with(|i| {
            arc_labs_weave::Store::store_embedding(i, note_id, vector, hash, model, dimensions)
        })
    }
    fn nearest_unlinked(
        &self,
        per_note: usize,
        threshold: f64,
    ) -> arc_labs_weave::Result<Vec<(i64, i64, f64)>> {
        self.with(|i| arc_labs_weave::Store::nearest_unlinked(i, per_note, threshold))
    }
    fn suggest_link(
        &self,
        src: i64,
        dst: i64,
        score: f64,
        model: &str,
    ) -> arc_labs_weave::Result<bool> {
        self.with(|i| arc_labs_weave::Store::suggest_link(i, src, dst, score, model))
    }
}

/// Shared, cheap to clone, and safe to touch from the keystroke path.
pub struct WeaveState {
    pub budget: Budget,
    running: Arc<AtomicBool>,
    last_pass: Mutex<Option<PassReport>>,
    /// Held for the length of a pass, so only one runs at a time.
    ///
    /// Without it the daemon's pass and a user's "look now" can overlap, and two
    /// passes in one minute is two budgets in one minute: measured at 31% of a
    /// core against a 15% ceiling. The budget is per-Weave, so there has to be
    /// one Weave.
    pass_lock: Mutex<()>,
}

impl WeaveState {
    pub fn new() -> WeaveState {
        WeaveState {
            budget: Budget::default(),
            running: Arc::new(AtomicBool::new(false)),
            last_pass: Mutex::new(None),
            pass_lock: Mutex::new(()),
        }
    }
}

impl Default for WeaveState {
    fn default() -> Self {
        WeaveState::new()
    }
}

impl crate::Api {
    /// Tell Weave the user is here.
    ///
    /// Called on every keystroke from the editor and on every save. One atomic
    /// store — Phase 1's whole budget is 16 ms per keystroke and none of it
    /// belongs to this.
    pub fn note_user_activity(&self) {
        self.weave.budget.note_user_activity();
    }

    /// The embedder the config asks for.
    ///
    /// `mock` is not a hidden test hook: it is the answer for someone who wants
    /// the inbox surface to exist on a machine with no Ollama, and it is
    /// labelled `mock-embed` in every suggestion it produces, so nothing it
    /// says can be mistaken for a real judgement.
    fn embedder(&self) -> Arc<dyn Embedder> {
        let config = self.config();
        if config.model.embed == "mock" {
            return Arc::new(MockEmbedder);
        }
        Arc::new(OllamaEmbedder::new(
            config.model.endpoint.clone(),
            config.model.embed.clone(),
        ))
    }

    fn weaver(&self) -> Weave {
        let config = self.config();
        let mut w = Weave::new(self.embedder());
        w.threshold = config.weave.threshold;
        // The config may only make the budget *tighter*. The 15% ceiling is a
        // spec gate, not a preference, so a config file cannot raise it.
        w.budget = Budget::new(
            config
                .weave
                .cpu_fraction
                .min(arc_labs_weave::budget::DEFAULT_CPU_FRACTION),
            arc_labs_weave::budget::DEFAULT_QUIET_PERIOD,
            arc_labs_weave::budget::DEFAULT_QUEUE_LIMIT,
        );
        w
    }

    /// Run one pass: embed what changed, then suggest links.
    ///
    /// Synchronous and interruptible. The daemon loop calls this; so does
    /// `arc-labs weave --once`, which is how you get suggestions without leaving
    /// a background thread running.
    pub fn weave_pass(&self) -> ApiResult<PassReport> {
        // One pass at a time. A second caller is told so rather than queued:
        // waiting would mean holding a request open for a minute, and running
        // anyway would mean spending the budget twice.
        let Ok(_guard) = self.weave.pass_lock.try_lock() else {
            return Ok(PassReport {
                stopped_because: Some("AlreadyRunning".into()),
                ..Default::default()
            });
        };

        let weaver = Weave {
            budget: self.weave.budget.clone(),
            ..self.weaver()
        };

        // The vault is behind a read lock, which is shared, so holding it for
        // the pass costs nothing. The *index* is behind a mutex and is taken per
        // statement — see [`LockedIndex`].
        let state = self.state.read().expect("state lock poisoned");
        let vault = state.vault.as_ref().ok_or_else(ApiError::no_vault)?;
        {
            let guard = self.index.lock().expect("index lock poisoned");
            if guard.is_none() {
                return Err(ApiError::new(
                    ErrorCode::NoVault,
                    "the index is not ready yet",
                ));
            }
        }

        let store = LockedIndex { index: &self.index };
        let report = weaver
            .pass(&store, vault)
            .map_err(|e| ApiError::new(ErrorCode::Io, e.to_string()))?;
        *self.weave.last_pass.lock().expect("weave lock poisoned") = Some(report.clone());
        Ok(report)
    }

    pub fn weave_status(&self) -> ApiResult<WeaveStatus> {
        let config = self.config();
        let embedder = self.embedder();
        let (embedded, total) = self
            .with_index(|i| i.embedding_progress(embedder.name(), embedder.dimensions()))
            .unwrap_or((0, 0));
        let open = self.suggestions(500).map(|s| s.len()).unwrap_or(0);

        Ok(WeaveStatus {
            running: self.weave.running.load(Ordering::Relaxed),
            enabled: config.weave.enabled,
            model: embedder.name().to_string(),
            embedded,
            total,
            open_suggestions: open,
            cpu_fraction: self.weave.budget.observed_fraction(),
            cooling_secs: self.weave.budget.cooling_for().as_secs(),
            last_pass: self
                .weave
                .last_pass
                .lock()
                .expect("weave lock poisoned")
                .clone(),
        })
    }

    /// Open suggestions, best first.
    pub fn suggestions(&self, limit: usize) -> ApiResult<Vec<LinkSuggestion>> {
        let rows = self.with_index(|i| i.suggestions_detailed(limit.min(500)))?;
        Ok(rows
            .into_iter()
            .map(
                |(id, src_path, src_title, dst_path, dst_title, score, model, created_at)| {
                    LinkSuggestion {
                        id,
                        src_path,
                        src_title,
                        dst_path,
                        dst_title,
                        score,
                        model,
                        created_at,
                        inferred: true,
                    }
                },
            )
            .collect())
    }

    /// Refuse a suggestion. It does not come back.
    pub fn dismiss_suggestion(&self, id: i64) -> ApiResult<()> {
        self.with_index(|i| i.set_suggestion_state(id, "dismissed"))
    }

    /// Turn a suggestion into a real link in the source note.
    ///
    /// The write goes through the normal path, so it is atomic, fidelity-
    /// preserving and ledgered — and it is ledgered *twice*, as described at the
    /// top of this file.
    pub fn accept_suggestion(&self, id: i64) -> ApiResult<SaveResult> {
        let all = self.with_index(|i| i.suggestions_detailed(500))?;
        let row = all.into_iter().find(|r| r.0 == id).ok_or_else(|| {
            ApiError::new(ErrorCode::NoteNotFound, format!("no open suggestion {id}"))
        })?;
        let (_, src_path, _, dst_path, dst_title, score, model, _) = row;

        let path = VaultPath::new(&src_path)
            .map_err(|e| ApiError::new(ErrorCode::InvalidPath, e.to_string()))?;
        let note = self.read_note_for_edit(&path)?;
        let current = note.text.clone().unwrap_or_default();
        let target = link_target(&dst_path);
        let updated = with_related_link(&current, &target);

        if updated == current {
            // Already there. Close the suggestion rather than writing a no-op.
            self.dismiss_suggestion(id)?;
            return Ok(SaveResult {
                written: false,
                bytes: current.len(),
                hash: note.hash,
            });
        }

        let reason = format!("link to {dst_title} — similarity {score:.2}");
        let proposal = self.propose(&path, "weave", &model, "weave", &reason, &updated)?;
        let saved = self.accept(&path, proposal.index)?;
        self.dismiss_suggestion(id)?;
        Ok(saved)
    }
}

/// How a note is addressed in a wikilink: its path without the `.md`.
///
/// Not the bare stem. Two notes called `Index.md` in different folders are
/// normal in a real vault, and `[[Index]]` would then be ambiguous — Obsidian
/// resolves it to whichever it feels like. A path-qualified link is unambiguous
/// and Obsidian accepts it.
fn link_target(dst_path: &str) -> String {
    dst_path.strip_suffix(".md").unwrap_or(dst_path).to_string()
}

/// Add `[[target]]` under a `## Related` heading, creating the heading if there
/// isn't one.
///
/// Appending a bare wikilink to the bottom of a note is the obvious
/// implementation and it is wrong: a month later the note has a drift of
/// unexplained links at the end and no record of why. A named section says what
/// they are, and gives the user one thing to delete if they change their mind.
pub fn with_related_link(text: &str, target: &str) -> String {
    let link = format!("[[{target}]]");
    if text.contains(&link) {
        return text.to_string();
    }

    let heading = "## Related";
    if let Some(at) = text.find(heading) {
        // Insert at the end of the existing section, i.e. before the next
        // heading or at the end of the note.
        let after = at + heading.len();
        let rest = &text[after..];
        let end = rest
            .match_indices('\n')
            .find(|(i, _)| rest[i + 1..].starts_with("#"))
            .map(|(i, _)| after + i)
            .unwrap_or(text.len());
        let (head, tail) = text.split_at(end);
        let sep = if head.ends_with('\n') { "" } else { "\n" };
        return format!("{head}{sep}- {link}\n{}", tail.trim_start_matches('\n'));
    }

    // Exactly one blank line before the heading, whatever the note ended with.
    let sep = match text {
        _ if text.is_empty() || text.ends_with("\n\n") => "",
        _ if text.ends_with('\n') => "\n",
        _ => "\n\n",
    };
    format!("{text}{sep}{heading}\n\n- {link}\n")
}

/// A running Weave daemon. Dropping it stops the thread.
///
/// A free function rather than a method on [`crate::Api`] so the API does not
/// have to hold an `Arc` to itself.
pub struct WeaveDaemon {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl WeaveDaemon {
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for WeaveDaemon {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Start the background daemon.
///
/// It sleeps in short slices rather than one long one, so stopping the app does
/// not wait out a whole interval — the difference between a clean exit and one
/// that looks like a hang.
pub fn spawn(api: Arc<crate::Api>) -> WeaveDaemon {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    api.weave.running.store(true, Ordering::Relaxed);
    let running = api.weave.running.clone();

    let handle = std::thread::Builder::new()
        .name("arc-weave".into())
        .spawn(move || {
            let interval = api.config().weave.interval_secs.max(5);
            while !flag.load(Ordering::Relaxed) {
                // A pass is deliberately bounded, so "there is more to do" is
                // the normal case on a fresh vault. Go straight round again
                // rather than sleeping out the interval between every 32 notes —
                // the budget, not the interval, is what keeps this quiet.
                let (more, owed_ms) = match api.weave_pass() {
                    Ok(report) => {
                        if report.embedded > 0 || report.suggested > 0 {
                            tracing::info!(
                                embedded = report.embedded,
                                suggested = report.suggested,
                                remaining = report.remaining,
                                owed_ms = report.owed_ms,
                                cpu = report.cpu_fraction,
                                "weave pass"
                            );
                        }
                        (
                            report.remaining > 0 && report.stopped_because.is_none(),
                            report.owed_ms,
                        )
                    }
                    Err(e) => {
                        tracing::debug!(error = %e.message, "weave pass skipped");
                        (false, 0)
                    }
                };
                // Pay what the pass owes. This is where the 15% ceiling is
                // actually held, and it happens *here* — after `weave_pass`
                // returned and every lock it took is gone — rather than inside
                // the pass, so idling to stay within budget cannot block a save.
                //
                // Sleep in slices, so stopping the app does not wait out a whole
                // interval — the difference between a clean exit and a hang.
                let owed_slices = owed_ms / 250;
                let slices = if more {
                    owed_slices
                } else {
                    owed_slices + interval * 4
                };
                for _ in 0..slices.max(1) {
                    if flag.load(Ordering::Relaxed) {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
            }
            running.store(false, Ordering::Relaxed);
        })
        .expect("spawning the weave thread");

    WeaveDaemon {
        stop,
        handle: Some(handle),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Api, Capabilities};
    use arc_labs_core::{Config, VaultPath};

    fn api_with(files: &[(&str, &str)]) -> (tempfile::TempDir, std::sync::Arc<Api>) {
        let tmp = tempfile::tempdir().unwrap();
        for (name, body) in files {
            std::fs::write(tmp.path().join(name), body.as_bytes()).unwrap();
        }
        let mut config = Config::default();
        // The deterministic embedder, so this runs with Ollama switched off.
        config.model.embed = "mock".into();
        config.weave.threshold = -1.0;
        let api = std::sync::Arc::new(Api::new(config, None, Capabilities::desktop()));
        api.open_vault(tmp.path()).unwrap();
        api.open_index(false).unwrap();
        (tmp, api)
    }

    /// **Two passes at once is two budgets at once.**
    ///
    /// Measured at 31% of a core against a 15% ceiling before this was fixed,
    /// when a user pressed "look now" while the daemon was mid-pass.
    #[test]
    fn only_one_pass_runs_at_a_time() {
        let (_t, api) = api_with(&[
            (
                "a.md",
                "# A

body
",
            ),
            (
                "b.md",
                "# B

body
",
            ),
        ]);

        let held = api.weave.pass_lock.lock().unwrap();
        let report = api.weave_pass().unwrap();
        assert_eq!(report.stopped_because.as_deref(), Some("AlreadyRunning"));
        assert_eq!(report.embedded, 0, "a second pass did work anyway");
        drop(held);

        // And once the first one is done, the next may proceed.
        assert!(api.weave_pass().unwrap().embedded > 0);
    }

    /// The browser shell's only activity signal.
    #[test]
    fn saving_a_note_tells_weave_a_person_is_here() {
        let (_t, api) = api_with(&[(
            "a.md",
            "# A

body
",
        )]);
        let path = VaultPath::new("a.md").unwrap();
        let note = api.read_note_for_edit(&path).unwrap();

        api.write_note(
            &path,
            "# A

edited
",
            Some(&note.hash),
        )
        .unwrap();

        assert_eq!(
            api.weave.budget.may_work(),
            arc_labs_weave::Decision::UserActive,
            "a save must silence weave; it is the browser shell's only signal"
        );
    }

    /// The whole point, end to end: a suggestion becomes a real link only when a
    /// person says so, and the ledger records both halves of that.
    #[test]
    fn accepting_a_suggestion_links_the_notes_and_records_who_wanted_it() {
        let (_t, api) = api_with(&[
            (
                "a.md",
                "# Alpha

provenance and ledgers
",
            ),
            (
                "b.md",
                "# Beta

canvases and graphs
",
            ),
        ]);
        api.weave_pass().unwrap();

        let open = api.suggestions(10).unwrap();
        assert!(
            !open.is_empty(),
            "the mock embedder should have produced one"
        );
        let first = open[0].clone();
        assert!(first.inferred);
        assert_eq!(first.model, "mock-embed");

        let src = VaultPath::new(&first.src_path).unwrap();
        let before = api.read_note_for_edit(&src).unwrap().text.unwrap();

        api.accept_suggestion(first.id).unwrap();

        let after = api.read_note_for_edit(&src).unwrap().text.unwrap();
        assert_ne!(after, before);
        assert!(after.contains("## Related"), "got {after}");

        // Two entries: the agent proposed, the person accepted.
        let timeline = api.timeline(&src).unwrap();
        let proposed = timeline
            .iter()
            .find(|e| e.op == "propose")
            .expect("no propose entry");
        let accepted = timeline
            .iter()
            .find(|e| e.op == "accept")
            .expect("no accept entry");
        assert_eq!(proposed.actor_kind, "agent");
        assert_eq!(proposed.actor_id, "weave");
        assert!(!proposed.touched_file, "a proposal must not touch the file");
        // The accept stays in the agent register: an agent wrote those words,
        // and a timeline that turned amber the moment someone clicked Accept
        // would hide the very thing it exists to show. Who accepted is in the
        // reason, where the shared-vault question gets its answer.
        assert_eq!(accepted.actor_kind, "agent");
        assert!(
            accepted.touched_file,
            "an accepted proposal must change the file"
        );
        assert!(
            accepted.reason.starts_with("accepted by "),
            "got {}",
            accepted.reason
        );

        // And it is gone from the inbox rather than offered again.
        assert!(!api
            .suggestions(10)
            .unwrap()
            .iter()
            .any(|s| s.id == first.id));
    }

    #[test]
    fn dismissing_a_suggestion_leaves_the_note_alone() {
        let (_t, api) = api_with(&[
            (
                "a.md",
                "# A

body
",
            ),
            (
                "b.md",
                "# B

body
",
            ),
        ]);
        api.weave_pass().unwrap();
        let open = api.suggestions(10).unwrap();
        assert!(!open.is_empty());

        let src = VaultPath::new(&open[0].src_path).unwrap();
        let before = api.read_note_for_edit(&src).unwrap().text.unwrap();
        api.dismiss_suggestion(open[0].id).unwrap();

        assert_eq!(api.read_note_for_edit(&src).unwrap().text.unwrap(), before);
        // Gone, and it does not come back on the next pass. Other pairs are
        // untouched — dismissing one suggestion is not dismissing the inbox.
        api.weave_pass().unwrap();
        assert!(!api
            .suggestions(10)
            .unwrap()
            .iter()
            .any(|s| s.id == open[0].id));
    }

    #[test]
    fn a_related_section_is_created_once_and_reused() {
        let out = with_related_link("# A\n\nbody\n", "Notes/Beta");
        assert_eq!(out, "# A\n\nbody\n\n## Related\n\n- [[Notes/Beta]]\n");

        let twice = with_related_link(&out, "Notes/Gamma");
        assert_eq!(
            twice.matches("## Related").count(),
            1,
            "made a second section: {twice}"
        );
        assert!(twice.contains("- [[Notes/Beta]]"));
        assert!(twice.contains("- [[Notes/Gamma]]"));
    }

    #[test]
    fn adding_a_link_that_is_already_there_changes_nothing() {
        let text = "# A\n\n[[Beta]] is mentioned inline.\n";
        assert_eq!(with_related_link(text, "Beta"), text);
    }

    #[test]
    fn a_related_section_in_the_middle_does_not_swallow_what_follows() {
        let text = "# A\n\n## Related\n\n- [[One]]\n\n## Notes\n\nkeep me\n";
        let out = with_related_link(text, "Two");
        assert!(
            out.contains("## Notes\n\nkeep me\n"),
            "lost the next section: {out}"
        );
        let related = out.find("## Related").unwrap();
        let notes = out.find("## Notes").unwrap();
        assert!(
            out[related..notes].contains("[[Two]]"),
            "link landed outside the section: {out}"
        );
    }

    #[test]
    fn an_empty_note_gets_a_clean_section() {
        assert_eq!(with_related_link("", "Beta"), "## Related\n\n- [[Beta]]\n");
    }

    #[test]
    fn a_link_target_is_path_qualified_without_the_extension() {
        assert_eq!(link_target("Notes/Beta.md"), "Notes/Beta");
        assert_eq!(link_target("Beta.md"), "Beta");
        // Not a markdown file: leave it alone rather than guessing.
        assert_eq!(link_target("Diagrams/Plan.canvas"), "Diagrams/Plan.canvas");
    }
}
