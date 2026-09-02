//! Reproducing a canvas file's own formatting.
//!
//! Obsidian writes `.canvas` in a specific shape — tabs, LF, one compact node
//! per line, no trailing newline — and `serde_json::to_string_pretty` produces
//! none of it. Emitting canonical JSON instead would mean every canvas ARC-LABS
//! opened came back as a whole-file diff, which fails constraint 2 just as surely
//! as losing data would.
//!
//! So the format is detected on read and reproduced on write, exactly as
//! [`arc_labs_core::NoteText`] does for line endings in markdown.

use serde_json::{Map, Value};

use crate::{Edge, Node};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Indent {
    Tab,
    Spaces(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanvasFormat {
    pub indent: Indent,
    /// Whether each array element sits on its own line as compact JSON, which is
    /// what Obsidian does. The alternative — fully pretty-printed, every key on
    /// its own line — is what most other writers produce.
    pub compact_elements: bool,
    pub trailing_newline: bool,
    pub crlf: bool,
}

impl CanvasFormat {
    /// What Obsidian writes.
    pub fn obsidian() -> CanvasFormat {
        CanvasFormat {
            indent: Indent::Tab,
            compact_elements: true,
            trailing_newline: false,
            crlf: false,
        }
    }

    pub fn detect(source: &str) -> CanvasFormat {
        let crlf = source.contains("\r\n");
        let normalised = source.replace("\r\n", "\n");
        let mut lines = normalised.lines();

        // The indent used by the first indented line.
        let indent = lines
            .find(|l| l.starts_with('\t') || l.starts_with(' '))
            .map(|l| {
                if l.starts_with('\t') {
                    Indent::Tab
                } else {
                    Indent::Spaces(l.len() - l.trim_start_matches(' ').len())
                }
            })
            .unwrap_or(Indent::Tab);

        // Compact elements: a line whose trimmed form is a whole object.
        let compact_elements = normalised
            .lines()
            .any(|l| {
                let t = l.trim();
                t.starts_with('{') && (t.ends_with('}') || t.ends_with("},"))
            })
            // An empty canvas has no elements to judge by, so assume Obsidian's.
            || !normalised.contains('{');

        CanvasFormat {
            indent,
            compact_elements,
            trailing_newline: source.ends_with('\n'),
            crlf,
        }
    }

    fn unit(&self) -> String {
        match self.indent {
            Indent::Tab => "\t".to_string(),
            Indent::Spaces(n) => " ".repeat(n),
        }
    }

    pub fn render(
        &self,
        nodes: &[Node],
        edges: &[Edge],
        extra: &Map<String, Value>,
    ) -> String {
        // An object with nothing in it at all: `{}` and no newlines, which is
        // what a freshly created empty canvas looks like.
        if nodes.is_empty() && edges.is_empty() && extra.is_empty() {
            let mut s = "{}".to_string();
            if self.trailing_newline {
                s.push('\n');
            }
            return self.apply_endings(s);
        }

        let one = self.unit();
        let two = one.repeat(2);
        let mut out = String::from("{\n");
        let mut parts: Vec<String> = Vec::new();

        // `nodes` and `edges` first, in that order, because that is where
        // Obsidian puts them and diff noise is the thing being avoided.
        for (field, count, body) in [
            ("nodes", nodes.len(), render_array(nodes.iter().map(Node::as_map), &one, &two, self)),
            ("edges", edges.len(), render_array(edges.iter().map(Edge::as_map), &one, &two, self)),
        ] {
            let _ = count;
            parts.push(format!("{one}\"{field}\":{body}"));
        }

        for (key, value) in extra {
            let rendered = if self.compact_elements {
                serde_json::to_string(value).unwrap_or_else(|_| "null".into())
            } else {
                indent_block(&serde_json::to_string_pretty(value).unwrap_or_else(|_| "null".into()), &one)
            };
            parts.push(format!("{one}\"{key}\":{rendered}"));
        }

        out.push_str(&parts.join(",\n"));
        out.push_str("\n}");
        if self.trailing_newline {
            out.push('\n');
        }
        self.apply_endings(out)
    }

    fn apply_endings(&self, s: String) -> String {
        if self.crlf {
            s.replace('\n', "\r\n")
        } else {
            s
        }
    }
}

fn render_array<'a>(
    items: impl Iterator<Item = &'a Map<String, Value>>,
    one: &str,
    two: &str,
    fmt: &CanvasFormat,
) -> String {
    let rendered: Vec<String> = items
        .map(|m| {
            let v = Value::Object(m.clone());
            if fmt.compact_elements {
                format!("{two}{}", serde_json::to_string(&v).unwrap_or_else(|_| "{}".into()))
            } else {
                let pretty = serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".into());
                format!("{two}{}", indent_block(&pretty, two))
            }
        })
        .collect();

    if rendered.is_empty() {
        return "[]".to_string();
    }
    format!("[\n{}\n{one}]", rendered.join(",\n"))
}

/// Re-indent a multi-line block so its continuation lines line up.
fn indent_block(s: &str, prefix: &str) -> String {
    let mut lines = s.lines();
    let first = lines.next().unwrap_or_default().to_string();
    let rest: Vec<String> = lines.map(|l| format!("{prefix}{l}")).collect();
    if rest.is_empty() {
        first
    } else {
        format!("{first}\n{}", rest.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Canvas;

    #[test]
    fn detects_obsidians_shape() {
        let src = "{\n\t\"nodes\":[\n\t\t{\"id\":\"a\"}\n\t],\n\t\"edges\":[]\n}";
        let f = CanvasFormat::detect(src);
        assert_eq!(f, CanvasFormat::obsidian());
    }

    #[test]
    fn detects_a_pretty_printed_canvas_written_by_something_else() {
        let src = "{\n  \"nodes\": [\n    {\n      \"id\": \"a\"\n    }\n  ],\n  \"edges\": []\n}\n";
        let f = CanvasFormat::detect(src);
        assert_eq!(f.indent, Indent::Spaces(2));
        assert!(!f.compact_elements);
        assert!(f.trailing_newline);
    }

    #[test]
    fn detects_crlf() {
        let src = "{\r\n\t\"nodes\":[],\r\n\t\"edges\":[]\r\n}";
        assert!(CanvasFormat::detect(src).crlf);
    }

    /// A canvas from another tool must survive too, not just Obsidian's.
    #[test]
    fn a_foreign_format_round_trips_unchanged() {
        let sources = [
            // Obsidian.
            "{\n\t\"nodes\":[\n\t\t{\"id\":\"a\",\"x\":1}\n\t],\n\t\"edges\":[]\n}",
            // Trailing newline.
            "{\n\t\"nodes\":[],\n\t\"edges\":[]\n}\n",
            // CRLF, as a Windows tool might write.
            "{\r\n\t\"nodes\":[],\r\n\t\"edges\":[]\r\n}",
            // Bare empty object.
            "{}",
        ];
        for src in sources {
            let c = Canvas::parse(src).unwrap();
            assert_eq!(c.to_string(), src, "round trip changed bytes for {src:?}");
            assert!(c.is_unchanged(), "reported changed for {src:?}");
        }
    }

    #[test]
    fn an_edited_canvas_keeps_the_files_own_formatting() {
        let src = "{\r\n\t\"nodes\":[\r\n\t\t{\"id\":\"a\",\"x\":1,\"y\":2}\r\n\t],\r\n\t\"edges\":[]\r\n}\r\n";
        let mut c = Canvas::parse(src).unwrap();
        c.node_mut("a").unwrap().set_position(9.0, 9.0);

        let out = c.to_string();
        assert!(out.contains("\r\n"), "CRLF should survive an edit");
        assert!(out.ends_with("\r\n"), "the trailing newline should survive");
        assert!(out.contains("\"x\":9,\"y\":9"));
    }
}
