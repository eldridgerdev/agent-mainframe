//! SQLite persistence for plan-mode discovery interviews.
//!
//! A feature keeps at most one **draft** (an interview in progress, saved as
//! answers are given so abandoning the mode or restarting AMF does not lose
//! them) and at most one **final** transcript (the Q&A behind the plan the user
//! accepted). They are separate rows so re-running the interview on a feature
//! that already has an accepted plan can save progress without destroying the
//! plan it is revising. See `docs/backlog/plan-mode-interview-plan.md`, Epic 5.
//!
//! Like `todos`, these rows live *outside* the `ProjectStore` JSON blob and its
//! full-replace save path, and `feature_id` is plain TEXT with no FK for
//! exactly that reason — cleanup on feature deletion is explicit
//! ([`delete_for_feature`]).
//!
//! The store lands ahead of the UI that drives it (Epic 5's remaining items),
//! so parts of this API are unused for now.
#![allow(dead_code)]

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};

use crate::plan_interview::PlanQuestion;

/// Which of a feature's two possible interview rows a record is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PlanInterviewStage {
    /// An interview in progress. Overwritten as answers are given, and
    /// discarded once its plan is accepted or the user chooses to drop it.
    /// The default: every interview is a draft until its plan is accepted.
    #[default]
    Draft,
    /// The interview behind an accepted plan. Read back to pre-fill answers
    /// when the interview is re-run for the same feature.
    Final,
}

impl PlanInterviewStage {
    pub fn as_db_str(self) -> &'static str {
        match self {
            PlanInterviewStage::Draft => "draft",
            PlanInterviewStage::Final => "final",
        }
    }
}

/// One stored interview: the questions asked, the answers collected, and (once
/// synthesis has run) the plan they produced.
///
/// `questions` and `answers` are positionally paired and always the same
/// length — [`load`] normalizes a mismatched row rather than handing readers a
/// ragged pair. Prefer [`PlanInterviewRecord::answer_for`] over indexing when
/// matching against a *current* question set, since a config change can add,
/// remove, or reorder questions between runs.
///
/// Build one for writing with a struct literal over [`Default`]; the timestamps
/// are DB-owned and whatever they hold is ignored by [`save`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanInterviewRecord {
    pub feature_id: String,
    pub stage: PlanInterviewStage,
    /// The feature's name when the interview ran. Kept alongside the id so a
    /// transcript still identifies itself after a rename.
    pub feature_name: String,
    pub brief: String,
    pub questions: Vec<PlanQuestion>,
    pub answers: Vec<Option<String>>,
    /// The synthesized plan. `None` for a draft abandoned before synthesis.
    pub plan: Option<String>,
    /// AI-adaptive rounds already spent. Persisted rather than derived from
    /// `questions` because a round that returned nothing usable still counted
    /// against the cap — resuming a draft must not hand back paid rounds.
    pub ai_rounds_completed: usize,
    /// DB-owned timestamps. Ignored on [`save`], which sets them itself.
    pub created_at: String,
    pub updated_at: String,
}

impl PlanInterviewRecord {
    /// The answer recorded for `question_id`, or `None` if that question was
    /// never asked, was skipped, or was answered blank.
    ///
    /// Question ids are the stable slug that survives across runs, so this —
    /// not the positional index — is how a re-run pre-fills answers against a
    /// question bank the user may have edited in between.
    pub fn answer_for(&self, question_id: &str) -> Option<&str> {
        let index = self
            .questions
            .iter()
            .position(|question| question.id == question_id)?;
        self.answers
            .get(index)?
            .as_deref()
            .filter(|answer| !answer.trim().is_empty())
    }
}

/// Load a feature's interview at `stage`, or `None` if it has none.
///
/// A row whose JSON columns will not parse is returned as an error rather than
/// silently as `None`: callers offering "resume or discard" should treat a
/// failure as "no draft", but the distinction belongs in the debug log.
pub fn load(
    conn: &Connection,
    feature_id: &str,
    stage: PlanInterviewStage,
) -> Result<Option<PlanInterviewRecord>> {
    let row = conn
        .query_row(
            "SELECT feature_name, brief, questions, answers, plan,
                    ai_rounds_completed, created_at, updated_at
             FROM plan_interviews WHERE feature_id = ?1 AND stage = ?2",
            params![feature_id, stage.as_db_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                ))
            },
        )
        .optional()?;

    let Some((
        feature_name,
        brief,
        questions_json,
        answers_json,
        plan,
        ai_rounds_completed,
        created_at,
        updated_at,
    )) = row
    else {
        return Ok(None);
    };

    let questions: Vec<PlanQuestion> =
        serde_json::from_str(&questions_json).with_context(|| {
            format!("stored plan-interview questions for feature {feature_id} are unreadable")
        })?;
    let mut answers: Vec<Option<String>> =
        serde_json::from_str(&answers_json).with_context(|| {
            format!("stored plan-interview answers for feature {feature_id} are unreadable")
        })?;
    // Every reader may assume the pair is aligned; a row written by an older
    // build, or by a question set that changed underneath a draft, is squared
    // up here rather than at each call site.
    answers.resize(questions.len(), None);

    Ok(Some(PlanInterviewRecord {
        feature_id: feature_id.to_string(),
        stage,
        feature_name,
        brief,
        questions,
        answers,
        plan,
        ai_rounds_completed: ai_rounds_completed.max(0) as usize,
        created_at,
        updated_at,
    }))
}

/// Insert or replace the interview at `record.stage`, preserving the original
/// `created_at` when overwriting. Answers are stored padded to the question
/// count so the stored pair is aligned even if the caller's was not.
pub fn save(conn: &Connection, record: &PlanInterviewRecord) -> Result<()> {
    let questions = serde_json::to_string(&record.questions)?;
    let mut answers = record.answers.clone();
    answers.resize(record.questions.len(), None);
    let answers = serde_json::to_string(&answers)?;

    conn.execute(
        "INSERT INTO plan_interviews
            (feature_id, stage, feature_name, brief, questions, answers, plan,
             ai_rounds_completed, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, datetime('now'), datetime('now'))
         ON CONFLICT(feature_id, stage) DO UPDATE SET
            feature_name        = excluded.feature_name,
            brief               = excluded.brief,
            questions           = excluded.questions,
            answers             = excluded.answers,
            plan                = excluded.plan,
            ai_rounds_completed = excluded.ai_rounds_completed,
            updated_at          = datetime('now')",
        params![
            record.feature_id,
            record.stage.as_db_str(),
            record.feature_name,
            record.brief,
            questions,
            answers,
            record.plan,
            record.ai_rounds_completed as i64,
        ],
    )?;
    Ok(())
}

/// Promote a feature's draft to its accepted transcript, attaching `plan` as
/// the plan that was accepted. Returns `false` when there is no draft to
/// promote, leaving any existing final transcript untouched.
///
/// Runs as one transaction: an interrupted accept must not be able to drop the
/// draft without having written the transcript that replaces it.
pub fn finalize_draft(conn: &Connection, feature_id: &str, plan: &str) -> Result<bool> {
    let Some(mut draft) = load(conn, feature_id, PlanInterviewStage::Draft)? else {
        return Ok(false);
    };
    draft.stage = PlanInterviewStage::Final;
    draft.plan = Some(plan.to_string());

    conn.execute_batch("BEGIN IMMEDIATE;")?;
    match do_finalize_draft(conn, feature_id, &draft) {
        Ok(()) => {
            conn.execute_batch("COMMIT;")?;
            Ok(true)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK;");
            Err(e)
        }
    }
}

fn do_finalize_draft(
    conn: &Connection,
    feature_id: &str,
    finalized: &PlanInterviewRecord,
) -> Result<()> {
    save(conn, finalized)?;
    delete(conn, feature_id, PlanInterviewStage::Draft)
}

/// Delete one stage of a feature's interview — the "discard draft" action.
pub fn delete(conn: &Connection, feature_id: &str, stage: PlanInterviewStage) -> Result<()> {
    conn.execute(
        "DELETE FROM plan_interviews WHERE feature_id = ?1 AND stage = ?2",
        params![feature_id, stage.as_db_str()],
    )?;
    Ok(())
}

/// Delete both stages for a feature. Called when the feature is deleted, since
/// there is no FK cascade from `features`.
pub fn delete_for_feature(conn: &Connection, feature_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM plan_interviews WHERE feature_id = ?1",
        params![feature_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::AmfDb;
    use crate::plan_interview::{PlanQuestionKind, QuestionSource};
    use tempfile::NamedTempFile;

    fn open_temp_db() -> (NamedTempFile, AmfDb) {
        let tmp = NamedTempFile::new().unwrap();
        let db = AmfDb::open(tmp.path()).unwrap();
        (tmp, db)
    }

    fn question(id: &str, text: &str) -> PlanQuestion {
        PlanQuestion {
            id: id.into(),
            text: text.into(),
            kind: PlanQuestionKind::FreeText,
            source: QuestionSource::Builtin,
            optional: true,
        }
    }

    fn draft(feature_id: &str) -> PlanInterviewRecord {
        PlanInterviewRecord {
            feature_id: feature_id.into(),
            stage: PlanInterviewStage::Draft,
            feature_name: "guided-plans".into(),
            brief: "Collect a brief before launch.".into(),
            questions: vec![
                question("scope", "What is in scope?"),
                PlanQuestion {
                    id: "surface".into(),
                    text: "Which surface?".into(),
                    kind: PlanQuestionKind::Select(vec!["TUI".into(), "CLI".into()]),
                    source: QuestionSource::Template,
                    optional: false,
                },
                PlanQuestion {
                    id: "risk".into(),
                    text: "Biggest risk?".into(),
                    kind: PlanQuestionKind::FreeText,
                    source: QuestionSource::Ai { round: 1 },
                    optional: true,
                },
            ],
            answers: vec![Some("Native TUI only.".into()), Some("TUI".into()), None],
            plan: None,
            ai_rounds_completed: 1,
            ..Default::default()
        }
    }

    #[test]
    fn missing_interview_loads_as_none() {
        let (_tmp, db) = open_temp_db();
        assert!(db.plan_interview_draft("feat-1").unwrap().is_none());
        assert!(db.plan_interview_final("feat-1").unwrap().is_none());
    }

    #[test]
    fn draft_round_trips_every_question_shape() {
        let (_tmp, db) = open_temp_db();
        let record = draft("feat-1");
        db.save_plan_interview(&record).unwrap();

        let loaded = db.plan_interview_draft("feat-1").unwrap().unwrap();
        // Select options, template/AI provenance, and the round an AI question
        // came from all have to survive the JSON column.
        assert_eq!(loaded.questions, record.questions);
        assert_eq!(loaded.answers, record.answers);
        assert_eq!(loaded.brief, record.brief);
        assert_eq!(loaded.feature_name, "guided-plans");
        assert_eq!(loaded.ai_rounds_completed, 1);
        assert!(loaded.plan.is_none());
        assert!(!loaded.created_at.is_empty());
    }

    #[test]
    fn saving_again_overwrites_the_same_draft() {
        let (_tmp, db) = open_temp_db();
        db.save_plan_interview(&draft("feat-1")).unwrap();

        let mut updated = draft("feat-1");
        updated.answers[2] = Some("Concurrency.".into());
        db.save_plan_interview(&updated).unwrap();

        let loaded = db.plan_interview_draft("feat-1").unwrap().unwrap();
        assert_eq!(loaded.answers[2].as_deref(), Some("Concurrency."));
        // Still one draft, not two.
        assert_eq!(db.count_plan_interviews("feat-1").unwrap(), 1);
    }

    #[test]
    fn draft_and_final_coexist_for_one_feature() {
        let (_tmp, db) = open_temp_db();
        let mut accepted = draft("feat-1");
        accepted.stage = PlanInterviewStage::Final;
        accepted.plan = Some("# Plan: guided-plans\n".into());
        db.save_plan_interview(&accepted).unwrap();

        // Re-running the interview saves a fresh draft; the accepted transcript
        // it is revising must survive that.
        let mut rerun = draft("feat-1");
        rerun.brief = "Second pass.".into();
        db.save_plan_interview(&rerun).unwrap();

        let final_record = db.plan_interview_final("feat-1").unwrap().unwrap();
        assert_eq!(final_record.brief, "Collect a brief before launch.");
        assert_eq!(final_record.plan.as_deref(), Some("# Plan: guided-plans\n"));
        assert_eq!(
            db.plan_interview_draft("feat-1").unwrap().unwrap().brief,
            "Second pass."
        );
    }

    #[test]
    fn finalizing_promotes_the_draft_and_leaves_none_behind() {
        let (_tmp, db) = open_temp_db();
        db.save_plan_interview(&draft("feat-1")).unwrap();

        assert!(
            db.finalize_plan_interview_draft("feat-1", "# Plan: guided-plans\n")
                .unwrap()
        );

        assert!(db.plan_interview_draft("feat-1").unwrap().is_none());
        let final_record = db.plan_interview_final("feat-1").unwrap().unwrap();
        assert_eq!(final_record.plan.as_deref(), Some("# Plan: guided-plans\n"));
        assert_eq!(final_record.answers[0].as_deref(), Some("Native TUI only."));
    }

    #[test]
    fn finalizing_without_a_draft_keeps_the_existing_transcript() {
        let (_tmp, db) = open_temp_db();
        let mut accepted = draft("feat-1");
        accepted.stage = PlanInterviewStage::Final;
        accepted.plan = Some("# Original\n".into());
        db.save_plan_interview(&accepted).unwrap();

        // An accept with nothing staged must not blank the plan already stored.
        assert!(
            !db.finalize_plan_interview_draft("feat-1", "# Replacement\n")
                .unwrap()
        );
        assert_eq!(
            db.plan_interview_final("feat-1")
                .unwrap()
                .unwrap()
                .plan
                .as_deref(),
            Some("# Original\n")
        );
    }

    #[test]
    fn answers_are_looked_up_by_stable_question_id() {
        let (_tmp, db) = open_temp_db();
        db.save_plan_interview(&draft("feat-1")).unwrap();
        let loaded = db.plan_interview_draft("feat-1").unwrap().unwrap();

        assert_eq!(loaded.answer_for("scope"), Some("Native TUI only."));
        // Skipped, never-asked, and blank answers are all "no answer" to a
        // re-run pre-fill.
        assert_eq!(loaded.answer_for("risk"), None);
        assert_eq!(loaded.answer_for("nonexistent"), None);
    }

    #[test]
    fn blank_answers_do_not_pre_fill() {
        let (_tmp, db) = open_temp_db();
        let mut record = draft("feat-1");
        record.answers[0] = Some("   ".into());
        db.save_plan_interview(&record).unwrap();

        let loaded = db.plan_interview_draft("feat-1").unwrap().unwrap();
        assert_eq!(loaded.answer_for("scope"), None);
    }

    #[test]
    fn short_answer_vectors_are_padded_to_the_question_count() {
        let (_tmp, db) = open_temp_db();
        let mut record = draft("feat-1");
        record.answers = vec![Some("Native TUI only.".into())];
        db.save_plan_interview(&record).unwrap();

        let loaded = db.plan_interview_draft("feat-1").unwrap().unwrap();
        // Readers index answers by question position, so the pair is squared up
        // rather than handed back ragged.
        assert_eq!(loaded.answers.len(), loaded.questions.len());
        assert_eq!(loaded.answers[1], None);
    }

    #[test]
    fn discarding_a_draft_keeps_the_accepted_transcript() {
        let (_tmp, db) = open_temp_db();
        let mut accepted = draft("feat-1");
        accepted.stage = PlanInterviewStage::Final;
        accepted.plan = Some("# Plan\n".into());
        db.save_plan_interview(&accepted).unwrap();
        db.save_plan_interview(&draft("feat-1")).unwrap();

        db.delete_plan_interview_draft("feat-1").unwrap();

        assert!(db.plan_interview_draft("feat-1").unwrap().is_none());
        assert!(db.plan_interview_final("feat-1").unwrap().is_some());
    }

    #[test]
    fn deleting_a_feature_removes_both_stages_and_only_that_feature() {
        let (_tmp, db) = open_temp_db();
        db.save_plan_interview(&draft("feat-1")).unwrap();
        let mut accepted = draft("feat-1");
        accepted.stage = PlanInterviewStage::Final;
        db.save_plan_interview(&accepted).unwrap();
        db.save_plan_interview(&draft("feat-2")).unwrap();

        db.delete_plan_interviews_for_feature("feat-1").unwrap();

        assert_eq!(db.count_plan_interviews("feat-1").unwrap(), 0);
        assert_eq!(db.count_plan_interviews("feat-2").unwrap(), 1);
    }

    /// Interview rows must outlive an ordinary store save, which full-replaces
    /// `projects`/`features`. That is why `feature_id` carries no FK.
    #[test]
    fn interviews_survive_a_full_store_save() {
        let (_tmp, db) = open_temp_db();
        db.save_plan_interview(&draft("feat-1")).unwrap();

        let store = db.load_store().unwrap();
        db.save_store(&store).unwrap();

        assert!(db.plan_interview_draft("feat-1").unwrap().is_some());
    }
}
