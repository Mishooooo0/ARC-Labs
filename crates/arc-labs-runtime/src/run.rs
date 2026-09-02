//! Running a pipeline.
//!
//! # Egress is recorded before the bytes leave
//!
//! The spec is careful here, and so is this: a run that sends vault content off
//! the machine writes its `egress` ledger entry **first**. Recording afterwards
//! would mean a crash, a network failure or a kill mid-request produces exactly
//! the case the ledger exists to rule out — bytes that left with no record that
//! they did.
//!
//! The three-state policy is enforced in one place, [`Runner::check_egress`], so
//! there is no path to a model that skips it.
//!
//! # Agent output never touches a file
//!
//! A pipeline that writes to a note produces a **proposal**. The note is not
//! modified, its mtime does not move, and a human decides. That is constraint 4,
//! and it is why [`Runner`] holds a ledger but no write path of its own.

use std::collections::HashMap;

use arc_labs_canvas::{ArcKind, Canvas};
use arc_labs_core::{Config, ModelAccess, Vault, VaultPath};
use arc_labs_index::Index;
use arc_labs_ledger::{Actor, Ledger};

use crate::graph::{plan_for, Graph, Plan};
use crate::llm::{Cancel, GenerateRequest, Llm, LlmError, RunCost};
use crate::transform::Transform;

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("this canvas has a cycle: {}", .0.join(", "))]
    Cycle(Vec<String>),
    #[error("no node {0} on this canvas")]
    NoSuchNode(String),
    #[error("model access is set to local-only, and {0} is not this machine")]
    EgressBlocked(String),
    #[error("this run needs approval to send vault content to {0}")]
    EgressNeedsApproval(String),
    #[error(transparent)]
    Llm(#[from] LlmError),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, RunError>;

/// What one node produced.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeResult {
    pub id: String,
    pub kind: String,
    pub output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<RunCostView>,
    /// Set when this node's output became a proposal on a note.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposed_to: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunCostView {
    pub tokens: usize,
    pub elapsed_ms: u128,
    pub tokens_per_sec: f64,
    pub peak_rss_bytes: u64,
}

impl From<RunCost> for RunCostView {
    fn from(c: RunCost) -> Self {
        RunCostView {
            tokens: c.tokens,
            elapsed_ms: c.elapsed_ms,
            tokens_per_sec: c.tokens_per_sec,
            peak_rss_bytes: c.peak_rss_bytes,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunReport {
    pub canvas: String,
    pub results: Vec<NodeResult>,
    pub total_ms: u128,
    pub total_tokens: usize,
    /// Where vault bytes went, when they went anywhere.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub egress_to: Option<String>,
}

/// What the caller is told while a run is in flight.
#[derive(Debug, Clone)]
pub enum Event {
    NodeStarted {
        id: String,
        kind: String,
    },
    Token {
        id: String,
        text: String,
    },
    NodeFinished {
        id: String,
        output: String,
        cost: Option<RunCost>,
    },
    /// Emitted immediately *after* the egress entry is written and *before* any
    /// byte is sent, so a surface can raise its in-flight indicator at the right
    /// moment rather than optimistically.
    Egress {
        destination: String,
        bytes: u64,
    },
}

pub struct Runner<'a> {
    pub vault: &'a Vault,
    pub index: Option<&'a Index>,
    pub ledger: &'a Ledger,
    pub llm: &'a dyn Llm,
    pub config: &'a Config,
    /// The session id recorded on every entry this run produces, so a whole run
    /// can be reviewed — or undone — as a unit.
    pub session: String,
}

impl Runner<'_> {
    /// Enforce the model-access policy.
    ///
    /// The single gate between a pipeline and the network. `AskEachRun` returns
    /// an error rather than prompting: this crate has no user to ask, and a
    /// library that blocked on a dialog would be unusable from a server or a
    /// test. The shell asks, then re-runs with approval.
    fn check_egress(&self, approved: bool) -> Result<Option<String>> {
        let endpoint = &self.config.model.endpoint;
        if crate::llm::is_local(endpoint) {
            return Ok(None);
        }
        match self.config.model.access {
            ModelAccess::LocalOnly => Err(RunError::EgressBlocked(endpoint.clone())),
            ModelAccess::TrustedEndpoint => Ok(Some(endpoint.clone())),
            ModelAccess::AskEachRun if approved => Ok(Some(endpoint.clone())),
            ModelAccess::AskEachRun => Err(RunError::EgressNeedsApproval(endpoint.clone())),
        }
    }

    /// Run the subgraph feeding `target`.
    pub fn run(
        &self,
        canvas_path: &VaultPath,
        target: &str,
        approved_egress: bool,
        cancel: &Cancel,
        on_event: &mut dyn FnMut(Event),
    ) -> Result<RunReport> {
        let started = std::time::Instant::now();

        let source = self
            .vault
            .read_note(canvas_path)
            .map_err(|e| RunError::Other(e.public()))?;
        let canvas = Canvas::parse(source.text()).map_err(|e| RunError::Other(e.to_string()))?;
        let graph = Graph::from_canvas(&canvas);

        if !graph.is_executable(target) {
            return Err(RunError::NoSuchNode(target.to_string()));
        }
        let order = match plan_for(&graph, target) {
            Plan::Order(o) => o,
            Plan::Cycle(nodes) => return Err(RunError::Cycle(nodes)),
        };

        let destination = self.check_egress(approved_egress)?;

        let mut outputs: HashMap<String, String> = HashMap::new();
        let mut results: Vec<NodeResult> = Vec::new();
        let mut total_tokens = 0usize;
        let mut egress_recorded = false;

        for id in &order {
            if cancel.is_cancelled() {
                return Err(RunError::Llm(LlmError::Cancelled));
            }

            // Non-executable cards contribute their content as input.
            if let Some(file) = graph.files.get(id) {
                let text = VaultPath::new(file)
                    .ok()
                    .and_then(|p| self.vault.read_note(&p).ok())
                    .map(|n| n.text().to_string())
                    .unwrap_or_default();
                outputs.insert(id.clone(), text);
                continue;
            }
            if let Some(text) = graph.texts.get(id) {
                outputs.insert(id.clone(), text.clone());
                continue;
            }

            let Some(node) = graph.nodes.get(id) else {
                continue;
            };
            let inputs = self.inputs_for(&graph, id, &outputs);
            on_event(Event::NodeStarted {
                id: id.clone(),
                kind: node.kind.as_str().to_string(),
            });

            let (output, cost) = match node.kind {
                ArcKind::Query => (self.run_query(node, &inputs)?, None),
                ArcKind::Transform => (Transform::parse(&node.text).apply(&inputs), None),
                ArcKind::Prompt => {
                    let prompt = fill_slots(&node.text, &graph, id, &outputs, &inputs);

                    // Recorded *before* the bytes leave. Afterwards would mean a
                    // crash mid-request produces the one case the ledger exists
                    // to rule out: content that left with no record.
                    if let (Some(dest), false) = (destination.as_ref(), egress_recorded) {
                        self.ledger
                            .record_egress(
                                canvas_path,
                                self.agent(node),
                                dest.clone(),
                                prompt.len() as u64,
                            )
                            .map_err(|e| RunError::Other(e.public()))?;
                        on_event(Event::Egress {
                            destination: dest.clone(),
                            bytes: prompt.len() as u64,
                        });
                        egress_recorded = true;
                    }

                    let mut req = GenerateRequest::new(
                        node.option_str("model")
                            .unwrap_or(&self.config.model.instruct),
                        prompt,
                    );
                    req.temperature = node.option_f64("temperature").unwrap_or(0.0) as f32;
                    // Both overridable per card, for the case where a bigger
                    // context genuinely is worth the wait.
                    if let Some(n) = node.option_usize("context") {
                        req.context_tokens = n;
                    }
                    if let Some(n) = node.option_usize("maxTokens") {
                        req.max_tokens = n;
                    }
                    let id_for_tokens = id.clone();
                    let generated = self.llm.generate(&req, cancel, &mut |t| {
                        on_event(Event::Token {
                            id: id_for_tokens.clone(),
                            text: t.to_string(),
                        })
                    })?;
                    total_tokens += generated.cost.tokens;
                    let text = if generated.thought_but_did_not_answer {
                        // Better than an empty card: say what happened and what
                        // to change.
                        format!(
                            "_The model used its whole {}-token budget reasoning and did not                              reach an answer. Raise `maxTokens` on this card._",
                            req.max_tokens
                        )
                    } else {
                        generated.text
                    };
                    (text, Some(generated.cost))
                }
            };

            outputs.insert(id.clone(), output.clone());
            on_event(Event::NodeFinished {
                id: id.clone(),
                output: output.clone(),
                cost,
            });

            // A downstream file card is a target: the output becomes a proposal
            // on that note, never a write.
            let proposed_to = self.propose_downstream(&graph, id, &output, node)?;

            results.push(NodeResult {
                id: id.clone(),
                kind: node.kind.as_str().to_string(),
                output,
                cost: cost.map(Into::into),
                proposed_to,
            });
        }

        Ok(RunReport {
            canvas: canvas_path.to_string(),
            results,
            total_ms: started.elapsed().as_millis(),
            total_tokens,
            egress_to: destination,
        })
    }

    fn agent(&self, node: &crate::graph::RunNode) -> Actor {
        Actor::agent(
            "canvas",
            node.option_str("model")
                .unwrap_or(&self.config.model.instruct),
            // The node id is in the session, so an entry names the card that
            // produced it and a run can be traced back to a point on a canvas.
            format!("{}#{}", self.session, node.id),
        )
    }

    fn inputs_for(
        &self,
        graph: &Graph,
        id: &str,
        outputs: &HashMap<String, String>,
    ) -> Vec<String> {
        // Sorted by node id, so a node with several inputs sees them in the same
        // order every run. The determinism gate depends on it.
        let mut sources: Vec<&String> = graph
            .inbound
            .get(id)
            .map(|v| v.iter().collect())
            .unwrap_or_default();
        sources.sort();
        sources
            .iter()
            .filter_map(|s| outputs.get(*s).cloned())
            .collect()
    }

    fn run_query(&self, node: &crate::graph::RunNode, inputs: &[String]) -> Result<String> {
        let Some(index) = self.index else {
            return Ok(String::new());
        };
        // A query card can take its text from an upstream node, so a pipeline
        // can search for something it just computed.
        let q = if node.text.trim().is_empty() {
            inputs.join(" ")
        } else {
            node.text.clone()
        };
        let limit = node.option_usize("limit").unwrap_or(10).min(100);

        let hits = index
            .search(q.trim(), limit)
            .map_err(|e| RunError::Other(e.to_string()))?;
        Ok(hits
            .iter()
            .map(|h| format!("## {}\n\n{}\n\n_{}_", h.title, h.snippet, h.path))
            .collect::<Vec<_>>()
            .join("\n\n"))
    }

    /// Turn output into a proposal on any downstream `file` card.
    fn propose_downstream(
        &self,
        graph: &Graph,
        id: &str,
        output: &str,
        node: &crate::graph::RunNode,
    ) -> Result<Option<String>> {
        let mut targets: Vec<&String> = graph
            .out
            .get(id)
            .map(|v| v.iter().collect())
            .unwrap_or_default();
        targets.sort();

        for target_id in targets {
            let Some(file) = graph.files.get(target_id) else {
                continue;
            };
            let Ok(path) = VaultPath::new(file) else {
                continue;
            };
            let Ok(current) = self.vault.read_note(&path) else {
                continue;
            };

            let mode = node.option_str("write").unwrap_or("append");
            let proposed = match mode {
                "replace" => output.to_string(),
                // Append by default. Replacing someone's note is a bigger claim
                // than a pipeline should make without being told to.
                _ => format!("{}\n\n{}\n", current.text().trim_end(), output.trim()),
            };

            self.ledger
                .propose(
                    &path,
                    self.agent(node),
                    format!("canvas node {} ({})", node.id, node.kind.as_str()),
                    current.text(),
                    &proposed,
                )
                .map_err(|e| RunError::Other(e.public()))?;
            return Ok(Some(path.to_string()));
        }
        Ok(None)
    }
}

/// Fill `{{slots}}` in a prompt template.
///
/// A slot may name an upstream node id, or `{{input}}` for all inputs joined.
/// An unknown slot is left as written rather than replaced with nothing —
/// silently emptying it would send the model a prompt the user never wrote.
pub fn fill_slots(
    template: &str,
    graph: &Graph,
    id: &str,
    outputs: &HashMap<String, String>,
    inputs: &[String],
) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;

    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = after[..end].trim();

        let replacement = if name == "input" || name.is_empty() {
            Some(inputs.join("\n\n"))
        } else if let Some(v) = outputs.get(name) {
            Some(v.clone())
        } else {
            // Also allow naming an inbound edge by index: {{1}}.
            name.parse::<usize>()
                .ok()
                .and_then(|i| inputs.get(i).cloned())
        };

        match replacement {
            Some(v) => out.push_str(&v),
            // Left verbatim: a slot the user mistyped should be visible in the
            // prompt, not silently blank.
            None => out.push_str(&rest[start..start + 2 + end + 2]),
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    let _ = (graph, id);
    out
}
