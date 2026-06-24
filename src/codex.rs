use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedCodexSession {
    pub id: String,
    pub rollout_path: PathBuf,
    pub updated_at: i64,
}

fn state_db_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".codex").join("state_5.sqlite"))
}

fn open_state_db(path: &Path) -> Option<Connection> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()
}

pub fn indexed_session(workdir: &Path, session_id: &str) -> Option<IndexedCodexSession> {
    indexed_session_from_db(&state_db_path()?, workdir, session_id)
}

pub fn indexed_session_in(
    home_dir: &Path,
    workdir: &Path,
    session_id: &str,
) -> Option<IndexedCodexSession> {
    indexed_session_from_db(
        &home_dir.join(".codex").join("state_5.sqlite"),
        workdir,
        session_id,
    )
}

fn indexed_session_from_db(
    db_path: &Path,
    workdir: &Path,
    session_id: &str,
) -> Option<IndexedCodexSession> {
    let conn = open_state_db(db_path)?;
    conn.query_row(
        "SELECT id, rollout_path, updated_at FROM threads WHERE id = ?1 AND cwd = ?2",
        params![session_id, workdir.to_string_lossy()],
        |row| {
            Ok(IndexedCodexSession {
                id: row.get(0)?,
                rollout_path: PathBuf::from(row.get::<_, String>(1)?),
                updated_at: row.get(2)?,
            })
        },
    )
    .optional()
    .ok()
    .flatten()
}

pub fn indexed_sessions_in(home_dir: &Path, workdir: &Path) -> Option<Vec<IndexedCodexSession>> {
    indexed_sessions_from_db(&home_dir.join(".codex").join("state_5.sqlite"), workdir)
}

fn indexed_sessions_from_db(db_path: &Path, workdir: &Path) -> Option<Vec<IndexedCodexSession>> {
    let conn = open_state_db(db_path)?;
    let mut stmt = conn
        .prepare(
            "SELECT id, rollout_path, updated_at FROM threads \
             WHERE cwd = ?1 AND archived = 0 ORDER BY updated_at DESC",
        )
        .ok()?;
    stmt.query_map(params![workdir.to_string_lossy()], |row| {
        Ok(IndexedCodexSession {
            id: row.get(0)?,
            rollout_path: PathBuf::from(row.get::<_, String>(1)?),
            updated_at: row.get(2)?,
        })
    })
    .ok()?
    .collect::<rusqlite::Result<Vec<_>>>()
    .ok()
}

pub struct CodexLauncher;

impl CodexLauncher {
    /// Check if codex CLI is available
    pub fn check_available() -> Result<()> {
        let output = Command::new("codex")
            .arg("--version")
            .output()
            .context("codex CLI not found - is Codex installed?")?;

        if !output.status.success() {
            anyhow::bail!("codex CLI returned an error");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_index(dir: &TempDir) -> PathBuf {
        let path = dir.path().join("state.sqlite");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                rollout_path TEXT NOT NULL,
                updated_at INTEGER NOT NULL,
                cwd TEXT NOT NULL,
                archived INTEGER NOT NULL DEFAULT 0
            );",
        )
        .unwrap();
        path
    }

    #[test]
    fn indexed_session_returns_exact_rollout_path() {
        let dir = TempDir::new().unwrap();
        let db = create_index(&dir);
        let workdir = dir.path().join("repo");
        let rollout = dir.path().join("rollout.jsonl");
        let conn = Connection::open(&db).unwrap();
        conn.execute(
            "INSERT INTO threads (id, rollout_path, updated_at, cwd) VALUES (?1, ?2, 42, ?3)",
            params![
                "session-1",
                rollout.to_string_lossy(),
                workdir.to_string_lossy()
            ],
        )
        .unwrap();

        let session = indexed_session_from_db(&db, &workdir, "session-1").unwrap();
        assert_eq!(session.rollout_path, rollout);
        assert_eq!(session.updated_at, 42);
        assert!(indexed_session_from_db(&db, &workdir, "other").is_none());
    }

    #[test]
    fn indexed_sessions_filters_workdir_and_archived_rows() {
        let dir = TempDir::new().unwrap();
        let db = create_index(&dir);
        let workdir = dir.path().join("repo");
        let conn = Connection::open(&db).unwrap();
        for (id, cwd, archived, updated) in [
            ("old", workdir.as_path(), 0, 1),
            ("new", workdir.as_path(), 0, 2),
            ("archived", workdir.as_path(), 1, 3),
            ("other", dir.path(), 0, 4),
        ] {
            conn.execute(
                "INSERT INTO threads (id, rollout_path, updated_at, cwd, archived) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    id,
                    dir.path().join(id).to_string_lossy(),
                    updated,
                    cwd.to_string_lossy(),
                    archived
                ],
            )
            .unwrap();
        }

        let sessions = indexed_sessions_from_db(&db, &workdir).unwrap();
        assert_eq!(
            sessions
                .iter()
                .map(|session| session.id.as_str())
                .collect::<Vec<_>>(),
            vec!["new", "old"]
        );
    }
}
