//! The index schema.
//!
//! Every table here is derived from the vault and can be rebuilt by re-reading
//! it. Nothing is stored that is not recoverable — no user text that is not
//! already in a note, no state that exists only in the database. That is what
//! makes "delete `.arc/index.db` and reopen, lose nothing" a property rather
//! than a hope.
//!
//! # Why link targets are stored as text
//!
//! `links.target` holds what the user wrote — `[[Some Note]]` — not a foreign
//! key to a `notes` row. Two reasons, and both matter:
//!
//! - **Unresolved links are first-class.** A link to a note that does not exist
//!   yet is a real and useful thing in a notebook; it is how you write forward.
//!   A foreign key would make it unrepresentable.
//! - **Resolution is a rule, not a fact.** It depends on case-folding and on
//!   shortest-unique-path matching, and if the rule changes, re-resolving is a
//!   query rather than a migration.
//!
//! Constraint 7 lives here too: this schema stores only *observed* relationships
//! — links and tags the user actually wrote. Inferred edges arrive in Phase 6 in
//! their own table, so nothing can accidentally join the two together and
//! present a guess as a fact.

use rusqlite::Connection;

use crate::Result;

/// Bumped whenever the schema changes shape.
///
/// There is no migration path and there does not need to be one: on a mismatch
/// the index is deleted and rebuilt from the vault. That is the whole benefit of
/// a cache that holds no irreplaceable state.
pub const SCHEMA_VERSION: i64 = 3;

const DDL: &str = r#"
CREATE TABLE IF NOT EXISTS arc_meta (
    id            INTEGER PRIMARY KEY CHECK (id = 1),
    version       INTEGER NOT NULL,
    built_at      TEXT    NOT NULL
);

-- One row per file in the vault.
CREATE TABLE IF NOT EXISTS notes (
    id            INTEGER PRIMARY KEY,
    -- Vault-relative, forward-slashed. The same string a VaultPath serialises to.
    path          TEXT    NOT NULL UNIQUE,
    -- Filename without extension: what a [[wikilink]] matches against.
    stem          TEXT    NOT NULL,
    -- Lowercased stem. Link resolution is case-insensitive because Windows and
    -- macOS filesystems are, and a vault must resolve identically on all three.
    stem_folded   TEXT    NOT NULL,
    -- Lowercased full path, stored rather than computed.
    --
    -- This exists purely so link resolution can use an index. Comparing
    -- `lower(n.path)` in the WHERE clause is a function on a column, which
    -- SQLite cannot index, so every link forced a full scan of `notes`: the
    -- backlinks query measured 44 ms on a 5,000-note vault, against a 50 ms
    -- budget, and the graph query took minutes. Storing the folded form makes
    -- all of it indexed equality.
    path_folded   TEXT    NOT NULL,
    title         TEXT,
    -- Bytes and mtime let the watcher skip a file whose content cannot have
    -- changed, which is most of them on any given rescan.
    size          INTEGER NOT NULL,
    mtime         INTEGER NOT NULL,
    -- blake3 of the normalised text. The authority on "did this actually
    -- change", since mtime moves for reasons content does not.
    hash          TEXT    NOT NULL,
    is_canvas     INTEGER NOT NULL DEFAULT 0,
    frontmatter   TEXT
);
CREATE INDEX IF NOT EXISTS notes_stem_folded ON notes(stem_folded);
CREATE INDEX IF NOT EXISTS notes_path_folded ON notes(path_folded);
CREATE INDEX IF NOT EXISTS notes_is_canvas   ON notes(is_canvas);

-- Observed links only: a [[wikilink]] the user actually wrote.
CREATE TABLE IF NOT EXISTS links (
    id            INTEGER PRIMARY KEY,
    src           INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
    -- As written, so it can be shown back verbatim.
    target        TEXT    NOT NULL,
    target_folded TEXT    NOT NULL,
    anchor        TEXT,
    alias         TEXT,
    is_embed      INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS links_src           ON links(src);
CREATE INDEX IF NOT EXISTS links_target_folded ON links(target_folded);

CREATE TABLE IF NOT EXISTS tags (
    id            INTEGER PRIMARY KEY,
    src           INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
    name          TEXT    NOT NULL,
    name_folded   TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS tags_src         ON tags(src);
CREATE INDEX IF NOT EXISTS tags_name_folded ON tags(name_folded);

-- Headings, for outline navigation and for resolving [[note#anchor]].
CREATE TABLE IF NOT EXISTS blocks (
    id            INTEGER PRIMARY KEY,
    src           INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
    level         INTEGER NOT NULL,
    text          TEXT    NOT NULL,
    text_folded   TEXT    NOT NULL,
    -- 1-based line within the body, so a click can jump straight to it.
    line          INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS blocks_src ON blocks(src);

-- Full-text search.
--
-- This stores the note body, which makes the index roughly the size of the
-- vault. That is a deliberate trade: a contentless table (`content=''`) would
-- halve the size, but FTS5's snippet() and highlight() need the text, and
-- search results without surrounding context are close to useless.
--
-- Storing it is only acceptable because of the property the whole design rests
-- on: this file is disposable. It holds a *copy*, never the original, and
-- deleting it loses nothing. `.arc/` is gitignored for the same reason.
CREATE VIRTUAL TABLE IF NOT EXISTS notes_fts USING fts5(
    body,
    title,
    path UNINDEXED,
    tokenize='unicode61 remove_diacritics 2'
);

-- ── Phase 6: inference ──────────────────────────────────────────────────────
--
-- Everything below this line is *inferred*. Everything above it is *observed*.
-- They are separate tables, and no query joins across the line, because
-- constraint 7 says a user must never have to click a connection to find out
-- whether it is real. Keeping them apart in the schema is what makes an
-- accidental blend impossible rather than merely discouraged.

-- Note embeddings. `vec0` is the sqlite-vec virtual table, spiked in Phase 2.
CREATE VIRTUAL TABLE IF NOT EXISTS note_vectors USING vec0(
    note_id INTEGER PRIMARY KEY,
    embedding float[768]
);

-- What has been embedded, and from which content. The hash is what makes Weave
-- resumable and idempotent: a note whose content has not changed is skipped, so
-- killing the daemon mid-batch costs at most the note it was working on.
CREATE TABLE IF NOT EXISTS embed_state (
    note_id     INTEGER PRIMARY KEY REFERENCES notes(id) ON DELETE CASCADE,
    hash        TEXT NOT NULL,
    model       TEXT NOT NULL,
    dimensions  INTEGER NOT NULL,
    embedded_at TEXT NOT NULL
);

-- Suggested links. **Never** the `links` table.
--
-- Each row carries its score and the model that produced it, because the spec
-- requires an inferred edge to show its source and score wherever it appears.
-- `state` records the user's decision so a dismissed suggestion does not come
-- back on the next pass.
CREATE TABLE IF NOT EXISTS suggested_links (
    id         INTEGER PRIMARY KEY,
    src        INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
    dst        INTEGER NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
    score      REAL    NOT NULL,
    model      TEXT    NOT NULL,
    created_at TEXT    NOT NULL,
    -- open | accepted | dismissed
    state      TEXT    NOT NULL DEFAULT 'open',
    UNIQUE(src, dst)
);
CREATE INDEX IF NOT EXISTS suggested_src   ON suggested_links(src);
CREATE INDEX IF NOT EXISTS suggested_state ON suggested_links(state);
"#;

pub fn migrate(conn: &Connection) -> Result<()> {
    let found: Option<i64> = conn
        .query_row("SELECT version FROM arc_meta WHERE id = 1", [], |r| {
            r.get(0)
        })
        .ok();

    if let Some(v) = found {
        if v != SCHEMA_VERSION {
            // Not an error the caller must handle by hand: the index is a cache,
            // so the fix is always "throw it away and rebuild". The caller does
            // that by deleting the file and reopening.
            return Err(crate::IndexError::SchemaMismatch {
                found: v,
                expected: SCHEMA_VERSION,
            });
        }
        return Ok(());
    }

    conn.execute_batch(DDL)?;
    conn.execute(
        "INSERT OR REPLACE INTO arc_meta(id, version, built_at) VALUES (1, ?1, datetime('now'))",
        rusqlite::params![SCHEMA_VERSION],
    )?;
    Ok(())
}

/// Case-fold for comparison. One place, so link resolution, tag matching and
/// heading anchors all fold identically.
pub fn fold(s: &str) -> String {
    s.trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrate_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap();

        let n: i64 = conn
            .query_row("SELECT count(*) FROM arc_meta", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn a_schema_from_the_future_is_refused_rather_than_used() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn.execute("UPDATE arc_meta SET version = 99 WHERE id = 1", [])
            .unwrap();

        assert!(matches!(
            migrate(&conn),
            Err(crate::IndexError::SchemaMismatch {
                found: 99,
                expected: SCHEMA_VERSION
            })
        ));
    }

    #[test]
    fn deleting_a_note_cascades_to_everything_derived_from_it() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        migrate(&conn).unwrap();

        conn.execute_batch(
            "INSERT INTO notes(id, path, stem, stem_folded, path_folded, size, mtime, hash)
                 VALUES (1, 'a.md', 'a', 'a', 'a.md', 10, 0, 'blake3:x');
             INSERT INTO links(src, target, target_folded) VALUES (1, 'B', 'b');
             INSERT INTO tags(src, name, name_folded)      VALUES (1, 'Rust', 'rust');
             INSERT INTO blocks(src, level, text, text_folded, line)
                 VALUES (1, 1, 'Heading', 'heading', 1);",
        )
        .unwrap();

        conn.execute("DELETE FROM notes WHERE id = 1", []).unwrap();
        for table in ["links", "tags", "blocks"] {
            let n: i64 = conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 0, "{table} kept rows for a deleted note");
        }
    }

    #[test]
    fn folding_is_consistent_across_the_things_that_compare_names() {
        assert_eq!(fold("  Some Note  "), "some note");
        assert_eq!(fold("Daily/2026-09-02"), "daily/2026-09-02");
        assert_eq!(fold("RUST"), fold("rust"));
    }
}
