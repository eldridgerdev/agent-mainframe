mod ai_review_cache;
mod debug_log;
pub mod learning;
mod migrations;
pub mod plan_interviews;
mod pr_comment_triage;
mod pr_review_cache;
mod session_status;
pub mod store;
pub mod todos;
mod token_cache;

use anyhow::Result;
use rusqlite::Connection;
use std::env;
use std::path::{Path, PathBuf};

use crate::worktree::WorktreeManager;

pub struct AmfDb {
    conn: Connection,
    pub path: PathBuf,
}

impl AmfDb {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        migrations::run(&conn)?;
        Ok(Self {
            conn,
            path: path.to_path_buf(),
        })
    }

    /// Open `path`, seeding or merging legacy stores first if needed.
    ///
    /// Merge priority:
    /// 1. Import a legacy worktree-local `amf.db`.
    /// 2. Import a legacy worktree-local `projects.json`.
    /// 3. Import from the global `~/.config/amf/projects.json`.
    pub fn open_or_seed(path: &Path, _global_path: &Path) -> Result<Self> {
        let legacy_store = load_legacy_worktree_store();

        if !path.exists() {
            if let Some(store) = legacy_store {
                let db = AmfDb::open(path)?;
                db.save_store(&store)?;
                return Ok(db);
            }

            seed_from_json(path, Vec::new());
            return Self::open(path);
        }

        if let Some(store) = legacy_store {
            let db = Self::open(path)?;
            let mut merged = db.load_store()?;
            merged.merge_from(store);
            db.save_store(&merged)?;
        }

        Self::open(path)
    }

    pub fn load_store(&self) -> Result<crate::project::ProjectStore> {
        store::load(&self.conn)
    }

    pub fn save_store(&self, store: &crate::project::ProjectStore) -> Result<()> {
        store::save(&self.conn, store)
    }

    pub fn load_token_cache(&self) -> Result<Vec<crate::token_tracking::DbTokenCacheEntry>> {
        token_cache::load(&self.conn)
    }

    pub fn save_token_cache(
        &self,
        entries: &[crate::token_tracking::DbTokenCacheEntry],
    ) -> Result<()> {
        token_cache::save(&self.conn, entries)
    }

    pub fn evict_stale_token_cache(&self) -> Result<()> {
        token_cache::evict_stale(&self.conn)
    }

    /// Cached normalized PR review for `(pr_number, head_sha)`, or `None` on miss.
    pub fn load_pr_review_cache(
        &self,
        pr_number: u32,
        head_sha: &str,
    ) -> Result<Option<crate::app::pr_review::PrReview>> {
        pr_review_cache::load(&self.conn, pr_number, head_sha)
    }

    pub fn save_pr_review_cache(&self, review: &crate::app::pr_review::PrReview) -> Result<()> {
        pr_review_cache::save(&self.conn, review)
    }

    pub fn delete_pr_review_cache(&self, pr_number: u32, head_sha: &str) -> Result<()> {
        pr_review_cache::delete(&self.conn, pr_number, head_sha)
    }

    pub fn evict_stale_pr_review_cache(&self) -> Result<()> {
        pr_review_cache::evict_stale(&self.conn)
    }

    /// Local triage rows for `pr_number` as `comment_id -> (state, note)`,
    /// across every head SHA (triage survives a push).
    pub fn load_pr_comment_triage(
        &self,
        pr_number: u32,
    ) -> Result<std::collections::HashMap<u64, pr_comment_triage::TriageRow>> {
        pr_comment_triage::load(&self.conn, pr_number)
    }

    pub fn save_pr_comment_triage(
        &self,
        pr_number: u32,
        head_sha: &str,
        comment_id: u64,
        state: crate::app::pr_review::TriageState,
        note: Option<&str>,
    ) -> Result<()> {
        pr_comment_triage::upsert(&self.conn, pr_number, head_sha, comment_id, state, note)
    }

    pub fn begin_pr_comment_reply_draft(
        &self,
        pr_number: u32,
        comment_id: u64,
        request_id: &str,
        base_head_sha: &str,
    ) -> Result<()> {
        pr_comment_triage::begin_reply_draft(
            &self.conn,
            pr_number,
            comment_id,
            request_id,
            base_head_sha,
        )
    }

    pub fn capture_pr_comment_reply_draft(
        &self,
        pr_number: u32,
        comment_id: u64,
        request_id: &str,
        body: &str,
    ) -> Result<bool> {
        pr_comment_triage::capture_reply_draft(&self.conn, pr_number, comment_id, request_id, body)
    }

    #[cfg(test)]
    pub fn load_pr_comment_reply_draft(
        &self,
        pr_number: u32,
        comment_id: u64,
    ) -> Result<Option<String>> {
        Ok(
            pr_comment_triage::load_reply_draft(&self.conn, pr_number, comment_id)?
                .map(|(body, _)| body),
        )
    }

    pub fn load_pr_comment_reply_draft_with_base(
        &self,
        pr_number: u32,
        comment_id: u64,
    ) -> Result<Option<(String, String)>> {
        pr_comment_triage::load_reply_draft(&self.conn, pr_number, comment_id)
    }

    pub fn clear_pr_comment_reply_draft(&self, pr_number: u32, comment_id: u64) -> Result<()> {
        pr_comment_triage::clear_reply_draft(&self.conn, pr_number, comment_id)
    }

    pub fn evict_stale_pr_comment_triage(&self) -> Result<()> {
        pr_comment_triage::evict_stale(&self.conn)
    }

    /// Cached AI-review findings for `(pr_number, head_sha)`, or `None` on miss.
    pub fn load_ai_review_cache(
        &self,
        pr_number: u32,
        head_sha: &str,
    ) -> Result<Option<crate::app::ai_review::AiReviewCacheEntry>> {
        ai_review_cache::load(&self.conn, pr_number, head_sha)
    }

    pub fn save_ai_review_cache(
        &self,
        pr_number: u32,
        head_sha: &str,
        entry: &crate::app::ai_review::AiReviewCacheEntry,
    ) -> Result<()> {
        ai_review_cache::save(&self.conn, pr_number, head_sha, entry)
    }

    pub fn evict_stale_ai_review_cache(&self) -> Result<()> {
        ai_review_cache::evict_stale(&self.conn)
    }

    pub fn append_log_entry(&self, entry: &crate::debug::LogEntry) -> Result<()> {
        debug_log::append(&self.conn, entry)
    }

    pub fn load_recent_log(&self, limit: usize) -> Result<Vec<crate::debug::LogEntry>> {
        debug_log::load_recent(&self.conn, limit)
    }

    #[allow(dead_code)] // exercised only by unit tests
    pub fn load_session_status(&self, session_id: &str) -> Result<Option<String>> {
        session_status::load(&self.conn, session_id)
    }

    /// Returns `(status_text, file_mtime_nanos)` for the cached entry.
    /// `file_mtime_nanos` is `None` when the row was written without a mtime
    /// (e.g. from an IPC push or the old schema).
    pub fn load_session_status_with_mtime(
        &self,
        session_id: &str,
    ) -> Result<Option<(String, Option<u64>)>> {
        session_status::load_with_mtime(&self.conn, session_id)
    }

    pub fn upsert_session_status(
        &self,
        session_id: &str,
        feature_id: &str,
        status_text: &str,
        file_mtime_nanos: Option<u64>,
    ) -> Result<()> {
        session_status::upsert(
            &self.conn,
            session_id,
            feature_id,
            status_text,
            file_mtime_nanos,
        )
    }

    pub fn delete_session_status(&self, session_id: &str) -> Result<()> {
        session_status::delete_session(&self.conn, session_id)
    }

    pub fn delete_feature_statuses(&self, feature_id: &str) -> Result<()> {
        session_status::delete_feature(&self.conn, feature_id)
    }
}

/// Per-project TODO list persistence. Wired into the UI in later epics (see
/// `docs/backlog/feature-todos-plan.md`), so unused until then.
#[allow(dead_code)]
impl AmfDb {
    /// The project's TODO list, or `None` if it has no list yet.
    pub fn todo_list(&self, project_id: &str) -> Result<Option<todos::TodoList>> {
        todos::load_list(&self.conn, project_id)
    }

    /// Return the project's TODO list, creating one hosted by `feature_id` if
    /// none exists.
    pub fn load_or_create_todo_list(
        &self,
        project_id: &str,
        feature_id: &str,
    ) -> Result<todos::TodoList> {
        todos::load_or_create_list(&self.conn, project_id, feature_id)
    }

    /// Create the project's TODO list under `feature_id`; errors if one exists.
    pub fn create_todo_list(&self, project_id: &str, feature_id: &str) -> Result<todos::TodoList> {
        todos::create_list(&self.conn, project_id, feature_id)
    }

    pub fn set_todo_carry_over(&self, list_id: &str, carry_over: Option<&str>) -> Result<()> {
        todos::set_carry_over(&self.conn, list_id, carry_over)
    }

    pub fn set_todo_list_host_feature(&self, list_id: &str, feature_id: &str) -> Result<()> {
        todos::set_host_feature(&self.conn, list_id, feature_id)
    }

    pub fn delete_todo_list(&self, list_id: &str) -> Result<()> {
        todos::delete_list(&self.conn, list_id)
    }

    /// Delete the project's TODO list (and items) when the project is deleted.
    pub fn delete_todo_list_for_project(&self, project_id: &str) -> Result<()> {
        todos::delete_list_for_project(&self.conn, project_id)
    }

    pub fn todos(&self, list_id: &str) -> Result<Vec<todos::Todo>> {
        todos::list_todos(&self.conn, list_id)
    }

    pub fn add_todo(
        &self,
        list_id: &str,
        title: &str,
        body: Option<&str>,
        priority: todos::TodoPriority,
    ) -> Result<todos::Todo> {
        todos::add_todo(&self.conn, list_id, title, body, priority)
    }

    pub fn update_todo(&self, todo: &todos::Todo) -> Result<()> {
        todos::update_todo(&self.conn, todo)
    }

    pub fn delete_todo(&self, todo_id: &str) -> Result<()> {
        todos::delete_todo(&self.conn, todo_id)
    }

    pub fn reorder_todos(&self, ordered_ids: &[String]) -> Result<()> {
        todos::reorder_todos(&self.conn, ordered_ids)
    }
}

/// Learning Mode sessions and their anchored Q&A history (see
/// `docs/backlog/learning-mode-plan.md`). The overlay lands in later epics, so
/// parts of this are written before anything reads them.
#[allow(dead_code)]
impl AmfDb {
    /// The project's learning session, or `None` if it has never opened
    /// Learning Mode.
    pub fn learning_session(&self, project_id: &str) -> Result<Option<learning::LearningSession>> {
        learning::load_session(&self.conn, project_id)
    }

    /// Return the project's learning session, creating one hosted by
    /// `feature_id` if it has none.
    pub fn load_or_create_learning_session(
        &self,
        project_id: &str,
        feature_id: &str,
        title: &str,
        harness: &crate::project::AgentKind,
        level: learning::LearningLevel,
    ) -> Result<learning::LearningSession> {
        learning::load_or_create_session(&self.conn, project_id, feature_id, title, harness, level)
    }

    pub fn create_learning_session(
        &self,
        project_id: &str,
        feature_id: &str,
        title: &str,
        harness: &crate::project::AgentKind,
        level: learning::LearningLevel,
    ) -> Result<learning::LearningSession> {
        learning::create_session(&self.conn, project_id, feature_id, title, harness, level)
    }

    /// Persist a session's harness / level / onboarding flag.
    pub fn update_learning_session(&self, session: &learning::LearningSession) -> Result<()> {
        learning::update_session(&self.conn, session)
    }

    /// Persist the harness / level the user picked mid-session.
    pub fn set_learning_session_settings(
        &self,
        session_id: &str,
        harness: &crate::project::AgentKind,
        level: learning::LearningLevel,
    ) -> Result<()> {
        learning::set_session_settings(&self.conn, session_id, harness, level)
    }

    pub fn set_learning_onboarding_seen(&self, session_id: &str) -> Result<()> {
        learning::set_onboarding_seen(&self.conn, session_id)
    }

    pub fn delete_learning_session(&self, session_id: &str) -> Result<()> {
        learning::delete_session(&self.conn, session_id)
    }

    /// Drop a deleted project's learning history, since there is no FK cascade
    /// from `projects`.
    pub fn delete_learning_sessions_for_project(&self, project_id: &str) -> Result<()> {
        learning::delete_sessions_for_project(&self.conn, project_id)
    }

    pub fn learning_qa(&self, session_id: &str) -> Result<Vec<learning::LearningQa>> {
        learning::list_qa(&self.conn, session_id)
    }

    pub fn upsert_learning_qa(&self, qa: &learning::LearningQa) -> Result<()> {
        learning::upsert_qa(&self.conn, qa)
    }

    /// Delete one Q&A row; its follow-up thread cascades away with it.
    pub fn delete_learning_qa(&self, qa_id: &str) -> Result<()> {
        learning::delete_qa(&self.conn, qa_id)
    }
}

/// Plan-interview drafts and accepted transcripts. The re-run pre-fill that
/// reads accepted transcripts is still ahead (see
/// `docs/backlog/plan-mode-interview-plan.md`, Epic 5), so part of this is
/// written but not yet read.
#[allow(dead_code)]
impl AmfDb {
    /// The feature's in-progress interview, if it has one to resume.
    pub fn plan_interview_draft(
        &self,
        feature_id: &str,
    ) -> Result<Option<plan_interviews::PlanInterviewRecord>> {
        plan_interviews::load(
            &self.conn,
            feature_id,
            plan_interviews::PlanInterviewStage::Draft,
        )
    }

    /// The interview behind the feature's last accepted plan, used to pre-fill
    /// a re-run.
    pub fn plan_interview_final(
        &self,
        feature_id: &str,
    ) -> Result<Option<plan_interviews::PlanInterviewRecord>> {
        plan_interviews::load(
            &self.conn,
            feature_id,
            plan_interviews::PlanInterviewStage::Final,
        )
    }

    /// Insert or overwrite the interview at `record.stage`.
    pub fn save_plan_interview(&self, record: &plan_interviews::PlanInterviewRecord) -> Result<()> {
        plan_interviews::save(&self.conn, record)
    }

    /// Promote the draft filed under `draft_feature_id` to the accepted
    /// transcript of `final_feature_id`. `false` when there was no draft to
    /// promote. The keys differ only for a feature-creation interview, whose
    /// draft predates the feature id it is finalized under.
    pub fn finalize_plan_interview_draft(
        &self,
        draft_feature_id: &str,
        final_feature_id: &str,
        plan: &str,
    ) -> Result<bool> {
        plan_interviews::finalize_draft(&self.conn, draft_feature_id, final_feature_id, plan)
    }

    /// Discard the feature's in-progress interview, keeping any accepted
    /// transcript.
    pub fn delete_plan_interview_draft(&self, feature_id: &str) -> Result<()> {
        plan_interviews::delete(
            &self.conn,
            feature_id,
            plan_interviews::PlanInterviewStage::Draft,
        )
    }

    /// Drop both stages when the feature is deleted; there is no FK cascade.
    pub fn delete_plan_interviews_for_feature(&self, feature_id: &str) -> Result<()> {
        plan_interviews::delete_for_feature(&self.conn, feature_id)
    }

    #[cfg(test)]
    fn count_plan_interviews(&self, feature_id: &str) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM plan_interviews WHERE feature_id = ?1",
            rusqlite::params![feature_id],
            |row| row.get(0),
        )?)
    }
}

fn seed_from_json(db_path: &Path, extra_candidates: Vec<PathBuf>) {
    let mut json_candidates: Vec<PathBuf> = vec![
        db_path
            .parent()
            .map(|p| p.join("projects.json"))
            .unwrap_or_default(),
        dirs::config_dir()
            .unwrap_or_default()
            .join("amf")
            .join("projects.json"),
        dirs::config_dir()
            .unwrap_or_default()
            .join("claude-super-vibeless")
            .join("projects.json"),
    ];
    json_candidates.splice(0..0, extra_candidates);

    for json_path in json_candidates {
        if json_path.exists() {
            if let Ok(store) = crate::project::ProjectStore::load(&json_path)
                && let Ok(db) = AmfDb::open(db_path)
            {
                let _ = db.save_store(&store);
            }
            return;
        }
    }
}

fn load_legacy_worktree_store() -> Option<crate::project::ProjectStore> {
    let current_dir = env::current_dir().ok()?;
    if !WorktreeManager::is_worktree(&current_dir) {
        return None;
    }

    let root = WorktreeManager::repo_root(&current_dir).ok()?;
    let db_path = root.join(".amf").join("amf.db");
    let json_path = root.join(".amf").join("projects.json");

    let mut merged: Option<crate::project::ProjectStore> = None;

    if json_path.exists()
        && let Ok(store) = crate::project::ProjectStore::load(&json_path)
    {
        merged = Some(store);
    }

    if db_path.exists()
        && let Ok(source_db) = AmfDb::open(&db_path)
        && let Ok(store) = source_db.load_store()
    {
        match &mut merged {
            Some(existing) => existing.merge_from(store),
            None => merged = Some(store),
        }
    }

    merged
}
