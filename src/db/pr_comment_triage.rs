//! SQLite persistence for local PR-comment triage, keyed by
//! `PR# + comment id + head SHA`.
//!
//! GitHub thread resolution is the source of truth for done/not-done; this is
//! the *local* layer on top of it — "a fix was injected" ([`TriageState::Fixing`]),
//! "I marked this done" ([`TriageState::Done`]), skip reasons, etc. It is keyed
//! by head SHA so re-triage starts fresh after a push moves the PR's head.
//! See `docs/backlog/pr-comment-review-plan.md`.

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::{Connection, params};

use crate::app::pr_review::TriageState;

/// Per-comment triage row: the state plus an optional local note.
pub type TriageRow = (TriageState, Option<String>);

/// Load every triage row for `(pr_number, head_sha)` as `comment_id -> row`.
/// Rows with an unknown state token degrade to [`TriageState::Untriaged`].
pub fn load(
    conn: &Connection,
    pr_number: u32,
    head_sha: &str,
) -> Result<HashMap<u64, TriageRow>> {
    let mut stmt = conn.prepare(
        "SELECT comment_id, state, note FROM pr_comment_triage
         WHERE pr_number = ?1 AND head_sha = ?2",
    )?;
    let rows = stmt.query_map(params![pr_number as i64, head_sha], |row| {
        let comment_id: i64 = row.get(0)?;
        let state: String = row.get(1)?;
        let note: Option<String> = row.get(2)?;
        Ok((comment_id as u64, (TriageState::from_db_str(&state), note)))
    })?;

    let mut map = HashMap::new();
    for row in rows {
        let (comment_id, triage) = row?;
        map.insert(comment_id, triage);
    }
    Ok(map)
}

/// Upsert one comment's triage state (and optional note) under its
/// `PR# + comment id + head SHA` key.
pub fn upsert(
    conn: &Connection,
    pr_number: u32,
    head_sha: &str,
    comment_id: u64,
    state: TriageState,
    note: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO pr_comment_triage
            (pr_number, comment_id, head_sha, state, note, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
        params![
            pr_number as i64,
            comment_id as i64,
            head_sha,
            state.as_db_str(),
            note,
        ],
    )?;
    Ok(())
}

/// Drop triage rows older than a week so stale head-SHA entries don't accumulate.
pub fn evict_stale(conn: &Connection) -> Result<()> {
    conn.execute(
        "DELETE FROM pr_comment_triage
         WHERE updated_at < datetime('now', '-7 days')",
        [],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::app::pr_review::TriageState;
    use crate::db::AmfDb;
    use tempfile::NamedTempFile;

    fn open_temp_db() -> (NamedTempFile, AmfDb) {
        let tmp = NamedTempFile::new().unwrap();
        let db = AmfDb::open(tmp.path()).unwrap();
        (tmp, db)
    }

    #[test]
    fn roundtrips_state_and_note() {
        let (_tmp, db) = open_temp_db();
        db.save_pr_comment_triage(321, "sha", 11, TriageState::Fixing, None)
            .unwrap();
        db.save_pr_comment_triage(321, "sha", 12, TriageState::Skipped, Some("not needed"))
            .unwrap();

        let map = db.load_pr_comment_triage(321, "sha").unwrap();
        assert_eq!(map.len(), 2);
        assert_eq!(map[&11].0, TriageState::Fixing);
        assert_eq!(map[&11].1, None);
        assert_eq!(map[&12].0, TriageState::Skipped);
        assert_eq!(map[&12].1.as_deref(), Some("not needed"));
    }

    #[test]
    fn upsert_overwrites_same_key() {
        let (_tmp, db) = open_temp_db();
        db.save_pr_comment_triage(7, "sha", 1, TriageState::Fixing, None)
            .unwrap();
        db.save_pr_comment_triage(7, "sha", 1, TriageState::Done, None)
            .unwrap();

        let map = db.load_pr_comment_triage(7, "sha").unwrap();
        assert_eq!(map.len(), 1);
        assert_eq!(map[&1].0, TriageState::Done);
    }

    #[test]
    fn keyed_by_head_sha() {
        let (_tmp, db) = open_temp_db();
        db.save_pr_comment_triage(7, "old", 1, TriageState::Done, None)
            .unwrap();
        // A new head SHA starts fresh — no triage carries over.
        assert!(db.load_pr_comment_triage(7, "new").unwrap().is_empty());
    }
}
