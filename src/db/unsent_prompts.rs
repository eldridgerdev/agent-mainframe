//! SQLite persistence for prompts AMF computed to seed a session but could
//! not deliver because the launch that would have carried them failed.
//!
//! Deliberately generic: any launch primitive that seeds a composer can call
//! [`crate::app::App::stash_lost_prompt`] from its failure arm instead of
//! discarding the prompt. Rows are scoped by `workdir` (not a feature id, and
//! not a session), matching how `Latest Prompt` recall
//! (`App::open_latest_prompt_from_view`) already resolves a session's
//! transcript history — a stashed prompt surfaces there once a session
//! exists in that checkout to view it.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnsentPrompt {
    pub id: String,
    pub label: String,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

pub fn insert(
    conn: &Connection,
    id: &str,
    workdir: &Path,
    label: &str,
    body: &str,
    created_at: &DateTime<Utc>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO unsent_prompts (id, workdir, label, body, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            id,
            workdir.to_string_lossy(),
            label,
            body,
            created_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

/// Oldest first, matching the order a queue of missed launches should be
/// worked through.
pub fn load_for_workdir(conn: &Connection, workdir: &Path) -> Result<Vec<UnsentPrompt>> {
    let mut stmt = conn.prepare(
        "SELECT id, label, body, created_at FROM unsent_prompts
         WHERE workdir = ?1 ORDER BY created_at ASC",
    )?;
    let rows = stmt
        .query_map(params![workdir.to_string_lossy()], |row| {
            let created_at: String = row.get(3)?;
            Ok(UnsentPrompt {
                id: row.get(0)?,
                label: row.get(1)?,
                body: row.get(2)?,
                created_at: created_at.parse().unwrap_or_else(|_| Utc::now()),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn delete(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM unsent_prompts WHERE id = ?1", params![id])?;
    Ok(())
}

/// Drops every row filed under `workdir`, so a deleted feature's stashed
/// prompts cannot resurface under an unrelated later feature that reuses the
/// same checkout path.
pub fn delete_for_workdir(conn: &Connection, workdir: &Path) -> Result<()> {
    conn.execute(
        "DELETE FROM unsent_prompts WHERE workdir = ?1",
        params![workdir.to_string_lossy()],
    )?;
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
    fn insert_then_load_roundtrips_by_workdir() {
        let (_tmp, db) = open_temp_db();
        let workdir = Path::new("/repo/worktrees/feat-a");
        let now = Utc::now();

        db.insert_unsent_prompt("p1", workdir, "TODO: Fix bug", "Fix the bug", &now)
            .unwrap();

        let loaded = db.load_unsent_prompts_for_workdir(workdir).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "p1");
        assert_eq!(loaded[0].label, "TODO: Fix bug");
        assert_eq!(loaded[0].body, "Fix the bug");
    }

    #[test]
    fn load_only_returns_matching_workdir() {
        let (_tmp, db) = open_temp_db();
        let now = Utc::now();
        db.insert_unsent_prompt("p1", Path::new("/repo/a"), "A", "body a", &now)
            .unwrap();
        db.insert_unsent_prompt("p2", Path::new("/repo/b"), "B", "body b", &now)
            .unwrap();

        let loaded = db
            .load_unsent_prompts_for_workdir(Path::new("/repo/a"))
            .unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "p1");
    }

    #[test]
    fn delete_removes_only_that_row() {
        let (_tmp, db) = open_temp_db();
        let workdir = Path::new("/repo/a");
        let now = Utc::now();
        db.insert_unsent_prompt("p1", workdir, "A", "body a", &now)
            .unwrap();
        db.insert_unsent_prompt("p2", workdir, "B", "body b", &now)
            .unwrap();

        db.delete_unsent_prompt("p1").unwrap();

        let loaded = db.load_unsent_prompts_for_workdir(workdir).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "p2");
    }

    #[test]
    fn load_orders_oldest_first() {
        let (_tmp, db) = open_temp_db();
        let workdir = Path::new("/repo/a");
        let earlier = Utc::now() - chrono::Duration::seconds(60);
        let later = Utc::now();
        db.insert_unsent_prompt("later", workdir, "L", "later body", &later)
            .unwrap();
        db.insert_unsent_prompt("earlier", workdir, "E", "earlier body", &earlier)
            .unwrap();

        let loaded = db.load_unsent_prompts_for_workdir(workdir).unwrap();
        assert_eq!(loaded[0].id, "earlier");
        assert_eq!(loaded[1].id, "later");
    }

    #[test]
    fn delete_for_workdir_removes_only_that_workdirs_rows() {
        let (_tmp, db) = open_temp_db();
        let now = Utc::now();
        db.insert_unsent_prompt("p1", Path::new("/repo/a"), "A", "body a", &now)
            .unwrap();
        db.insert_unsent_prompt("p2", Path::new("/repo/a"), "A2", "body a2", &now)
            .unwrap();
        db.insert_unsent_prompt("p3", Path::new("/repo/b"), "B", "body b", &now)
            .unwrap();

        db.delete_unsent_prompts_for_workdir(Path::new("/repo/a"))
            .unwrap();

        assert!(
            db.load_unsent_prompts_for_workdir(Path::new("/repo/a"))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            db.load_unsent_prompts_for_workdir(Path::new("/repo/b"))
                .unwrap()
                .len(),
            1
        );
    }
}
