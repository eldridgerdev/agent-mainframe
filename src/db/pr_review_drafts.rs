//! SQLite persistence for manual PR review drafts (`MIGRATION_041`).
//!
//! One row per `(repo_key, pr_number)`. The draft body is opaque JSON here:
//! the review module owns its shape (`ReviewProgress`), so this layer never
//! has to change when a review field is added.

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrReviewDraftStatus {
    /// Being worked on, or closed without posting.
    Draft,
    /// Posted to GitHub. Kept so the list can say so.
    Posted,
}

impl PrReviewDraftStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Posted => "posted",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "posted" => Self::Posted,
            _ => Self::Draft,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrReviewDraft {
    pub repo_key: String,
    pub pr_number: u32,
    pub base_oid: String,
    pub head_oid: String,
    pub merge_base_oid: String,
    pub status: PrReviewDraftStatus,
    /// `ReviewProgress` JSON.
    pub progress: String,
    /// Path → diff fingerprint at `head_oid`.
    pub file_fingerprints: HashMap<String, String>,
    pub updated_at: String,
}

pub fn upsert(conn: &Connection, draft: &PrReviewDraft) -> Result<()> {
    conn.execute(
        "INSERT INTO pr_review_drafts
            (repo_key, pr_number, base_oid, head_oid, merge_base_oid, status,
             progress, file_fingerprints, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT (repo_key, pr_number) DO UPDATE SET
            base_oid = excluded.base_oid,
            head_oid = excluded.head_oid,
            merge_base_oid = excluded.merge_base_oid,
            status = excluded.status,
            progress = excluded.progress,
            file_fingerprints = excluded.file_fingerprints,
            updated_at = excluded.updated_at",
        params![
            draft.repo_key,
            draft.pr_number,
            draft.base_oid,
            draft.head_oid,
            draft.merge_base_oid,
            draft.status.as_str(),
            draft.progress,
            serde_json::to_string(&draft.file_fingerprints)?,
            draft.updated_at,
        ],
    )?;
    Ok(())
}

const COLUMNS: &str = "repo_key, pr_number, base_oid, head_oid, merge_base_oid, status, \
     progress, file_fingerprints, updated_at";

fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PrReviewDraft> {
    let status: String = row.get(5)?;
    let fingerprints: String = row.get(7)?;
    Ok(PrReviewDraft {
        repo_key: row.get(0)?,
        pr_number: row.get(1)?,
        base_oid: row.get(2)?,
        head_oid: row.get(3)?,
        merge_base_oid: row.get(4)?,
        status: PrReviewDraftStatus::parse(&status),
        progress: row.get(6)?,
        // A malformed map only loses the "changed since" flags, which then
        // read as "everything changed" — the safe direction.
        file_fingerprints: serde_json::from_str(&fingerprints).unwrap_or_default(),
        updated_at: row.get(8)?,
    })
}

pub fn load(conn: &Connection, repo_key: &str, pr_number: u32) -> Result<Option<PrReviewDraft>> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT {COLUMNS} FROM pr_review_drafts WHERE repo_key = ?1 AND pr_number = ?2"
            ),
            params![repo_key, pr_number],
            from_row,
        )
        .optional()?)
}

/// Every draft for one repository, for the Review tab's badges.
pub fn load_for_repo(conn: &Connection, repo_key: &str) -> Result<Vec<PrReviewDraft>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM pr_review_drafts WHERE repo_key = ?1 ORDER BY pr_number"
    ))?;
    let rows = stmt
        .query_map(params![repo_key], from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn delete(conn: &Connection, repo_key: &str, pr_number: u32) -> Result<()> {
    conn.execute(
        "DELETE FROM pr_review_drafts WHERE repo_key = ?1 AND pr_number = ?2",
        params![repo_key, pr_number],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        conn
    }

    fn draft(pr_number: u32, head: &str) -> PrReviewDraft {
        PrReviewDraft {
            repo_key: "github.com/acme/widgets".to_string(),
            pr_number,
            base_oid: "base".to_string(),
            head_oid: head.to_string(),
            merge_base_oid: "mb".to_string(),
            status: PrReviewDraftStatus::Draft,
            progress: r#"{"general_feedback":"looks good"}"#.to_string(),
            file_fingerprints: HashMap::from([("src/a.rs".to_string(), "f00d".to_string())]),
            updated_at: "2026-09-25T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn a_draft_round_trips() {
        let conn = conn();
        let saved = draft(7, "head1");
        upsert(&conn, &saved).unwrap();
        assert_eq!(load(&conn, &saved.repo_key, 7).unwrap(), Some(saved));
    }

    #[test]
    fn saving_again_replaces_the_draft_for_that_pr() {
        let conn = conn();
        upsert(&conn, &draft(7, "head1")).unwrap();
        let mut moved = draft(7, "head2");
        moved.status = PrReviewDraftStatus::Posted;
        upsert(&conn, &moved).unwrap();

        let all = load_for_repo(&conn, "github.com/acme/widgets").unwrap();
        assert_eq!(all, vec![moved]);
    }

    #[test]
    fn drafts_are_keyed_by_repository_and_number() {
        let conn = conn();
        upsert(&conn, &draft(7, "a")).unwrap();
        upsert(&conn, &draft(8, "b")).unwrap();
        let mut elsewhere = draft(7, "c");
        elsewhere.repo_key = "github.example.com/acme/widgets".to_string();
        upsert(&conn, &elsewhere).unwrap();

        assert_eq!(
            load(&conn, "github.com/acme/widgets", 7)
                .unwrap()
                .unwrap()
                .head_oid,
            "a"
        );
        assert_eq!(
            load_for_repo(&conn, "github.com/acme/widgets")
                .unwrap()
                .len(),
            2
        );
        delete(&conn, "github.com/acme/widgets", 7).unwrap();
        assert_eq!(load(&conn, "github.com/acme/widgets", 7).unwrap(), None);
        assert!(load(&conn, &elsewhere.repo_key, 7).unwrap().is_some());
    }
}
