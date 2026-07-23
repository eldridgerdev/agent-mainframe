//! SQLite cache for AI PR-review findings, keyed by `PR# + head SHA`.
//!
//! Split out of `pr_review_cache` so AI-review storage is fully decoupled from
//! comment-triage storage (see `docs/backlog/pr-comment-review-plan.md`'s "Does
//! AI review belong in this pane" open question, resolved by giving AI review
//! its own workflow/pane). Re-opening the AI Review pane for a PR whose head
//! commit hasn't moved is a cache hit — no regeneration, zero agent tokens. A
//! new head SHA (the PR moved) naturally starts empty; regenerate with `A`.

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

use crate::app::ai_review::AiReviewCacheEntry;

/// Load the cached AI-review findings for `(pr_number, head_sha)`, if any. A
/// corrupt or schema-drifted blob is treated as a miss rather than an error,
/// so a bad cache row never blocks re-generating.
pub fn load(
    conn: &Connection,
    pr_number: u32,
    head_sha: &str,
) -> Result<Option<AiReviewCacheEntry>> {
    let json: Option<String> = conn
        .query_row(
            "SELECT json FROM ai_review_cache WHERE pr_number = ?1 AND head_sha = ?2",
            params![pr_number as i64, head_sha],
            |row| row.get(0),
        )
        .optional()?;

    Ok(json.and_then(|s| serde_json::from_str::<AiReviewCacheEntry>(&s).ok()))
}

/// Upsert the findings into the cache under their `PR# + head SHA` key.
pub fn save(
    conn: &Connection,
    pr_number: u32,
    head_sha: &str,
    entry: &AiReviewCacheEntry,
) -> Result<()> {
    let json = serde_json::to_string(entry)?;
    conn.execute(
        "INSERT OR REPLACE INTO ai_review_cache (pr_number, head_sha, json, updated_at)
         VALUES (?1, ?2, ?3, datetime('now'))",
        params![pr_number as i64, head_sha, json],
    )?;
    Ok(())
}

/// Drop cache rows older than a week so stale head-SHA entries don't accumulate.
pub fn evict_stale(conn: &Connection) -> Result<()> {
    conn.execute(
        "DELETE FROM ai_review_cache
         WHERE updated_at < datetime('now', '-7 days')",
        [],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ai_review::{AiReviewFinding, AiReviewRun, AiReviewRunOutcome};
    use crate::db::AmfDb;
    use chrono::Local;
    use tempfile::NamedTempFile;

    fn entry(body: &str) -> AiReviewCacheEntry {
        AiReviewCacheEntry {
            findings: vec![AiReviewFinding {
                path: Some("src/lib.rs".into()),
                line: Some(42),
                body: body.into(),
                diff_hunk: Some("@@ -1 +1 @@".into()),
                skipped: false,
                published: false,
            }],
            last_run: Some(AiReviewRun {
                ran_at: Local::now(),
                outcome: AiReviewRunOutcome::Findings(1),
            }),
            summary: Some("One concurrency risk needs attention.".into()),
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
        db.save_ai_review_cache(321, "abc123", &entry("guard this"))
            .unwrap();

        let loaded = db.load_ai_review_cache(321, "abc123").unwrap().unwrap();
        assert_eq!(loaded.findings.len(), 1);
        assert_eq!(loaded.findings[0].body, "guard this");
        assert_eq!(
            loaded.summary.as_deref(),
            Some("One concurrency risk needs attention.")
        );
        assert!(matches!(
            loaded.last_run.unwrap().outcome,
            AiReviewRunOutcome::Findings(1)
        ));
    }

    #[test]
    fn miss_on_different_head_sha() {
        let (_tmp, db) = open_temp_db();
        db.save_ai_review_cache(321, "abc123", &entry("x")).unwrap();
        // Same PR number, new head commit → cache miss (must re-generate).
        assert!(db.load_ai_review_cache(321, "def456").unwrap().is_none());
    }

    #[test]
    fn resave_overwrites_same_key() {
        let (_tmp, db) = open_temp_db();
        db.save_ai_review_cache(7, "sha", &entry("first")).unwrap();
        db.save_ai_review_cache(7, "sha", &entry("second")).unwrap();

        let loaded = db.load_ai_review_cache(7, "sha").unwrap().unwrap();
        assert_eq!(loaded.findings[0].body, "second");
    }

    #[test]
    fn cache_json_without_summary_remains_readable() {
        let legacy = serde_json::json!({
            "findings": [],
            "last_run": null
        });
        let entry: AiReviewCacheEntry = serde_json::from_value(legacy).unwrap();
        assert!(entry.summary.is_none());
    }
}
