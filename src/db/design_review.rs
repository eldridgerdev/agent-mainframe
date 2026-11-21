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
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};

use crate::design_review::outbox::OutboxCursor;
use crate::design_review::record::DecisionRecord;
use crate::token_cost::cost::TokenCounts;
use crate::token_cost::ledger::{Agent, LedgerRow, Phase, Provenance, Task};
use crate::token_cost::pricing::{LongPromptTier, ModelRates};

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

/// One `design_review_items` row.
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
    /// The developer's latest reply kind (`done`, `pushback`, `answer`).
    pub reply_kind: Option<String>,
    pub reply: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// A round's items in agenda order.
pub fn list_items(conn: &Connection, round_id: &str) -> Result<Vec<DesignReviewItem>> {
    let sql = format!(
        "SELECT {ITEM_COLUMNS} FROM design_review_items WHERE round_id = ?1
         ORDER BY position ASC, rowid ASC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params![round_id], item_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

const ITEM_COLUMNS: &str = "id, round_id, feature_id, project_id, position, kind, ref_key, label,
                title, paths, state, stale, reply_kind, reply, created_at, updated_at";

fn item_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DesignReviewItem> {
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
        reply_kind: row.get(12)?,
        reply: row.get(13)?,
        created_at: row.get(14)?,
        updated_at: row.get(15)?,
    })
}

pub fn insert_item(conn: &Connection, item: &DesignReviewItem) -> Result<()> {
    conn.execute(
        "INSERT INTO design_review_items
            (id, round_id, feature_id, project_id, position, kind, ref_key, label,
             title, paths, state, stale, reply_kind, reply, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
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
            item.reply_kind,
            item.reply,
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

/// Store the brief's label on an item.
pub fn set_item_label(conn: &Connection, item_id: &str, label: &str) -> Result<()> {
    conn.execute(
        "UPDATE design_review_items SET label = ?2 WHERE id = ?1",
        params![item_id, label],
    )?;
    Ok(())
}

/// Store the developer's latest reply on an item.
pub fn set_item_reply(
    conn: &Connection,
    item_id: &str,
    kind: &str,
    reply: &str,
    now: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE design_review_items SET reply_kind = ?2, reply = ?3, updated_at = ?4
         WHERE id = ?1",
        params![item_id, kind, reply, now],
    )?;
    Ok(())
}

/// The item carrying `label` in the feature's newest round that has one.
pub fn find_item_by_label(
    conn: &Connection,
    feature_id: &str,
    label: &str,
) -> Result<Option<DesignReviewItem>> {
    let sql = format!(
        "SELECT {ITEM_COLUMNS} FROM design_review_items i
         WHERE i.feature_id = ?1 AND i.label = ?2
         ORDER BY (SELECT r.round FROM design_review_rounds r WHERE r.id = i.round_id) DESC,
                  i.position ASC
         LIMIT 1"
    );
    Ok(conn
        .query_row(&sql, params![feature_id, label], item_from_row)
        .optional()?)
}

/// The verdict of the round with this id, `None` while it is open or when
/// no such round exists.
pub fn round_verdict(conn: &Connection, round_id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT verdict FROM design_review_rounds WHERE id = ?1",
            params![round_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
}

/// Whether a thread turn with this id exists (replies use their outbox uid).
pub fn turn_exists(conn: &Connection, turn_id: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM design_review_turns WHERE id = ?1",
            params![turn_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Mark the feature's agent decision numbered `seq` as superseded. Returns
/// whether a row changed.
pub fn mark_decision_superseded(conn: &Connection, feature_id: &str, seq: i64) -> Result<bool> {
    let changed = conn.execute(
        "UPDATE design_decisions SET status = 'superseded'
         WHERE feature_id = ?1 AND seq = ?2 AND source = 'agent' AND status != 'superseded'",
        params![feature_id, seq],
    )?;
    Ok(changed > 0)
}

/// Approval: every open decision of the feature becomes accepted. Superseded
/// ones are untouched. Returns how many changed.
pub fn accept_open_decisions(conn: &Connection, feature_id: &str) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE design_decisions SET status = 'accepted'
         WHERE feature_id = ?1 AND status = 'open'",
        params![feature_id],
    )?)
}

/// Finish a round: its verdict, and for changes requested the brief and where
/// it went.
pub fn finish_round(
    conn: &Connection,
    round_id: &str,
    verdict: &str,
    brief: Option<&str>,
    destination: Option<&str>,
    now: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE design_review_rounds
         SET verdict = ?2, brief = ?3, destination = ?4, finished_at = ?5
         WHERE id = ?1",
        params![round_id, verdict, brief, destination, now],
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

/// One cached packet section (`design_review_artifacts`). `section` is a
/// `token_cost::ledger::Task` string (`brief`, `architecture`, `key-flow`,
/// `fit`, `unexplained`, `plan-drift`); `format` is `markdown`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub id: i64,
    pub feature_id: String,
    pub project_id: String,
    pub section: String,
    pub input_hash: String,
    pub format: String,
    pub body: String,
    /// The `design_review_usage` row that produced the body.
    pub usage_id: Option<i64>,
    pub created_at: String,
}

/// A section to cache; the id and timestamp are assigned on insert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewArtifact<'a> {
    pub feature_id: &'a str,
    pub project_id: &'a str,
    pub section: &'a str,
    pub input_hash: &'a str,
    pub format: &'a str,
    pub body: &'a str,
    pub usage_id: Option<i64>,
}

const ARTIFACT_COLUMNS: &str =
    "id, feature_id, project_id, section, input_hash, format, body, usage_id, created_at";

fn artifact_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Artifact> {
    Ok(Artifact {
        id: row.get(0)?,
        feature_id: row.get(1)?,
        project_id: row.get(2)?,
        section: row.get(3)?,
        input_hash: row.get(4)?,
        format: row.get(5)?,
        body: row.get(6)?,
        usage_id: row.get(7)?,
        created_at: row.get(8)?,
    })
}

/// Cache an artifact. When one with the same feature, section and hash is
/// already stored (two preparations raced), that row is kept and its id is
/// returned: the content is the same by construction.
pub fn insert_artifact(conn: &Connection, artifact: &NewArtifact<'_>) -> Result<i64> {
    conn.execute(
        "INSERT INTO design_review_artifacts
            (feature_id, project_id, section, input_hash, format, body, usage_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'))
         ON CONFLICT(feature_id, section, input_hash) DO NOTHING",
        params![
            artifact.feature_id,
            artifact.project_id,
            artifact.section,
            artifact.input_hash,
            artifact.format,
            artifact.body,
            artifact.usage_id,
        ],
    )?;
    let id = conn.query_row(
        "SELECT id FROM design_review_artifacts
         WHERE feature_id = ?1 AND section = ?2 AND input_hash = ?3",
        params![artifact.feature_id, artifact.section, artifact.input_hash],
        |row| row.get(0),
    )?;
    Ok(id)
}

/// The artifact for exactly these inputs, if one is cached.
pub fn artifact(
    conn: &Connection,
    feature_id: &str,
    section: &str,
    input_hash: &str,
) -> Result<Option<Artifact>> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT {ARTIFACT_COLUMNS} FROM design_review_artifacts
                 WHERE feature_id = ?1 AND section = ?2 AND input_hash = ?3"
            ),
            params![feature_id, section, input_hash],
            artifact_from_row,
        )
        .optional()?)
}

/// One cached packet section by row id, with its input hash and usage link.
pub fn artifact_by_id(conn: &Connection, id: i64) -> Result<Option<Artifact>> {
    Ok(conn
        .query_row(
            &format!("SELECT {ARTIFACT_COLUMNS} FROM design_review_artifacts WHERE id = ?1"),
            params![id],
            artifact_from_row,
        )
        .optional()?)
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

/// One stored ledger row with its ids.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageRecord {
    pub id: i64,
    pub feature_id: String,
    pub project_id: String,
    pub row: LedgerRow,
}

fn rates_json(rates: &ModelRates) -> String {
    serde_json::json!({
        "input": rates.input,
        "output": rates.output,
        "reasoning": rates.reasoning,
        "cache_read": rates.cache_read,
        "cache_write_5m": rates.cache_write_5m,
        "cache_write_1h": rates.cache_write_1h,
        "long_prompt": rates.long_prompt.map(|tier| serde_json::json!({
            "threshold_tokens": tier.threshold_tokens,
            "multiplier": tier.multiplier,
        })),
        "assumed": rates.assumed,
    })
    .to_string()
}

/// Rates stored on a row; a missing or unreadable value reads as all-zero,
/// assumed rates rather than failing the whole listing.
fn parse_rates(text: Option<&str>) -> ModelRates {
    let value: serde_json::Value = text
        .and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or(serde_json::Value::Null);
    let rate = |key: &str| value.get(key).and_then(serde_json::Value::as_f64);
    ModelRates {
        input: rate("input").unwrap_or(0.0),
        output: rate("output").unwrap_or(0.0),
        reasoning: rate("reasoning").unwrap_or(0.0),
        cache_read: rate("cache_read").unwrap_or(0.0),
        cache_write_5m: rate("cache_write_5m").unwrap_or(0.0),
        cache_write_1h: rate("cache_write_1h").unwrap_or(0.0),
        long_prompt: value.get("long_prompt").and_then(|tier| {
            Some(LongPromptTier {
                threshold_tokens: tier.get("threshold_tokens")?.as_u64()?,
                multiplier: tier.get("multiplier")?.as_f64()?,
            })
        }),
        assumed: value
            .get("assumed")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true),
    }
}

fn opt_i64(value: Option<u64>) -> Option<i64> {
    value.map(|v| i64::try_from(v).unwrap_or(i64::MAX))
}

fn opt_u64(value: Option<i64>) -> Option<u64> {
    value.map(|v| v.max(0) as u64)
}

/// Append one ledger row. Rows are never updated. Returns its id.
pub fn insert_usage(
    conn: &Connection,
    row: &LedgerRow,
    feature_id: &str,
    project_id: &str,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO design_review_usage (
            feature_id, project_id, phase, task, agent, harness, model, reasoning,
            round, item, input, output, cache_read, cache_write_5m, cache_write_1h,
            reasoning_tokens, effective_tokens, cost_usd, rates, provenance,
            price_assumed, cache_hit, saved_usd, duration_ms, started_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                   ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)",
        params![
            feature_id,
            project_id,
            row.phase.as_str(),
            row.task.as_str(),
            row.agent.as_str(),
            row.harness,
            row.model,
            row.reasoning,
            i64::from(row.round),
            row.agenda_item,
            opt_i64(row.counts.input),
            opt_i64(row.counts.output),
            opt_i64(row.counts.cache_read),
            opt_i64(row.counts.cache_write_5m),
            opt_i64(row.counts.cache_write_1h),
            opt_i64(row.counts.reasoning),
            opt_i64(row.effective_tokens),
            row.cost_usd,
            rates_json(&row.rates_applied),
            row.provenance.as_str(),
            row.price_assumed,
            row.cache_hit,
            row.saved_usd,
            opt_i64(row.duration_ms),
            row.started_at.to_rfc3339(),
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

const USAGE_COLUMNS: &str = "id, feature_id, project_id, phase, task, agent, harness, model,
    reasoning, round, item, input, output, cache_read, cache_write_5m, cache_write_1h,
    reasoning_tokens, effective_tokens, cost_usd, rates, provenance, price_assumed,
    cache_hit, saved_usd, duration_ms, started_at";

fn usage_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<UsageRecord> {
    let text = |idx: usize, value: String| {
        rusqlite::Error::FromSqlConversionFailure(
            idx,
            rusqlite::types::Type::Text,
            format!("unknown ledger value {value:?}").into(),
        )
    };
    let phase: String = row.get(3)?;
    let task: String = row.get(4)?;
    let agent: String = row.get(5)?;
    let provenance: String = row.get(20)?;
    let started_at: String = row.get(25)?;
    let rates: Option<String> = row.get(19)?;
    Ok(UsageRecord {
        id: row.get(0)?,
        feature_id: row.get(1)?,
        project_id: row.get(2)?,
        row: LedgerRow {
            phase: Phase::parse(&phase).ok_or_else(|| text(3, phase.clone()))?,
            task: Task::parse(&task).ok_or_else(|| text(4, task.clone()))?,
            agent: Agent::parse(&agent).ok_or_else(|| text(5, agent.clone()))?,
            harness: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
            model: row.get::<_, Option<String>>(7)?.unwrap_or_default(),
            reasoning: row.get::<_, Option<String>>(8)?.unwrap_or_default(),
            round: row
                .get::<_, Option<i64>>(9)?
                .map_or(0, |r| u32::try_from(r.max(0)).unwrap_or(u32::MAX)),
            agenda_item: row.get(10)?,
            counts: TokenCounts {
                input: opt_u64(row.get(11)?),
                output: opt_u64(row.get(12)?),
                cache_read: opt_u64(row.get(13)?),
                cache_write_5m: opt_u64(row.get(14)?),
                cache_write_1h: opt_u64(row.get(15)?),
                reasoning: opt_u64(row.get(16)?),
            },
            effective_tokens: opt_u64(row.get(17)?),
            cost_usd: row.get::<_, Option<f64>>(18)?.unwrap_or(0.0),
            rates_applied: parse_rates(rates.as_deref()),
            provenance: Provenance::parse(&provenance)
                .ok_or_else(|| text(20, provenance.clone()))?,
            price_assumed: row.get(21)?,
            cache_hit: row.get(22)?,
            saved_usd: row.get(23)?,
            duration_ms: opt_u64(row.get(24)?),
            started_at: DateTime::parse_from_rfc3339(&started_at)
                .map(|t| t.with_timezone(&Utc))
                .map_err(|_| text(25, started_at.clone()))?,
        },
    })
}

/// A feature's ledger in insertion order.
pub fn list_usage_records(conn: &Connection, feature_id: &str) -> Result<Vec<UsageRecord>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {USAGE_COLUMNS} FROM design_review_usage
         WHERE feature_id = ?1 ORDER BY id ASC"
    ))?;
    let rows = stmt
        .query_map(params![feature_id], usage_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// One ledger row by id.
pub fn usage(conn: &Connection, id: i64) -> Result<Option<UsageRecord>> {
    Ok(conn
        .query_row(
            &format!("SELECT {USAGE_COLUMNS} FROM design_review_usage WHERE id = ?1"),
            params![id],
            usage_from_row,
        )
        .optional()?)
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
            reply_kind: None,
            reply: None,
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

    fn new_artifact<'a>(section: &'a str, hash: &'a str, body: &'a str) -> NewArtifact<'a> {
        NewArtifact {
            feature_id: "f",
            project_id: "p",
            section,
            input_hash: hash,
            format: "markdown",
            body,
            usage_id: Some(7),
        }
    }

    #[test]
    fn artifact_is_found_by_hash_and_a_duplicate_keeps_the_first() {
        let (_tmp, db) = open_temp_db();
        assert_eq!(db.design_review_artifact("f", "brief", "h1").unwrap(), None);
        let id = db
            .insert_design_review_artifact(&new_artifact("brief", "h1", "one"))
            .unwrap();
        let again = db
            .insert_design_review_artifact(&new_artifact("brief", "h1", "other"))
            .unwrap();
        assert_eq!(id, again);
        let got = db
            .design_review_artifact("f", "brief", "h1")
            .unwrap()
            .unwrap();
        assert_eq!(got.id, id);
        assert_eq!(got.body, "one");
        assert_eq!(got.format, "markdown");
        assert_eq!(got.usage_id, Some(7));
        assert_eq!(got.project_id, "p");
        assert_eq!(db.design_review_artifact("f", "fit", "h1").unwrap(), None);
        assert_eq!(db.design_review_artifact("g", "brief", "h1").unwrap(), None);
    }

    #[test]
    fn latest_artifact_is_the_newest_of_its_section() {
        let (_tmp, db) = open_temp_db();
        assert_eq!(
            db.latest_design_review_artifact("f", "brief").unwrap(),
            None
        );
        db.insert_design_review_artifact(&new_artifact("brief", "h1", "old"))
            .unwrap();
        db.insert_design_review_artifact(&new_artifact("fit", "h9", "fit"))
            .unwrap();
        db.insert_design_review_artifact(&new_artifact("brief", "h2", "new"))
            .unwrap();
        let latest = db
            .latest_design_review_artifact("f", "brief")
            .unwrap()
            .unwrap();
        assert_eq!(latest.body, "new");
        let full = db.design_review_artifact_by_id(latest.id).unwrap().unwrap();
        assert_eq!(full.input_hash, "h2");
    }

    fn ledger_row(full: bool) -> LedgerRow {
        use crate::token_cost::ledger::LedgerContext;
        let ctx = LedgerContext {
            phase: Phase::Prepare,
            task: Task::KeyFlow,
            agent: Agent::Cartographer,
            harness: "claude".into(),
            model: "haiku".into(),
            reasoning: if full { "low".into() } else { String::new() },
            round: 2,
            agenda_item: full.then(|| "item-3".into()),
        };
        let counts = if full {
            TokenCounts {
                input: Some(1_200),
                output: Some(300),
                cache_read: Some(5_000),
                cache_write_5m: Some(40),
                cache_write_1h: Some(10),
                reasoning: Some(0),
            }
        } else {
            TokenCounts::default()
        };
        let started = DateTime::parse_from_rfc3339("2026-11-16T10:00:00.250Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut row = LedgerRow::from_counts(
            ctx,
            counts,
            if full {
                Provenance::Measured
            } else {
                Provenance::Estimated
            },
            full.then_some(1_500),
            started,
            &crate::token_tracking::TokenPricingConfig::default(),
        );
        if full {
            row.cache_hit = true;
            row.saved_usd = Some(0.0125);
            row.price_assumed = true;
        }
        row
    }

    #[test]
    fn usage_rows_round_trip_every_column() {
        let (_tmp, db) = open_temp_db();
        let full = ledger_row(true);
        let empty = ledger_row(false);
        assert!(full.rates_applied.long_prompt.is_some(), "haiku has a tier");
        let a = db.insert_design_review_usage(&full, "f", "p").unwrap();
        let b = db.insert_design_review_usage(&empty, "f", "p").unwrap();
        db.insert_design_review_usage(&full, "other", "p").unwrap();
        assert!(b > a);

        let rows = db.list_design_review_usage("f").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, a);
        assert_eq!(rows[0].feature_id, "f");
        assert_eq!(rows[0].project_id, "p");
        assert_eq!(rows[0].row, full);
        assert_eq!(rows[1].row, empty);
        assert!(rows[1].row.counts.is_all_unavailable());
        assert_eq!(rows[1].row.agenda_item, None);
        assert_eq!(rows[1].row.duration_ms, None);
        assert_eq!(rows[1].row.saved_usd, None);
        assert_eq!(rows[1].row.effective_tokens, None);

        assert_eq!(
            db.design_review_usage_record(b).unwrap().unwrap().row,
            empty
        );
        assert_eq!(db.design_review_usage_record(9_999).unwrap(), None);
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
