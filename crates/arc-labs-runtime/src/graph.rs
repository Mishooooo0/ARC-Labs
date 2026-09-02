//! The executable graph: what runs, in what order, and what refuses to run.
//!
//! # Cycles are rejected at edit time, not at run time
//!
//! The gate says a cycle is marked **within 100 ms** and disables Run. That is
//! not a performance target so much as a design constraint: the check has to be
//! cheap enough to do on every edit, so the user learns they have made a loop
//! while they are drawing it rather than when they press the button.
//!
//! Kahn's algorithm gives both answers from one pass — a topological order when
//! there is one, and the set of nodes still in the cycle when there is not. So
//! the same function that plans a run also draws the error.

use std::collections::{HashMap, HashSet, VecDeque};

use arc_labs_canvas::{ArcKind, Canvas};

/// A node that can run.
#[derive(Debug, Clone, PartialEq)]
pub struct RunNode {
    pub id: String,
    pub kind: ArcKind,
    /// The node's text: a prompt template, a query, or a transform spec.
    pub text: String,
    /// Options from the `arc` metadata key — model, temperature, and so on.
    pub options: serde_json::Map<String, serde_json::Value>,
}

impl RunNode {
    pub fn option_str(&self, key: &str) -> Option<&str> {
        self.options.get(key).and_then(|v| v.as_str())
    }
    pub fn option_f64(&self, key: &str) -> Option<f64> {
        self.options.get(key).and_then(|v| v.as_f64())
    }
    pub fn option_usize(&self, key: &str) -> Option<usize> {
        self.options
            .get(key)
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
    }
}

/// What a canvas contains, from the runtime's point of view.
#[derive(Debug, Clone, Default)]
pub struct Graph {
    /// Executable nodes, by id.
    pub nodes: HashMap<String, RunNode>,
    /// `from -> [to]`, restricted to edges between nodes that matter to a run.
    pub out: HashMap<String, Vec<String>>,
    /// `to -> [from]`. Inbound edges are what fill a prompt's slots.
    pub inbound: HashMap<String, Vec<String>>,
    /// Non-executable cards, kept because a `file` card is how a run reaches a
    /// note — as an input to read, or a target to write a proposal to.
    pub files: HashMap<String, String>,
    /// Text cards that are not ARC nodes: usable as literal inputs.
    pub texts: HashMap<String, String>,
}

impl Graph {
    /// Read a canvas into a runnable graph.
    pub fn from_canvas(canvas: &Canvas) -> Graph {
        let mut g = Graph::default();

        for node in &canvas.nodes {
            let id = node.id().to_string();
            match node.arc_kind() {
                Some(kind) => {
                    let options = node
                        .as_map()
                        .get(arc_labs_canvas::ARC_KEY)
                        .and_then(|v| v.as_object())
                        .cloned()
                        .unwrap_or_default();
                    g.nodes.insert(
                        id.clone(),
                        RunNode {
                            id,
                            kind,
                            text: node.text().unwrap_or_default().to_string(),
                            options,
                        },
                    );
                }
                None => {
                    if let Some(f) = node.file() {
                        g.files.insert(id, f.to_string());
                    } else if let Some(t) = node.text() {
                        g.texts.insert(id, t.to_string());
                    }
                }
            }
        }

        for edge in &canvas.edges {
            let (from, to) = (edge.from_node().to_string(), edge.to_node().to_string());
            g.out.entry(from.clone()).or_default().push(to.clone());
            g.inbound.entry(to).or_default().push(from);
        }
        g
    }

    pub fn is_executable(&self, id: &str) -> bool {
        self.nodes.contains_key(id)
    }
}

/// The result of planning a run.
#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    /// Execute in this order.
    Order(Vec<String>),
    /// A cycle. Holds the nodes still inside it, so the surface can mark the
    /// offending edges rather than saying "there is a cycle somewhere".
    Cycle(Vec<String>),
}

/// Every node that feeds `target`, including `target`, in dependency order.
///
/// The spec says Run executes *the ancestor subgraph*, not the whole canvas: a
/// canvas is a workspace and may hold several unrelated pipelines, so running
/// one must not run the others.
pub fn plan_for(graph: &Graph, target: &str) -> Plan {
    let mut wanted: HashSet<String> = HashSet::new();
    let mut stack = vec![target.to_string()];
    while let Some(id) = stack.pop() {
        if !wanted.insert(id.clone()) {
            continue;
        }
        for from in graph.inbound.get(&id).into_iter().flatten() {
            stack.push(from.clone());
        }
    }
    plan_subset(graph, &wanted)
}

/// A plan covering every executable node on the canvas.
pub fn plan_all(graph: &Graph) -> Plan {
    let mut wanted: HashSet<String> = graph.nodes.keys().cloned().collect();
    // Pull in the non-executable cards those nodes depend on, so a prompt fed by
    // a file card still sees it.
    for id in graph.nodes.keys() {
        for from in graph.inbound.get(id).into_iter().flatten() {
            wanted.insert(from.clone());
        }
    }
    plan_subset(graph, &wanted)
}

/// Kahn's algorithm over a subset.
///
/// One pass answers both questions: the order when the subgraph is acyclic, and
/// exactly which nodes remain when it is not. Anything left with a non-zero
/// in-degree at the end is in — or downstream of — a cycle.
fn plan_subset(graph: &Graph, wanted: &HashSet<String>) -> Plan {
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    for id in wanted {
        in_degree.entry(id.as_str()).or_insert(0);
    }
    for id in wanted {
        for from in graph.inbound.get(id).into_iter().flatten() {
            if wanted.contains(from) {
                *in_degree.entry(id.as_str()).or_insert(0) += 1;
            }
        }
    }

    // Sorted seeds so the order is deterministic. Phase 5's gate says the same
    // graph run twice produces identical output, and a HashMap iteration order
    // would quietly break that for any graph with two independent roots.
    let mut roots: Vec<&str> = in_degree
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(id, _)| *id)
        .collect();
    roots.sort_unstable();
    let mut queue: VecDeque<&str> = roots.into_iter().collect();

    let mut order: Vec<String> = Vec::with_capacity(wanted.len());
    while let Some(id) = queue.pop_front() {
        order.push(id.to_string());

        let mut ready: Vec<&str> = Vec::new();
        for to in graph.out.get(id).into_iter().flatten() {
            if !wanted.contains(to) {
                continue;
            }
            if let Some(d) = in_degree.get_mut(to.as_str()) {
                *d -= 1;
                if *d == 0 {
                    ready.push(to.as_str());
                }
            }
        }
        ready.sort_unstable();
        queue.extend(ready);
    }

    if order.len() == wanted.len() {
        Plan::Order(order)
    } else {
        let done: HashSet<&String> = order.iter().collect();
        let mut stuck: Vec<String> = wanted
            .iter()
            .filter(|id| !done.contains(id))
            .cloned()
            .collect();
        stuck.sort();
        Plan::Cycle(stuck)
    }
}

/// Whether the canvas contains a cycle at all.
///
/// The check the editor runs after every change. Cheap: one pass over nodes and
/// edges, no allocation per edge beyond the degree map.
pub fn find_cycle(graph: &Graph) -> Option<Vec<String>> {
    let all: HashSet<String> = graph
        .out
        .keys()
        .chain(graph.inbound.keys())
        .cloned()
        .collect();
    match plan_subset(graph, &all) {
        Plan::Cycle(nodes) => Some(nodes),
        Plan::Order(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas(nodes: &[(&str, &str, &str)], edges: &[(&str, &str)]) -> Canvas {
        // (id, arc kind or "", text)
        let node_json: Vec<String> = nodes
            .iter()
            .map(|(id, kind, text)| {
                let arc = if kind.is_empty() {
                    String::new()
                } else {
                    format!(",\"arc\":{{\"kind\":\"{kind}\"}}")
                };
                format!(
                    "{{\"id\":\"{id}\",\"type\":\"text\",\"text\":{},\"x\":0,\"y\":0,\"width\":10,\"height\":10{arc}}}",
                    serde_json::to_string(text).unwrap()
                )
            })
            .collect();
        let edge_json: Vec<String> = edges
            .iter()
            .enumerate()
            .map(|(i, (a, b))| {
                format!("{{\"id\":\"e{i}\",\"fromNode\":\"{a}\",\"toNode\":\"{b}\"}}")
            })
            .collect();
        let src = format!(
            "{{\n\t\"nodes\":[\n\t\t{}\n\t],\n\t\"edges\":[\n\t\t{}\n\t]\n}}",
            node_json.join(",\n\t\t"),
            edge_json.join(",\n\t\t")
        );
        Canvas::parse(&src).unwrap()
    }

    #[test]
    fn reads_arc_nodes_and_leaves_plain_cards_alone() {
        let c = canvas(
            &[
                ("q", "query", "ledger"),
                ("p", "prompt", "Summarise {{q}}"),
                ("plain", "", "hello"),
            ],
            &[("q", "p")],
        );
        let g = Graph::from_canvas(&c);

        assert_eq!(g.nodes.len(), 2);
        assert_eq!(g.nodes["q"].kind, ArcKind::Query);
        assert_eq!(g.nodes["p"].text, "Summarise {{q}}");
        assert!(!g.is_executable("plain"));
        assert_eq!(g.texts["plain"], "hello");
        assert_eq!(g.inbound["p"], ["q"]);
    }

    #[test]
    fn plans_a_pipeline_in_dependency_order() {
        let c = canvas(
            &[
                ("q", "query", "ledger"),
                ("p", "prompt", "{{q}}"),
                ("t", "transform", "dedupe"),
            ],
            &[("q", "p"), ("p", "t")],
        );
        let g = Graph::from_canvas(&c);
        assert_eq!(
            plan_for(&g, "t"),
            Plan::Order(vec!["q".into(), "p".into(), "t".into()])
        );
    }

    #[test]
    fn runs_only_the_ancestors_of_the_target() {
        // A canvas is a workspace and holds unrelated pipelines. Running one
        // must not run the others.
        let c = canvas(
            &[
                ("a", "query", "x"),
                ("b", "prompt", "{{a}}"),
                ("other", "query", "unrelated"),
                ("other2", "prompt", "{{other}}"),
            ],
            &[("a", "b"), ("other", "other2")],
        );
        let g = Graph::from_canvas(&c);
        let Plan::Order(order) = plan_for(&g, "b") else {
            panic!("expected an order")
        };
        assert_eq!(order, ["a", "b"]);
        assert!(!order.contains(&"other".to_string()));
    }

    #[test]
    fn a_cycle_is_reported_with_the_nodes_in_it() {
        let c = canvas(
            &[
                ("a", "prompt", "{{c}}"),
                ("b", "prompt", "{{a}}"),
                ("c", "prompt", "{{b}}"),
            ],
            &[("a", "b"), ("b", "c"), ("c", "a")],
        );
        let g = Graph::from_canvas(&c);

        let stuck = find_cycle(&g).expect("a cycle should be found");
        assert_eq!(stuck, ["a", "b", "c"], "all three are in the loop");
        assert!(matches!(plan_for(&g, "a"), Plan::Cycle(_)));
    }

    #[test]
    fn a_self_loop_is_a_cycle() {
        let c = canvas(&[("a", "prompt", "{{a}}")], &[("a", "a")]);
        assert!(find_cycle(&Graph::from_canvas(&c)).is_some());
    }

    #[test]
    fn an_acyclic_canvas_reports_no_cycle() {
        let c = canvas(
            &[
                ("a", "query", "x"),
                ("b", "prompt", "{{a}}"),
                ("c", "transform", "dedupe"),
            ],
            &[("a", "b"), ("b", "c"), ("a", "c")],
        );
        assert!(find_cycle(&Graph::from_canvas(&c)).is_none());
    }

    /// The determinism the "same graph twice" gate rests on.
    #[test]
    fn planning_is_deterministic_across_runs() {
        let c = canvas(
            &[
                ("z", "query", "1"),
                ("y", "query", "2"),
                ("x", "query", "3"),
                ("sink", "transform", "join"),
            ],
            &[("z", "sink"), ("y", "sink"), ("x", "sink")],
        );
        let g = Graph::from_canvas(&c);

        let first = plan_for(&g, "sink");
        for _ in 0..25 {
            assert_eq!(
                plan_for(&g, "sink"),
                first,
                "plan order varied between runs"
            );
        }
        // Independent roots come out sorted, not in hash order.
        assert_eq!(
            first,
            Plan::Order(vec!["x".into(), "y".into(), "z".into(), "sink".into()])
        );
    }

    #[test]
    fn cycle_detection_is_fast_enough_to_run_on_every_edit() {
        // The gate is 100 ms. Build something far larger than a real canvas and
        // confirm the check is not close to the budget.
        let nodes: Vec<(String, &str, String)> = (0..2000)
            .map(|i| (format!("n{i}"), "transform", String::new()))
            .collect();
        let node_refs: Vec<(&str, &str, &str)> = nodes
            .iter()
            .map(|(a, b, c)| (a.as_str(), *b, c.as_str()))
            .collect();
        let edge_owned: Vec<(String, String)> = (0..1999)
            .map(|i| (format!("n{i}"), format!("n{}", i + 1)))
            .collect();
        let mut edges: Vec<(&str, &str)> = edge_owned
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        // Close the loop, so the worst case is detecting a cycle rather than
        // finding an order.
        edges.push(("n1999", "n0"));

        let c = canvas(&node_refs, &edges);
        let g = Graph::from_canvas(&c);

        let start = std::time::Instant::now();
        let found = find_cycle(&g);
        let elapsed = start.elapsed();

        assert!(found.is_some());
        assert!(
            elapsed.as_millis() < 100,
            "cycle detection took {elapsed:?} on 2,000 nodes; the budget is 100 ms"
        );
    }
}
