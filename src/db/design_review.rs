//! SQLite persistence for Design Review (`MIGRATION_049`, `MIGRATION_050`,
//! `docs/development/features/design-review.md`).
//!
//! Like Learning Mode, these tables sit outside the store's full-replace save
//! and have no FK to `projects`/`features`: accepted decisions are project
//! memory and outlive the feature's worktree. Only project deletion removes
//! them ([`delete_for_project`]); deleting a feature does not.
//!
//! The agent never writes here. Decisions arrive through the outbox and are
//! inserted by uid, so ingesting the same line twice is a no-op.
//!
//! The persistence layer lands ahead of the review room (plan Phase 3), so
//! parts of this API are allowed to be unused for now.
#![allow(dead_code)]

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

use crate::design_review::outbox::OutboxCursor;
use crate::design_review::record::DecisionRecord;

/// Every Design Review table, for project-wide cleanup.
const TABLES: [&str; 7] = [
    "design_decisions",
    "design_review_rounds",
    "design_review_items",
    "design_review_turns",
    "design_review_artifacts",
    "design_review_usage",
    "design_review_outbox_offsets",
];

/// One `design_decisions` row.
#[derive(Debug, Clone, PartialEq)]
pub struct DesignDecision {
    pub uid: String,
    pub feature_id: String,
    pub project_id: String,
    pub seq: i64,
    pub kind: String,
    pub title: String,
    pub why: String,
    pub rejected: Vec<String>,
    pub ask: Option<String>,
    pub supersedes: Option<String>,
    pub paths: Vec<String>,
    pub head: Option<String>,
    pub session_id: Option<String>,
    pub harness: String,
    /// `agent` (recorded while working) or `review` (an answer given in review).
    pub source: String,
    /// `open`, `accepted` or `superseded`.
    pub status: String,
    pub created_at: String,
}

impl DesignDecision {
    /// A freshly ingested agent record: `source = agent`, `status = open`.
    pub fn from_record(project_id: &str, feature_id: &str, record: &DecisionRecord) -> Self {
        Self {
            uid: record.uid.clone(),
            feature_id: feature_id.to_string(),
            project_id: project_id.to_string(),
            seq: i64::from(record.seq),
            kind: record.kind.as_str().to_string(),
            title: record.title.clone(),
            why: record.why.clone(),
            rejected: record.rejected.clone(),
            ask: record.ask.clone(),
            supersedes: record.supersedes.clone(),
            paths: record.paths.clone(),
            head: record.head.clone(),
            session_id: record.session_id.clone(),
            harness: record.harness.clone(),
            source: "agent".to_string(),
            status: "open".to_string(),
            created_at: record.ts.clone(),
        }
    }
}

fn json_list(value: &[String]) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "[]".to_string())
}

fn parse_list(value: String) -> Vec<String> {
    serde_json::from_str(&value).unwrap_or_default()
}

/// Insert unless a row with the same uid exists. Returns whether it inserted.
pub fn insert_decision_if_absent(conn: &Connection, decision: &DesignDecision) -> Result<bool> {
    let changed = conn.execute(
        "INSERT OR IGNORE INTO design_decisions (
            uid, feature_id, project_id, seq, kind, title, why, rejected, ask,
            supersedes, paths, head, session_id, harness, source, status, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
        params![
            decision.uid,
            decision.feature_id,
            decision.project_id,
            decision.seq,
            decision.kind,
            decision.title,
            decision.why,
            json_list(&decision.rejected),
            decision.ask,
            decision.supersedes,
            json_list(&decision.paths),
            decision.head,
            decision.session_id,
            decision.harness,
            decision.source,
            decision.status,
            decision.created_at,
        ],
    )?;
    Ok(changed == 1)
}

/// A feature's decisions in record order.
pub fn list_decisions_for_feature(
    conn: &Connection,
    feature_id: &str,
) -> Result<Vec<DesignDecision>> {
    let mut stmt = conn.prepare(
        "SELECT uid, feature_id, project_id, seq, kind, title, why, rejected, ask,
                supersedes, paths, head, session_id, harness, source, status, created_at
         FROM design_decisions WHERE feature_id = ?1
         ORDER BY seq ASC, rowid ASC",
    )?;
    let rows = stmt
        .query_map(params![feature_id], |row| {
            Ok(DesignDecision {
                uid: row.get(0)?,
                feature_id: row.get(1)?,
                project_id: row.get(2)?,
                seq: row.get(3)?,
                kind: row.get(4)?,
                title: row.get(5)?,
                why: row.get(6)?,
                rejected: parse_list(row.get(7)?),
                ask: row.get(8)?,
                supersedes: row.get(9)?,
                paths: parse_list(row.get(10)?),
                head: row.get(11)?,
                session_id: row.get(12)?,
                harness: row.get(13)?,
                source: row.get(14)?,
                status: row.get(15)?,
                created_at: row.get(16)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Where ingest of the feature's outbox stopped; the default (offset 0) when
/// never ingested.
pub fn outbox_cursor(conn: &Connection, feature_id: &str) -> Result<OutboxCursor> {
    let row: Option<(i64, Option<String>)> = conn
        .query_row(
            "SELECT byte_offset, tail_sha256 FROM design_review_outbox_offsets
             WHERE feature_id = ?1",
            params![feature_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    Ok(
        row.map_or_else(OutboxCursor::default, |(offset, tail_sha256)| {
            OutboxCursor {
                offset: offset.max(0) as u64,
                tail_sha256,
            }
        }),
    )
}

pub fn set_outbox_cursor(
    conn: &Connection,
    project_id: &str,
    feature_id: &str,
    cursor: &OutboxCursor,
) -> Result<()> {
    conn.execute(
        "INSERT INTO design_review_outbox_offsets
            (feature_id, project_id, byte_offset, tail_sha256, updated_at)
         VALUES (?1, ?2, ?3, ?4, datetime('now'))
         ON CONFLICT(feature_id) DO UPDATE SET
            project_id = excluded.project_id,
            byte_offset = excluded.byte_offset,
            tail_sha256 = excluded.tail_sha256,
            updated_at = excluded.updated_at",
        params![
            feature_id,
            project_id,
            cursor.offset as i64,
            cursor.tail_sha256
        ],
    )?;
    Ok(())
}

/// One `design_review_rounds` row.
#[derive(Debug, Clone, PartialEq)]
pub struct DesignReviewRound {
    pub id: String,
    pub feature_id: String,
    pub project_id: String,
    pub round: i64,
    pub base_commit: Option<String>,
    pub head_commit: Option<String>,
    pub diff_fingerprint: Option<String>,
    pub verdict: Option<String>,
    pub created_at: String,
}

/// The feature's newest round, if it has one.
pub fn latest_round(conn: &Connection, feature_id: &str) -> Result<Option<DesignReviewRound>> {
    Ok(conn
        .query_row(
            "SELECT id, feature_id, project_id, round, base_commit, head_commit,
                    diff_fingerprint, verdict, created_at
             FROM design_review_rounds WHERE feature_id = ?1
             ORDER BY round DESC LIMIT 1",
            params![feature_id],
            |row| {
                Ok(DesignReviewRound {
                    id: row.get(0)?,
                    feature_id: row.get(1)?,
                    project_id: row.get(2)?,
                    round: row.get(3)?,
                    base_commit: row.get(4)?,
                    head_commit: row.get(5)?,
                    diff_fingerprint: row.get(6)?,
                    verdict: row.get(7)?,
                    created_at: row.get(8)?,
                })
            },
        )
        .optional()?)
}

pub fn insert_round(conn: &Connection, round: &DesignReviewRound) -> Result<()> {
    conn.execute(
        "INSERT INTO design_review_rounds
            (id, feature_id, project_id, round, base_commit, head_commit,
             diff_fingerprint, verdict, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            round.id,
            round.feature_id,
            round.project_id,
            round.round,
            round.base_commit,
            round.head_commit,
            round.diff_fingerprint,
            round.verdict,
            round.created_at,
        ],
    )?;
    Ok(())
}

/// One `design_review_items` row (the reply columns are Phase 4's).
#[derive(Debug, Clone, PartialEq)]
pub struct DesignReviewItem {
    pub id: String,
    pub round_id: String,
    pub feature_id: String,
    pub project_id: String,
    pub position: i64,
    pub kind: String,
    pub ref_key: Option<String>,
    pub label: Option<String>,
    pub title: String,
    pub paths: Vec<String>,
    pub state: String,
    pub stale: crate::design_review::agenda::Staleness,
    pub created_at: String,
    pub updated_at: String,
}

/// A round's items in agenda order.
pub fn list_items(conn: &Connection, round_id: &str) -> Result<Vec<DesignReviewItem>> {
    let mut stmt = conn.prepare(
        "SELECT id, round_id, feature_id, project_id, position, kind, ref_key, label,
                title, paths, state, stale, created_at, updated_at
         FROM design_review_items WHERE round_id = ?1
         ORDER BY position ASC, rowid ASC",
    )?;
    let rows = stmt
        .query_map(params![round_id], |row| {
            Ok(DesignReviewItem {
                id: row.get(0)?,
                round_id: row.get(1)?,
                feature_id: row.get(2)?,
                project_id: row.get(3)?,
                position: row.get(4)?,
                kind: row.get(5)?,
                ref_key: row.get(6)?,
                label: row.get(7)?,
                title: row.get(8)?,
                paths: parse_list(row.get(9)?),
                state: row.get(10)?,
                stale: crate::design_review::agenda::Staleness::from_i64(row.get(11)?),
                created_at: row.get(12)?,
                updated_at: row.get(13)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn insert_item(conn: &Connection, item: &DesignReviewItem) -> Result<()> {
    conn.execute(
        "INSERT INTO design_review_items
            (id, round_id, feature_id, project_id, position, kind, ref_key, label,
             title, paths, state, stale, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            item.id,
            item.round_id,
            item.feature_id,
            item.project_id,
            item.position,
            item.kind,
            item.ref_key,
            item.label,
            item.title,
            json_list(&item.paths),
            item.state,
            item.stale.as_i64(),
            item.created_at,
            item.updated_at,
        ],
    )?;
    Ok(())
}

/// Re-derivation: move an existing item and refresh what the inputs own,
/// leaving its state alone.
pub fn update_item_layout(conn: &Connection, item: &DesignReviewItem) -> Result<()> {
    conn.execute(
        "UPDATE design_review_items
         SET position = ?2, label = ?3, title = ?4, paths = ?5, stale = ?6
         WHERE id = ?1",
        params![
            item.id,
            item.position,
            item.label,
            item.title,
            json_list(&item.paths),
            item.stale.as_i64(),
        ],
    )?;
    Ok(())
}

pub fn set_item_state(conn: &Connection, item_id: &str, state: &str, now: &str) -> Result<()> {
    conn.execute(
        "UPDATE design_review_items SET state = ?2, updated_at = ?3 WHERE id = ?1",
        params![item_id, state, now],
    )?;
    Ok(())
}

/// One `design_review_turns` row.
#[derive(Debug, Clone, PartialEq)]
pub struct DesignReviewTurn {
    pub id: String,
    pub item_id: String,
    pub round_id: Option<String>,
    pub feature_id: String,
    pub project_id: String,
    pub seq: i64,
    /// `reviewer`, `record` (a lookup of the stored decision), and later
    /// `presenter` and `developer`.
    pub role: String,
    /// The ladder level that produced it (`L0`...), for non-reviewer turns.
    pub level: Option<String>,
    pub body: String,
    pub status: String,
    pub usage_id: Option<i64>,
    pub created_at: String,
}

/// An item's thread in order.
pub fn list_turns(conn: &Connection, item_id: &str) -> Result<Vec<DesignReviewTurn>> {
    let mut stmt = conn.prepare(
        "SELECT id, item_id, round_id, feature_id, project_id, seq, role, level,
                body, status, usage_id, created_at
         FROM design_review_turns WHERE item_id = ?1 ORDER BY seq ASC, rowid ASC",
    )?;
    let rows = stmt
        .query_map(params![item_id], |row| {
            Ok(DesignReviewTurn {
                id: row.get(0)?,
                item_id: row.get(1)?,
                round_id: row.get(2)?,
                feature_id: row.get(3)?,
                project_id: row.get(4)?,
                seq: row.get(5)?,
                role: row.get(6)?,
                level: row.get(7)?,
                body: row.get(8)?,
                status: row.get(9)?,
                usage_id: row.get(10)?,
                created_at: row.get(11)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Append a turn at the end of its item's thread; `turn.seq` is ignored.
pub fn append_turn(conn: &Connection, turn: &DesignReviewTurn) -> Result<()> {
    conn.execute(
        "INSERT INTO design_review_turns
            (id, item_id, round_id, feature_id, project_id, seq, role, level, body,
             status, usage_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5,
                 (SELECT COALESCE(MAX(seq), 0) + 1 FROM design_review_turns WHERE item_id = ?2),
                 ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            turn.id,
            turn.item_id,
            turn.round_id,
            turn.feature_id,
            turn.project_id,
            turn.role,
            turn.level,
            turn.body,
            turn.status,
            turn.usage_id,
            turn.created_at,
        ],
    )?;
    Ok(())
}

/// A cached packet section.
#[derive(Debug, Clone, PartialEq)]
pub struct DesignReviewArtifact {
    pub id: i64,
    pub section: String,
    pub format: String,
    pub body: String,
    pub created_at: String,
}

/// The newest artifact of a section, by id. A section is rewritten under a
/// new input hash when its inputs change, so the newest row is current.
pub fn latest_artifact(
    conn: &Connection,
    feature_id: &str,
    section: &str,
) -> Result<Option<DesignReviewArtifact>> {
    Ok(conn
        .query_row(
            "SELECT id, section, format, body, created_at FROM design_review_artifacts
             WHERE feature_id = ?1 AND section = ?2 ORDER BY id DESC LIMIT 1",
            params![feature_id, section],
            |row| {
                Ok(DesignReviewArtifact {
                    id: row.get(0)?,
                    section: row.get(1)?,
                    format: row.get(2)?,
                    body: row.get(3)?,
                    created_at: row.get(4)?,
                })
            },
        )
        .optional()?)
}

/// The feature's ledger rows, oldest first, in the Usage tab's shape.
pub fn list_usage(
    conn: &Connection,
    feature_id: &str,
) -> Result<Vec<crate::design_review::usage::UsageRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, phase, task, agent, model, round, item, input, output, cache_read,
                cache_write_5m, cache_write_1h, effective_tokens, cost_usd, provenance,
                price_assumed, cache_hit, saved_usd, duration_ms, started_at
         FROM design_review_usage WHERE feature_id = ?1
         ORDER BY started_at ASC, id ASC",
    )?;
    let count = |row: &rusqlite::Row, i: usize| -> rusqlite::Result<Option<u64>> {
        Ok(row.get::<_, Option<i64>>(i)?.map(|v| v.max(0) as u64))
    };
    let rows = stmt
        .query_map(params![feature_id], |row| {
            let write_5m = count(row, 10)?;
            let write_1h = count(row, 11)?;
            Ok(crate::design_review::usage::UsageRow {
                id: row.get(0)?,
                phase: row.get(1)?,
                task: row.get(2)?,
                agent: row.get(3)?,
                model: row.get(4)?,
                round: row.get(5)?,
                item: row.get(6)?,
                input: count(row, 7)?,
                output: count(row, 8)?,
                cache_read: count(row, 9)?,
                cache_write: match (write_5m, write_1h) {
                    (None, None) => None,
                    (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
                },
                effective_tokens: count(row, 12)?,
                cost_usd: row.get(13)?,
                estimated: row.get::<_, String>(14)? == "estimated",
                price_assumed: row.get::<_, i64>(15)? != 0,
                cache_hit: row.get::<_, i64>(16)? != 0,
                saved_usd: row.get(17)?,
                duration_ms: count(row, 18)?,
                started_at: row.get(19)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Remove every Design Review row of a deleted project.
pub fn delete_for_project(conn: &Connection, project_id: &str) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    for table in TABLES {
        tx.execute(
            &format!("DELETE FROM {table} WHERE project_id = ?1"),
            params![project_id],
        )?;
    }
    tx.commit()?;
    Ok(())
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

    #[test]
    fn a_failed_write_in_a_transaction_leaves_nothing_behind() {
        let (_tmp, db) = open_temp_db();
        db.conn
            .execute_batch(
                "CREATE TRIGGER reject_state BEFORE UPDATE OF state ON design_review_items
                 BEGIN SELECT RAISE(ABORT, 'write refused'); END;",
            )
            .unwrap();
        let turn = DesignReviewTurn {
            id: "t1".into(),
            item_id: "i1".into(),
            round_id: None,
            feature_id: "f".into(),
            project_id: "p".into(),
            seq: 0,
            role: "reviewer".into(),
            level: None,
            body: "note".into(),
            status: "done".into(),
            usage_id: None,
            created_at: "t".into(),
        };
        let item = DesignReviewItem {
            id: "i1".into(),
            round_id: "r".into(),
            feature_id: "f".into(),
            project_id: "p".into(),
            position: 0,
            kind: "brief".into(),
            ref_key: None,
            label: None,
            title: "Brief".into(),
            paths: vec![],
            state: "pending".into(),
            stale: Default::default(),
            created_at: "t".into(),
            updated_at: "t".into(),
        };
        db.insert_design_review_item(&item).unwrap();
        let result = db.design_review_transaction(|db| {
            db.append_design_review_turn(&turn)?;
            db.set_design_review_item_state("i1", "accepted", "t")
        });
        assert!(result.is_err());
        assert!(db.design_review_turns("i1").unwrap().is_empty());
    }

    fn decision(uid: &str, project: &str, feature: &str, seq: i64) -> DesignDecision {
        DesignDecision {
            uid: uid.into(),
            feature_id: feature.into(),
            project_id: project.into(),
            seq,
            kind: "architecture".into(),
            title: format!("title {seq}"),
            why: "why".into(),
            rejected: vec!["db: contention".into()],
            ask: Some("ok?".into()),
            supersedes: None,
            paths: vec!["src/a.rs".into()],
            head: Some("abc".into()),
            session_id: None,
            harness: "claude".into(),
            source: "agent".into(),
            status: "open".into(),
            created_at: "2026-11-13T00:00:00Z".into(),
        }
    }

    #[test]
    fn insert_is_idempotent_by_uid_and_lists_in_seq_order() {
        let (_tmp, db) = open_temp_db();
        assert!(
            db.insert_design_decision_if_absent(&decision("u2", "p", "f", 2))
                .unwrap()
        );
        assert!(
            db.insert_design_decision_if_absent(&decision("u1", "p", "f", 1))
                .unwrap()
        );
        assert!(
            !db.insert_design_decision_if_absent(&decision("u1", "p", "f", 1))
                .unwrap()
        );
        let rows = db.design_decisions_for_feature("f").unwrap();
        assert_eq!(
            rows.iter().map(|d| d.uid.as_str()).collect::<Vec<_>>(),
            ["u1", "u2"]
        );
        assert_eq!(rows[0], decision("u1", "p", "f", 1));
    }

    #[test]
    fn outbox_cursor_round_trips() {
        let (_tmp, db) = open_temp_db();
        assert_eq!(
            db.design_review_outbox_cursor("f").unwrap(),
            OutboxCursor::default()
        );
        let cursor = |offset, sha: &str| OutboxCursor {
            offset,
            tail_sha256: Some(sha.into()),
        };
        db.set_design_review_outbox_cursor("p", "f", &cursor(120, "aa"))
            .unwrap();
        db.set_design_review_outbox_cursor("p", "f", &cursor(240, "bb"))
            .unwrap();
        assert_eq!(
            db.design_review_outbox_cursor("f").unwrap(),
            cursor(240, "bb")
        );
    }

    #[test]
    fn project_deletion_removes_only_that_projects_rows() {
        let (_tmp, db) = open_temp_db();
        db.insert_design_decision_if_absent(&decision("u1", "p1", "f1", 1))
            .unwrap();
        db.insert_design_decision_if_absent(&decision("u2", "p2", "f2", 1))
            .unwrap();
        db.set_design_review_outbox_cursor(
            "p1",
            "f1",
            &OutboxCursor {
                offset: 10,
                tail_sha256: None,
            },
        )
        .unwrap();
        db.conn
            .execute(
                "INSERT INTO design_review_usage
                    (feature_id, project_id, phase, task, agent, provenance, started_at)
                 VALUES ('f1', 'p1', 'capture', 'decision-record', 'developer',
                         'estimated', datetime('now'))",
                [],
            )
            .unwrap();

        db.delete_design_review_for_project("p1").unwrap();

        assert!(db.design_decisions_for_feature("f1").unwrap().is_empty());
        assert_eq!(db.design_decisions_for_feature("f2").unwrap().len(), 1);
        assert_eq!(
            db.design_review_outbox_cursor("f1").unwrap(),
            OutboxCursor::default()
        );
        let usage: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM design_review_usage", [], |r| r.get(0))
            .unwrap();
        assert_eq!(usage, 0);
    }
}
