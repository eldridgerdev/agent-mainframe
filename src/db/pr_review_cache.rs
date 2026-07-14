//! SQLite cache for normalized PR reviews, keyed by `PR# + head SHA`.
//!
//! Re-opening a PR whose head commit hasn't moved is a cache hit: we deserialize
//! the stored [`PrReview`] instead of re-running four `gh` calls, so the pane
//! opens instantly and spends zero agent tokens. A manual refresh bypasses this
//! and overwrites the row. See `docs/backlog/pr-comment-review-plan.md`.

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

use crate::app::pr_review::PrReview;

/// Load the cached review for `(pr_number, head_sha)`, if any. A corrupt or
/// schema-drifted blob is treated as a miss (returns `None`) rather than an
/// error, so a bad cache row never blocks a fresh fetch.
pub fn load(conn: &Connection, pr_number: u32, head_sha: &str) -> Result<Option<PrReview>> {
    let json: Option<String> = conn
        .query_row(
            "SELECT json FROM pr_review_cache WHERE pr_number = ?1 AND head_sha = ?2",
            params![pr_number as i64, head_sha],
            |row| row.get(0),
        )
        .optional()?;

    Ok(json.and_then(|s| serde_json::from_str::<PrReview>(&s).ok()))
}

/// Upsert the review into the cache under its `PR# + head SHA` key.
pub fn save(conn: &Connection, review: &PrReview) -> Result<()> {
    let json = serde_json::to_string(review)?;
    conn.execute(
        "INSERT OR REPLACE INTO pr_review_cache (pr_number, head_sha, json, fetched_at)
         VALUES (?1, ?2, ?3, datetime('now'))",
        params![review.pr.number as i64, review.pr.head_sha, json],
    )?;
    Ok(())
}

/// Drop cache rows older than a week so stale head-SHA entries don't accumulate.
pub fn evict_stale(conn: &Connection) -> Result<()> {
    conn.execute(
        "DELETE FROM pr_review_cache
         WHERE fetched_at < datetime('now', '-7 days')",
        [],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::app::pr_review::{CommentKind, PrComment, PrReview, TriageState};
    use crate::db::AmfDb;
    use crate::github::PrRef;
    use chrono::Local;
    use tempfile::NamedTempFile;

    fn review(number: u32, head_sha: &str, snippet: &str) -> PrReview {
        PrReview {
            pr: PrRef {
                number,
                head_sha: head_sha.into(),
                url: format!("https://github.com/o/r/pull/{number}"),
                owner: "o".into(),
                repo: "r".into(),
            },
            comments: vec![PrComment {
                id: 1,
                kind: CommentKind::Inline,
                author: "alice".into(),
                is_bot: false,
                path: Some("src/lib.rs".into()),
                line: Some(42),
                side: Some("RIGHT".into()),
                outdated: false,
                file_level: false,
                diff_hunk: Some("@@ -1 +1 @@".into()),
                body: "guard this behind the lock".into(),
                snippet: snippet.into(),
                in_reply_to: None,
                thread_id: Some("T1".into()),
                is_resolved: false,
                triage: TriageState::Untriaged,
                local_note: None,
                ai_generated: false,
                ai_published: false,
                github_id: None,
                github_review_id: None,
            }],
            fetched_at: Local::now(),
        }
    }

    fn open_temp_db() -> (NamedTempFile, AmfDb) {
        let tmp = NamedTempFile::new().unwrap();
        let db = AmfDb::open(tmp.path()).unwrap();
        (tmp, db)
    }

    #[test]
    fn roundtrips_by_pr_and_head_sha() {
        let (_tmp, db) = open_temp_db();
        let r = review(321, "abc123", "guard this");
        db.save_pr_review_cache(&r).unwrap();

        let loaded = db.load_pr_review_cache(321, "abc123").unwrap().unwrap();
        assert_eq!(loaded.pr.number, 321);
        assert_eq!(loaded.comments.len(), 1);
        assert_eq!(loaded.comments[0].snippet, "guard this");
        assert_eq!(loaded.comments[0].thread_id.as_deref(), Some("T1"));
    }

    #[test]
    fn miss_on_different_head_sha() {
        let (_tmp, db) = open_temp_db();
        db.save_pr_review_cache(&review(321, "abc123", "x"))
            .unwrap();
        // Same PR number, new head commit → cache miss (must re-fetch).
        assert!(db.load_pr_review_cache(321, "def456").unwrap().is_none());
    }

    #[test]
    fn resave_overwrites_same_key() {
        let (_tmp, db) = open_temp_db();
        db.save_pr_review_cache(&review(7, "sha", "first")).unwrap();
        db.save_pr_review_cache(&review(7, "sha", "second"))
            .unwrap();

        let loaded = db.load_pr_review_cache(7, "sha").unwrap().unwrap();
        assert_eq!(loaded.comments[0].snippet, "second");
    }
}
