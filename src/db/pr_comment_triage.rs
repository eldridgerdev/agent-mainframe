//! SQLite persistence for local PR-comment triage, keyed by
//! `PR# + comment id`.
//!
//! GitHub thread resolution is the source of truth for done/not-done; this is
//! the *local* layer on top of it — "a fix was injected" ([`TriageState::Fixing`]),
//! "I marked this done" ([`TriageState::Done`]), skip reasons, etc. The GitHub
//! comment id is stable across commits, so triage is keyed by `PR# + comment id`
//! and **survives a push** that moves the PR's head; `head_sha` is recorded only
//! as the last SHA a mark was set under, not part of the identity. (It was
//! originally keyed by head SHA too, which silently dropped every mark on the
//! next re-resolve — see migration 010.)
//! See `docs/backlog/pr-comment-review-plan.md`.

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::{Connection, params};

use crate::app::pr_review::TriageState;

/// Per-comment triage row: the state plus an optional local note.
pub type TriageRow = (TriageState, Option<String>);

/// Load every triage row for `pr_number` as `comment_id -> row`, regardless of
/// the head SHA the mark was set under (so triage survives a push). Rows with an
/// unknown state token degrade to [`TriageState::Untriaged`].
pub fn load(conn: &Connection, pr_number: u32) -> Result<HashMap<u64, TriageRow>> {
    let mut stmt = conn.prepare(
        "SELECT comment_id, state, note FROM pr_comment_triage
         WHERE pr_number = ?1",
    )?;
    let rows = stmt.query_map(params![pr_number as i64], |row| {
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
/// `PR# + comment id` key. `head_sha` is recorded as the SHA the mark was set
/// under but is not part of the identity, so re-marking after a push overwrites
/// the same row rather than creating a per-SHA duplicate.
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

/// Drop triage rows older than a week so abandoned PRs don't accumulate.
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

        let map = db.load_pr_comment_triage(321).unwrap();
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

        let map = db.load_pr_comment_triage(7).unwrap();
        assert_eq!(map.len(), 1);
        assert_eq!(map[&1].0, TriageState::Done);
    }

    #[test]
    fn survives_head_sha_change() {
        let (_tmp, db) = open_temp_db();
        // Mark a comment done under one head SHA.
        db.save_pr_comment_triage(7, "old", 1, TriageState::Done, None)
            .unwrap();
        // After a push moves the head, the mark still loads (keyed by comment id,
        // not SHA) — this is the bug fix.
        let map = db.load_pr_comment_triage(7).unwrap();
        assert_eq!(map[&1].0, TriageState::Done);

        // Re-marking under the new SHA overwrites the same row (no duplicate).
        db.save_pr_comment_triage(7, "new", 1, TriageState::Skipped, Some("nope"))
            .unwrap();
        let map = db.load_pr_comment_triage(7).unwrap();
        assert_eq!(map.len(), 1);
        assert_eq!(map[&1].0, TriageState::Skipped);
        assert_eq!(map[&1].1.as_deref(), Some("nope"));
    }
}
