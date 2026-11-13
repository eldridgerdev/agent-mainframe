//! SQLite persistence for Design Review (`MIGRATION_049`,
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

/// Byte offset into the feature's outbox up to which lines were ingested;
/// 0 when never ingested.
pub fn outbox_offset(conn: &Connection, feature_id: &str) -> Result<u64> {
    let offset: Option<i64> = conn
        .query_row(
            "SELECT byte_offset FROM design_review_outbox_offsets WHERE feature_id = ?1",
            params![feature_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(offset.map_or(0, |o| o.max(0) as u64))
}

pub fn set_outbox_offset(
    conn: &Connection,
    project_id: &str,
    feature_id: &str,
    offset: u64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO design_review_outbox_offsets (feature_id, project_id, byte_offset, updated_at)
         VALUES (?1, ?2, ?3, datetime('now'))
         ON CONFLICT(feature_id) DO UPDATE SET
            project_id = excluded.project_id,
            byte_offset = excluded.byte_offset,
            updated_at = excluded.updated_at",
        params![feature_id, project_id, offset as i64],
    )?;
    Ok(())
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
    fn outbox_offset_round_trips() {
        let (_tmp, db) = open_temp_db();
        assert_eq!(db.design_review_outbox_offset("f").unwrap(), 0);
        db.set_design_review_outbox_offset("p", "f", 120).unwrap();
        db.set_design_review_outbox_offset("p", "f", 240).unwrap();
        assert_eq!(db.design_review_outbox_offset("f").unwrap(), 240);
    }

    #[test]
    fn project_deletion_removes_only_that_projects_rows() {
        let (_tmp, db) = open_temp_db();
        db.insert_design_decision_if_absent(&decision("u1", "p1", "f1", 1))
            .unwrap();
        db.insert_design_decision_if_absent(&decision("u2", "p2", "f2", 1))
            .unwrap();
        db.set_design_review_outbox_offset("p1", "f1", 10).unwrap();
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
        assert_eq!(db.design_review_outbox_offset("f1").unwrap(), 0);
        let usage: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM design_review_usage", [], |r| r.get(0))
            .unwrap();
        assert_eq!(usage, 0);
    }
}
