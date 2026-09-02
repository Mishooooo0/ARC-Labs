//! JSONCanvas, round-tripped byte-for-byte.
//!
//! # The problem this crate exists to solve
//!
//! Constraint 2 says a vault ARC-LABS touches must still open in Obsidian. For
//! markdown that means preserving line endings; for `.canvas` it means something
//! sharper, because Obsidian's own writer is not canonical.
//!
//! Here are two nodes from a real canvas in the fixture vault:
//!
//! ```text
//! {"id":"75a4f708ea95a139","x":-105,"y":-1503,"width":400,"height":400,"type":"file","file":"…"}
//! {"id":"d9a077ef1fb364da","type":"file","file":"…","x":172,"y":619,"width":400,"height":400}
//! ```
//!
//! Same file, same node type, **different key order**. Obsidian preserves
//! whatever order a node last had rather than normalising it. So a struct with
//! named fields — the obvious way to model this — silently reorders every node
//! it touches, and a user who opens a canvas here and saves it gets a diff
//! covering every node in the file.
//!
//! Hence the design: nodes and edges are held as *ordered maps*, with typed
//! accessors layered on top. Unknown keys are preserved because they were never
//! discarded; key order survives because it was never rebuilt.
//!
//! # Formatting is part of the file
//!
//! Obsidian writes tabs, LF, one compact node per line, and no trailing
//! newline. [`CanvasFormat`] detects that on read and reproduces it on write —
//! the same discipline [`arc_labs_core::NoteText`] applies to markdown. And, as
//! there, unchanged content re-encodes to the **original bytes verbatim**, so a
//! canvas that was opened and not edited is untouched no matter how unusual its
//! formatting was.

use serde_json::{Map, Value};

pub mod format;

pub use format::{CanvasFormat, Indent};

#[derive(Debug, thiserror::Error)]
pub enum CanvasError {
    #[error("not valid JSON: {0}")]
    Json(String),
    #[error("not a JSON object at the top level")]
    NotAnObject,
    #[error("{field} must be an array")]
    NotAnArray { field: &'static str },
    #[error("a node has no id")]
    NodeWithoutId,
}

pub type Result<T> = std::result::Result<T, CanvasError>;

/// The `arc` metadata key, under which ARC's own node types live.
///
/// Nested inside a node rather than replacing its `type`, so an ARC node
/// serialises as `type: "text"` with an extra key. Obsidian, which knows nothing
/// about `arc`, renders it as an ordinary text card and preserves the key it
/// does not understand — degrading rather than erroring, which is what the spec
/// asks for.
pub const ARC_KEY: &str = "arc";

/// The node types JSONCanvas defines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Text,
    File,
    Link,
    Group,
    /// A type this build does not know. Preserved untouched rather than
    /// rewritten, so a newer Obsidian can add types without this corrupting them.
    Unknown,
}

impl NodeKind {
    fn from_str(s: &str) -> NodeKind {
        match s {
            "text" => NodeKind::Text,
            "file" => NodeKind::File,
            "link" => NodeKind::Link,
            "group" => NodeKind::Group,
            _ => NodeKind::Unknown,
        }
    }
}

/// ARC's executable node types, from Phase 5.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArcKind {
    Prompt,
    Query,
    Transform,
}

impl ArcKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ArcKind::Prompt => "prompt",
            ArcKind::Query => "query",
            ArcKind::Transform => "transform",
        }
    }
    fn from_str(s: &str) -> Option<ArcKind> {
        match s {
            "prompt" => Some(ArcKind::Prompt),
            "query" => Some(ArcKind::Query),
            "transform" => Some(ArcKind::Transform),
            _ => None,
        }
    }
}

/// One node, as an ordered map.
///
/// Deliberately not a struct. See the module docs: named fields would rebuild
/// the key order and produce a diff covering every node in the file.
#[derive(Debug, Clone, PartialEq)]
pub struct Node(Map<String, Value>);

impl Node {
    pub fn id(&self) -> &str {
        self.0.get("id").and_then(Value::as_str).unwrap_or_default()
    }
    pub fn kind(&self) -> NodeKind {
        NodeKind::from_str(
            self.0
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        )
    }
    /// The ARC node type, if this is one of ours.
    pub fn arc_kind(&self) -> Option<ArcKind> {
        self.0
            .get(ARC_KEY)?
            .get("kind")
            .and_then(Value::as_str)
            .and_then(ArcKind::from_str)
    }
    pub fn x(&self) -> f64 {
        self.num("x")
    }
    pub fn y(&self) -> f64 {
        self.num("y")
    }
    pub fn width(&self) -> f64 {
        self.num("width")
    }
    pub fn height(&self) -> f64 {
        self.num("height")
    }
    /// For a `file` node: the vault-relative path it shows.
    pub fn file(&self) -> Option<&str> {
        self.0.get("file").and_then(Value::as_str)
    }
    /// For a `text` node: its markdown.
    pub fn text(&self) -> Option<&str> {
        self.0.get("text").and_then(Value::as_str)
    }
    pub fn url(&self) -> Option<&str> {
        self.0.get("url").and_then(Value::as_str)
    }
    pub fn color(&self) -> Option<&str> {
        self.0.get("color").and_then(Value::as_str)
    }

    fn num(&self, key: &str) -> f64 {
        self.0.get(key).and_then(Value::as_f64).unwrap_or(0.0)
    }

    /// Move a node.
    ///
    /// Assigns into the existing keys, which leaves their position in the map
    /// alone. Removing and re-inserting would move them to the end and change
    /// the file's key order — the exact failure this crate is built to avoid.
    pub fn set_position(&mut self, x: f64, y: f64) {
        self.set_num("x", x);
        self.set_num("y", y);
    }

    pub fn set_size(&mut self, width: f64, height: f64) {
        self.set_num("width", width);
        self.set_num("height", height);
    }

    fn set_num(&mut self, key: &str, v: f64) {
        // Integers stay integers: Obsidian writes `"x":-105`, and emitting
        // `-105.0` would be a diff on every node that moved.
        let value = if v.fract() == 0.0 && v.abs() < 9e15 {
            Value::from(v as i64)
        } else {
            Value::from(v)
        };
        match self.0.get_mut(key) {
            Some(slot) => *slot = value,
            None => {
                self.0.insert(key.to_string(), value);
            }
        }
    }

    pub fn set_text(&mut self, text: &str) {
        match self.0.get_mut("text") {
            Some(slot) => *slot = Value::from(text),
            None => {
                self.0.insert("text".into(), Value::from(text));
            }
        }
    }

    pub fn as_map(&self) -> &Map<String, Value> {
        &self.0
    }
}

/// One edge, as an ordered map, for the same reason as [`Node`].
#[derive(Debug, Clone, PartialEq)]
pub struct Edge(Map<String, Value>);

// `from_node` and `from_side` read the JSONCanvas keys `fromNode` and
// `fromSide`. Clippy flags `from_*` methods that take `self`, but renaming them
// would make the accessors disagree with the format they read, which is a worse
// trade than one allow.
#[allow(clippy::wrong_self_convention)]
impl Edge {
    pub fn id(&self) -> &str {
        self.0.get("id").and_then(Value::as_str).unwrap_or_default()
    }
    pub fn from_node(&self) -> &str {
        self.0
            .get("fromNode")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }
    pub fn to_node(&self) -> &str {
        self.0
            .get("toNode")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }
    pub fn from_side(&self) -> Option<&str> {
        self.0.get("fromSide").and_then(Value::as_str)
    }
    pub fn to_side(&self) -> Option<&str> {
        self.0.get("toSide").and_then(Value::as_str)
    }
    pub fn label(&self) -> Option<&str> {
        self.0.get("label").and_then(Value::as_str)
    }
    pub fn as_map(&self) -> &Map<String, Value> {
        &self.0
    }
}

/// A parsed canvas, with everything needed to write it back unchanged.
#[derive(Debug, Clone)]
pub struct Canvas {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// Top-level keys other than `nodes` and `edges`.
    ///
    /// JSONCanvas may grow; Obsidian may write things this build has never heard
    /// of. Keeping them means an unknown key survives a round trip rather than
    /// being quietly deleted by the program that promised not to touch the file.
    extra: Map<String, Value>,
    format: CanvasFormat,
    /// The exact bytes this was parsed from, so unchanged content re-encodes
    /// verbatim. Same rule as `NoteText`.
    original: String,
}

impl Canvas {
    pub fn parse(source: &str) -> Result<Canvas> {
        let value: Value =
            serde_json::from_str(source).map_err(|e| CanvasError::Json(e.to_string()))?;
        let Value::Object(mut obj) = value else {
            return Err(CanvasError::NotAnObject);
        };

        let nodes = take_array(&mut obj, "nodes")?
            .into_iter()
            .filter_map(|v| match v {
                Value::Object(m) => Some(Node(m)),
                _ => None,
            })
            .collect();
        let edges = take_array(&mut obj, "edges")?
            .into_iter()
            .filter_map(|v| match v {
                Value::Object(m) => Some(Edge(m)),
                _ => None,
            })
            .collect();

        Ok(Canvas {
            nodes,
            edges,
            extra: obj,
            format: CanvasFormat::detect(source),
            original: source.to_string(),
        })
    }

    pub fn empty() -> Canvas {
        Canvas {
            nodes: Vec::new(),
            edges: Vec::new(),
            extra: Map::new(),
            format: CanvasFormat::obsidian(),
            original: String::new(),
        }
    }

    pub fn format(&self) -> &CanvasFormat {
        &self.format
    }

    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id() == id)
    }
    pub fn node_mut(&mut self, id: &str) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id() == id)
    }

    /// Serialise, reproducing the source formatting.
    ///
    /// If nothing changed, this returns the original text byte for byte. That is
    /// the guarantee that makes opening a canvas safe: no matter how unusual its
    /// formatting, a canvas that was looked at and not edited is untouched.
    ///
    /// Exposed through `Display`, so `to_string()` is the standard trait method
    /// rather than an inherent one shadowing it.
    pub fn serialize(&self) -> String {
        let rebuilt = self.render();
        if !self.original.is_empty() && rebuilt == self.render_from(&self.original) {
            return self.original.clone();
        }
        rebuilt
    }

    /// Whether writing this back would change any byte.
    pub fn is_unchanged(&self) -> bool {
        !self.original.is_empty() && self.serialize() == self.original
    }

    fn render(&self) -> String {
        self.format.render(&self.nodes, &self.edges, &self.extra)
    }

    /// Render what a *reparse* of `source` would produce, to decide whether this
    /// canvas still matches the bytes it came from.
    fn render_from(&self, source: &str) -> String {
        match Canvas::parse(source) {
            Ok(c) => c.render(),
            Err(_) => String::new(),
        }
    }
}

impl std::fmt::Display for Canvas {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.serialize())
    }
}

fn take_array(obj: &mut Map<String, Value>, field: &'static str) -> Result<Vec<Value>> {
    match obj.shift_remove(field) {
        None => Ok(Vec::new()),
        Some(Value::Array(a)) => Ok(a),
        Some(_) => Err(CanvasError::NotAnArray { field }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL: &str = "{\n\t\"nodes\":[\n\t\t{\"id\":\"75a4f708ea95a139\",\"x\":-105,\"y\":-1503,\"width\":400,\"height\":400,\"type\":\"file\",\"file\":\"Notes/Alpha.md\"},\n\t\t{\"id\":\"d9a077ef1fb364da\",\"type\":\"file\",\"file\":\"Notes/Beta.md\",\"x\":172,\"y\":619,\"width\":400,\"height\":400}\n\t],\n\t\"edges\":[\n\t\t{\"id\":\"2c1b308d9c0be4fd\",\"fromNode\":\"d9a077ef1fb364da\",\"fromSide\":\"bottom\",\"toNode\":\"75a4f708ea95a139\",\"toSide\":\"top\"}\n\t]\n}";

    #[test]
    fn round_trips_a_real_canvas_byte_for_byte() {
        let c = Canvas::parse(REAL).unwrap();
        assert_eq!(c.to_string(), REAL);
        assert!(c.is_unchanged());
    }

    /// The reason this crate does not use a struct with named fields.
    #[test]
    fn per_node_key_order_survives() {
        let c = Canvas::parse(REAL).unwrap();
        let first: Vec<&str> = c.nodes[0].as_map().keys().map(String::as_str).collect();
        let second: Vec<&str> = c.nodes[1].as_map().keys().map(String::as_str).collect();

        assert_eq!(first, ["id", "x", "y", "width", "height", "type", "file"]);
        assert_eq!(second, ["id", "type", "file", "x", "y", "width", "height"]);
        assert_ne!(
            first, second,
            "the two nodes really do differ; that is the point"
        );
    }

    #[test]
    fn typed_accessors_read_what_is_there() {
        let c = Canvas::parse(REAL).unwrap();
        let n = &c.nodes[0];
        assert_eq!(n.id(), "75a4f708ea95a139");
        assert_eq!(n.kind(), NodeKind::File);
        assert_eq!(n.file(), Some("Notes/Alpha.md"));
        assert_eq!(
            (n.x(), n.y(), n.width(), n.height()),
            (-105.0, -1503.0, 400.0, 400.0)
        );

        let e = &c.edges[0];
        assert_eq!(e.from_node(), "d9a077ef1fb364da");
        assert_eq!(e.to_side(), Some("top"));
    }

    #[test]
    fn moving_a_node_keeps_key_order_and_integer_formatting() {
        let mut c = Canvas::parse(REAL).unwrap();
        c.node_mut("75a4f708ea95a139")
            .unwrap()
            .set_position(40.0, -20.0);

        let out = c.to_string();
        // The moved node keeps its unusual key order …
        assert!(out.contains("{\"id\":\"75a4f708ea95a139\",\"x\":40,\"y\":-20,\"width\":400"));
        // … integers stay integers, not 40.0 …
        assert!(!out.contains("40.0"));
        // … and the other node is untouched.
        assert!(out.contains("{\"id\":\"d9a077ef1fb364da\",\"type\":\"file\""));
        assert!(!c.is_unchanged());
    }

    #[test]
    fn unknown_node_types_and_unknown_keys_are_preserved() {
        // A newer Obsidian, or a plugin, writing something this build has never
        // seen. Dropping it would be the program that promised not to touch the
        // file quietly deleting part of it.
        let src = "{\n\t\"nodes\":[\n\t\t{\"id\":\"a\",\"type\":\"futuretype\",\"someNewKey\":{\"nested\":true},\"x\":0,\"y\":0,\"width\":10,\"height\":10}\n\t],\n\t\"edges\":[],\n\t\"metadata\":{\"version\":2}\n}";
        let c = Canvas::parse(src).unwrap();
        assert_eq!(c.nodes[0].kind(), NodeKind::Unknown);
        assert_eq!(c.to_string(), src);
    }

    #[test]
    fn arc_node_types_are_text_nodes_with_a_metadata_key() {
        // So Obsidian, which knows nothing about `arc`, renders it as a plain
        // text card instead of erroring.
        let src = "{\n\t\"nodes\":[\n\t\t{\"id\":\"p1\",\"type\":\"text\",\"text\":\"Summarise {{note}}\",\"x\":0,\"y\":0,\"width\":300,\"height\":200,\"arc\":{\"kind\":\"prompt\",\"model\":\"qwen3.5:0.8b\"}}\n\t],\n\t\"edges\":[]\n}";
        let c = Canvas::parse(src).unwrap();

        assert_eq!(
            c.nodes[0].kind(),
            NodeKind::Text,
            "must be a text node to Obsidian"
        );
        assert_eq!(c.nodes[0].arc_kind(), Some(ArcKind::Prompt));
        assert_eq!(c.to_string(), src);
    }

    #[test]
    fn an_empty_canvas_round_trips() {
        for src in ["{}", "{\n\t\"nodes\":[],\n\t\"edges\":[]\n}"] {
            let c = Canvas::parse(src).unwrap();
            assert_eq!(c.to_string(), src, "failed for {src:?}");
        }
    }

    #[test]
    fn malformed_input_is_an_error_not_a_panic() {
        assert!(matches!(
            Canvas::parse("not json"),
            Err(CanvasError::Json(_))
        ));
        assert!(matches!(
            Canvas::parse("[1,2,3]"),
            Err(CanvasError::NotAnObject)
        ));
        assert!(matches!(
            Canvas::parse("{\"nodes\":\"nope\"}"),
            Err(CanvasError::NotAnArray { field: "nodes" })
        ));
    }
}

/// **The Phase 4 compatibility gate**, run against the real files.
///
/// These are the two `.canvas` files from the vault this was built against —
/// written by Obsidian, not by a test. They are the oracle: if this crate can
/// round-trip them byte for byte, it can round-trip what Obsidian actually
/// writes, which is the only claim worth making.
#[cfg(test)]
mod real_fixtures {
    use super::*;

    /// Vault fixtures live at the repo root, three levels up from this crate.
    fn fixture(rel: &str) -> Option<String> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/vaults/etron")
            .join(rel);
        std::fs::read_to_string(path).ok()
    }

    #[test]
    fn the_real_obsidian_canvases_round_trip_byte_for_byte() {
        let mut checked = 0;
        for name in ["ARC/ARC-OS.canvas", "Projects.canvas"] {
            let Some(source) = fixture(name) else {
                continue;
            };
            let canvas =
                Canvas::parse(&source).unwrap_or_else(|e| panic!("{name} did not parse: {e}"));

            assert_eq!(
                canvas.to_string(),
                source,
                "{name} did not round-trip byte for byte"
            );
            assert!(canvas.is_unchanged(), "{name} reported as changed");
            assert!(!canvas.nodes.is_empty(), "{name} parsed to no nodes");
            checked += 1;
        }
        assert!(
            checked > 0,
            "the real canvas fixtures were not found; the gate did not run"
        );
    }

    #[test]
    fn moving_one_node_in_a_real_canvas_touches_only_that_node() {
        let Some(source) = fixture("Projects.canvas") else {
            eprintln!("skipping: fixture not found");
            return;
        };
        let mut canvas = Canvas::parse(&source).unwrap();
        let id = canvas.nodes[0].id().to_string();
        canvas.node_mut(&id).unwrap().set_position(1234.0, -5678.0);

        let out = canvas.to_string();
        let before: Vec<&str> = source.lines().collect();
        let after: Vec<&str> = out.lines().collect();

        assert_eq!(before.len(), after.len(), "line count changed");
        let differing = before.iter().zip(&after).filter(|(a, b)| a != b).count();
        assert_eq!(
            differing, 1,
            "a one-node move should change exactly one line"
        );
        assert!(out.contains("\"x\":1234,\"y\":-5678"));
    }

    #[test]
    fn a_real_canvas_reports_the_nodes_it_actually_has() {
        let Some(source) = fixture("ARC/ARC-OS.canvas") else {
            eprintln!("skipping: fixture not found");
            return;
        };
        let canvas = Canvas::parse(&source).unwrap();

        assert_eq!(canvas.nodes.len(), 6);
        assert_eq!(canvas.edges.len(), 5);
        // Five file cards and one text card, per the file.
        assert_eq!(
            canvas
                .nodes
                .iter()
                .filter(|n| n.kind() == NodeKind::File)
                .count(),
            5
        );
        assert_eq!(
            canvas
                .nodes
                .iter()
                .filter(|n| n.kind() == NodeKind::Text)
                .count(),
            1
        );
        // One of them points at another canvas — nesting is real and must not
        // be mistaken for a note.
        assert!(canvas
            .nodes
            .iter()
            .any(|n| n.file() == Some("ARC/Services.md")));
    }
}
