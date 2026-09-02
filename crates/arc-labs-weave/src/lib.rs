//! Weave: the background daemon that embeds notes and suggests links.
//!
//! # What it may and may not do
//!
//! It proposes links between notes that are semantically close and structurally
//! unlinked. It writes those proposals to an **inbox table** in the index. It
//! does not write to files, it does not edit notes, and it does not add links.
//!
//! That is constraint 7 as an architecture rather than a rule: suggestions live
//! in `suggested_links`, observed links live in `links`, and no query joins
//! across the two. A user must never have to click a connection to find out
//! whether it is real, and the way to guarantee that is for the inferred edges
//! to be physically incapable of arriving through the same door as the real ones.
//!
//! # It runs inside a budget, and the budget wins
//!
//! Phase 1's typing target outranks everything here. See [`budget`] — the rules
//! are enforced by the budget rather than trusted to this worker, and this
//! worker asks permission before every unit and charges the budget after it.
//!
//! A pass never sleeps. It holds the index open while it runs, and the save path
//! wants that same lock; sleeping with it held would make the duty cycle — the
//! mechanism protecting the editor — the thing that stalls it. So a pass returns
//! what it owes and the daemon pays it with nothing locked.

pub mod budget;
pub mod embed;

use std::sync::Arc;
use std::time::Instant;

use arc_labs_index::Index;

/// The slice of the index Weave touches.
///
/// Weave takes this rather than an `&Index` for one reason, and it is the
/// difference between a background daemon and a background daemon that freezes
/// the app: **an embedding call takes seconds, and the index lock must not be
/// held across it.**
///
/// The shell keeps the index behind a mutex. Handing Weave the guard for the
/// length of a pass meant every save queued behind an HTTP round trip to Ollama
/// — and on the server, saves run on the async runtime's worker threads, so
/// enough of them blocked the whole process, not just the editor. Every method
/// here is one short statement, so the shell can take and release the lock
/// around each one and the longest a save can ever wait is a single SQL write.
pub trait Store {
    fn pending(
        &self,
        model: &str,
        dimensions: usize,
    ) -> Result<Vec<arc_labs_index::vectors::Pending>>;
    fn store_embedding(
        &self,
        note_id: i64,
        vector: &[f32],
        hash: &str,
        model: &str,
        dimensions: usize,
    ) -> Result<()>;
    fn nearest_unlinked(&self, per_note: usize, threshold: f64) -> Result<Vec<(i64, i64, f64)>>;
    fn suggest_link(&self, src: i64, dst: i64, score: f64, model: &str) -> Result<bool>;
}

/// The direct implementation, for tests and anything holding the index outright.
impl Store for Index {
    fn pending(
        &self,
        model: &str,
        dimensions: usize,
    ) -> Result<Vec<arc_labs_index::vectors::Pending>> {
        self.notes_needing_embedding(model, dimensions)
            .map_err(|e| WeaveError::Index(e.to_string()))
    }
    fn store_embedding(
        &self,
        note_id: i64,
        vector: &[f32],
        hash: &str,
        model: &str,
        dimensions: usize,
    ) -> Result<()> {
        Index::store_embedding(self, note_id, vector, hash, model, dimensions)
            .map_err(|e| WeaveError::Index(e.to_string()))
    }
    fn nearest_unlinked(&self, per_note: usize, threshold: f64) -> Result<Vec<(i64, i64, f64)>> {
        Index::nearest_unlinked(self, per_note, threshold)
            .map_err(|e| WeaveError::Index(e.to_string()))
    }
    fn suggest_link(&self, src: i64, dst: i64, score: f64, model: &str) -> Result<bool> {
        Index::suggest_link(self, src, dst, score, model)
            .map_err(|e| WeaveError::Index(e.to_string()))
    }
}

pub use budget::{Budget, Decision};
pub use embed::{Embedder, MockEmbedder, OllamaEmbedder};

#[derive(Debug, thiserror::Error)]
pub enum WeaveError {
    #[error("index: {0}")]
    Index(String),
    #[error("embedding: {0}")]
    Embed(String),
}

pub type Result<T> = std::result::Result<T, WeaveError>;

/// What one pass accomplished.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PassReport {
    pub embedded: usize,
    pub skipped_unchanged: usize,
    pub suggested: usize,
    /// Why the pass ended, if it stopped early.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stopped_because: Option<String>,
    pub elapsed_ms: u128,
    pub cpu_fraction: f64,
    /// How long the caller must idle before doing more work, to keep inside the
    /// budget.
    ///
    /// Returned rather than slept off inside the pass, because a pass holds the
    /// index open and sleeping there would block the save path it exists to
    /// protect. The daemon pays this after releasing everything.
    pub owed_ms: u64,
    /// Notes still waiting after this pass.
    ///
    /// The daemon reads it to decide whether to loop straight round again or
    /// sleep out the interval. It is on the report rather than a separate query
    /// because the caller has just done the work of counting.
    pub remaining: usize,
}

/// A link Weave thinks might belong.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub id: i64,
    pub src_path: String,
    pub src_title: String,
    pub dst_path: String,
    pub dst_title: String,
    /// Cosine similarity, 0..1. Shown to the user, because the spec requires an
    /// inferred edge to carry its score wherever it appears.
    pub score: f64,
    /// The model that produced it. Also required to be visible.
    pub model: String,
    pub created_at: String,
}

pub struct Weave {
    pub embedder: Arc<dyn Embedder>,
    pub budget: Budget,
    /// Similarity above which a pair is worth suggesting.
    ///
    /// 0.82 is deliberately high. A notebook's value is that its connections
    /// mean something; an inbox full of weak suggestions trains the user to
    /// ignore it, which costs more than showing nothing would.
    pub threshold: f64,
    /// Suggestions per note per pass.
    pub per_note: usize,
    /// Notes embedded per batch.
    pub batch: usize,
    /// Characters of each note fed to the embedder.
    pub note_chars: usize,
    /// Notes one call to [`Weave::pass`] may embed before returning.
    ///
    /// A bound on how long one pass runs, so the daemon comes up for air:
    /// re-reads the budget, notices the user started typing, and lets the shell
    /// stop it. Passes are resumable by construction, so returning early costs
    /// nothing but a re-query.
    pub max_notes: usize,
}

impl Weave {
    pub fn new(embedder: Arc<dyn Embedder>) -> Weave {
        Weave {
            embedder,
            budget: Budget::default(),
            threshold: 0.82,
            per_note: 3,
            batch: 8,
            note_chars: 2000,
            max_notes: 32,
        }
    }

    /// Embed everything that has changed, then suggest links.
    ///
    /// Resumable by construction: progress is recorded per note in
    /// `embed_state`, so a pass that is killed halfway loses at most the batch
    /// in flight, and the next pass picks up exactly where it left off.
    pub fn pass(&self, index: &dyn Store, vault: &arc_labs_core::Vault) -> Result<PassReport> {
        let started = Instant::now();
        let mut report = PassReport::default();

        // The entry fee. A pass may not begin until the last one has been paid
        // for — which is what stops a user pressing "look now" from spending the
        // budget a second time in the same minute.
        if let other @ (Decision::UserActive
        | Decision::QueueBacked
        | Decision::Stopped
        | Decision::Cooling) = self.budget.may_work()
        {
            report.stopped_because = Some(format!("{other:?}"));
            report.elapsed_ms = started.elapsed().as_millis();
            report.cpu_fraction = self.budget.observed_fraction();
            return Ok(report);
        }

        // Time actually spent working, accumulated across the whole pass and
        // charged once at the end. Charging per batch instead meant the debt
        // from the last batch of embedding blocked the suggestion step in the
        // same pass, so suggestions only ever appeared a pass late.
        let mut worked = std::time::Duration::ZERO;

        let pending = index.pending(self.embedder.name(), self.embedder.dimensions())?;

        for chunk in pending.chunks(self.batch) {
            if report.embedded >= self.max_notes {
                break;
            }
            if let Some(stop) = self.budget.interruption() {
                report.stopped_because = Some(format!("{stop:?}"));
                break;
            }
            let unit = Instant::now();

            let mut ids = Vec::new();
            let mut texts = Vec::new();
            let mut hashes = Vec::new();
            for (id, path, title, hash) in chunk {
                let Ok(vp) = arc_labs_core::VaultPath::new(path) else {
                    continue;
                };
                let Ok(note) = vault.read_note(&vp) else {
                    continue;
                };
                ids.push(*id);
                hashes.push(hash.clone());
                texts.push(embed::text_for_note(title, note.text(), self.note_chars));
            }
            if ids.is_empty() {
                continue;
            }

            let vectors = self
                .embedder
                .embed(&texts)
                .map_err(|e| WeaveError::Embed(e.to_string()))?;
            for ((id, hash), mut v) in ids.iter().zip(hashes).zip(vectors) {
                if v.len() != self.embedder.dimensions() {
                    continue;
                }
                embed::normalise(&mut v);
                index.store_embedding(
                    *id,
                    &v,
                    &hash,
                    self.embedder.name(),
                    self.embedder.dimensions(),
                )?;
                report.embedded += 1;
            }

            worked += unit.elapsed();
        }

        report.remaining = pending.len().saturating_sub(report.embedded);
        report.skipped_unchanged = report.remaining;
        // Only look for links once the vault is fully embedded. Suggesting from
        // a half-embedded index produces a first batch of suggestions drawn from
        // whichever notes happened to be indexed first, which reads as a
        // judgement about those notes and is not one.
        if report.stopped_because.is_none() && report.remaining == 0 {
            let unit = Instant::now();
            report.suggested = self.suggest(index)?;
            worked += unit.elapsed();
        }

        report.elapsed_ms = started.elapsed().as_millis();
        report.owed_ms = self.budget.charge(worked).as_millis() as u64;
        report.cpu_fraction = self.budget.observed_fraction();
        Ok(report)
    }

    /// Propose links between close, unlinked notes.
    fn suggest(&self, index: &dyn Store) -> Result<usize> {
        let candidates = index.nearest_unlinked(self.per_note, self.threshold)?;

        let mut written = 0;
        for (src, dst, score) in candidates {
            if self.budget.interruption().is_some() {
                break;
            }
            if index.suggest_link(src, dst, score, self.embedder.name())? {
                written += 1;
            }
        }
        Ok(written)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arc_labs_core::Vault;

    /// A Weave whose budget is out of the way.
    ///
    /// The budget is tested in [`budget`], thoroughly. These tests are about
    /// what a pass *does*, and running them under the real 15% ceiling would
    /// mean each one sleeping out a cooling period to prove something about
    /// embedding. An explicit unbudgeted Weave says that plainly; the
    /// alternative — tests that quietly depend on the budget being lenient — is
    /// how the five-second-cap regression survived as long as it did.
    fn unbudgeted() -> Weave {
        let mut w = Weave::new(Arc::new(MockEmbedder));
        w.budget = Budget::new(1.0, std::time::Duration::ZERO, usize::MAX);
        w
    }

    fn vault_with(files: &[(&str, &str)]) -> (tempfile::TempDir, Vault, Index) {
        let tmp = tempfile::tempdir().unwrap();
        for (name, body) in files {
            std::fs::write(tmp.path().join(name), body.as_bytes()).unwrap();
        }
        let vault = Vault::open(tmp.path()).unwrap();
        let mut index = Index::open_for_vault(tmp.path()).unwrap();
        index.build(&vault, false, |_| {}).unwrap();
        (tmp, vault, index)
    }

    #[test]
    fn a_pass_embeds_every_note_once_and_skips_them_afterwards() {
        let (_t, vault, index) = vault_with(&[
            ("a.md", "# Alpha\n\nabout provenance and ledgers\n"),
            ("b.md", "# Beta\n\nabout canvases and graphs\n"),
        ]);
        let w = unbudgeted();

        let first = w.pass(&index, &vault).unwrap();
        assert_eq!(first.embedded, 2);

        // Resumability and idempotence: nothing has changed, so nothing is done.
        let second = w.pass(&index, &vault).unwrap();
        assert_eq!(second.embedded, 0, "unchanged notes should be skipped");
    }

    #[test]
    fn a_changed_note_is_re_embedded() {
        let (tmp, vault, index) = vault_with(&[("a.md", "# A\n\noriginal\n")]);
        let w = unbudgeted();
        w.pass(&index, &vault).unwrap();

        std::fs::write(
            tmp.path().join("a.md"),
            b"# A\n\ncompletely different content\n",
        )
        .unwrap();
        let mut index = index;
        index.build(&vault, false, |_| {}).unwrap();

        assert_eq!(w.pass(&index, &vault).unwrap().embedded, 1);
    }

    /// **Constraint 7 as an architecture.**
    #[test]
    fn suggestions_never_reach_the_observed_links_table() {
        let (_t, vault, index) = vault_with(&[
            ("a.md", "# A\n\nsome content\n"),
            ("b.md", "# B\n\nother content\n"),
        ]);
        let mut w = unbudgeted();
        // Force suggestions regardless of what the mock's vectors happen to be.
        w.threshold = -1.0;
        let report = w.pass(&index, &vault).unwrap();
        assert!(
            report.suggested > 0,
            "the test needs at least one suggestion"
        );

        // Observed links: still none. The notes do not link to each other.
        let observed = index.outgoing("a.md").unwrap();
        assert!(
            observed.is_empty(),
            "a suggestion leaked into observed links: {observed:?}"
        );

        // And the suggestion carries its score and its model, as the spec requires.
        let inbox = index.suggestions(50).unwrap();
        assert!(!inbox.is_empty());
        assert!(inbox[0].2 >= -1.0 && inbox[0].2 <= 1.0);
        assert_eq!(inbox[0].3, "mock-embed");
    }

    /// Similarity is symmetric, so the scan finds every pair twice. The inbox
    /// must not show it twice.
    #[test]
    fn a_pair_is_suggested_once_not_once_per_direction() {
        let (_t, vault, index) = vault_with(&[
            (
                "a.md",
                "# A

content
",
            ),
            (
                "b.md",
                "# B

content
",
            ),
        ]);
        let mut w = unbudgeted();
        w.threshold = -1.0;
        w.pass(&index, &vault).unwrap();

        let inbox = index.suggestions(50).unwrap();
        assert_eq!(
            inbox.len(),
            1,
            "the same relationship appeared twice: {inbox:?}"
        );
    }

    #[test]
    fn a_note_is_never_suggested_to_itself() {
        let (_t, vault, index) = vault_with(&[("only.md", "# Only\n\nalone here\n")]);
        let mut w = unbudgeted();
        w.threshold = -1.0;
        w.pass(&index, &vault).unwrap();
        assert_eq!(index.suggestions(50).unwrap().len(), 0);
    }

    #[test]
    fn an_already_linked_pair_is_not_suggested() {
        // Suggesting a link that already exists is noise, and worse, it makes
        // the inbox look like it does not know what is in the vault.
        let (_t, vault, index) = vault_with(&[
            ("a.md", "# A\n\nthis links to [[b]]\n"),
            ("b.md", "# B\n\nplain\n"),
        ]);
        let mut w = unbudgeted();
        w.threshold = -1.0;
        w.pass(&index, &vault).unwrap();

        for (src, dst, _, _, _) in index.suggestions(50).unwrap() {
            assert!(
                !(src == "a.md" && dst == "b.md"),
                "suggested a link that already exists"
            );
        }
    }

    /// **The budget gates a pass at the door, not only the daemon's sleep.**
    ///
    /// A user pressing "look now" while the daemon has just finished a pass used
    /// to get a second full pass for free: two batches in one minute, 29% of a
    /// core against a 15% ceiling.
    #[test]
    fn a_pass_will_not_start_until_the_last_one_is_paid_for() {
        let (_t, vault, index) = vault_with(&[
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
            (
                "c.md",
                "# C

body
",
            ),
        ]);
        // The real ceiling this time, not `unbudgeted()`.
        let w = Weave::new(Arc::new(MockEmbedder));

        let first = w.pass(&index, &vault).unwrap();
        assert!(first.embedded > 0);
        assert!(first.owed_ms > 0, "a pass that did work must owe something");

        let second = w.pass(&index, &vault).unwrap();
        assert_eq!(second.stopped_because.as_deref(), Some("Cooling"));
        assert_eq!(second.embedded, 0, "a second pass ran inside the budget");
    }

    /// Suggestions must land in the same pass that finishes the embedding.
    ///
    /// They used to be a pass late: the last batch charged the budget, and the
    /// suggestion step — checking the same budget — found it already in debt and
    /// bailed. Charging once for the whole pass is what fixes it.
    #[test]
    fn the_pass_that_finishes_embedding_also_suggests() {
        let (_t, vault, index) = vault_with(&[
            (
                "a.md",
                "# A

provenance
",
            ),
            (
                "b.md",
                "# B

canvases
",
            ),
        ]);
        let mut w = Weave::new(Arc::new(MockEmbedder));
        w.threshold = -1.0;

        let report = w.pass(&index, &vault).unwrap();
        assert_eq!(report.remaining, 0);
        assert!(
            report.suggested > 0,
            "the pass that emptied the queue should also have suggested"
        );
    }

    /// A pass must be short enough that a save is not made to wait behind it.
    #[test]
    fn a_pass_stops_at_max_notes_and_reports_what_is_left() {
        let files: Vec<(String, String)> = (0..20)
            .map(|i| {
                (
                    format!("n{i}.md"),
                    format!(
                        "# Note {i}

body {i}
"
                    ),
                )
            })
            .collect();
        let refs: Vec<(&str, &str)> = files
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        let (_t, vault, index) = vault_with(&refs);

        let mut w = unbudgeted();
        w.max_notes = 8;
        w.batch = 4;

        let first = w.pass(&index, &vault).unwrap();
        assert_eq!(first.embedded, 8);
        assert_eq!(first.remaining, 12);
        // The debt comes back rather than being slept off under the lock.
        assert!(
            first.elapsed_ms < 2000,
            "a pass should not sleep: {} ms",
            first.elapsed_ms
        );
        // Nothing is suggested from a half-embedded vault.
        assert_eq!(first.suggested, 0);

        // Resumable: the next pass picks up exactly where this one stopped.
        let second = w.pass(&index, &vault).unwrap();
        assert_eq!(second.embedded, 8);
        assert_eq!(second.remaining, 4);

        let third = w.pass(&index, &vault).unwrap();
        assert_eq!(third.embedded, 4);
        assert_eq!(third.remaining, 0);
        assert_eq!(w.pass(&index, &vault).unwrap().embedded, 0);
    }

    #[test]
    fn typing_stops_a_pass_and_it_says_so() {
        let files: Vec<(String, String)> = (0..60)
            .map(|i| (format!("n{i}.md"), format!("# Note {i}\n\nbody {i}\n")))
            .collect();
        let refs: Vec<(&str, &str)> = files
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        let (_t, vault, index) = vault_with(&refs);

        let w = Weave::new(Arc::new(MockEmbedder));
        // The user is typing right now.
        w.budget.note_user_activity();

        let report = w.pass(&index, &vault).unwrap();
        assert_eq!(
            report.embedded, 0,
            "nothing should run while the user types"
        );
        assert_eq!(report.stopped_because.as_deref(), Some("UserActive"));
    }

    #[test]
    fn a_backed_up_index_stops_a_pass() {
        let (_t, vault, index) = vault_with(&[("a.md", "# A\n"), ("b.md", "# B\n")]);
        let w = Weave::new(Arc::new(MockEmbedder));
        w.budget.set_pending_writes(10_000);

        let report = w.pass(&index, &vault).unwrap();
        assert_eq!(report.stopped_because.as_deref(), Some("QueueBacked"));
        assert_eq!(report.embedded, 0);
    }

    #[test]
    fn dismissing_a_suggestion_keeps_it_from_coming_back() {
        let (_t, vault, index) =
            vault_with(&[("a.md", "# A\n\ncontent\n"), ("b.md", "# B\n\ncontent\n")]);
        let mut w = unbudgeted();
        w.threshold = -1.0;
        w.pass(&index, &vault).unwrap();

        let before = index.suggestions(50).unwrap();
        assert!(!before.is_empty());
        index
            .set_suggestion_state(before[0].4, "dismissed")
            .unwrap();

        // Another pass must not resurrect it.
        w.pass(&index, &vault).unwrap();
        let after = index.suggestions(50).unwrap();
        assert!(
            !after.iter().any(|s| s.4 == before[0].4),
            "a dismissed suggestion came back"
        );
    }
}
