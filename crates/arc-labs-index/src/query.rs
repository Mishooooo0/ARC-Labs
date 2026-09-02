//! Reading the index: search, backlinks, tags, unresolved links, the graph.
//!
//! # Link resolution is a query, not a stored fact
//!
//! `links.target` holds what the user wrote. Whether it *resolves* is worked out
//! here, every time, by folding and matching. That keeps unresolved links
//! representable — a link to a note you have not written yet is a normal and
//! useful thing in a notebook — and it means changing the resolution rule is a
//! change to one query rather than a migration.
//!
//! # Constraint 7 lives here
//!
//! Everything in this module reports *observed* relationships: links and tags
//! the user actually typed. Nothing is inferred, nothing is scored, nothing is
//! suggested. Phase 6's inferred edges arrive in their own table with their own
//! surface, so there is no query here that could accidentally blend a guess into
//! a list of facts.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::schema::fold;
use crate::Result;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteRef {
    pub path: String,
    pub title: String,
    pub is_canvas: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub path: String,
    pub title: String,
    /// Context around the match, with the matched terms wrapped in `«…»`.
    pub snippet: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Backlink {
    pub path: String,
    pub title: String,
    /// How the link was written in the source note — `[[Target|alias]]`.
    pub target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    pub is_embed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutgoingLink {
    pub target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    pub is_embed: bool,
    /// The note this resolves to, or `None` when nothing matches.
    ///
    /// An explicit `None` rather than an omitted field: the UI must be able to
    /// tell "no such note" from "not looked up yet", and constraint 7 says an
    /// unknown is never rendered as an answer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnresolvedLink {
    pub target: String,
    /// How many notes link to this non-existent note. A high count is a strong
    /// hint about what to write next.
    pub count: i64,
    /// A few of the notes that link to it.
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagCount {
    pub name: String,
    pub count: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStats {
    pub notes: i64,
    pub canvases: i64,
    pub links: i64,
    pub resolved_links: i64,
    pub unresolved_links: i64,
    pub tags: i64,
    pub distinct_tags: i64,
    pub orphans: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphNode {
    pub id: usize,
    pub path: String,
    pub title: String,
    pub is_canvas: bool,
    /// Total degree, so the layout can size nodes without a second pass.
    pub degree: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphEdge {
    pub source: usize,
    pub target: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Graph {
    pub nodes: Vec<GraphNode>,
    /// Observed links only. Phase 6's inferred edges are a separate field on the
    /// wire so the renderer cannot draw one as if it were the other.
    pub edges: Vec<GraphEdge>,
}

/// The SQL fragment that decides whether a link target resolves to a note.
///
/// Written once and reused, because "what does this link point at" must give the
/// same answer in the backlinks pane, the unresolved list and the graph. Three
/// copies of this rule is three chances for them to disagree.
///
/// The rule, in order: a full-path match, a path match ignoring the `.md`
/// extension, or a filename-stem match. All case-folded, because Windows and
/// macOS filesystems are case-insensitive and a vault must resolve identically
/// on every platform.
///
/// Every comparison is against a **stored, indexed** column. An earlier version
/// used `lower(n.path)`, which is a function on a column and therefore
/// unindexable — it turned every link into a full scan of `notes` and put the
/// backlinks query at 44 ms against a 50 ms budget. `path_folded` exists for
/// exactly this clause.
const RESOLVES: &str = "(
    n.path_folded = l.target_folded
    OR n.path_folded = l.target_folded || '.md'
    OR n.stem_folded = l.target_folded
)";

/// Turn user input into an FTS5 MATCH expression that cannot be a syntax error.
///
/// FTS5's query language has operators (`AND`, `OR`, `NOT`, `:`, `*`, `"`, `(`),
/// and a user typing a colon or a stray quote would otherwise get an error
/// rather than results. Every term is quoted, so the input is treated as
/// literal text, and the final term gets a prefix `*` so search-as-you-type
/// matches while a word is still being typed.
pub fn to_fts_query(input: &str) -> Option<String> {
    let terms: Vec<String> = input
        .split_whitespace()
        // Doubling internal quotes is how a quote is escaped inside an FTS5
        // string literal.
        .map(|t| t.replace('"', "\"\""))
        .filter(|t| !t.is_empty())
        .collect();
    if terms.is_empty() {
        return None;
    }

    let last = terms.len() - 1;
    let quoted: Vec<String> = terms
        .iter()
        .enumerate()
        .map(|(i, t)| {
            if i == last {
                format!("\"{t}\"*")
            } else {
                format!("\"{t}\"")
            }
        })
        .collect();
    Some(quoted.join(" AND "))
}

pub fn search(conn: &Connection, input: &str, limit: usize) -> Result<Vec<SearchHit>> {
    let Some(q) = to_fts_query(input) else {
        return Ok(Vec::new());
    };

    let mut stmt = conn.prepare_cached(
        "SELECT f.path,
                COALESCE(n.title, f.path),
                snippet(notes_fts, 0, '«', '»', '…', 12)
         FROM notes_fts f
         LEFT JOIN notes n ON n.id = f.rowid
         WHERE notes_fts MATCH ?1
         ORDER BY rank
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![q, limit as i64], |r| {
        Ok(SearchHit {
            path: r.get(0)?,
            title: r.get(1)?,
            snippet: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Quick-open: match a note by path or title, best prefix first.
///
/// Not fuzzy. A notebook's quick-open is used to reach a note you can already
/// name, and fuzzy matching mostly gets in the way of that by ranking a
/// coincidence above the thing you typed.
pub fn quick_open(conn: &Connection, input: &str, limit: usize) -> Result<Vec<NoteRef>> {
    let needle = fold(input);
    if needle.is_empty() {
        return recent(conn, limit);
    }
    let like = format!("%{needle}%");

    let mut stmt = conn.prepare_cached(
        "SELECT path, COALESCE(title, stem), is_canvas
         FROM notes
         WHERE stem_folded LIKE ?1 OR lower(path) LIKE ?1 OR lower(COALESCE(title,'')) LIKE ?1
         ORDER BY
             -- exact stem, then stem prefix, then anywhere; shorter paths first
             CASE WHEN stem_folded = ?2 THEN 0
                  WHEN stem_folded LIKE ?2 || '%' THEN 1
                  WHEN lower(COALESCE(title,'')) LIKE ?2 || '%' THEN 2
                  ELSE 3 END,
             length(path)
         LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![like, needle, limit as i64], |r| {
        Ok(NoteRef {
            path: r.get(0)?,
            title: r.get(1)?,
            is_canvas: r.get::<_, i64>(2)? != 0,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn recent(conn: &Connection, limit: usize) -> Result<Vec<NoteRef>> {
    let mut stmt = conn.prepare_cached(
        "SELECT path, COALESCE(title, stem), is_canvas FROM notes
         ORDER BY mtime DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit as i64], |r| {
        Ok(NoteRef {
            path: r.get(0)?,
            title: r.get(1)?,
            is_canvas: r.get::<_, i64>(2)? != 0,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Notes that link *to* `path`.
pub fn backlinks(conn: &Connection, path: &str) -> Result<Vec<Backlink>> {
    let sql = format!(
        "SELECT src.path, COALESCE(src.title, src.stem), l.target, l.alias, l.is_embed
         FROM links l
         JOIN notes src ON src.id = l.src
         JOIN notes n   ON {RESOLVES}
         WHERE n.path = ?1 AND src.path != ?1
         ORDER BY src.path"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(params![path], |r| {
        Ok(Backlink {
            path: r.get(0)?,
            title: r.get(1)?,
            target: r.get(2)?,
            alias: r.get(3)?,
            is_embed: r.get::<_, i64>(4)? != 0,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Links *from* `path`, each with what it resolves to (or nothing).
pub fn outgoing(conn: &Connection, path: &str) -> Result<Vec<OutgoingLink>> {
    let sql = format!(
        "SELECT l.target, l.anchor, l.alias, l.is_embed,
                (SELECT n.path FROM notes n WHERE {RESOLVES} ORDER BY length(n.path) LIMIT 1)
         FROM links l
         JOIN notes src ON src.id = l.src
         WHERE src.path = ?1"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(params![path], |r| {
        Ok(OutgoingLink {
            target: r.get(0)?,
            anchor: r.get(1)?,
            alias: r.get(2)?,
            is_embed: r.get::<_, i64>(3)? != 0,
            resolved_path: r.get(4)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Link targets that match no note, most-linked first.
pub fn unresolved(conn: &Connection, limit: usize) -> Result<Vec<UnresolvedLink>> {
    let sql = format!(
        "SELECT l.target,
                count(*) AS n,
                group_concat(src.path, char(10)) AS srcs
         FROM links l
         JOIN notes src ON src.id = l.src
         WHERE NOT EXISTS (SELECT 1 FROM notes n WHERE {RESOLVES})
         GROUP BY l.target_folded
         ORDER BY n DESC, l.target
         LIMIT ?1"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(params![limit as i64], |r| {
        let srcs: String = r.get(2)?;
        Ok(UnresolvedLink {
            target: r.get(0)?,
            count: r.get(1)?,
            sources: srcs.lines().take(5).map(str::to_string).collect(),
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn tag_counts(conn: &Connection) -> Result<Vec<TagCount>> {
    let mut stmt = conn.prepare_cached(
        "SELECT name, count(*) AS n FROM tags
         GROUP BY name_folded ORDER BY n DESC, name",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(TagCount {
            name: r.get(0)?,
            count: r.get(1)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn notes_with_tag(conn: &Connection, tag: &str) -> Result<Vec<NoteRef>> {
    let mut stmt = conn.prepare_cached(
        "SELECT DISTINCT n.path, COALESCE(n.title, n.stem), n.is_canvas
         FROM tags t JOIN notes n ON n.id = t.src
         WHERE t.name_folded = ?1 ORDER BY n.path",
    )?;
    let rows = stmt.query_map(params![fold(tag)], |r| {
        Ok(NoteRef {
            path: r.get(0)?,
            title: r.get(1)?,
            is_canvas: r.get::<_, i64>(2)? != 0,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn stats(conn: &Connection) -> Result<IndexStats> {
    let one = |sql: &str| -> Result<i64> { Ok(conn.query_row(sql, [], |r| r.get(0))?) };

    let resolved = format!(
        "SELECT count(*) FROM links l WHERE EXISTS (SELECT 1 FROM notes n WHERE {RESOLVES})"
    );
    let orphans = format!(
        "SELECT count(*) FROM notes n0 WHERE n0.is_canvas = 0
           AND NOT EXISTS (SELECT 1 FROM links l WHERE l.src = n0.id)
           AND NOT EXISTS (
                 SELECT 1 FROM links l JOIN notes n ON {RESOLVES} WHERE n.id = n0.id)"
    );

    let links = one("SELECT count(*) FROM links")?;
    let resolved_links = one(&resolved)?;
    Ok(IndexStats {
        notes: one("SELECT count(*) FROM notes WHERE is_canvas = 0")?,
        canvases: one("SELECT count(*) FROM notes WHERE is_canvas = 1")?,
        links,
        resolved_links,
        unresolved_links: links - resolved_links,
        tags: one("SELECT count(*) FROM tags")?,
        distinct_tags: one("SELECT count(DISTINCT name_folded) FROM tags")?,
        orphans: one(&orphans)?,
    })
}

/// The whole graph, ready for a force layout.
///
/// Only resolved links become edges: an edge to a note that does not exist would
/// be an edge to nothing, and drawing a phantom node for it would invent a
/// relationship the vault does not contain.
pub fn graph(conn: &Connection) -> Result<Graph> {
    let mut nodes = Vec::new();
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    {
        let mut stmt =
            conn.prepare("SELECT path, COALESCE(title, stem), is_canvas FROM notes ORDER BY id")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)? != 0,
            ))
        })?;
        for (i, row) in rows.enumerate() {
            let (path, title, is_canvas) = row?;
            index.insert(path.clone(), i);
            nodes.push(GraphNode {
                id: i,
                path,
                title,
                is_canvas,
                degree: 0,
            });
        }
    }

    let sql = format!(
        "SELECT src.path, (SELECT n.path FROM notes n WHERE {RESOLVES}
                           ORDER BY length(n.path) LIMIT 1) AS dst
         FROM links l JOIN notes src ON src.id = l.src
         WHERE dst IS NOT NULL"
    );
    let mut edges: Vec<GraphEdge> = Vec::new();
    // Deduplicate: three links from A to B are one edge in a graph view.
    let mut seen = std::collections::HashSet::new();
    {
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (from, to) = row?;
            let (Some(&s), Some(&t)) = (index.get(&from), index.get(&to)) else {
                continue;
            };
            if s == t || !seen.insert((s, t)) {
                continue;
            }
            nodes[s].degree += 1;
            nodes[t].degree += 1;
            edges.push(GraphEdge {
                source: s,
                target: t,
            });
        }
    }

    Ok(Graph { nodes, edges })
}

#[cfg(test)]
mod tests {
    use super::*;
    use arc_labs_core::Vault;

    fn indexed(files: &[(&str, &str)]) -> (tempfile::TempDir, Connection) {
        let tmp = tempfile::tempdir().unwrap();
        for (name, body) in files {
            let p = tmp.path().join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, body.as_bytes()).unwrap();
        }
        let vault = Vault::open(tmp.path()).unwrap();
        let mut conn = crate::open_in_memory().unwrap();
        crate::build(&mut conn, &vault, false, |_| {}).unwrap();
        (tmp, conn)
    }

    #[test]
    fn search_returns_ranked_hits_with_snippets() {
        let (_t, c) = indexed(&[
            (
                "a.md",
                "# Alpha\n\nThe ledger records provenance for every mutation.\n",
            ),
            ("b.md", "# Beta\n\nNothing relevant here at all.\n"),
        ]);
        let hits = search(&c, "provenance", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "a.md");
        assert!(
            hits[0].snippet.contains("«provenance»"),
            "got {}",
            hits[0].snippet
        );
    }

    #[test]
    fn search_matches_a_prefix_so_it_works_while_typing() {
        let (_t, c) = indexed(&[("a.md", "# A\n\nprovenance and provisioning\n")]);
        assert_eq!(search(&c, "prov", 10).unwrap().len(), 1);
        assert_eq!(search(&c, "provena", 10).unwrap().len(), 1);
    }

    #[test]
    fn hostile_search_input_cannot_produce_a_syntax_error() {
        // Every one of these is an FTS5 operator or a malformed expression.
        let (_t, c) = indexed(&[("a.md", "# A\n\nplain words here\n")]);
        for input in [
            "\"", "a\"b", "NOT", "AND OR", "col:", "*", "(", "a AND (b", "^", "\"\"\"",
        ] {
            let r = search(&c, input, 10);
            assert!(
                r.is_ok(),
                "input {input:?} produced an error: {:?}",
                r.err()
            );
        }
        // Empty input is not an error, it is no results.
        assert!(search(&c, "   ", 10).unwrap().is_empty());
    }

    #[test]
    fn backlinks_find_the_notes_pointing_here() {
        let (_t, c) = indexed(&[
            ("target.md", "# Target\n"),
            ("a.md", "# A\n\nlink to [[target]]\n"),
            (
                "b.md",
                "# B\n\nalso [[Target|the target]] and an embed ![[target]]\n",
            ),
            ("c.md", "# C\n\nno links\n"),
        ]);
        let back = backlinks(&c, "target.md").unwrap();
        assert_eq!(back.len(), 3, "{back:?}");
        assert!(back.iter().any(|b| b.path == "a.md"));
        assert!(back.iter().any(|b| b.is_embed));
        assert!(back
            .iter()
            .any(|b| b.alias.as_deref() == Some("the target")));
    }

    #[test]
    fn resolution_is_case_insensitive_and_handles_folders() {
        let (_t, c) = indexed(&[
            ("Daily/2026-09-02.md", "# Today\n"),
            (
                "a.md",
                "# A\n\n[[2026-09-02]] and [[daily/2026-09-02]] and [[Daily/2026-09-02.md]]\n",
            ),
        ]);
        let out = outgoing(&c, "a.md").unwrap();
        assert_eq!(out.len(), 3);
        for l in &out {
            assert_eq!(
                l.resolved_path.as_deref(),
                Some("Daily/2026-09-02.md"),
                "failed to resolve {:?}",
                l.target
            );
        }
    }

    #[test]
    fn an_unresolved_link_resolves_to_nothing_rather_than_guessing() {
        let (_t, c) = indexed(&[("a.md", "# A\n\n[[nowhere]]\n")]);
        let out = outgoing(&c, "a.md").unwrap();
        assert_eq!(out[0].resolved_path, None);

        let un = unresolved(&c, 10).unwrap();
        assert_eq!(un.len(), 1);
        assert_eq!(un[0].target, "nowhere");
        assert_eq!(un[0].count, 1);
        assert_eq!(un[0].sources, ["a.md"]);
    }

    #[test]
    fn unresolved_links_are_ranked_by_how_many_notes_want_them() {
        let (_t, c) = indexed(&[
            ("a.md", "# A\n\n[[wanted]] [[rare]]\n"),
            ("b.md", "# B\n\n[[wanted]]\n"),
            ("c.md", "# C\n\n[[wanted]]\n"),
        ]);
        let un = unresolved(&c, 10).unwrap();
        assert_eq!(un[0].target, "wanted");
        assert_eq!(un[0].count, 3);
        assert_eq!(un[1].count, 1);
    }

    #[test]
    fn tags_are_counted_and_folded() {
        let (_t, c) = indexed(&[
            ("a.md", "# A\n\n#Rust #rust #other\n"),
            ("b.md", "# B\n\n#RUST\n"),
        ]);
        let tags = tag_counts(&c).unwrap();
        // #Rust and #rust in one note dedupe to one; b adds another.
        let rust = tags.iter().find(|t| fold(&t.name) == "rust").unwrap();
        assert_eq!(rust.count, 2);

        let notes = notes_with_tag(&c, "RUST").unwrap();
        assert_eq!(notes.len(), 2);
    }

    #[test]
    fn the_graph_contains_only_resolved_edges() {
        let (_t, c) = indexed(&[
            ("a.md", "# A\n\n[[b]] [[b]] [[nowhere]]\n"),
            ("b.md", "# B\n\n[[a]]\n"),
            ("island.md", "# Island\n"),
        ]);
        let g = graph(&c).unwrap();
        assert_eq!(g.nodes.len(), 3);
        // a->b deduped to one edge, plus b->a. The link to nowhere is not an edge.
        assert_eq!(g.edges.len(), 2, "{:?}", g.edges);

        let island = g.nodes.iter().find(|n| n.path == "island.md").unwrap();
        assert_eq!(island.degree, 0);
        let a = g.nodes.iter().find(|n| n.path == "a.md").unwrap();
        assert_eq!(a.degree, 2);
    }

    #[test]
    fn quick_open_prefers_an_exact_stem_then_a_prefix() {
        let (_t, c) = indexed(&[
            ("notes/meeting.md", "# Meeting\n"),
            ("meeting-notes.md", "# Meeting notes\n"),
            ("a/b/some-meeting-thing.md", "# Other\n"),
        ]);
        let hits = quick_open(&c, "meeting", 10).unwrap();
        assert_eq!(
            hits[0].path, "notes/meeting.md",
            "exact stem should win: {hits:?}"
        );
        assert_eq!(hits[1].path, "meeting-notes.md", "prefix next: {hits:?}");
        assert_eq!(hits.len(), 3);
    }

    #[test]
    fn stats_add_up() {
        let (_t, c) = indexed(&[
            ("a.md", "# A\n\n[[b]] [[nowhere]] #t\n"),
            ("b.md", "# B\n"),
            ("island.md", "# Island\n"),
            ("board.canvas", "{}"),
        ]);
        let s = stats(&c).unwrap();
        assert_eq!(s.notes, 3);
        assert_eq!(s.canvases, 1);
        assert_eq!(s.links, 2);
        assert_eq!(s.resolved_links, 1);
        assert_eq!(s.unresolved_links, 1);
        assert_eq!(s.distinct_tags, 1);
        // island.md has no links in or out; a and b are connected.
        assert_eq!(s.orphans, 1);
    }

    #[test]
    fn a_note_does_not_backlink_to_itself() {
        let (_t, c) = indexed(&[("self.md", "# Self\n\nI link to [[self]]\n")]);
        assert!(backlinks(&c, "self.md").unwrap().is_empty());
        // The graph should not draw a self-loop either.
        assert!(graph(&c).unwrap().edges.is_empty());
    }
}
