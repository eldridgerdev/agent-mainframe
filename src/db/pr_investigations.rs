//! SQLite persistence for PR Triage's **Investigate** action
//! (`AMF_PLAN.md` — "PR Triage: Investigate").
//!
//! A [`PrInvestigation`] is one strictly read-only headless investigation of a
//! single review comment. There is exactly one per `(project_id, PR#, comment
//! id)` — Investigate is single-item and never batched, so unlike
//! `pr_comment_triage` there is no shared `batch_id`.
//!
//! Like todo lists and Learning Mode history, this data lives *outside* the
//! `ProjectStore` JSON blob and its full-replace save path, so running an
//! investigation doesn't rewrite the store and a store save doesn't rewrite the
//! findings. `project_id` is plain TEXT with no FK to `projects`, so cleanup on
//! project deletion is explicit — see [`delete_for_project`].
//!
//! The triage overlay keeps its own in-memory copy of these rows as the source
//! of truth (again like todos and Learning Mode), so the feature works with no
//! DB at all; these functions persist that copy when a database is present and
//! reload it when the overlay reopens.
//!
//! `head_sha` records the PR head the investigation ran against — a staleness
//! signal, exactly as `pr_comment_triage` uses it, not part of the identity, so
//! a push that moves the PR head doesn't orphan the finding.
//!
//! The persistence layer lands ahead of its UI consumers (later `AMF_PLAN.md`
//! tasks), so parts of this API are allowed to be unused for now — like
//! `src/db/todos.rs` and `src/db/learning.rs`.
#![allow(dead_code)]

use anyhow::Result;
use rusqlite::{Connection, params};

use super::learning::now_timestamp;
use super::store::{agent_from_str, agent_to_str};
pub use crate::app::pr_review::{PrInvestigationStatus, PrInvestigationTurn};
use crate::project::AgentKind;

/// One persisted investigation row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrInvestigation {
    pub project_id: String,
    pub pr_number: u32,
    pub comment_id: u64,
    /// PR head the run was made against (staleness only — see the module doc).
    pub head_sha: String,
    /// Harness the operator picked for the initial run.
    pub harness: AgentKind,
    /// The exact minimal context handed to the agent (comment body + PR
    /// title/description + PR diff / changed files). Kept so a reopened finding
    /// can show what it was based on.
    pub context_snapshot: String,
    /// Answer markdown; `None` while the run is in flight or if it failed.
    pub answer: Option<String>,
    /// Ordered follow-up turns (Learning Mode `F` behaviour).
    pub follow_ups: Vec<PrInvestigationTurn>,
    pub status: PrInvestigationStatus,
    /// Failure message when `status == Failed`.
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl PrInvestigation {
    /// A fresh `Running` row for `comment_id`, stamped now. Callers write this
    /// *before* the blocking headless call so a crash mid-run is visible on
    /// reopen.
    pub fn new_running(
        project_id: impl Into<String>,
        pr_number: u32,
        comment_id: u64,
        head_sha: impl Into<String>,
        harness: AgentKind,
        context_snapshot: impl Into<String>,
    ) -> Self {
        let now = now_timestamp();
        Self {
            project_id: project_id.into(),
            pr_number,
            comment_id,
            head_sha: head_sha.into(),
            harness,
            context_snapshot: context_snapshot.into(),
            answer: None,
            follow_ups: Vec::new(),
            status: PrInvestigationStatus::Running,
            error: None,
            created_at: now.clone(),
            updated_at: now,
        }
    }
}

/// Every investigation for one PR in one project, oldest first.
pub fn load_by_pr(
    conn: &Connection,
    project_id: &str,
    pr_number: u32,
) -> Result<Vec<PrInvestigation>> {
    let mut stmt = conn.prepare(
        "SELECT project_id, pr_number, comment_id, head_sha, harness,
                context_snapshot, answer, follow_ups, status, error,
                created_at, updated_at
         FROM pr_investigations
         WHERE project_id = ?1 AND pr_number = ?2
         ORDER BY created_at ASC, comment_id ASC",
    )?;
    let rows = stmt.query_map(params![project_id, pr_number as i64], row_to_investigation)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Insert or update one investigation, keyed by `(project_id, pr_number,
/// comment_id)`.
///
/// `ON CONFLICT DO UPDATE` rather than `INSERT OR REPLACE`: REPLACE deletes the
/// existing row first, needlessly churning it, and would drop `created_at`.
pub fn upsert(conn: &Connection, inv: &PrInvestigation) -> Result<()> {
    let follow_ups = serde_json::to_string(&inv.follow_ups)?;
    let created_at = if inv.created_at.is_empty() {
        now_timestamp()
    } else {
        inv.created_at.clone()
    };
    conn.execute(
        "INSERT INTO pr_investigations
            (project_id, pr_number, comment_id, head_sha, harness,
             context_snapshot, answer, follow_ups, status, error,
             created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT(project_id, pr_number, comment_id) DO UPDATE SET
            head_sha = excluded.head_sha,
            harness = excluded.harness,
            context_snapshot = excluded.context_snapshot,
            answer = excluded.answer,
            follow_ups = excluded.follow_ups,
            status = excluded.status,
            error = excluded.error,
            updated_at = excluded.updated_at",
        params![
            inv.project_id,
            inv.pr_number as i64,
            inv.comment_id as i64,
            inv.head_sha,
            agent_to_str(&inv.harness),
            inv.context_snapshot,
            inv.answer,
            follow_ups,
            inv.status.as_db_str(),
            inv.error,
            created_at,
            now_timestamp(),
        ],
    )?;
    Ok(())
}

/// Record the outcome of a finished run against one row, addressed by key.
///
/// Deliberately narrower than [`upsert`]: a blocking run can still outlive the
/// overlay if the operator closes AMF, and there is then no in-memory row to
/// write back — only the key the run was launched with. Returns whether a row
/// was actually updated, so a completion for an investigation that has since
/// been deleted can be reported rather than silently dropped.
///
/// `answer` is coalesced, not overwritten: a failed follow-up must not erase an
/// answer an earlier run already produced.
pub fn finish(
    conn: &Connection,
    project_id: &str,
    pr_number: u32,
    comment_id: u64,
    answer: Option<&str>,
    status: PrInvestigationStatus,
    error: Option<&str>,
) -> Result<bool> {
    let updated = conn.execute(
        "UPDATE pr_investigations
            SET answer = COALESCE(?4, answer),
                status = ?5,
                error = ?6,
                updated_at = ?7
          WHERE project_id = ?1 AND pr_number = ?2 AND comment_id = ?3",
        params![
            project_id,
            pr_number as i64,
            comment_id as i64,
            answer,
            status.as_db_str(),
            error,
            now_timestamp(),
        ],
    )?;
    Ok(updated > 0)
}

/// Set just the status (the `dismiss` action). Returns whether a row matched.
pub fn set_status(
    conn: &Connection,
    project_id: &str,
    pr_number: u32,
    comment_id: u64,
    status: PrInvestigationStatus,
) -> Result<bool> {
    let updated = conn.execute(
        "UPDATE pr_investigations
            SET status = ?4, updated_at = ?5
          WHERE project_id = ?1 AND pr_number = ?2 AND comment_id = ?3",
        params![
            project_id,
            pr_number as i64,
            comment_id as i64,
            status.as_db_str(),
            now_timestamp(),
        ],
    )?;
    Ok(updated > 0)
}

/// Delete one investigation.
pub fn delete(conn: &Connection, project_id: &str, pr_number: u32, comment_id: u64) -> Result<()> {
    conn.execute(
        "DELETE FROM pr_investigations
         WHERE project_id = ?1 AND pr_number = ?2 AND comment_id = ?3",
        params![project_id, pr_number as i64, comment_id as i64],
    )?;
    Ok(())
}

/// Drop every investigation belonging to a project, for use when the project is
/// deleted (there is no FK cascade — see the module doc).
pub fn delete_for_project(conn: &Connection, project_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM pr_investigations WHERE project_id = ?1",
        params![project_id],
    )?;
    Ok(())
}

fn row_to_investigation(row: &rusqlite::Row<'_>) -> rusqlite::Result<PrInvestigation> {
    let pr_number: i64 = row.get(1)?;
    let comment_id: i64 = row.get(2)?;
    let harness: String = row.get(4)?;
    let follow_ups_json: String = row.get(7)?;
    let status: String = row.get(8)?;
    // A corrupt `follow_ups` blob degrades to "no follow-ups" rather than
    // failing the whole load — the initial answer is the valuable part.
    let follow_ups = serde_json::from_str(&follow_ups_json).unwrap_or_default();
    Ok(PrInvestigation {
        project_id: row.get(0)?,
        pr_number: pr_number as u32,
        comment_id: comment_id as u64,
        head_sha: row.get(3)?,
        harness: agent_from_str(&harness),
        context_snapshot: row.get(5)?,
        answer: row.get(6)?,
        follow_ups,
        status: PrInvestigationStatus::from_db_str(&status),
        error: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::AmfDb;
    use tempfile::NamedTempFile;

    fn open_temp_db() -> (NamedTempFile, AmfDb) {
        let tmp = NamedTempFile::new().unwrap();
        let db = AmfDb::open(tmp.path()).unwrap();
        (tmp, db)
    }

    fn sample(project_id: &str, pr_number: u32, comment_id: u64) -> PrInvestigation {
        PrInvestigation::new_running(
            project_id,
            pr_number,
            comment_id,
            "headsha",
            AgentKind::Codex,
            "comment body + PR title/description + diff",
        )
    }

    #[test]
    fn round_trips_a_running_row_and_lists_by_pr() {
        let (_tmp, db) = open_temp_db();
        db.upsert_pr_investigation(&sample("proj-1", 42, 100))
            .unwrap();
        db.upsert_pr_investigation(&sample("proj-1", 42, 101))
            .unwrap();
        // A different PR, and a different project — neither must leak in.
        db.upsert_pr_investigation(&sample("proj-1", 43, 200))
            .unwrap();
        db.upsert_pr_investigation(&sample("proj-2", 42, 300))
            .unwrap();

        let rows = db.load_pr_investigations("proj-1", 42).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].comment_id, 100);
        assert_eq!(rows[0].harness, AgentKind::Codex);
        assert_eq!(rows[0].status, PrInvestigationStatus::Running);
        assert_eq!(rows[0].answer, None);
        assert!(rows[0].follow_ups.is_empty());
    }

    #[test]
    fn upsert_overwrites_same_key_and_keeps_created_at() {
        let (_tmp, db) = open_temp_db();
        let mut row = sample("p", 1, 1);
        row.created_at = "2020-01-01 00:00:00.000".to_string();
        db.upsert_pr_investigation(&row).unwrap();

        row.status = PrInvestigationStatus::Complete;
        row.answer = Some("The anchor moved in commit abc123.".to_string());
        row.follow_ups.push(PrInvestigationTurn {
            question: "does it still repro on main?".to_string(),
            answer: "no".to_string(),
            harness: AgentKind::Claude,
            created_at: "2020-01-02 00:00:00.000".to_string(),
        });
        db.upsert_pr_investigation(&row).unwrap();

        let rows = db.load_pr_investigations("p", 1).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, PrInvestigationStatus::Complete);
        assert_eq!(
            rows[0].answer.as_deref(),
            Some("The anchor moved in commit abc123.")
        );
        assert_eq!(rows[0].follow_ups.len(), 1);
        assert_eq!(rows[0].follow_ups[0].harness, AgentKind::Claude);
        assert_eq!(rows[0].created_at, "2020-01-01 00:00:00.000");
    }

    #[test]
    fn finish_coalesces_answer_and_reports_missing_rows() {
        let (_tmp, db) = open_temp_db();
        db.upsert_pr_investigation(&sample("p", 1, 1)).unwrap();

        assert!(
            db.finish_pr_investigation(
                "p",
                1,
                1,
                Some("found it"),
                PrInvestigationStatus::Complete,
                None
            )
            .unwrap()
        );
        // A later failed follow-up run must not erase the answer.
        assert!(
            db.finish_pr_investigation(
                "p",
                1,
                1,
                None,
                PrInvestigationStatus::Failed,
                Some("timeout")
            )
            .unwrap()
        );
        let rows = db.load_pr_investigations("p", 1).unwrap();
        assert_eq!(rows[0].answer.as_deref(), Some("found it"));
        assert_eq!(rows[0].status, PrInvestigationStatus::Failed);
        assert_eq!(rows[0].error.as_deref(), Some("timeout"));

        // Unknown key: nothing updated.
        assert!(
            !db.finish_pr_investigation(
                "p",
                1,
                999,
                Some("x"),
                PrInvestigationStatus::Complete,
                None
            )
            .unwrap()
        );
    }

    #[test]
    fn set_status_dismisses_without_touching_the_answer() {
        let (_tmp, db) = open_temp_db();
        let mut row = sample("p", 1, 1);
        row.status = PrInvestigationStatus::Complete;
        row.answer = Some("keep me".to_string());
        db.upsert_pr_investigation(&row).unwrap();

        assert!(
            db.set_pr_investigation_status("p", 1, 1, PrInvestigationStatus::Dismissed)
                .unwrap()
        );
        let rows = db.load_pr_investigations("p", 1).unwrap();
        assert_eq!(rows[0].status, PrInvestigationStatus::Dismissed);
        assert_eq!(rows[0].answer.as_deref(), Some("keep me"));
    }

    #[test]
    fn delete_for_project_only_touches_that_project() {
        let (_tmp, db) = open_temp_db();
        db.upsert_pr_investigation(&sample("p1", 1, 1)).unwrap();
        db.upsert_pr_investigation(&sample("p1", 2, 2)).unwrap();
        db.upsert_pr_investigation(&sample("p2", 1, 1)).unwrap();

        db.delete_pr_investigations_for_project("p1").unwrap();
        assert!(db.load_pr_investigations("p1", 1).unwrap().is_empty());
        assert!(db.load_pr_investigations("p1", 2).unwrap().is_empty());
        assert_eq!(db.load_pr_investigations("p2", 1).unwrap().len(), 1);
    }

    #[test]
    fn unknown_status_token_degrades_to_failed() {
        assert_eq!(
            PrInvestigationStatus::from_db_str("garbage"),
            PrInvestigationStatus::Failed
        );
        for status in [
            PrInvestigationStatus::Running,
            PrInvestigationStatus::Complete,
            PrInvestigationStatus::Failed,
            PrInvestigationStatus::Dismissed,
        ] {
            assert_eq!(
                PrInvestigationStatus::from_db_str(status.as_db_str()),
                status
            );
        }
    }
}
