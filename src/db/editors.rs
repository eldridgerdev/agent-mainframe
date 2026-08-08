//! SQLite persistence for editors AMF launched on a feature's behalf.
//!
//! A record exists so that stopping a feature can reclaim the editor window and
//! the language servers under it — routinely the largest memory consumers on
//! the machine, and invisible to tmux. Records outlive AMF restarts, so a
//! stored PID is never trusted on its own: `command` is kept alongside it as an
//! identity check against PID recycling (see `MIGRATION_017`).
//!
//! Like `todos`, these rows live outside the `ProjectStore` JSON and its
//! full-replace save path, so `feature_id` carries no foreign key and deletion
//! is explicit.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use std::path::PathBuf;
use uuid::Uuid;

/// Which editor a record describes. Unknown tokens read back as
/// [`EditorKind::Other`] rather than failing the load.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorKind {
    Vscode,
    Other,
}

impl EditorKind {
    pub fn as_db_str(self) -> &'static str {
        match self {
            EditorKind::Vscode => "vscode",
            EditorKind::Other => "other",
        }
    }

    pub fn from_db_str(s: &str) -> Self {
        match s {
            "vscode" => EditorKind::Vscode,
            _ => EditorKind::Other,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            EditorKind::Vscode => "VS Code",
            EditorKind::Other => "editor",
        }
    }
}

/// One editor process AMF started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchedEditor {
    pub id: String,
    pub feature_id: String,
    /// The `FeatureSession` the launch came from, when there was one.
    pub session_id: Option<String>,
    pub kind: EditorKind,
    pub pid: i64,
    pub worktree_path: PathBuf,
    /// True only when AMF opened a window it owns and may therefore kill.
    pub dedicated: bool,
    /// The argv AMF spawned, used to prove a live PID is still this process.
    pub command: String,
    pub started_at: DateTime<Utc>,
}

/// Record a launch. A feature can accumulate several (one per editor window),
/// so this always inserts.
#[allow(clippy::too_many_arguments)]
pub fn record_launch(
    conn: &Connection,
    feature_id: &str,
    session_id: Option<&str>,
    kind: EditorKind,
    pid: i64,
    worktree_path: &std::path::Path,
    dedicated: bool,
    command: &str,
) -> Result<LaunchedEditor> {
    let editor = LaunchedEditor {
        id: Uuid::new_v4().to_string(),
        feature_id: feature_id.to_string(),
        session_id: session_id.map(str::to_string),
        kind,
        pid,
        worktree_path: worktree_path.to_path_buf(),
        dedicated,
        command: command.to_string(),
        started_at: Utc::now(),
    };

    conn.execute(
        "INSERT INTO launched_editors
            (id, feature_id, session_id, kind, pid, worktree_path, dedicated, command, started_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            editor.id,
            editor.feature_id,
            editor.session_id,
            editor.kind.as_db_str(),
            editor.pid,
            editor.worktree_path.to_string_lossy(),
            editor.dedicated as i64,
            editor.command,
            editor.started_at.to_rfc3339(),
        ],
    )?;

    Ok(editor)
}

/// Editors recorded for one feature, oldest launch first.
pub fn list_for_feature(conn: &Connection, feature_id: &str) -> Result<Vec<LaunchedEditor>> {
    let mut stmt = conn.prepare(
        "SELECT id, feature_id, session_id, kind, pid, worktree_path, dedicated, command, started_at
         FROM launched_editors
         WHERE feature_id = ?1
         ORDER BY started_at",
    )?;
    let rows = stmt.query_map(params![feature_id], row_to_editor)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Every recorded editor, oldest launch first — the `amf doctor` view.
pub fn list_all(conn: &Connection) -> Result<Vec<LaunchedEditor>> {
    let mut stmt = conn.prepare(
        "SELECT id, feature_id, session_id, kind, pid, worktree_path, dedicated, command, started_at
         FROM launched_editors
         ORDER BY started_at",
    )?;
    let rows = stmt.query_map([], row_to_editor)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Attach the resolved owner process to a record.
///
/// Ownership is settled after the launch, not during it: the `code` CLI hands
/// off and exits, so the window process only appears seconds later — and may
/// never appear as a local process at all (a remote/WSL window lives on the
/// other side). Until this runs, the record stands as not-owned.
pub fn set_owner(conn: &Connection, id: &str, pid: i64, dedicated: bool) -> Result<()> {
    conn.execute(
        "UPDATE launched_editors SET pid = ?2, dedicated = ?3 WHERE id = ?1",
        params![id, pid, dedicated as i64],
    )?;
    Ok(())
}

/// Forget one record — after the process is killed, or once it is found gone.
pub fn delete(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM launched_editors WHERE id = ?1", params![id])?;
    Ok(())
}

/// Forget every record for a feature (feature deleted, or all editors closed).
pub fn delete_for_feature(conn: &Connection, feature_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM launched_editors WHERE feature_id = ?1",
        params![feature_id],
    )?;
    Ok(())
}

fn row_to_editor(row: &rusqlite::Row<'_>) -> rusqlite::Result<LaunchedEditor> {
    let kind: String = row.get(3)?;
    let worktree_path: String = row.get(5)?;
    let dedicated: i64 = row.get(6)?;
    let started_at: String = row.get(8)?;
    Ok(LaunchedEditor {
        id: row.get(0)?,
        feature_id: row.get(1)?,
        session_id: row.get(2)?,
        kind: EditorKind::from_db_str(&kind),
        pid: row.get(4)?,
        worktree_path: PathBuf::from(worktree_path),
        dedicated: dedicated != 0,
        command: row.get(7)?,
        started_at: DateTime::parse_from_rfc3339(&started_at)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::AmfDb;
    use std::path::Path;
    use tempfile::NamedTempFile;

    fn open_temp_db() -> (NamedTempFile, AmfDb) {
        let tmp = NamedTempFile::new().unwrap();
        let db = AmfDb::open(tmp.path()).unwrap();
        (tmp, db)
    }

    #[test]
    fn records_and_lists_launches_per_feature() {
        let (_tmp, db) = open_temp_db();
        assert!(
            db.launched_editors_for_feature("feat-1")
                .unwrap()
                .is_empty()
        );

        let recorded = db
            .record_launched_editor(
                "feat-1",
                Some("sess-1"),
                EditorKind::Vscode,
                4242,
                Path::new("/tmp/wt"),
                true,
                "code --new-window /tmp/wt",
            )
            .unwrap();

        let loaded = db.launched_editors_for_feature("feat-1").unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0], recorded);
        assert_eq!(loaded[0].pid, 4242);
        assert!(loaded[0].dedicated);
        assert_eq!(loaded[0].kind, EditorKind::Vscode);
        assert_eq!(loaded[0].worktree_path, PathBuf::from("/tmp/wt"));

        // Another feature's launches stay separate.
        assert!(
            db.launched_editors_for_feature("feat-2")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_feature_can_hold_several_editor_records() {
        let (_tmp, db) = open_temp_db();
        for pid in [1, 2, 3] {
            db.record_launched_editor(
                "feat-1",
                None,
                EditorKind::Vscode,
                pid,
                Path::new("/tmp/wt"),
                true,
                "code --new-window /tmp/wt",
            )
            .unwrap();
        }
        assert_eq!(db.launched_editors_for_feature("feat-1").unwrap().len(), 3);
        assert_eq!(db.all_launched_editors().unwrap().len(), 3);
    }

    #[test]
    fn non_dedicated_launches_round_trip_as_not_owned() {
        let (_tmp, db) = open_temp_db();
        db.record_launched_editor(
            "feat-1",
            None,
            EditorKind::Other,
            77,
            Path::new("/tmp/wt"),
            false,
            "nvim /tmp/wt",
        )
        .unwrap();

        let loaded = db.launched_editors_for_feature("feat-1").unwrap();
        assert!(
            !loaded[0].dedicated,
            "reused instances are never AMF's to kill"
        );
        assert_eq!(loaded[0].kind, EditorKind::Other);
        assert!(loaded[0].session_id.is_none());
    }

    #[test]
    fn deleting_one_record_leaves_the_others() {
        let (_tmp, db) = open_temp_db();
        let first = db
            .record_launched_editor(
                "feat-1",
                None,
                EditorKind::Vscode,
                1,
                Path::new("/tmp/wt"),
                true,
                "code",
            )
            .unwrap();
        db.record_launched_editor(
            "feat-1",
            None,
            EditorKind::Vscode,
            2,
            Path::new("/tmp/wt"),
            true,
            "code",
        )
        .unwrap();

        db.delete_launched_editor(&first.id).unwrap();

        let left = db.launched_editors_for_feature("feat-1").unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].pid, 2);
    }

    #[test]
    fn deleting_a_feature_forgets_its_editors() {
        let (_tmp, db) = open_temp_db();
        db.record_launched_editor(
            "feat-1",
            None,
            EditorKind::Vscode,
            1,
            Path::new("/tmp/wt"),
            true,
            "code",
        )
        .unwrap();
        db.record_launched_editor(
            "feat-2",
            None,
            EditorKind::Vscode,
            2,
            Path::new("/tmp/wt2"),
            true,
            "code",
        )
        .unwrap();

        db.delete_launched_editors_for_feature("feat-1").unwrap();

        assert!(
            db.launched_editors_for_feature("feat-1")
                .unwrap()
                .is_empty()
        );
        assert_eq!(db.launched_editors_for_feature("feat-2").unwrap().len(), 1);
    }

    #[test]
    fn unknown_editor_kinds_degrade_to_other() {
        assert_eq!(EditorKind::from_db_str("emacs"), EditorKind::Other);
        assert_eq!(EditorKind::from_db_str("vscode"), EditorKind::Vscode);
    }
}
