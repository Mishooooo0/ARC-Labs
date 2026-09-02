//! Markdown rendering: a pure function from note text to HTML plus metadata.
//!
//! No I/O of any kind. That is deliberate and load-bearing: it makes rendering
//! golden-file testable, keeps embeds from turning into a recursive file-reading
//! loop, and means the same function serves the desktop webview, the browser
//! client and (later) the index without three code paths drifting apart.
//!
//! # Two decisions worth knowing
//!
//! **Raw HTML in notes is escaped, not emitted.** Obsidian renders it.
//! ARC-LABS does not, because ARC-LABS has a server shell: a note containing
//! `<script>` would otherwise execute in the browser of anyone the vault is
//! served to, and vault content is not always self-authored — it arrives from
//! clipboards, imports and, from Phase 5, from a model. Escaping is the safe
//! default; a per-vault opt-in can be added later when there is a consent
//! surface for it. This is a knowing deviation from Obsidian, not an oversight.
//!
//! **Wikilinks become links with an `arc://` scheme, not raw HTML.** Injecting
//! `<a data-target=…>` would require enabling comrak's unsafe mode, which would
//! re-open exactly the hole the previous paragraph closes. Encoding the
//! destination in the URL keeps the output structured, keeps escaping on, and
//! gives both shells one thing to intercept.
//!
//! comrak's own `wikilinks` extension is deliberately left off: it does not
//! model `![[embeds]]`, `#anchors` or `#tags`, and if it consumed `[[…]]` first
//! this module's scanner — the same scanner the index will use — would never see
//! them. One scanner means the links the reader sees and the links the index
//! records can never disagree.

use std::cell::RefCell;

use comrak::nodes::{Ast, AstNode, LineColumn, NodeLink, NodeValue};
use comrak::{format_html, parse_document, Arena, Options};

use super::frontmatter;
use super::wikilink::{self, Found, Item, WikiLink};

/// The result of rendering one note.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenderedNote {
    pub html: String,
    /// Raw YAML, exactly as written. Never re-serialised — see [`frontmatter`].
    pub frontmatter: Option<String>,
    /// `[[wikilinks]]`, in document order, excluding embeds.
    pub links: Vec<WikiLink>,
    /// `![[embeds]]`, in document order.
    pub embeds: Vec<WikiLink>,
    /// Tag names without the leading `#`, in document order, deduplicated.
    pub tags: Vec<String>,
    /// Headings, in document order. Collected from the same walk that renders,
    /// so the outline can never disagree with what is on screen.
    pub headings: Vec<Heading>,
}

/// A heading, for the outline and for resolving `[[note#anchor]]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    /// 1-6.
    pub level: u8,
    /// The heading's text with markdown syntax removed — what an anchor matches.
    pub text: String,
    /// 1-based line in the *body*, after any frontmatter.
    pub line: usize,
}

/// URL scheme the UI intercepts. Not a real protocol — a marker that survives
/// HTML escaping and tells both shells "this is a vault-internal destination".
pub const SCHEME: &str = "arc://";

pub fn options() -> Options<'static> {
    let mut o = Options::default();
    o.extension.table = true;
    o.extension.tasklist = true;
    o.extension.strikethrough = true;
    o.extension.footnotes = true;
    o.extension.autolink = true;
    // Obsidian's default: a single newline is a line break. Matching it matters
    // because a note that looks right in Obsidian must look right here.
    o.render.hardbreaks = true;
    // Left false on purpose. See the module docs.
    o.render.r#unsafe = false;
    // With `unsafe` off, comrak's default is to *drop* raw HTML and leave a
    // `<!-- raw HTML omitted -->` comment, which makes the user's own text
    // silently disappear from their own note. `escape` keeps the text visible as
    // literal characters instead. Safe and lossless; dropping is only safe.
    o.render.escape = true;
    o
}

/// Render a note. `text` must already be normalised to `\n` line endings —
/// [`crate::NoteText`] does that on read.
pub fn render(text: &str) -> RenderedNote {
    let split = frontmatter::split(text);

    let arena = Arena::new();
    let opts = options();
    let root = parse_document(&arena, split.body, &opts);

    let mut out = RenderedNote {
        frontmatter: split.frontmatter.map(str::to_owned),
        ..Default::default()
    };

    // Collect first: descendants() holds a borrow, and the rewrite mutates the
    // tree underneath it.
    let text_nodes: Vec<&AstNode> = root
        .descendants()
        .filter(|n| matches!(n.data.borrow().value, NodeValue::Text(_)))
        .collect();

    // Headings come from a separate, cheap pass over the top level rather than
    // from the rewrite loop below, which only visits text nodes and would miss
    // a heading whose text is entirely emphasis or code.
    for node in root.children() {
        let (level, line) = {
            let data = node.data.borrow();
            match &data.value {
                NodeValue::Heading(h) => (h.level, data.sourcepos.start.line),
                _ => continue,
            }
        };
        let mut text = String::new();
        collect_text(node, &mut text);
        out.headings.push(Heading { level, text: text.trim().to_string(), line });
    }

    for node in text_nodes {
        // Never rewrite inside an existing link: `[see [[X]]](url)` is one link
        // whose label happens to contain brackets, not a nested wikilink.
        if node.ancestors().skip(1).any(|a| matches!(a.data.borrow().value, NodeValue::Link(_))) {
            continue;
        }

        let literal = match &node.data.borrow().value {
            NodeValue::Text(t) => t.to_string(),
            _ => continue,
        };
        let found = wikilink::scan(&literal);
        if found.is_empty() {
            continue;
        }

        let pos = node.data.borrow().sourcepos.start;
        rewrite(&arena, node, &literal, &found, pos, &mut out);
        node.detach();
    }

    // format_html only fails if the writer fails, and writing to a String cannot.
    let _ = format_html(root, &opts, &mut out.html);
    out
}

/// Replace one text node with an interleaving of plain text and link nodes.
fn rewrite<'a>(
    arena: &'a Arena<'a>,
    node: &'a AstNode<'a>,
    literal: &str,
    found: &[Found],
    pos: LineColumn,
    out: &mut RenderedNote,
) {
    let mut cursor = 0usize;

    for f in found {
        if f.start > cursor {
            node.insert_before(text_node(arena, &literal[cursor..f.start], pos));
        }

        let replacement = match &f.item {
            Item::Link(link) => {
                let url = link_url(link);
                if link.embed {
                    out.embeds.push(link.clone());
                } else {
                    out.links.push(link.clone());
                }
                link_node(arena, &url, link.display(), pos)
            }
            Item::Tag(name) => {
                // Case-insensitive, keeping the first casing seen. `#Rust` and
                // `#rust` are one tag — that is what Obsidian does, and counting
                // them separately would make every tag total wrong.
                if !out.tags.iter().any(|t| t.eq_ignore_ascii_case(name)) {
                    out.tags.push(name.clone());
                }
                link_node(arena, &format!("{SCHEME}tag/{}", pct(name)), &format!("#{name}"), pos)
            }
        };
        node.insert_before(replacement);
        cursor = f.end;
    }

    if cursor < literal.len() {
        node.insert_before(text_node(arena, &literal[cursor..], pos));
    }
}

/// Flatten a node's visible text, ignoring markdown syntax.
fn collect_text<'a>(node: &'a AstNode<'a>, out: &mut String) {
    for child in node.descendants() {
        match &child.data.borrow().value {
            NodeValue::Text(t) => out.push_str(t),
            NodeValue::Code(c) => out.push_str(&c.literal),
            NodeValue::SoftBreak | NodeValue::LineBreak => out.push(' '),
            _ => {}
        }
    }
}

fn link_url(link: &WikiLink) -> String {
    let kind = if link.embed { "embed" } else { "note" };
    let mut url = format!("{SCHEME}{kind}/{}", pct(&link.target));
    if let Some(anchor) = &link.anchor {
        url.push('#');
        url.push_str(&pct(anchor));
    }
    url
}

fn text_node<'a>(arena: &'a Arena<'a>, s: &str, pos: LineColumn) -> &'a AstNode<'a> {
    node(arena, NodeValue::Text(s.to_owned().into()), pos)
}

fn link_node<'a>(
    arena: &'a Arena<'a>,
    url: &str,
    label: &str,
    pos: LineColumn,
) -> &'a AstNode<'a> {
    let link = node(
        arena,
        NodeValue::Link(Box::new(NodeLink { url: url.to_owned(), title: String::new() })),
        pos,
    );
    link.append(text_node(arena, label, pos));
    link
}

fn node<'a>(arena: &'a Arena<'a>, value: NodeValue, pos: LineColumn) -> &'a AstNode<'a> {
    arena.alloc(AstNode::new(RefCell::new(Ast::new(value, pos))))
}

/// Percent-encode everything that is not unreserved. Note names contain spaces,
/// `&`, `?` and `#` routinely, and every one of them would otherwise change what
/// the URL means.
fn pct(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_headings_with_levels_and_lines() {
        let r = render("# One

text

## Two *emphasised*

### `code heading`
");
        let got: Vec<(u8, &str)> =
            r.headings.iter().map(|h| (h.level, h.text.as_str())).collect();
        assert_eq!(got, [(1, "One"), (2, "Two emphasised"), (3, "code heading")]);
        // Lines are 1-based and in document order.
        assert!(r.headings.windows(2).all(|w| w[0].line < w[1].line));
        assert_eq!(r.headings[0].line, 1);
    }

    #[test]
    fn heading_lines_are_relative_to_the_body_not_the_file() {
        // Frontmatter is split off before parsing, so a heading on the first
        // body line is line 1 regardless of how long the frontmatter was.
        let r = render("---
a: 1
b: 2
c: 3
---
# First
");
        assert_eq!(r.headings[0].line, 1);
    }

    #[test]
    fn renders_ordinary_markdown() {
        let r = render("# Title\n\nSome **bold** and `code`.\n");
        assert!(r.html.contains("<h1>Title</h1>"));
        assert!(r.html.contains("<strong>bold</strong>"));
        assert!(r.html.contains("<code>code</code>"));
    }

    #[test]
    fn renders_gfm_extensions() {
        let table = render("| a | b |\n| - | - |\n| 1 | 2 |\n");
        assert!(table.html.contains("<table>"));

        let tasks = render("- [ ] todo\n- [x] done\n");
        assert!(tasks.html.contains("type=\"checkbox\""));

        let strike = render("~~gone~~\n");
        assert!(strike.html.contains("<del>gone</del>"));
    }

    #[test]
    fn separates_frontmatter_from_the_rendered_body() {
        let r = render("---\ntitle: T\n---\n# Body\n");
        assert_eq!(r.frontmatter.as_deref(), Some("title: T\n"));
        assert!(r.html.contains("<h1>Body</h1>"));
        assert!(!r.html.contains("title: T"), "frontmatter must not render as content");
    }

    #[test]
    fn wikilinks_become_arc_scheme_links() {
        let r = render("See [[Other Note|the alias]].\n");
        assert!(r.html.contains("href=\"arc://note/Other%20Note\""), "got: {}", r.html);
        assert!(r.html.contains(">the alias</a>"));
        assert_eq!(r.links.len(), 1);
        assert_eq!(r.links[0].target, "Other Note");
    }

    #[test]
    fn embeds_and_anchors_are_distinguishable_in_the_url() {
        let r = render("![[Diagram.png]] and [[Note#Heading]]\n");
        assert!(r.html.contains("arc://embed/Diagram.png"));
        assert!(r.html.contains("arc://note/Note#Heading"));
        assert_eq!(r.embeds.len(), 1);
        assert_eq!(r.links.len(), 1);
    }

    #[test]
    fn tags_are_collected_and_deduplicated() {
        let r = render("#rust and #arc-labs and #rust again\n");
        assert_eq!(r.tags, ["rust", "arc-labs"]);
        assert!(r.html.contains("arc://tag/rust"));
    }

    /// The bug this whole design exists to prevent.
    #[test]
    fn tag_deduplication_ignores_case_and_keeps_the_first_spelling() {
        // `#Rust` and `#rust` are one tag, as they are in Obsidian. Counting
        // them separately makes every tag total in the index wrong.
        let r = render("#Rust then #rust then #RUST\n");
        assert_eq!(r.tags, ["Rust"], "one tag, spelled as first written");
        // All three still render as links — only the collected list is deduped.
        assert_eq!(r.html.matches("arc://tag/").count(), 3);
    }

    #[test]
    fn code_is_never_linkified() {
        let r = render(
            "Inline `[[NotALink]]` and `#nottag`.\n\n```\n[[AlsoNot]]\n#nope\n```\n\n    [[Indented]]\n",
        );
        assert!(r.links.is_empty(), "found links in code: {:?}", r.links);
        assert!(r.tags.is_empty(), "found tags in code: {:?}", r.tags);
        assert!(!r.html.contains("arc://"));
        // …and the code still renders as code, with its brackets intact.
        assert!(r.html.contains("[[NotALink]]"));
    }

    #[test]
    fn existing_markdown_links_are_left_alone() {
        let r = render("[label [[X]]](https://example.com)\n");
        assert!(r.links.is_empty(), "rewrote inside an existing link: {:?}", r.links);
        assert!(r.html.contains("https://example.com"));
    }

    #[test]
    fn raw_html_in_a_note_is_escaped_not_executed() {
        // The server shell serves this to a browser. It must never execute.
        let r = render("<script>alert(1)</script>\n\n<img src=x onerror=alert(1)>\n");
        // What matters is that no tag survives as a *tag*. The attribute text
        // may still appear as literal characters inside an escaped element —
        // that is the point of escaping rather than dropping, and it is inert.
        assert!(!r.html.contains("<script"), "raw script survived: {}", r.html);
        assert!(!r.html.contains("<img"), "raw img survived: {}", r.html);
        assert!(r.html.contains("&lt;script&gt;"), "text was dropped instead of escaped");
        assert!(r.html.contains("&lt;img src=x onerror=alert(1)&gt;"));
    }

    #[test]
    fn a_hostile_note_name_cannot_break_out_of_the_href() {
        let r = render("[[evil\" onmouseover=\"alert(1)]]\n");
        assert!(!r.html.contains("onmouseover=\"alert"), "escaped the attribute: {}", r.html);
        assert!(r.html.contains("%22"), "the quote should be percent-encoded: {}", r.html);
    }

    #[test]
    fn text_around_a_link_survives_intact() {
        let r = render("before [[X]] after\n");
        assert!(r.html.contains("before "));
        assert!(r.html.contains(" after"));
    }

    #[test]
    fn handles_the_degenerate_notes_in_the_real_vault() {
        // Two notes in E-Tron-Memory are zero bytes, and one of them is
        // referenced by a canvas. Rendering must not panic or produce garbage.
        for text in ["", "\n", "   \n", "---\n---\n"] {
            let r = render(text);
            assert!(r.links.is_empty() && r.tags.is_empty());
        }
    }
}
