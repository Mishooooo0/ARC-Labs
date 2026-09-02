//! Live run state.
//!
//! # Why polling, and not a stream
//!
//! Tokens have to reach the screen as they arrive. The obvious answer is a
//! push channel — SSE for the browser shell, Tauri events for the desktop — but
//! that is two transports to build, two to test, and a second way for the shells
//! to drift apart, which is the failure mode this architecture exists to avoid.
//!
//! Instead a run accumulates into a small shared buffer and the surface asks for
//! it. At 10 Hz the text arrives visibly live and the throughput figure updates
//! smoothly, the cost is one cheap lock per poll, and it works identically in
//! all four shells with no new transport machinery at all.
//!
//! If a later phase needs true push — a long-running Weave daemon reporting
//! progress, say — the buffer is already the right shape to push *from*.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use arc_labs_runtime::llm::Cancel;

use crate::types::{RunNodeState, RunState, RunStatus};

/// One run in flight, or recently finished.
#[derive(Debug, Clone)]
pub struct Run {
    pub status: RunStatus,
    pub cancel: Cancel,
}

/// Every run this process knows about.
#[derive(Default)]
pub struct Runs {
    inner: Mutex<HashMap<String, Run>>,
}

impl Runs {
    pub fn new() -> Arc<Runs> {
        Arc::new(Runs::default())
    }

    pub fn start(&self, id: &str, canvas: &str, target: &str, cancel: Cancel) {
        let status = RunStatus {
            id: id.to_string(),
            canvas: canvas.to_string(),
            target: target.to_string(),
            state: RunState::Running,
            nodes: Vec::new(),
            total_tokens: 0,
            elapsed_ms: 0,
            error: None,
            egress_to: None,
            egress_bytes: None,
        };
        self.with(|m| {
            m.insert(id.to_string(), Run { status, cancel });
            // Keep the last few runs and drop the rest: a canvas session can
            // produce many, and none of them are worth leaking memory for.
            prune(m);
        });
    }

    pub fn get(&self, id: &str) -> Option<RunStatus> {
        self.with(|m| m.get(id).map(|r| r.status.clone()))
    }

    pub fn list(&self) -> Vec<RunStatus> {
        let mut all: Vec<RunStatus> = self.with(|m| m.values().map(|r| r.status.clone()).collect());
        all.sort_by(|a, b| a.id.cmp(&b.id));
        all
    }

    pub fn cancel(&self, id: &str) -> bool {
        self.with(|m| match m.get_mut(id) {
            Some(run) => {
                run.cancel.cancel();
                run.status.state = RunState::Cancelled;
                true
            }
            None => false,
        })
    }

    pub fn node_started(&self, id: &str, node: &str, kind: &str) {
        self.update(id, |s| {
            s.nodes.push(RunNodeState {
                id: node.to_string(),
                kind: kind.to_string(),
                running: true,
                output: String::new(),
                tokens: None,
                tokens_per_sec: None,
                elapsed_ms: None,
                peak_rss_bytes: None,
                proposed_to: None,
            });
        });
    }

    pub fn token(&self, id: &str, node: &str, text: &str) {
        self.update(id, |s| {
            if let Some(n) = s.nodes.iter_mut().find(|n| n.id == node) {
                n.output.push_str(text);
            }
        });
    }

    pub fn node_finished(
        &self,
        id: &str,
        node: &str,
        output: &str,
        cost: Option<arc_labs_runtime::RunCost>,
    ) {
        self.update(id, |s| {
            if let Some(n) = s.nodes.iter_mut().find(|n| n.id == node) {
                n.running = false;
                n.output = output.to_string();
                if let Some(c) = cost {
                    n.tokens = Some(c.tokens);
                    n.tokens_per_sec = Some(c.tokens_per_sec);
                    n.elapsed_ms = Some(c.elapsed_ms);
                    n.peak_rss_bytes = Some(c.peak_rss_bytes);
                    s.total_tokens += c.tokens;
                }
            }
        });
    }

    /// Raise the in-flight egress indicator. Called after the ledger entry is
    /// written and before any byte leaves.
    pub fn egress(&self, id: &str, destination: &str, bytes: u64) {
        self.update(id, |s| {
            s.egress_to = Some(destination.to_string());
            s.egress_bytes = Some(s.egress_bytes.unwrap_or(0) + bytes);
        });
    }

    pub fn finish(&self, id: &str, state: RunState, error: Option<String>, elapsed_ms: u128) {
        self.update(id, |s| {
            // A run cancelled by the user stays cancelled: the error the
            // cancellation caused should not overwrite the reason.
            if s.state != RunState::Cancelled {
                s.state = state;
            }
            s.error = error.clone();
            s.elapsed_ms = elapsed_ms;
            for n in &mut s.nodes {
                n.running = false;
            }
        });
    }

    pub fn set_proposed(&self, id: &str, node: &str, path: &str) {
        self.update(id, |s| {
            if let Some(n) = s.nodes.iter_mut().find(|n| n.id == node) {
                n.proposed_to = Some(path.to_string());
            }
        });
    }

    fn update(&self, id: &str, f: impl FnOnce(&mut RunStatus)) {
        self.with(|m| {
            if let Some(run) = m.get_mut(id) {
                f(&mut run.status);
            }
        });
    }

    fn with<T>(&self, f: impl FnOnce(&mut HashMap<String, Run>) -> T) -> T {
        // A poisoned lock here means a panic inside a run. Recovering the data
        // is better than propagating the panic into every later poll.
        let mut guard = match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        f(&mut guard)
    }
}

const KEEP: usize = 20;

fn prune(m: &mut HashMap<String, Run>) {
    if m.len() <= KEEP {
        return;
    }
    let mut ids: Vec<String> = m
        .iter()
        .filter(|(_, r)| r.status.state != RunState::Running)
        .map(|(k, _)| k.clone())
        .collect();
    ids.sort();
    let excess = m.len().saturating_sub(KEEP);
    for id in ids.into_iter().take(excess) {
        m.remove(&id);
    }
}

/// A run id that sorts chronologically.
///
/// Time-prefixed so `list()` comes out oldest-first without storing a timestamp,
/// and suffixed with a counter so two runs started in the same millisecond do
/// not collide.
pub fn next_run_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!(
        "{ms:013}-{:04}",
        COUNTER.fetch_add(1, Ordering::Relaxed) % 10_000
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_accumulates_streamed_tokens() {
        let runs = Runs::new();
        runs.start("r1", "c.canvas", "n", Cancel::new());
        runs.node_started("r1", "p", "prompt");
        for piece in ["Hello", " ", "world"] {
            runs.token("r1", "p", piece);
        }

        let s = runs.get("r1").unwrap();
        assert_eq!(s.state, RunState::Running);
        assert_eq!(s.nodes[0].output, "Hello world");
        assert!(s.nodes[0].running);
    }

    #[test]
    fn finishing_a_node_records_its_throughput() {
        let runs = Runs::new();
        runs.start("r1", "c", "n", Cancel::new());
        runs.node_started("r1", "p", "prompt");
        runs.node_finished(
            "r1",
            "p",
            "done",
            Some(arc_labs_runtime::RunCost {
                tokens: 40,
                elapsed_ms: 2000,
                tokens_per_sec: 20.0,
                peak_rss_bytes: 1234,
            }),
        );

        let s = runs.get("r1").unwrap();
        assert_eq!(s.nodes[0].tokens, Some(40));
        assert_eq!(s.nodes[0].tokens_per_sec, Some(20.0));
        assert_eq!(s.total_tokens, 40);
        assert!(!s.nodes[0].running);
    }

    #[test]
    fn cancelling_sets_the_flag_and_the_state() {
        let runs = Runs::new();
        let cancel = Cancel::new();
        runs.start("r1", "c", "n", cancel.clone());

        assert!(runs.cancel("r1"));
        assert!(cancel.is_cancelled(), "the runner must actually see it");
        assert_eq!(runs.get("r1").unwrap().state, RunState::Cancelled);
        assert!(!runs.cancel("nope"));
    }

    #[test]
    fn a_cancelled_run_is_not_relabelled_as_failed() {
        // Cancelling causes the runner to return an error. That error must not
        // overwrite the reason the run stopped.
        let runs = Runs::new();
        runs.start("r1", "c", "n", Cancel::new());
        runs.cancel("r1");
        runs.finish(
            "r1",
            RunState::Failed,
            Some("the run was cancelled".into()),
            10,
        );

        assert_eq!(runs.get("r1").unwrap().state, RunState::Cancelled);
    }

    #[test]
    fn egress_is_visible_while_it_is_happening() {
        let runs = Runs::new();
        runs.start("r1", "c", "n", Cancel::new());
        runs.egress("r1", "http://workstation.local:11434", 512);
        runs.egress("r1", "http://workstation.local:11434", 256);

        let s = runs.get("r1").unwrap();
        assert_eq!(
            s.egress_to.as_deref(),
            Some("http://workstation.local:11434")
        );
        assert_eq!(s.egress_bytes, Some(768), "bytes accumulate across nodes");
    }

    #[test]
    fn old_finished_runs_are_pruned_but_running_ones_are_kept() {
        let runs = Runs::new();
        for i in 0..30 {
            let id = format!("r{i:03}");
            runs.start(&id, "c", "n", Cancel::new());
            if i < 25 {
                runs.finish(&id, RunState::Done, None, 1);
            }
        }
        let all = runs.list();
        assert!(
            all.len() <= KEEP + 5,
            "runs should be pruned, got {}",
            all.len()
        );
        // Everything still running survived.
        assert_eq!(
            all.iter().filter(|s| s.state == RunState::Running).count(),
            5
        );
    }

    #[test]
    fn run_ids_sort_chronologically_and_do_not_collide() {
        let ids: Vec<String> = (0..500).map(|_| next_run_id()).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted, "ids should already be in creation order");
        let unique: std::collections::HashSet<&String> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len(), "ids collided");
    }
}
