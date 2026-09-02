//! Building and maintaining the index.
//!
//! # Two paths, one extraction
//!
//! A full build and an incremental update both go through [`index_one`], so a
//! note indexed by the watcher is indexed identically to one indexed at startup.
//! Two extraction paths that drift apart is how a search index quietly starts
//! disagreeing with the vault.
//!
//! # Speed
//!
//! The gate is 5,000 notes in under ten seconds. Three things do most of that
//! work:
//!
//! - **One transaction for the whole build.** SQLite's per-statement fsync is
//!   the difference between seconds and minutes here.
//! - **Prepared statements, reused.** Re-parsing the same INSERT 20,000 times is
//!   pure waste.
//! - **Skip by hash on rescan.** Most notes have not changed, and reading a file
//!   is cheaper than rendering it.

use std::collections::HashMap;
use std::time::Instant;

use arc_labs_core::{Vault, VaultPath};
use rusqlite::{params, Connection, Transaction};

use crate::schema::fold;
use crate::Result;

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct BuildStats {
    pub notes: usize,
    pub canvases: usize,
    pub links: usize,
    pub tags: usize,
    pub headings: usize,
    /// Notes whose content was unchanged and so were not re-rendered.
    pub skipped_unchanged: usize,
    /// Files that could not be indexed, with the reason. Reported rather than
    /// swallowed — a vault that is only partly searchable should say so.
    pub failed: Vec<(String, String)>,
    pub elapsed_ms: u128,
}

/// What the caller is told while a build runs.
pub struct Progress {
    pub done: usize,
    pub total: usize,
}

/// Build or refresh the whole index.
///
/// Incremental by default: a note whose size, mtime and hash are unchanged is
/// left alone. Pass `force` to re-render everything, which is what
/// `arc-labs reindex` does.
pub fn build(
    conn: &mut Connection,
    vault: &Vault,
    force: bool,
    mut on_progress: impl FnMut(Progress),
) -> Result<BuildStats> {
    let started = Instant::now();
    let mut stats = BuildStats::default();

    let tree = match vault.tree() {
        Ok(t) => t,
        Err(e) => {
            stats.failed.push((String::new(), e.public()));
            return Ok(stats);
        }
    };
    let files: Vec<&arc_labs_core::TreeEntry> = tree
        .entries
        .iter()
        .filter(|e| !e.is_dir && (e.path.is_markdown() || e.path.is_canvas()))
        .collect();
    let total = files.len();

    let tx = conn.transaction()?;

    // Everything currently indexed, so anything not seen this pass can be pruned
    // and anything unchanged can be skipped.
    let existing: HashMap<String, (i64, String)> = {
        let mut stmt = tx.prepare("SELECT path, id, hash FROM notes")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                (r.get::<_, i64>(1)?, r.get::<_, String>(2)?),
            ))
        })?;
        rows.collect::<std::result::Result<_, _>>()?
    };

    let mut seen: Vec<String> = Vec::with_capacity(total);

    for (i, entry) in files.iter().enumerate() {
        let path = &entry.path;
        seen.push(path.as_str().to_string());

        match index_one(&tx, vault, path, existing.get(path.as_str()), force) {
            Ok(Outcome::Indexed {
                links,
                tags,
                headings,
                is_canvas,
            }) => {
                if is_canvas {
                    stats.canvases += 1;
                } else {
                    stats.notes += 1;
                }
                stats.links += links;
                stats.tags += tags;
                stats.headings += headings;
            }
            Ok(Outcome::Unchanged { is_canvas }) => {
                if is_canvas {
                    stats.canvases += 1;
                } else {
                    stats.notes += 1;
                }
                stats.skipped_unchanged += 1;
            }
            Err(reason) => stats.failed.push((path.as_str().to_string(), reason)),
        }

        // Progress every 64 notes: often enough for a bar to move, rarely enough
        // that the callback is not the bottleneck.
        if i % 64 == 0 || i + 1 == total {
            on_progress(Progress { done: i + 1, total });
        }
    }

    prune(&tx, &seen)?;
    tx.commit()?;

    stats.elapsed_ms = started.elapsed().as_millis();
    Ok(stats)
}

enum Outcome {
    Indexed {
        links: usize,
        tags: usize,
        headings: usize,
        is_canvas: bool,
    },
    Unchanged {
        is_canvas: bool,
    },
}

/// Index a single file. The one extraction path.
fn index_one(
    tx: &Transaction<'_>,
    vault: &Vault,
    path: &VaultPath,
    existing: Option<&(i64, String)>,
    force: bool,
) -> std::result::Result<Outcome, String> {
    let is_canvas = path.is_canvas();

    let abs = vault
        .root()
        .resolve_existing(path)
        .map_err(|e| e.public())?;
    let meta = std::fs::metadata(&abs).map_err(|e| e.kind().to_string())?;
    let size = meta.len() as i64;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    // A canvas is indexed for its existence and name, not its contents —
    // Phase 4 owns its structure. Reading it as prose would put JSON in the
    // search index, which is worse than useless.
    if is_canvas {
        let hash = format!("size:{size}:mtime:{mtime}");
        if !force {
            if let Some((_, prev)) = existing {
                if prev == &hash {
                    return Ok(Outcome::Unchanged { is_canvas });
                }
            }
        }
        upsert_note(tx, path, None, size, mtime, &hash, true, None).map_err(|e| e.to_string())?;
        return Ok(Outcome::Indexed {
            links: 0,
            tags: 0,
            headings: 0,
            is_canvas,
        });
    }

    let note = vault.read_note(path).map_err(|e| e.public())?;
    let hash = note.content_hash();

    if !force {
        if let Some((_, prev)) = existing {
            if prev == &hash {
                return Ok(Outcome::Unchanged { is_canvas });
            }
        }
    }

    let rendered = arc_labs_core::render(note.text());
    // A note's title is its first H1 if it has one, else its filename. This is
    // what the palette and search results show.
    let title = rendered
        .headings
        .iter()
        .find(|h| h.level == 1)
        .map(|h| h.text.clone())
        .unwrap_or_else(|| path.stem().to_string());

    let id = upsert_note(
        tx,
        path,
        Some(&title),
        size,
        mtime,
        &hash,
        false,
        rendered.frontmatter.as_deref(),
    )
    .map_err(|e| e.to_string())?;

    // Wholesale replace rather than diff: a note's links and tags are small, and
    // a diff would be more code for less certainty.
    for table in ["links", "tags", "blocks"] {
        tx.execute(&format!("DELETE FROM {table} WHERE src = ?1"), params![id])
            .map_err(|e| e.to_string())?;
    }

    let all_links = rendered.links.iter().chain(rendered.embeds.iter());
    let mut link_count = 0usize;
    {
        let mut stmt = tx
            .prepare_cached(
                "INSERT INTO links(src, target, target_folded, anchor, alias, is_embed)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .map_err(|e| e.to_string())?;
        for l in all_links {
            stmt.execute(params![
                id,
                l.target,
                fold(&l.target),
                l.anchor,
                l.alias,
                l.embed as i64
            ])
            .map_err(|e| e.to_string())?;
            link_count += 1;
        }
    }

    {
        let mut stmt = tx
            .prepare_cached("INSERT INTO tags(src, name, name_folded) VALUES (?1, ?2, ?3)")
            .map_err(|e| e.to_string())?;
        for t in &rendered.tags {
            stmt.execute(params![id, t, fold(t)])
                .map_err(|e| e.to_string())?;
        }
    }

    {
        let mut stmt = tx
            .prepare_cached(
                "INSERT INTO blocks(src, level, text, text_folded, line) VALUES (?1,?2,?3,?4,?5)",
            )
            .map_err(|e| e.to_string())?;
        for h in &rendered.headings {
            stmt.execute(params![
                id,
                h.level as i64,
                h.text,
                fold(&h.text),
                h.line as i64
            ])
            .map_err(|e| e.to_string())?;
        }
    }

    // FTS rows are keyed by the note id, so replacing means delete-then-insert.
    tx.execute("DELETE FROM notes_fts WHERE rowid = ?1", params![id])
        .map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT INTO notes_fts(rowid, body, title, path) VALUES (?1, ?2, ?3, ?4)",
        params![id, note.text(), title, path.as_str()],
    )
    .map_err(|e| e.to_string())?;

    Ok(Outcome::Indexed {
        links: link_count,
        tags: rendered.tags.len(),
        headings: rendered.headings.len(),
        is_canvas,
    })
}

#[allow(clippy::too_many_arguments)]
fn upsert_note(
    tx: &Transaction<'_>,
    path: &VaultPath,
    title: Option<&str>,
    size: i64,
    mtime: i64,
    hash: &str,
    is_canvas: bool,
    frontmatter: Option<&str>,
) -> rusqlite::Result<i64> {
    let mut stmt = tx.prepare_cached(
        "INSERT INTO notes(path, stem, stem_folded, path_folded, title, size, mtime, hash,
                           is_canvas, frontmatter)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
         ON CONFLICT(path) DO UPDATE SET
             stem=excluded.stem, stem_folded=excluded.stem_folded,
             path_folded=excluded.path_folded, title=excluded.title,
             size=excluded.size, mtime=excluded.mtime, hash=excluded.hash,
             is_canvas=excluded.is_canvas, frontmatter=excluded.frontmatter
         RETURNING id",
    )?;
    stmt.query_row(
        params![
            path.as_str(),
            path.stem(),
            fold(path.stem()),
            fold(path.as_str()),
            title,
            size,
            mtime,
            hash,
            is_canvas as i64,
            frontmatter
        ],
        |r| r.get(0),
    )
}

/// Remove notes that are no longer in the vault.
fn prune(tx: &Transaction<'_>, seen: &[String]) -> Result<()> {
    // A temp table beats a giant IN (...) clause: SQLite has a parameter limit,
    // and 5,000 bound values would exceed it on some builds.
    tx.execute_batch("CREATE TEMP TABLE IF NOT EXISTS seen_paths(path TEXT PRIMARY KEY); DELETE FROM seen_paths;")?;
    {
        let mut stmt = tx.prepare("INSERT OR IGNORE INTO seen_paths(path) VALUES (?1)")?;
        for p in seen {
            stmt.execute(params![p])?;
        }
    }
    tx.execute(
        "DELETE FROM notes_fts WHERE rowid IN
             (SELECT id FROM notes WHERE path NOT IN (SELECT path FROM seen_paths))",
        [],
    )?;
    tx.execute(
        "DELETE FROM notes WHERE path NOT IN (SELECT path FROM seen_paths)",
        [],
    )?;
    Ok(())
}

/// Index one note after an external change. Used by the Phase 2 watcher.
pub fn reindex_note(conn: &mut Connection, vault: &Vault, path: &VaultPath) -> Result<()> {
    let tx = conn.transaction()?;
    let existing: Option<(i64, String)> = tx
        .query_row(
            "SELECT id, hash FROM notes WHERE path = ?1",
            params![path.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    // `force` is false: the hash check still short-circuits a spurious watcher
    // event, which on Windows and inside OneDrive is most of them.
    let _ = index_one(&tx, vault, path, existing.as_ref(), false);
    tx.commit()?;
    Ok(())
}

/// Drop a note from the index after it is deleted from the vault.
pub fn forget_note(conn: &Connection, path: &VaultPath) -> Result<()> {
    conn.execute(
        "DELETE FROM notes_fts WHERE rowid IN (SELECT id FROM notes WHERE path = ?1)",
        params![path.as_str()],
    )?;
    conn.execute("DELETE FROM notes WHERE path = ?1", params![path.as_str()])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault_with(files: &[(&str, &str)]) -> (tempfile::TempDir, Vault) {
        let tmp = tempfile::tempdir().unwrap();
        for (name, body) in files {
            let p = tmp.path().join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, body.as_bytes()).unwrap();
        }
        let v = Vault::open(tmp.path()).unwrap();
        (tmp, v)
    }

    fn count(conn: &Connection, table: &str) -> i64 {
        conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn builds_an_index_from_a_vault() {
        let (_t, v) = vault_with(&[
            (
                "a.md",
                "---\ntitle: A\n---\n# Alpha\n\nSee [[b]] and [[missing]] #rust\n\n## Sub\n",
            ),
            ("b.md", "# Beta\n\nBack to [[a]] #rust #other\n"),
            ("board.canvas", "{\"nodes\":[]}"),
        ]);
        let mut conn = crate::open_in_memory().unwrap();
        let stats = build(&mut conn, &v, false, |_| {}).unwrap();

        assert_eq!(stats.notes, 2);
        assert_eq!(stats.canvases, 1);
        assert_eq!(stats.links, 3);
        assert_eq!(stats.tags, 3);
        assert!(stats.failed.is_empty(), "{:?}", stats.failed);

        assert_eq!(count(&conn, "notes"), 3);
        assert_eq!(count(&conn, "blocks"), 3); // Alpha, Sub, Beta

        // The title comes from the first H1, not the filename.
        let title: String = conn
            .query_row("SELECT title FROM notes WHERE path='a.md'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(title, "Alpha");

        // Frontmatter is stored verbatim.
        let fm: String = conn
            .query_row("SELECT frontmatter FROM notes WHERE path='a.md'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(fm, "title: A\n");
    }

    #[test]
    fn a_canvas_is_indexed_by_name_but_its_json_is_not_searchable() {
        let (_t, v) = vault_with(&[("board.canvas", "{\"nodes\":[{\"id\":\"deadbeef\"}]}")]);
        let mut conn = crate::open_in_memory().unwrap();
        build(&mut conn, &v, false, |_| {}).unwrap();

        assert_eq!(count(&conn, "notes"), 1);
        // Its JSON must not pollute search results.
        assert_eq!(count(&conn, "notes_fts"), 0);
    }

    #[test]
    fn rebuilding_skips_notes_whose_content_did_not_change() {
        let (_t, v) = vault_with(&[("a.md", "# A\n"), ("b.md", "# B\n")]);
        let mut conn = crate::open_in_memory().unwrap();

        let first = build(&mut conn, &v, false, |_| {}).unwrap();
        assert_eq!(first.skipped_unchanged, 0);

        let second = build(&mut conn, &v, false, |_| {}).unwrap();
        assert_eq!(
            second.skipped_unchanged, 2,
            "nothing changed, so nothing should re-render"
        );

        let forced = build(&mut conn, &v, true, |_| {}).unwrap();
        assert_eq!(
            forced.skipped_unchanged, 0,
            "force should re-render everything"
        );
    }

    #[test]
    fn a_changed_note_replaces_its_links_rather_than_accumulating_them() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.md"), b"# A\n\n[[one]] [[two]]\n").unwrap();
        let v = Vault::open(tmp.path()).unwrap();
        let mut conn = crate::open_in_memory().unwrap();

        build(&mut conn, &v, false, |_| {}).unwrap();
        assert_eq!(count(&conn, "links"), 2);

        std::fs::write(tmp.path().join("a.md"), b"# A\n\n[[three]]\n").unwrap();
        build(&mut conn, &v, false, |_| {}).unwrap();
        assert_eq!(
            count(&conn, "links"),
            1,
            "old links should be gone, not merged"
        );

        let target: String = conn
            .query_row("SELECT target FROM links", [], |r| r.get(0))
            .unwrap();
        assert_eq!(target, "three");
    }

    #[test]
    fn a_deleted_note_is_pruned_along_with_everything_derived_from_it() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.md"), b"# A\n\n[[x]] #t\n").unwrap();
        std::fs::write(tmp.path().join("b.md"), b"# B\n").unwrap();
        let v = Vault::open(tmp.path()).unwrap();
        let mut conn = crate::open_in_memory().unwrap();

        build(&mut conn, &v, false, |_| {}).unwrap();
        assert_eq!(count(&conn, "notes"), 2);

        std::fs::remove_file(tmp.path().join("a.md")).unwrap();
        build(&mut conn, &v, false, |_| {}).unwrap();

        assert_eq!(count(&conn, "notes"), 1);
        assert_eq!(count(&conn, "links"), 0);
        assert_eq!(count(&conn, "tags"), 0);
        assert_eq!(
            count(&conn, "notes_fts"),
            1,
            "the FTS row should be pruned too"
        );
    }

    #[test]
    fn a_note_that_cannot_be_read_is_reported_not_silently_dropped() {
        // Latin-1 is not UTF-8; the vault is UTF-8 only.
        let (_t, v) = vault_with(&[("good.md", "# fine\n")]);
        std::fs::write(v.root().path().join("bad.md"), b"caf\xE9\n").unwrap();

        let mut conn = crate::open_in_memory().unwrap();
        let stats = build(&mut conn, &v, false, |_| {}).unwrap();

        assert_eq!(stats.failed.len(), 1);
        assert_eq!(stats.failed[0].0, "bad.md");
        assert_eq!(stats.notes, 1);
    }

    /// **The Phase 2 disposability gate.**
    ///
    /// Constraint 1 says the index is derived. This proves it: build an index,
    /// delete the database outright, rebuild from the vault alone, and every
    /// answer the index gives must be identical. If anything here diverges, some
    /// state exists only in the database — which would make it something whose
    /// loss actually mattered.
    #[test]
    fn deleting_the_index_and_rebuilding_loses_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let vault_dir = tmp.path().join("vault");
        std::fs::create_dir_all(vault_dir.join("Daily")).unwrap();
        for (name, body) in [
            (
                "a.md",
                "---\nzeta: 1\ntitle: A\n---\n# Alpha\n\n[[b]] [[nowhere]] #rust #x\n\n## Sub\n",
            ),
            ("b.md", "# Beta\n\n![[a]] and [[Daily/log]] #rust\n"),
            ("Daily/log.md", "# Log\n\nplain text about provenance\n"),
            ("board.canvas", "{\"nodes\":[]}"),
        ] {
            std::fs::write(vault_dir.join(name), body.as_bytes()).unwrap();
        }
        let vault = Vault::open(&vault_dir).unwrap();

        // Everything the index can be asked, in one snapshot.
        let snapshot = |index: &crate::Index| {
            (
                index.stats().unwrap(),
                index.search("provenance", 10).unwrap(),
                index.backlinks("a.md").unwrap(),
                index.outgoing("a.md").unwrap(),
                index.unresolved(10).unwrap(),
                index.tag_counts().unwrap(),
                index.graph().unwrap(),
                index.quick_open("log", 10).unwrap(),
            )
        };

        let mut index = crate::Index::open_for_vault(&vault_dir).unwrap();
        index.build(&vault, false, |_| {}).unwrap();
        let before = snapshot(&index);
        let db_path = index.path().to_path_buf();
        drop(index);

        assert!(db_path.exists());
        crate::remove_database(&db_path);
        assert!(!db_path.exists(), "the database should be gone");

        // Rebuild from the vault alone.
        let mut rebuilt = crate::Index::open_for_vault(&vault_dir).unwrap();
        rebuilt.build(&vault, false, |_| {}).unwrap();
        let after = snapshot(&rebuilt);

        assert_eq!(before.0, after.0, "stats differ after rebuild");
        assert_eq!(before.1, after.1, "search results differ after rebuild");
        assert_eq!(before.2, after.2, "backlinks differ after rebuild");
        assert_eq!(before.3, after.3, "outgoing links differ after rebuild");
        assert_eq!(before.4, after.4, "unresolved links differ after rebuild");
        assert_eq!(before.5, after.5, "tags differ after rebuild");
        assert_eq!(before.6, after.6, "the graph differs after rebuild");
        assert_eq!(before.7, after.7, "quick-open differs after rebuild");

        // And the vault itself is untouched by any of it.
        assert_eq!(
            std::fs::read(vault_dir.join("a.md")).unwrap(),
            b"---\nzeta: 1\ntitle: A\n---\n# Alpha\n\n[[b]] [[nowhere]] #rust #x\n\n## Sub\n"
        );
    }

    /// A corrupt database is discarded and rebuilt rather than propagated.
    #[test]
    fn a_corrupt_index_is_thrown_away_rather_than_failing_the_app() {
        let tmp = tempfile::tempdir().unwrap();
        let vault_dir = tmp.path().join("vault");
        std::fs::create_dir_all(&vault_dir).unwrap();
        std::fs::write(vault_dir.join("a.md"), b"# A\n\nsearchable content\n").unwrap();
        let vault = Vault::open(&vault_dir).unwrap();

        let db = vault_dir.join(".arc").join("index.db");
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        std::fs::write(&db, b"this is not a database, it is garbage").unwrap();

        let mut index = crate::Index::open_for_vault(&vault_dir).unwrap();
        index.build(&vault, false, |_| {}).unwrap();
        assert_eq!(index.search("searchable", 10).unwrap().len(), 1);
    }

    #[test]
    fn progress_reaches_the_total() {
        let files: Vec<(String, String)> = (0..200)
            .map(|i| (format!("n{i}.md"), format!("# note {i}\n")))
            .collect();
        let refs: Vec<(&str, &str)> = files
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        let (_t, v) = vault_with(&refs);

        let mut conn = crate::open_in_memory().unwrap();
        let mut last = 0;
        let stats = build(&mut conn, &v, false, |p| last = p.done).unwrap();
        assert_eq!(last, 200);
        assert_eq!(stats.notes, 200);
    }
}
