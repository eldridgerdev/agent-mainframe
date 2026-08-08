//! SQLite persistence for Learning Mode (`docs/backlog/learning-mode-plan.md`).
//!
//! A [`LearningSession`] holds the settings that outlive a single question
//! (harness, explanation level, whether the first-open help has been shown);
//! each [`LearningQa`] row is one anchored question and its answer. Follow-ups
//! point at their parent through `parent_qa_id` and are cascade-deleted with
//! it, just as a session's rows are cascade-deleted with the session.
//!
//! Like todo lists, this data lives *outside* the `ProjectStore` JSON blob and
//! its full-replace save path, so asking a question doesn't rewrite the store
//! and a store save doesn't rewrite history. `project_id` / `feature_id` are
//! plain TEXT with no FK to projects/features, so cleanup on project deletion
//! is explicit — see [`delete_sessions_for_project`].
//!
//! The overlay keeps its own in-memory copy as the source of truth (again like
//! todos), so Learning Mode works with no DB at all; these functions persist it
//! when one is present.
//!
//! The persistence layer lands ahead of its UI consumers (the plan's Epics
//! 2-5), so parts of this API are allowed to be unused for now.
#![allow(dead_code)]

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use super::store::{agent_from_str, agent_to_str};
// The row types live beside the overlay's other state (`src/app/state.rs`)
// because the enums they embed are the UI's own vocabulary; re-exported here so
// callers can spell them `db::learning::LearningQa` like every other table's
// rows.
pub use crate::app::{
    LearningAnchor, LearningLevel, LearningQa, LearningQaIntent, LearningQaStatus, LearningRunMode,
    LearningSession,
};
use crate::project::AgentKind;

/// Timestamp in the same shape the rest of the schema uses (`datetime('now')`),
/// with milliseconds so rows created in the same second still sort in the order
/// they were asked.
pub fn now_timestamp() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string()
}

/// The project's learning session, or `None` if it has never opened Learning
/// Mode. When a project somehow has several (the schema permits it — see
/// MIGRATION_017), the most recently updated one wins.
pub fn load_session(conn: &Connection, project_id: &str) -> Result<Option<LearningSession>> {
    let row = conn
        .query_row(
            "SELECT id, project_id, feature_id, title, harness, level,
                    onboarding_seen, created_at, updated_at
             FROM learning_sessions WHERE project_id = ?1
             ORDER BY updated_at DESC LIMIT 1",
            params![project_id],
            row_to_session,
        )
        .optional()?;
    Ok(row)
}

/// Create a learning session for `project_id`, hosted by `feature_id`.
pub fn create_session(
    conn: &Connection,
    project_id: &str,
    feature_id: &str,
    title: &str,
    harness: &AgentKind,
    level: LearningLevel,
) -> Result<LearningSession> {
    let session = LearningSession {
        id: Uuid::new_v4().to_string(),
        project_id: project_id.to_string(),
        feature_id: feature_id.to_string(),
        title: title.to_string(),
        harness: harness.clone(),
        level,
        onboarding_seen: false,
        created_at: now_timestamp(),
        updated_at: now_timestamp(),
    };
    conn.execute(
        "INSERT INTO learning_sessions
            (id, project_id, feature_id, title, harness, level, onboarding_seen,
             created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?8)",
        params![
            session.id,
            session.project_id,
            session.feature_id,
            session.title,
            agent_to_str(&session.harness),
            session.level.as_str(),
            session.created_at,
            session.updated_at,
        ],
    )?;
    Ok(session)
}

/// The project's learning session, created under `feature_id` if it has none.
/// This is where v1's one-session-per-project behaviour is enforced; the schema
/// itself stays open to more (see MIGRATION_017).
pub fn load_or_create_session(
    conn: &Connection,
    project_id: &str,
    feature_id: &str,
    title: &str,
    harness: &AgentKind,
    level: LearningLevel,
) -> Result<LearningSession> {
    match load_session(conn, project_id)? {
        Some(session) => Ok(session),
        None => create_session(conn, project_id, feature_id, title, harness, level),
    }
}

/// Persist the mutable fields of a session (harness, level, onboarding flag,
/// title, host feature). `updated_at` is bumped.
pub fn update_session(conn: &Connection, session: &LearningSession) -> Result<()> {
    conn.execute(
        "UPDATE learning_sessions SET
            feature_id = ?2, title = ?3, harness = ?4, level = ?5,
            onboarding_seen = ?6, updated_at = ?7
         WHERE id = ?1",
        params![
            session.id,
            session.feature_id,
            session.title,
            agent_to_str(&session.harness),
            session.level.as_str(),
            session.onboarding_seen as i64,
            now_timestamp(),
        ],
    )?;
    Ok(())
}

/// Persist just the two settings the overlay lets the user change mid-session,
/// without needing the whole row in hand.
pub fn set_session_settings(
    conn: &Connection,
    session_id: &str,
    harness: &AgentKind,
    level: LearningLevel,
) -> Result<()> {
    conn.execute(
        "UPDATE learning_sessions SET harness = ?2, level = ?3, updated_at = ?4 WHERE id = ?1",
        params![
            session_id,
            agent_to_str(harness),
            level.as_str(),
            now_timestamp()
        ],
    )?;
    Ok(())
}

/// Record that the first-open help overlay has been shown for this session.
pub fn set_onboarding_seen(conn: &Connection, session_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE learning_sessions SET onboarding_seen = 1, updated_at = ?2 WHERE id = ?1",
        params![session_id, now_timestamp()],
    )?;
    Ok(())
}

/// Whether the first-open help overlay has already been shown for this
/// session. A missing row counts as "seen" so a lookup failure can never make
/// the intro reappear on every open.
pub fn onboarding_seen(conn: &Connection, session_id: &str) -> Result<bool> {
    let seen: Option<i64> = conn
        .query_row(
            "SELECT onboarding_seen FROM learning_sessions WHERE id = ?1",
            params![session_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(seen.map(|v| v != 0).unwrap_or(true))
}

/// Delete a session and (via cascade) every Q&A row under it.
pub fn delete_session(conn: &Connection, session_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM learning_sessions WHERE id = ?1",
        params![session_id],
    )?;
    Ok(())
}

/// Delete every learning session (and its Q&A rows) for `project_id`. Called
/// when a project is deleted, since there is no FK cascade from `projects`.
pub fn delete_sessions_for_project(conn: &Connection, project_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM learning_sessions WHERE project_id = ?1",
        params![project_id],
    )?;
    Ok(())
}

/// Every Q&A row in a session, oldest first — the order the overlay renders
/// history in before it threads follow-ups under their parents.
pub fn list_qa(conn: &Connection, session_id: &str) -> Result<Vec<LearningQa>> {
    let mut stmt = conn.prepare(
        "SELECT id, learning_session_id, parent_qa_id, file_path, anchor_kind,
                line_start, line_end, selection_text, question, intent, level,
                answer, harness, run_mode, status, error, todo_id,
                spawned_session_id, created_at, updated_at
         FROM learning_qa WHERE learning_session_id = ?1
         ORDER BY created_at ASC, rowid ASC",
    )?;
    let rows = stmt.query_map(params![session_id], row_to_qa)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Insert or update one Q&A row.
///
/// Deliberately `ON CONFLICT DO UPDATE` rather than `INSERT OR REPLACE`:
/// REPLACE deletes the existing row first, which would cascade away every
/// follow-up hanging off it every time its status changed.
pub fn upsert_qa(conn: &Connection, qa: &LearningQa) -> Result<()> {
    let (line_start, line_end) = qa.anchor.line_range();
    conn.execute(
        "INSERT INTO learning_qa
            (id, learning_session_id, parent_qa_id, file_path, anchor_kind,
             line_start, line_end, selection_text, question, intent, level,
             answer, harness, run_mode, status, error, todo_id,
             spawned_session_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                 ?15, ?16, ?17, ?18, ?19, ?20)
         ON CONFLICT(id) DO UPDATE SET
            parent_qa_id = excluded.parent_qa_id,
            file_path = excluded.file_path,
            anchor_kind = excluded.anchor_kind,
            line_start = excluded.line_start,
            line_end = excluded.line_end,
            selection_text = excluded.selection_text,
            question = excluded.question,
            intent = excluded.intent,
            level = excluded.level,
            answer = excluded.answer,
            harness = excluded.harness,
            run_mode = excluded.run_mode,
            status = excluded.status,
            error = excluded.error,
            todo_id = excluded.todo_id,
            spawned_session_id = excluded.spawned_session_id,
            updated_at = excluded.updated_at",
        params![
            qa.id,
            qa.session_id,
            qa.parent_qa_id,
            qa.file_path,
            qa.anchor.kind_str(),
            line_start.map(|v| v as i64),
            line_end.map(|v| v as i64),
            qa.selection_text,
            qa.question,
            qa.intent.as_str(),
            qa.level.as_str(),
            qa.answer,
            agent_to_str(&qa.harness),
            qa.run_mode.as_str(),
            qa.status.as_str(),
            qa.error,
            qa.todo_id,
            qa.spawned_session_id,
            if qa.created_at.is_empty() {
                now_timestamp()
            } else {
                qa.created_at.clone()
            },
            now_timestamp(),
        ],
    )?;
    Ok(())
}

/// Delete one Q&A row; its follow-ups cascade away with it.
pub fn delete_qa(conn: &Connection, qa_id: &str) -> Result<()> {
    conn.execute("DELETE FROM learning_qa WHERE id = ?1", params![qa_id])?;
    Ok(())
}

fn row_to_session(row: &rusqlite::Row) -> rusqlite::Result<LearningSession> {
    let harness: String = row.get(4)?;
    let level: String = row.get(5)?;
    let onboarding_seen: i64 = row.get(6)?;
    Ok(LearningSession {
        id: row.get(0)?,
        project_id: row.get(1)?,
        feature_id: row.get(2)?,
        title: row.get(3)?,
        harness: agent_from_str(&harness),
        level: LearningLevel::from_str(&level),
        onboarding_seen: onboarding_seen != 0,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

fn row_to_qa(row: &rusqlite::Row) -> rusqlite::Result<LearningQa> {
    let anchor_kind: String = row.get(4)?;
    let line_start: Option<i64> = row.get(5)?;
    let line_end: Option<i64> = row.get(6)?;
    let intent: String = row.get(9)?;
    let level: String = row.get(10)?;
    let harness: String = row.get(12)?;
    let run_mode: String = row.get(13)?;
    let status: String = row.get(14)?;
    Ok(LearningQa {
        id: row.get(0)?,
        session_id: row.get(1)?,
        parent_qa_id: row.get(2)?,
        file_path: row.get(3)?,
        anchor: LearningAnchor::from_parts(
            &anchor_kind,
            line_start.map(|v| v as usize),
            line_end.map(|v| v as usize),
        ),
        selection_text: row.get(7)?,
        question: row.get(8)?,
        intent: LearningQaIntent::from_str(&intent),
        level: LearningLevel::from_str(&level),
        answer: row.get(11)?,
        harness: agent_from_str(&harness),
        run_mode: LearningRunMode::from_str(&run_mode),
        status: LearningQaStatus::from_str(&status),
        error: row.get(15)?,
        todo_id: row.get(16)?,
        spawned_session_id: row.get(17)?,
        created_at: row.get(18)?,
        updated_at: row.get(19)?,
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

    fn sample_qa(session_id: &str, question: &str) -> LearningQa {
        LearningQa {
            id: Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            parent_qa_id: None,
            file_path: Some("src/app/learning.rs".to_string()),
            anchor: LearningAnchor::Lines { start: 40, end: 58 },
            selection_text: "fn open_learning(&mut self) {}".to_string(),
            question: question.to_string(),
            intent: LearningQaIntent::Explain,
            level: LearningLevel::Newcomer,
            answer: None,
            harness: AgentKind::Claude,
            run_mode: LearningRunMode::NoTools,
            status: LearningQaStatus::Pending,
            error: None,
            todo_id: None,
            spawned_session_id: None,
            created_at: now_timestamp(),
            updated_at: now_timestamp(),
        }
    }

    #[test]
    fn create_and_load_session() {
        let (_tmp, db) = open_temp_db();
        assert!(db.learning_session("proj-1").unwrap().is_none());

        let created = db
            .create_learning_session(
                "proj-1",
                "feat-1",
                "amf",
                &AgentKind::Codex,
                LearningLevel::Newcomer,
            )
            .unwrap();

        let loaded = db.learning_session("proj-1").unwrap().unwrap();
        assert_eq!(loaded.id, created.id);
        assert_eq!(loaded.feature_id, "feat-1");
        assert_eq!(loaded.harness, AgentKind::Codex);
        assert_eq!(loaded.level, LearningLevel::Newcomer);
        assert!(!loaded.onboarding_seen);
    }

    #[test]
    fn load_or_create_returns_the_existing_session() {
        let (_tmp, db) = open_temp_db();
        let a = db
            .load_or_create_learning_session(
                "proj-1",
                "feat-1",
                "amf",
                &AgentKind::Claude,
                LearningLevel::Newcomer,
            )
            .unwrap();
        let b = db
            .load_or_create_learning_session(
                "proj-1",
                "feat-9",
                "amf",
                &AgentKind::Pi,
                LearningLevel::Familiar,
            )
            .unwrap();
        assert_eq!(a.id, b.id);
        // The first create's settings stick; the second call doesn't overwrite.
        assert_eq!(b.feature_id, "feat-1");
        assert_eq!(b.harness, AgentKind::Claude);
    }

    #[test]
    fn session_settings_round_trip() {
        let (_tmp, db) = open_temp_db();
        let mut session = db
            .create_learning_session(
                "proj-1",
                "feat-1",
                "amf",
                &AgentKind::Claude,
                LearningLevel::Newcomer,
            )
            .unwrap();

        session.harness = AgentKind::Opencode;
        session.level = LearningLevel::Familiar;
        session.onboarding_seen = true;
        db.update_learning_session(&session).unwrap();

        let loaded = db.learning_session("proj-1").unwrap().unwrap();
        assert_eq!(loaded.harness, AgentKind::Opencode);
        assert_eq!(loaded.level, LearningLevel::Familiar);
        assert!(loaded.onboarding_seen);
    }

    #[test]
    fn onboarding_flag_is_sticky() {
        let (_tmp, db) = open_temp_db();
        let session = db
            .create_learning_session(
                "proj-1",
                "feat-1",
                "amf",
                &AgentKind::Claude,
                LearningLevel::Newcomer,
            )
            .unwrap();
        db.set_learning_onboarding_seen(&session.id).unwrap();
        assert!(db.learning_session("proj-1").unwrap().unwrap().onboarding_seen);
    }

    #[test]
    fn qa_round_trips_every_field() {
        let (_tmp, db) = open_temp_db();
        let session = db
            .create_learning_session(
                "proj-1",
                "feat-1",
                "amf",
                &AgentKind::Claude,
                LearningLevel::Newcomer,
            )
            .unwrap();

        let mut qa = sample_qa(&session.id, "What does this do?");
        qa.intent = LearningQaIntent::Action;
        qa.level = LearningLevel::Familiar;
        qa.run_mode = LearningRunMode::DeepDive;
        qa.status = LearningQaStatus::Answered;
        qa.answer = Some("It opens the overlay.".to_string());
        qa.todo_id = Some("todo-7".to_string());
        qa.spawned_session_id = Some("sess-3".to_string());
        db.upsert_learning_qa(&qa).unwrap();

        let rows = db.learning_qa(&session.id).unwrap();
        assert_eq!(rows.len(), 1);
        let loaded = &rows[0];
        assert_eq!(loaded.id, qa.id);
        assert_eq!(loaded.file_path.as_deref(), Some("src/app/learning.rs"));
        assert_eq!(loaded.anchor, LearningAnchor::Lines { start: 40, end: 58 });
        assert_eq!(loaded.selection_text, qa.selection_text);
        assert_eq!(loaded.intent, LearningQaIntent::Action);
        assert_eq!(loaded.level, LearningLevel::Familiar);
        assert_eq!(loaded.run_mode, LearningRunMode::DeepDive);
        assert_eq!(loaded.status, LearningQaStatus::Answered);
        assert_eq!(loaded.answer.as_deref(), Some("It opens the overlay."));
        assert_eq!(loaded.todo_id.as_deref(), Some("todo-7"));
        assert_eq!(loaded.spawned_session_id.as_deref(), Some("sess-3"));
    }

    #[test]
    fn project_anchor_round_trips_without_a_file() {
        let (_tmp, db) = open_temp_db();
        let session = db
            .create_learning_session(
                "proj-1",
                "feat-1",
                "amf",
                &AgentKind::Claude,
                LearningLevel::Newcomer,
            )
            .unwrap();

        let mut qa = sample_qa(&session.id, "Give me a tour of this project.");
        qa.anchor = LearningAnchor::Project;
        qa.file_path = None;
        db.upsert_learning_qa(&qa).unwrap();

        let loaded = &db.learning_qa(&session.id).unwrap()[0];
        assert_eq!(loaded.anchor, LearningAnchor::Project);
        assert!(loaded.file_path.is_none());
    }

    /// The default, non-actioned path: an answered explain entry with no
    /// follow-up survives a reload untouched.
    #[test]
    fn answered_explain_entry_reloads_unchanged() {
        let (_tmp, db) = open_temp_db();
        let session = db
            .create_learning_session(
                "proj-1",
                "feat-1",
                "amf",
                &AgentKind::Claude,
                LearningLevel::Newcomer,
            )
            .unwrap();

        let mut qa = sample_qa(&session.id, "Explain this line by line.");
        qa.status = LearningQaStatus::Answered;
        qa.answer = Some("## What it does\n\n- opens the overlay".to_string());
        db.upsert_learning_qa(&qa).unwrap();

        let loaded = db.learning_qa(&session.id).unwrap().remove(0);
        assert_eq!(loaded.question, qa.question);
        assert_eq!(loaded.answer, qa.answer);
        assert_eq!(loaded.intent, LearningQaIntent::Explain);
        assert!(loaded.todo_id.is_none());
        assert!(loaded.spawned_session_id.is_none());
        assert!(loaded.parent_qa_id.is_none());
    }

    #[test]
    fn upsert_updates_in_place_and_keeps_follow_ups() {
        let (_tmp, db) = open_temp_db();
        let session = db
            .create_learning_session(
                "proj-1",
                "feat-1",
                "amf",
                &AgentKind::Claude,
                LearningLevel::Newcomer,
            )
            .unwrap();

        let mut parent = sample_qa(&session.id, "What is this?");
        db.upsert_learning_qa(&parent).unwrap();

        let mut child = sample_qa(&session.id, "What's a trait?");
        child.parent_qa_id = Some(parent.id.clone());
        db.upsert_learning_qa(&child).unwrap();

        // Status transition on the parent must not disturb its follow-up.
        parent.status = LearningQaStatus::Answered;
        parent.answer = Some("It's the overlay entry point.".to_string());
        db.upsert_learning_qa(&parent).unwrap();

        let rows = db.learning_qa(&session.id).unwrap();
        assert_eq!(rows.len(), 2, "upsert must not drop the follow-up");
        let reloaded_parent = rows.iter().find(|r| r.id == parent.id).unwrap();
        assert_eq!(reloaded_parent.status, LearningQaStatus::Answered);
    }

    #[test]
    fn deleting_a_parent_cascades_to_follow_ups() {
        let (_tmp, db) = open_temp_db();
        let session = db
            .create_learning_session(
                "proj-1",
                "feat-1",
                "amf",
                &AgentKind::Claude,
                LearningLevel::Newcomer,
            )
            .unwrap();

        let parent = sample_qa(&session.id, "What is this?");
        db.upsert_learning_qa(&parent).unwrap();
        let mut child = sample_qa(&session.id, "What's a trait?");
        child.parent_qa_id = Some(parent.id.clone());
        db.upsert_learning_qa(&child).unwrap();
        let mut grandchild = sample_qa(&session.id, "And a generic?");
        grandchild.parent_qa_id = Some(child.id.clone());
        db.upsert_learning_qa(&grandchild).unwrap();

        db.delete_learning_qa(&parent.id).unwrap();
        assert!(
            db.learning_qa(&session.id).unwrap().is_empty(),
            "the whole thread should go with its root"
        );
    }

    #[test]
    fn deleting_a_session_cascades_to_its_qa() {
        let (_tmp, db) = open_temp_db();
        let session = db
            .create_learning_session(
                "proj-1",
                "feat-1",
                "amf",
                &AgentKind::Claude,
                LearningLevel::Newcomer,
            )
            .unwrap();
        db.upsert_learning_qa(&sample_qa(&session.id, "What is this?"))
            .unwrap();

        db.delete_learning_session(&session.id).unwrap();
        assert!(db.learning_session("proj-1").unwrap().is_none());
        assert!(db.learning_qa(&session.id).unwrap().is_empty());
    }

    #[test]
    fn project_cleanup_removes_sessions_and_qa() {
        let (_tmp, db) = open_temp_db();
        let doomed = db
            .create_learning_session(
                "proj-1",
                "feat-1",
                "amf",
                &AgentKind::Claude,
                LearningLevel::Newcomer,
            )
            .unwrap();
        let kept = db
            .create_learning_session(
                "proj-2",
                "feat-2",
                "other",
                &AgentKind::Claude,
                LearningLevel::Newcomer,
            )
            .unwrap();
        db.upsert_learning_qa(&sample_qa(&doomed.id, "q1")).unwrap();
        db.upsert_learning_qa(&sample_qa(&kept.id, "q2")).unwrap();

        db.delete_learning_sessions_for_project("proj-1").unwrap();

        assert!(db.learning_session("proj-1").unwrap().is_none());
        assert!(db.learning_qa(&doomed.id).unwrap().is_empty());
        assert!(db.learning_session("proj-2").unwrap().is_some());
        assert_eq!(db.learning_qa(&kept.id).unwrap().len(), 1);
    }

    #[test]
    fn qa_lists_oldest_first() {
        let (_tmp, db) = open_temp_db();
        let session = db
            .create_learning_session(
                "proj-1",
                "feat-1",
                "amf",
                &AgentKind::Claude,
                LearningLevel::Newcomer,
            )
            .unwrap();

        for q in ["first", "second", "third"] {
            db.upsert_learning_qa(&sample_qa(&session.id, q)).unwrap();
        }

        let questions: Vec<String> = db
            .learning_qa(&session.id)
            .unwrap()
            .into_iter()
            .map(|r| r.question)
            .collect();
        assert_eq!(questions, vec!["first", "second", "third"]);
    }
}
