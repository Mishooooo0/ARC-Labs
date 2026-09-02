//! The derived index: SQLite with FTS5 for text and `sqlite-vec` for vectors.
//!
//! # This file is a cache, and it is meant to be deleted
//!
//! Constraint 1 says files are the source of truth and the index is derived.
//! That is not a slogan here — `.arc/index.db` can be deleted at any moment and
//! rebuilt with zero data loss, and that is a Phase 2 acceptance gate rather
//! than a claim. So nothing is ever stored here that is not recoverable by
//! re-reading the vault: no user text that is not already in a note, no state
//! that only exists in the database.
//!
//! # Why the vector extension is spiked now
//!
//! `sqlite-vec` is not used until Phase 6, but it is the least-proven dependency
//! in the stack and it has to link on three targets: Windows MSVC, Linux glibc,
//! and the Docker base image. Discovering a linking problem in Phase 6 would be
//! a rewrite; discovering it here is an afternoon. So the schema, the extension
//! registration and a round-trip test all land in Phase 2 even though nothing
//! reads a vector yet.

use std::path::Path;

use rusqlite::Connection;

pub mod indexer;
pub mod query;
pub mod schema;

pub use indexer::{build, BuildStats};
pub use query::{Graph, IndexStats, NoteRef, SearchHit};
pub use schema::SCHEMA_VERSION;

#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("index is at schema version {found}, this build expects {expected}")]
    SchemaMismatch { found: i64, expected: i64 },
    #[error("io error at {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub type Result<T> = std::result::Result<T, IndexError>;

/// Open (or create) the index database at `path`.
pub fn open(path: &Path) -> Result<Connection> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| IndexError::Io { path: dir.to_path_buf(), source: e })?;
    }
    // Registering before the connection is opened is what makes vec0 available
    // to every connection this process creates, including ones opened later.
    register_vector_extension();
    let conn = Connection::open(path)?;
    configure(&conn)?;
    schema::migrate(&conn)?;
    Ok(conn)
}

/// An open index.
///
/// The whole point of this type is that `rusqlite` stops here. `arc-labs-api`
/// holds an `Index`, not a `Connection`, so nothing above this crate learns that
/// the index is SQLite at all — which is what makes swapping the storage a
/// change to one crate rather than to every caller.
pub struct Index {
    conn: Connection,
    path: std::path::PathBuf,
}

impl std::fmt::Debug for Index {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Index").field("path", &self.path).finish()
    }
}

impl Index {
    /// Open the index for a vault, at `<vault>/.arc/index.db`.
    ///
    /// A database that cannot be opened — corrupt, or written by a different
    /// schema version — is **deleted and rebuilt**, not repaired. That recovery
    /// path is the entire benefit of a derived cache, and exercising it here
    /// routinely is what keeps it working when it is actually needed.
    pub fn open_for_vault(vault_root: &Path) -> Result<Index> {
        let path = vault_root.join(".arc").join("index.db");
        match open(&path) {
            Ok(conn) => Ok(Index { conn, path }),
            Err(e) => {
                tracing::warn!(error = %e, "index unusable; discarding and rebuilding");
                remove_database(&path);
                let conn = open(&path)?;
                Ok(Index { conn, path })
            }
        }
    }

    pub fn in_memory() -> Result<Index> {
        Ok(Index { conn: open_in_memory()?, path: std::path::PathBuf::from(":memory:") })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn build(
        &mut self,
        vault: &arc_labs_core::Vault,
        force: bool,
        on_progress: impl FnMut(indexer::Progress),
    ) -> Result<BuildStats> {
        indexer::build(&mut self.conn, vault, force, on_progress)
    }

    pub fn reindex_note(
        &mut self,
        vault: &arc_labs_core::Vault,
        path: &arc_labs_core::VaultPath,
    ) -> Result<()> {
        indexer::reindex_note(&mut self.conn, vault, path)
    }

    pub fn forget_note(&self, path: &arc_labs_core::VaultPath) -> Result<()> {
        indexer::forget_note(&self.conn, path)
    }

    pub fn search(&self, q: &str, limit: usize) -> Result<Vec<query::SearchHit>> {
        query::search(&self.conn, q, limit)
    }
    pub fn quick_open(&self, q: &str, limit: usize) -> Result<Vec<query::NoteRef>> {
        query::quick_open(&self.conn, q, limit)
    }
    pub fn recent(&self, limit: usize) -> Result<Vec<query::NoteRef>> {
        query::recent(&self.conn, limit)
    }
    pub fn backlinks(&self, path: &str) -> Result<Vec<query::Backlink>> {
        query::backlinks(&self.conn, path)
    }
    pub fn outgoing(&self, path: &str) -> Result<Vec<query::OutgoingLink>> {
        query::outgoing(&self.conn, path)
    }
    pub fn unresolved(&self, limit: usize) -> Result<Vec<query::UnresolvedLink>> {
        query::unresolved(&self.conn, limit)
    }
    pub fn tag_counts(&self) -> Result<Vec<query::TagCount>> {
        query::tag_counts(&self.conn)
    }
    pub fn notes_with_tag(&self, tag: &str) -> Result<Vec<query::NoteRef>> {
        query::notes_with_tag(&self.conn, tag)
    }
    pub fn graph(&self) -> Result<query::Graph> {
        query::graph(&self.conn)
    }
    pub fn stats(&self) -> Result<query::IndexStats> {
        query::stats(&self.conn)
    }
}

/// Remove a database and the files WAL mode keeps beside it.
///
/// All three, or a reopen finds the old journal and resurrects the broken index.
pub fn remove_database(path: &Path) {
    let _ = std::fs::remove_file(path);
    for suffix in ["-wal", "-shm"] {
        let mut p = path.as_os_str().to_os_string();
        p.push(suffix);
        let _ = std::fs::remove_file(std::path::PathBuf::from(p));
    }
}

/// An in-memory index. Used by tests, and by `reindex --dry-run`.
pub fn open_in_memory() -> Result<Connection> {
    register_vector_extension();
    let conn = Connection::open_in_memory()?;
    configure(&conn)?;
    schema::migrate(&conn)?;
    Ok(conn)
}

/// Statically link `sqlite-vec` into every connection in this process.
///
/// `sqlite3_vec_init` is the extension's *entry point*, not a registration
/// call — invoking it directly does nothing useful. It has to be handed to
/// SQLite's `sqlite3_auto_extension` hook, which then runs it against every
/// connection opened afterwards, including ones opened by other threads.
///
/// Through `Once` because that hook is global process state: registering twice
/// would run the initialiser twice on every connection.
fn register_vector_extension() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        // The signature SQLite expects for an auto-extension differs from the
        // one bindgen produced for the extension entry point, so a cast is
        // unavoidable. Annotated explicitly rather than inferred: an untyped
        // transmute between function pointers is exactly the kind of thing that
        // silently keeps compiling after a signature changes underneath it.
        type ExtInit = unsafe extern "C" fn(
            *mut rusqlite::ffi::sqlite3,
            *mut *mut std::os::raw::c_char,
            *const rusqlite::ffi::sqlite3_api_routines,
        ) -> std::os::raw::c_int;
        let init = std::mem::transmute::<*const (), ExtInit>(
            sqlite_vec::sqlite3_vec_init as *const (),
        );
        rusqlite::ffi::sqlite3_auto_extension(Some(init));
    });
}

fn configure(conn: &Connection) -> Result<()> {
    // WAL: the Phase 6 Weave daemon writes embeddings while the editor reads.
    // Without WAL those block each other, and Phase 1's typing budget is the
    // thing that loses.
    conn.pragma_update(None, "journal_mode", "WAL")?;
    // NORMAL is the right trade for a derived cache: a crash can cost the last
    // transaction, and the answer to that is to rebuild from the files, which
    // is exactly what this index is designed to survive.
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    // 64 MB. Enough to keep a 5,000-note index hot; small enough not to matter
    // on a machine with 23 GB where most of it is already spoken for.
    conn.pragma_update(None, "cache_size", -64_000)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fts5_is_compiled_in() {
        // rusqlite's `bundled` feature is expected to enable FTS5. Asserting it
        // here means a dependency bump that silently drops it fails a test
        // instead of failing search at runtime.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE VIRTUAL TABLE t USING fts5(body);").expect("FTS5 missing");
        conn.execute("INSERT INTO t(body) VALUES ('the ledger records provenance')", [])
            .unwrap();

        let hit: String = conn
            .query_row("SELECT body FROM t WHERE t MATCH 'provenance'", [], |r| r.get(0))
            .unwrap();
        assert!(hit.contains("provenance"));
    }

    #[test]
    fn fts5_supports_snippets_and_ranking() {
        // Both are used by the search surface; both are FTS5 auxiliary
        // functions that a minimal build can omit.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE VIRTUAL TABLE t USING fts5(body);
             INSERT INTO t(body) VALUES ('the ledger records provenance for every mutation');",
        )
        .unwrap();

        let snip: String = conn
            .query_row(
                "SELECT snippet(t, 0, '[', ']', '…', 8) FROM t WHERE t MATCH 'provenance'
                 ORDER BY rank",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(snip.contains("[provenance]"), "got {snip}");
    }

    /// **The Phase 2 spike.** If this fails on any target, Phase 6 has no vector
    /// search and we find out now rather than five phases from now.
    #[test]
    fn sqlite_vec_links_and_round_trips_a_vector() {
        register_vector_extension();
        let conn = Connection::open_in_memory().unwrap();

        let version: String =
            conn.query_row("SELECT vec_version()", [], |r| r.get(0)).expect("vec0 not loaded");
        assert!(!version.is_empty());

        // nomic-embed-text is 768-dimensional; exercise the real width.
        conn.execute_batch(
            "CREATE VIRTUAL TABLE v USING vec0(note_id INTEGER PRIMARY KEY, embedding float[768]);",
        )
        .expect("vec0 virtual table failed");

        let a: Vec<f32> = (0..768).map(|i| (i as f32 % 7.0) - 3.0).collect();
        let b: Vec<f32> = (0..768).map(|i| (i as f32 % 11.0) - 5.0).collect();
        for (id, v) in [(1i64, &a), (2i64, &b)] {
            conn.execute(
                "INSERT INTO v(note_id, embedding) VALUES (?1, ?2)",
                rusqlite::params![id, bytemuck_cast(v)],
            )
            .unwrap();
        }

        // A KNN query against the first vector must return it first.
        let nearest: i64 = conn
            .query_row(
                "SELECT note_id FROM v WHERE embedding MATCH ?1 AND k = 1 ORDER BY distance",
                rusqlite::params![bytemuck_cast(&a)],
                |r| r.get(0),
            )
            .expect("KNN query failed");
        assert_eq!(nearest, 1);
    }

    /// f32 slice to the little-endian bytes vec0 expects.
    #[cfg(test)]
    fn bytemuck_cast(v: &[f32]) -> Vec<u8> {
        v.iter().flat_map(|f| f.to_le_bytes()).collect()
    }

    #[test]
    fn opening_creates_a_usable_database_with_our_schema() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join(".arc").join("index.db");
        let conn = open(&db).unwrap();

        let version: i64 =
            conn.query_row("SELECT version FROM arc_meta WHERE id = 1", [], |r| r.get(0)).unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        assert!(db.exists(), "the parent directory should have been created");
    }
}
