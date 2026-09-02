//! Vector storage and inferred links.
//!
//! Everything in this module is **inferred**. Everything in [`crate::query`] is
//! **observed**. That split is constraint 7 expressed as file layout, and the
//! queries here never touch `links` except to *exclude* pairs that are already
//! linked — which is the one direction the two may safely meet.

use rusqlite::{params, Connection};

use crate::Result;

/// A note that needs embedding: `(id, path, title, hash)`.
pub type Pending = (i64, String, String, String);

/// An open suggestion, minimal: `(src_path, dst_path, score, model, id)`.
pub type SuggestionRow = (String, String, f64, String, i64);

/// An open suggestion with everything the inbox shows:
/// `(id, src_path, src_title, dst_path, dst_title, score, model, created_at)`.
pub type SuggestionDetail = (i64, String, String, String, String, f64, String, String);

/// Notes whose content has changed since they were last embedded — or which
/// were embedded by a different model or at a different width.
///
/// The model and dimension checks matter: switching embedding models makes
/// every stored vector meaningless, and comparing across two models produces
/// confident nonsense rather than an error.
pub fn notes_needing_embedding(
    conn: &Connection,
    model: &str,
    dimensions: usize,
) -> Result<Vec<Pending>> {
    let mut stmt = conn.prepare_cached(
        "SELECT n.id, n.path, COALESCE(n.title, n.stem), n.hash
         FROM notes n
         LEFT JOIN embed_state e ON e.note_id = n.id
         WHERE n.is_canvas = 0
           AND (e.note_id IS NULL
                OR e.hash != n.hash
                OR e.model != ?1
                OR e.dimensions != ?2)
         ORDER BY n.id",
    )?;
    let rows = stmt.query_map(params![model, dimensions as i64], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn store_embedding(
    conn: &Connection,
    note_id: i64,
    vector: &[f32],
    hash: &str,
    model: &str,
    dimensions: usize,
) -> Result<()> {
    let bytes: Vec<u8> = vector.iter().flat_map(|f| f.to_le_bytes()).collect();
    // vec0 has no upsert, so replace explicitly.
    conn.execute(
        "DELETE FROM note_vectors WHERE note_id = ?1",
        params![note_id],
    )?;
    conn.execute(
        "INSERT INTO note_vectors(note_id, embedding) VALUES (?1, ?2)",
        params![note_id, bytes],
    )?;
    conn.execute(
        "INSERT INTO embed_state(note_id, hash, model, dimensions, embedded_at)
         VALUES (?1, ?2, ?3, ?4, datetime('now'))
         ON CONFLICT(note_id) DO UPDATE SET
             hash=excluded.hash, model=excluded.model,
             dimensions=excluded.dimensions, embedded_at=excluded.embedded_at",
        params![note_id, hash, model, dimensions as i64],
    )?;
    Ok(())
}

/// Pairs of notes that are semantically close and **not** already linked.
///
/// The exclusion is the point. Suggesting a link that already exists is noise,
/// and it makes the inbox look like it does not know what is in the vault.
///
/// Returns `(src_id, dst_id, similarity)` with similarity in 0..1.
pub fn nearest_unlinked(
    conn: &Connection,
    per_note: usize,
    threshold: f64,
) -> Result<Vec<(i64, i64, f64)>> {
    let ids: Vec<i64> = {
        let mut stmt = conn.prepare("SELECT note_id FROM note_vectors ORDER BY note_id")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect::<std::result::Result<_, _>>()?
    };

    let mut out = Vec::new();
    for src in ids {
        let Some(vector) = embedding_of(conn, src)? else {
            continue;
        };

        // k+1 because a vector's nearest neighbour is always itself.
        let mut stmt = conn.prepare_cached(
            "SELECT v.note_id, v.distance
             FROM note_vectors v
             WHERE v.embedding MATCH ?1 AND k = ?2
             ORDER BY v.distance",
        )?;
        let neighbours: Vec<(i64, f64)> = {
            let rows = stmt.query_map(params![vector, (per_note + 1) as i64], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?))
            })?;
            rows.collect::<std::result::Result<_, _>>()?
        };

        for (dst, distance) in neighbours {
            if dst == src {
                continue;
            }
            // Vectors are unit length, so L2 distance and cosine similarity are
            // related by  cos = 1 - d²/2.
            let similarity = 1.0 - (distance * distance) / 2.0;
            if similarity < threshold {
                continue;
            }
            if already_linked(conn, src, dst)? {
                continue;
            }
            out.push((src, dst, similarity));
        }
    }
    Ok(out)
}

fn embedding_of(conn: &Connection, note_id: i64) -> Result<Option<Vec<u8>>> {
    let mut stmt = conn.prepare_cached("SELECT embedding FROM note_vectors WHERE note_id = ?1")?;
    Ok(stmt
        .query_row(params![note_id], |r| r.get::<_, Vec<u8>>(0))
        .ok())
}

/// Whether an observed link already connects these two, in either direction.
fn already_linked(conn: &Connection, a: i64, b: i64) -> Result<bool> {
    let sql = "SELECT 1 FROM links l
               JOIN notes n ON (
                    n.path_folded = l.target_folded
                 OR n.path_folded = l.target_folded || '.md'
                 OR n.stem_folded = l.target_folded)
               WHERE (l.src = ?1 AND n.id = ?2) OR (l.src = ?2 AND n.id = ?1)
               LIMIT 1";
    let mut stmt = conn.prepare_cached(sql)?;
    Ok(stmt.query_row(params![a, b], |_| Ok(())).is_ok())
}

/// Record a suggestion. Returns whether it was new.
///
/// A pair the user already dismissed is left alone: a suggestion inbox that
/// keeps re-proposing what you refused is one you stop reading.
///
/// The "already there" check is **direction-blind**. Similarity is symmetric, so
/// the nearest-neighbour scan finds every pair twice — once from each end — and
/// a direction-sensitive check let both through. The inbox then showed the same
/// relationship as two cards with identical scores, which reads as the vault not
/// knowing what it has already told you.
pub fn suggest_link(
    conn: &Connection,
    src: i64,
    dst: i64,
    score: f64,
    model: &str,
) -> Result<bool> {
    let seen: bool = conn
        .query_row(
            "SELECT 1 FROM suggested_links
             WHERE (src = ?1 AND dst = ?2) OR (src = ?2 AND dst = ?1)",
            params![src, dst],
            |_| Ok(()),
        )
        .is_ok();
    if seen {
        return Ok(false);
    }
    conn.execute(
        "INSERT OR IGNORE INTO suggested_links(src, dst, score, model, created_at, state)
         VALUES (?1, ?2, ?3, ?4, datetime('now'), 'open')",
        params![src, dst, score, model],
    )?;
    Ok(true)
}

/// Open suggestions, best first: `(src_path, dst_path, score, model, id)`.
pub fn suggestions(conn: &Connection, limit: usize) -> Result<Vec<SuggestionRow>> {
    let mut stmt = conn.prepare_cached(
        "SELECT s.id, a.path, b.path, s.score, s.model
         FROM suggested_links s
         JOIN notes a ON a.id = s.src
         JOIN notes b ON b.id = s.dst
         WHERE s.state = 'open'
         ORDER BY s.score DESC, s.id
         LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit as i64], |r| {
        Ok((r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(0)?))
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Rich form for the inbox surface, with titles.
pub fn suggestions_detailed(conn: &Connection, limit: usize) -> Result<Vec<SuggestionDetail>> {
    let mut stmt = conn.prepare_cached(
        "SELECT s.id, a.path, COALESCE(a.title, a.stem), b.path, COALESCE(b.title, b.stem),
                s.score, s.model, s.created_at
         FROM suggested_links s
         JOIN notes a ON a.id = s.src
         JOIN notes b ON b.id = s.dst
         WHERE s.state = 'open'
         ORDER BY s.score DESC, s.id
         LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit as i64], |r| {
        Ok((
            r.get(0)?,
            r.get(1)?,
            r.get(2)?,
            r.get(3)?,
            r.get(4)?,
            r.get(5)?,
            r.get(6)?,
            r.get(7)?,
        ))
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn set_suggestion_state(conn: &Connection, id: i64, state: &str) -> Result<()> {
    conn.execute(
        "UPDATE suggested_links SET state = ?2 WHERE id = ?1",
        params![id, state],
    )?;
    Ok(())
}

pub fn embedding_progress(conn: &Connection, model: &str, dimensions: usize) -> Result<(i64, i64)> {
    let total: i64 = conn.query_row("SELECT count(*) FROM notes WHERE is_canvas = 0", [], |r| {
        r.get(0)
    })?;
    let done: i64 = conn.query_row(
        "SELECT count(*) FROM embed_state e JOIN notes n ON n.id = e.note_id
         WHERE e.hash = n.hash AND e.model = ?1 AND e.dimensions = ?2",
        params![model, dimensions as i64],
        |r| r.get(0),
    )?;
    Ok((done, total))
}
