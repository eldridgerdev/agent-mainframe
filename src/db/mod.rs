mod debug_log;
mod migrations;
mod pr_review_cache;
mod session_status;
pub mod store;
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

    pub fn evict_stale_pr_review_cache(&self) -> Result<()> {
        pr_review_cache::evict_stale(&self.conn)
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
            if let Ok(store) = crate::project::ProjectStore::load(&json_path) {
                if let Ok(db) = AmfDb::open(db_path) {
                    let _ = db.save_store(&store);
                }
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
