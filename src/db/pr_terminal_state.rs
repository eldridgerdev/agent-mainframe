//! SQLite cache of a branch's *terminal* PR state — merged or closed —
//! keyed by `(repo, branch)`.
//!
//! Unlike `pr_review_cache` (keyed by head SHA, invalidated the moment the PR
//! moves), this is durable on purpose: merged and closed are outcomes a PR
//! never leaves, so a stored hit is never stale and is loaded once at
//! startup rather than re-derived from `gh` every launch.

use anyhow::Result;
use rusqlite::{Connection, params};
use std::collections::HashMap;

use crate::github::{TerminalPr, TerminalPrState};

/// Load every cached terminal PR, keyed by `(repo, branch)`. Called once at
/// startup; the sweep applies fresh results on top as they arrive.
pub fn load_all(conn: &Connection) -> Result<HashMap<(String, String), TerminalPr>> {
    let mut stmt =
        conn.prepare("SELECT repo, branch, pr_number, state, at FROM pr_terminal_state")?;
    let rows = stmt.query_map([], |row| {
        let repo: String = row.get(0)?;
        let branch: String = row.get(1)?;
        let number: i64 = row.get(2)?;
        let state: String = row.get(3)?;
        let at: String = row.get(4)?;
        Ok((repo, branch, number, state, at))
    })?;

    let mut out = HashMap::new();
    for row in rows {
        let (repo, branch, number, state, at) = row?;
        // A row whose `state` column doesn't parse (hand-edited DB, a future
        // downgrade) is dropped rather than surfaced as an error — the same
        // "corrupt cache is a miss" contract `pr_review_cache::load` uses.
        let Some(state) = TerminalPrState::from_gh(&state) else {
            continue;
        };
        let Ok(number) = u32::try_from(number) else {
            continue;
        };
        out.insert((repo, branch), TerminalPr { number, state, at });
    }
    Ok(out)
}

/// Upsert one branch's terminal PR state. Idempotent — re-saving the same
/// value is a no-op in effect, so the sweep can call this on every positive
/// lookup without checking whether the row already matches.
pub fn save(conn: &Connection, repo: &str, branch: &str, pr: &TerminalPr) -> Result<()> {
    let state = match pr.state {
        TerminalPrState::Merged => "MERGED",
        TerminalPrState::Closed => "CLOSED",
    };
    conn.execute(
        "INSERT OR REPLACE INTO pr_terminal_state (repo, branch, pr_number, state, at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![repo, branch, pr.number as i64, state, pr.at],
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

    fn merged(number: u32) -> TerminalPr {
        TerminalPr {
            number,
            state: TerminalPrState::Merged,
            at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn roundtrips_by_repo_and_branch() {
        let (_tmp, db) = open_temp_db();
        db.save_pr_terminal_state("/repo/a", "feat-x", &merged(42))
            .unwrap();

        let all = db.load_all_pr_terminal_state().unwrap();
        let pr = all
            .get(&("/repo/a".to_string(), "feat-x".to_string()))
            .unwrap();
        assert_eq!(pr.number, 42);
        assert_eq!(pr.state, TerminalPrState::Merged);
    }

    #[test]
    fn two_repos_can_reuse_the_same_branch_name_without_colliding() {
        let (_tmp, db) = open_temp_db();
        db.save_pr_terminal_state("/repo/a", "main", &merged(1))
            .unwrap();
        db.save_pr_terminal_state("/repo/b", "main", &merged(2))
            .unwrap();

        let all = db.load_all_pr_terminal_state().unwrap();
        assert_eq!(
            all.get(&("/repo/a".to_string(), "main".to_string()))
                .unwrap()
                .number,
            1
        );
        assert_eq!(
            all.get(&("/repo/b".to_string(), "main".to_string()))
                .unwrap()
                .number,
            2
        );
    }

    #[test]
    fn resave_overwrites_the_same_key() {
        let (_tmp, db) = open_temp_db();
        db.save_pr_terminal_state("/repo/a", "feat-x", &merged(1))
            .unwrap();
        let closed = TerminalPr {
            number: 1,
            state: TerminalPrState::Closed,
            at: "2026-03-01T00:00:00Z".to_string(),
        };
        db.save_pr_terminal_state("/repo/a", "feat-x", &closed)
            .unwrap();

        let all = db.load_all_pr_terminal_state().unwrap();
        let pr = all
            .get(&("/repo/a".to_string(), "feat-x".to_string()))
            .unwrap();
        assert_eq!(pr.state, TerminalPrState::Closed);
    }
}
