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
use rusqlite::{Connection, OptionalExtension, params};

use crate::app::pr_review::TriageState;

/// Per-comment triage row: the state, an optional local note, and — when the
/// comment was fixed as part of a combined batch (`B`) — the id shared by every
/// resolved comment in that batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriageRow {
    pub state: TriageState,
    pub note: Option<String>,
    /// `Some` only for a comment fixed inside a combined batch; every
    /// pre-`MIGRATION_032` row and every single-comment fix reads back `None`.
    pub batch_id: Option<String>,
    /// The preformatted USD figure the batch's single agent run cost, shared by
    /// every sibling. Written once, when the first sibling is resolved (see
    /// [`set_batch_fix_cost`]); `None` until then and for a non-batch row.
    pub batch_fix_cost: Option<String>,
}

/// Load every triage row for `pr_number` as `comment_id -> row`, regardless of
/// the head SHA the mark was set under (so triage survives a push). Rows with an
/// unknown state token degrade to [`TriageState::Untriaged`].
pub fn load(conn: &Connection, pr_number: u32) -> Result<HashMap<u64, TriageRow>> {
    let mut stmt = conn.prepare(
        "SELECT comment_id, state, note, batch_id, batch_fix_cost FROM pr_comment_triage
         WHERE pr_number = ?1",
    )?;
    let rows = stmt.query_map(params![pr_number as i64], |row| {
        let comment_id: i64 = row.get(0)?;
        let state: String = row.get(1)?;
        let note: Option<String> = row.get(2)?;
        let batch_id: Option<String> = row.get(3)?;
        let batch_fix_cost: Option<String> = row.get(4)?;
        Ok((
            comment_id as u64,
            TriageRow {
                state: TriageState::from_db_str(&state),
                note,
                batch_id,
                batch_fix_cost,
            },
        ))
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
///
/// `batch_id` is *sticky*: pass `Some` to stamp the row (the combined-batch
/// dispatch does this before the agent run), and `None` — every ordinary
/// caller — to leave whatever is already there untouched. That is what lets a
/// batch membership survive the row's later transition to `Done`/`Skipped`,
/// which is written by a plain `upsert(..., None)` from the reply flow.
pub fn upsert(
    conn: &Connection,
    pr_number: u32,
    head_sha: &str,
    comment_id: u64,
    state: TriageState,
    note: Option<&str>,
    batch_id: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO pr_comment_triage
            (pr_number, comment_id, head_sha, state, note, updated_at, batch_id)
         VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'), ?6)
         ON CONFLICT(pr_number, comment_id) DO UPDATE SET
            head_sha = excluded.head_sha,
            state = excluded.state,
            note = excluded.note,
            updated_at = excluded.updated_at,
            batch_id = COALESCE(excluded.batch_id, pr_comment_triage.batch_id)",
        params![
            pr_number as i64,
            comment_id as i64,
            head_sha,
            state.as_db_str(),
            note,
            batch_id,
        ],
    )?;
    Ok(())
}

/// Comment ids in `pr_number` that share `batch_id` — the siblings of a
/// combined-batch fix. Used to highlight sibling rows in the triage overlay and
/// to note the batch (and its size) in each posted GitHub reply. Sorted for a
/// stable order. Empty for an unknown / `NULL` batch id.
pub fn batch_sibling_ids(conn: &Connection, pr_number: u32, batch_id: &str) -> Result<Vec<u64>> {
    let mut stmt = conn.prepare(
        "SELECT comment_id FROM pr_comment_triage
         WHERE pr_number = ?1 AND batch_id = ?2
         ORDER BY comment_id",
    )?;
    let ids = stmt
        .query_map(params![pr_number as i64, batch_id], |row| {
            row.get::<_, i64>(0).map(|id| id as u64)
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(ids)
}

/// Record the batch's shared fix cost on every sibling row that doesn't have
/// one yet, so the figure survives the reply-draft it was derived from being
/// deleted on post. **First writer wins** (`batch_fix_cost IS NULL` guard): a
/// later sibling resolving with its own slightly-larger session delta doesn't
/// overwrite the batch's agreed cost. Returns the number of rows updated (0
/// once the cost is already set, or for an unknown batch id).
pub fn set_batch_fix_cost(
    conn: &Connection,
    pr_number: u32,
    batch_id: &str,
    cost: &str,
) -> Result<usize> {
    let changed = conn.execute(
        "UPDATE pr_comment_triage
         SET batch_fix_cost = ?3, updated_at = datetime('now')
         WHERE pr_number = ?1 AND batch_id = ?2 AND batch_fix_cost IS NULL",
        params![pr_number as i64, batch_id, cost],
    )?;
    Ok(changed)
}

/// Drop triage rows older than a week so abandoned PRs don't accumulate.
pub fn evict_stale(conn: &Connection) -> Result<()> {
    conn.execute(
        "DELETE FROM pr_comment_triage
         WHERE updated_at < datetime('now', '-7 days')",
        [],
    )?;
    conn.execute(
        "DELETE FROM pr_comment_reply_drafts
         WHERE updated_at < datetime('now', '-7 days')",
        [],
    )?;
    Ok(())
}

/// A stored reply draft: the agent's body, the PR head it was requested
/// against, and the opaque provenance blob describing the session that produced
/// it (see [`begin_reply_draft`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyDraftRow {
    pub body: String,
    pub base_head_sha: String,
    pub provenance: Option<String>,
}

/// Start a new reply-draft request for one comment. Replacing the request id
/// and clearing `body` invalidates any prior draft before the fix prompt is
/// delivered; a late response carrying the old id is ignored by [`capture_reply_draft`].
///
/// `provenance` is written at the same moment for the same reason as
/// `request_id`: this is the only point where the session about to write the
/// draft is known. It is stored opaquely (JSON owned by the caller) and travels
/// with the draft, so the disclosure posted later describes the session that
/// really generated it rather than whatever the pane is pointing at by then.
pub fn begin_reply_draft(
    conn: &Connection,
    pr_number: u32,
    comment_id: u64,
    request_id: &str,
    base_head_sha: &str,
    provenance: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO pr_comment_reply_drafts
            (pr_number, comment_id, request_id, body, updated_at, base_head_sha,
             provenance)
         VALUES (?1, ?2, ?3, NULL, datetime('now'), ?4, ?5)
         ON CONFLICT(pr_number, comment_id) DO UPDATE SET
            request_id = excluded.request_id,
            body = NULL,
            updated_at = excluded.updated_at,
            base_head_sha = excluded.base_head_sha,
            provenance = excluded.provenance",
        params![
            pr_number as i64,
            comment_id as i64,
            request_id,
            base_head_sha,
            provenance
        ],
    )?;
    Ok(())
}

/// Store the agent's reply only when it belongs to the comment's latest fix
/// request. Returns `false` for an expired/unknown request id.
pub fn capture_reply_draft(
    conn: &Connection,
    pr_number: u32,
    comment_id: u64,
    request_id: &str,
    body: &str,
) -> Result<bool> {
    let changed = conn.execute(
        "UPDATE pr_comment_reply_drafts
         SET body = ?4, updated_at = datetime('now')
         WHERE pr_number = ?1 AND comment_id = ?2 AND request_id = ?3",
        params![pr_number as i64, comment_id as i64, request_id, body],
    )?;
    Ok(changed == 1)
}

/// Latest captured draft for a comment, if the current request has produced
/// one. A freshly-started request deliberately reads as `None`.
pub fn load_reply_draft(
    conn: &Connection,
    pr_number: u32,
    comment_id: u64,
) -> Result<Option<ReplyDraftRow>> {
    let row = conn
        .query_row(
            "SELECT body, base_head_sha, provenance FROM pr_comment_reply_drafts
             WHERE pr_number = ?1 AND comment_id = ?2",
            params![pr_number as i64, comment_id as i64],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()?;
    Ok(row.and_then(|(body, base_head_sha, provenance)| {
        body.map(|body| ReplyDraftRow {
            body,
            base_head_sha,
            provenance,
        })
    }))
}

/// Remove a consumed reply draft after AMF successfully posts it.
pub fn clear_reply_draft(conn: &Connection, pr_number: u32, comment_id: u64) -> Result<()> {
    conn.execute(
        "DELETE FROM pr_comment_reply_drafts
         WHERE pr_number = ?1 AND comment_id = ?2",
        params![pr_number as i64, comment_id as i64],
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
        db.save_pr_comment_triage(321, "sha", 11, TriageState::Fixing, None, None)
            .unwrap();
        db.save_pr_comment_triage(
            321,
            "sha",
            12,
            TriageState::Skipped,
            Some("not needed"),
            None,
        )
        .unwrap();

        let map = db.load_pr_comment_triage(321).unwrap();
        assert_eq!(map.len(), 2);
        assert_eq!(map[&11].state, TriageState::Fixing);
        assert_eq!(map[&11].note, None);
        assert_eq!(map[&11].batch_id, None);
        assert_eq!(map[&12].state, TriageState::Skipped);
        assert_eq!(map[&12].note.as_deref(), Some("not needed"));
    }

    #[test]
    fn upsert_overwrites_same_key() {
        let (_tmp, db) = open_temp_db();
        db.save_pr_comment_triage(7, "sha", 1, TriageState::Fixing, None, None)
            .unwrap();
        db.save_pr_comment_triage(7, "sha", 1, TriageState::Done, None, None)
            .unwrap();

        let map = db.load_pr_comment_triage(7).unwrap();
        assert_eq!(map.len(), 1);
        assert_eq!(map[&1].state, TriageState::Done);
    }

    #[test]
    fn survives_head_sha_change() {
        let (_tmp, db) = open_temp_db();
        // Mark a comment done under one head SHA.
        db.save_pr_comment_triage(7, "old", 1, TriageState::Done, None, None)
            .unwrap();
        // After a push moves the head, the mark still loads (keyed by comment id,
        // not SHA) — this is the bug fix.
        let map = db.load_pr_comment_triage(7).unwrap();
        assert_eq!(map[&1].state, TriageState::Done);

        // Re-marking under the new SHA overwrites the same row (no duplicate).
        db.save_pr_comment_triage(7, "new", 1, TriageState::Skipped, Some("nope"), None)
            .unwrap();
        let map = db.load_pr_comment_triage(7).unwrap();
        assert_eq!(map.len(), 1);
        assert_eq!(map[&1].state, TriageState::Skipped);
        assert_eq!(map[&1].note.as_deref(), Some("nope"));
    }

    #[test]
    fn batch_id_round_trips_and_is_sticky_across_state_changes() {
        let (_tmp, db) = open_temp_db();
        // A combined batch stamps every comment it resolves with one shared id
        // alongside the `Fixing` mark.
        db.save_pr_comment_triage(9, "sha", 100, TriageState::Fixing, None, Some("batch-a"))
            .unwrap();
        db.save_pr_comment_triage(9, "sha", 101, TriageState::Fixing, None, Some("batch-a"))
            .unwrap();
        // A sibling that never resolved gets no batch id (partial batch).
        db.save_pr_comment_triage(9, "sha", 102, TriageState::Untriaged, None, None)
            .unwrap();

        let map = db.load_pr_comment_triage(9).unwrap();
        assert_eq!(map[&100].batch_id.as_deref(), Some("batch-a"));
        assert_eq!(map[&101].batch_id.as_deref(), Some("batch-a"));
        assert_eq!(map[&102].batch_id, None);

        // The reply flow later moves 100 to `Done` with a plain upsert (no batch
        // id). Membership must survive for reply grouping.
        db.save_pr_comment_triage(9, "sha2", 100, TriageState::Done, None, None)
            .unwrap();
        let map = db.load_pr_comment_triage(9).unwrap();
        assert_eq!(map[&100].state, TriageState::Done);
        assert_eq!(
            map[&100].batch_id.as_deref(),
            Some("batch-a"),
            "a None batch id on a later upsert keeps the existing membership"
        );
    }

    #[test]
    fn batch_fix_cost_is_written_once_and_shared_by_every_sibling() {
        let (_tmp, db) = open_temp_db();
        db.save_pr_comment_triage(9, "sha", 1, TriageState::Fixing, None, Some("b1"))
            .unwrap();
        db.save_pr_comment_triage(9, "sha", 2, TriageState::Fixing, None, Some("b1"))
            .unwrap();

        // First sibling to resolve records the run's cost for the whole batch.
        assert_eq!(
            db.set_pr_comment_batch_fix_cost(9, "b1", "$0.06").unwrap(),
            2
        );
        let map = db.load_pr_comment_triage(9).unwrap();
        assert_eq!(map[&1].batch_fix_cost.as_deref(), Some("$0.06"));
        assert_eq!(map[&2].batch_fix_cost.as_deref(), Some("$0.06"));

        // A later sibling resolving with a slightly different figure does not
        // overwrite the agreed batch cost (first writer wins).
        assert_eq!(
            db.set_pr_comment_batch_fix_cost(9, "b1", "$0.07").unwrap(),
            0
        );
        let map = db.load_pr_comment_triage(9).unwrap();
        assert_eq!(map[&1].batch_fix_cost.as_deref(), Some("$0.06"));

        // Unknown batch id: nothing to update.
        assert_eq!(
            db.set_pr_comment_batch_fix_cost(9, "nope", "$1").unwrap(),
            0
        );
    }

    #[test]
    fn batch_sibling_ids_returns_only_same_batch_same_pr_sorted() {
        let (_tmp, db) = open_temp_db();
        db.save_pr_comment_triage(9, "sha", 202, TriageState::Fixing, None, Some("batch-a"))
            .unwrap();
        db.save_pr_comment_triage(9, "sha", 200, TriageState::Done, None, Some("batch-a"))
            .unwrap();
        db.save_pr_comment_triage(9, "sha", 201, TriageState::Fixing, None, Some("batch-b"))
            .unwrap();
        db.save_pr_comment_triage(9, "sha", 203, TriageState::Untriaged, None, None)
            .unwrap();
        // Same batch id string, different PR — must not leak across PRs.
        db.save_pr_comment_triage(10, "sha", 999, TriageState::Fixing, None, Some("batch-a"))
            .unwrap();

        assert_eq!(
            db.pr_comment_triage_batch_siblings(9, "batch-a").unwrap(),
            vec![200, 202]
        );
        assert_eq!(
            db.pr_comment_triage_batch_siblings(9, "batch-b").unwrap(),
            vec![201]
        );
        assert!(
            db.pr_comment_triage_batch_siblings(9, "no-such-batch")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn reply_draft_only_accepts_the_latest_request() {
        let (_tmp, db) = open_temp_db();
        db.begin_pr_comment_reply_draft(7, 11, "old", "base-old", Some("{\"harness\":\"Codex\"}"))
            .unwrap();
        assert!(
            db.capture_pr_comment_reply_draft(7, 11, "old", "First draft")
                .unwrap()
        );
        assert_eq!(
            db.load_pr_comment_reply_draft(7, 11).unwrap().as_deref(),
            Some("First draft")
        );

        db.begin_pr_comment_reply_draft(7, 11, "new", "base-new", Some("{\"harness\":\"Claude\"}"))
            .unwrap();
        assert_eq!(db.load_pr_comment_reply_draft(7, 11).unwrap(), None);
        assert!(
            !db.capture_pr_comment_reply_draft(7, 11, "old", "Stale")
                .unwrap()
        );
        assert!(
            db.capture_pr_comment_reply_draft(7, 11, "new", "Current")
                .unwrap()
        );
        assert_eq!(
            db.load_pr_comment_reply_draft(7, 11).unwrap().as_deref(),
            Some("Current")
        );
        // The provenance recorded when the *current* request began travels with
        // the draft it produced — the superseded request's is gone with it.
        assert_eq!(
            db.load_pr_comment_reply_draft_row(7, 11).unwrap(),
            Some(super::ReplyDraftRow {
                body: "Current".to_string(),
                base_head_sha: "base-new".to_string(),
                provenance: Some("{\"harness\":\"Claude\"}".to_string()),
            })
        );

        db.clear_pr_comment_reply_draft(7, 11).unwrap();
        assert_eq!(db.load_pr_comment_reply_draft(7, 11).unwrap(), None);
    }
}
