//! SQLite persistence for **feature-** and **global-**scope headless prompt
//! overrides (the "Editable headless prompts" feature).
//!
//! Precedence across the full set of layers — feature → project → global →
//! built-in default — lives in `src/prompts/`. This module owns only the two
//! layers that persist in `amf.db`:
//!
//! - **Feature** scope is keyed by the feature's **workdir path**, exactly
//!   like [`crate::db::todos::TodoScope::Worktree`]: the override belongs to
//!   the checkout, not to whichever feature row points at it, and it does not
//!   follow a feature across worktree recreation.
//! - **Global** scope has no key — one row per `(prompt_id, harness)`.
//!
//! **Project** scope does not live here: it is a `.amf/prompts/` file store
//! checked into the repo (see `src/prompts/`), so a project-wide override is
//! reviewable and shared like any other repo file.
//!
//! Within a scope, a per-harness row (`harness = Some(..)`) is more specific
//! than the shared row (`harness = None`); [`PromptOverrides::effective_at`]
//! applies that. Rows carry no foreign key to `features` (like `todos` and
//! `pr_terminal_state`): the workdir key outlives any feature row and cleanup
//! is explicit via [`delete_for_workdir`].
//!
//! Templates are stored and returned verbatim. There is no placeholder
//! validation anywhere in this feature — a user override may drop a required
//! `{{token}}` or add an unknown one and it is persisted and rendered as-is.
//!
//! Like `db/todos.rs`, the API is written ahead of its UI consumers.
#![allow(dead_code)]

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

use crate::project::AgentKind;

/// Which persisted layer an override belongs to. Project scope is a file
/// store, not a row here, so it is deliberately absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverrideScope {
    /// One override per checkout, keyed by the workdir path.
    Feature { workdir: String },
    /// One override for the whole machine.
    Global,
}

impl OverrideScope {
    /// The `prompt_overrides.scope` token.
    pub fn as_db_str(&self) -> &'static str {
        match self {
            OverrideScope::Feature { .. } => "feature",
            OverrideScope::Global => "global",
        }
    }

    /// The `prompt_overrides.scope_key` value: the workdir for a feature
    /// override, `None` for global.
    pub fn key(&self) -> Option<&str> {
        match self {
            OverrideScope::Feature { workdir } => Some(workdir),
            OverrideScope::Global => None,
        }
    }

    /// Rebuild a scope from the two stored columns. An unrecognized token, or
    /// a `feature` row that somehow lost its key, reads as [`Self::Global`] —
    /// the only scope that stays addressable when the key column is missing.
    fn from_row(scope: &str, key: Option<String>) -> Self {
        match (scope, key) {
            ("feature", Some(workdir)) => OverrideScope::Feature { workdir },
            _ => OverrideScope::Global,
        }
    }
}

/// One stored override row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptOverride {
    /// Stable [`crate::prompts::PromptId`] key, e.g. `"pr_review.ai_review"`.
    pub prompt_id: String,
    pub scope: OverrideScope,
    /// `None` = shared across every harness; `Some(_)` = specific to one.
    pub harness: Option<AgentKind>,
    /// The template text, stored and served verbatim.
    pub template: String,
    pub created_at: String,
    pub updated_at: String,
}

/// The `prompt_overrides.harness` token for a harness — [`AgentKind::slug`].
fn harness_token(harness: &AgentKind) -> &'static str {
    harness.slug()
}

/// Parse a stored `harness` token. A NULL or unrecognized token is treated as
/// `None` (shared) rather than dropped — the row stays usable.
fn harness_from_token(token: Option<String>) -> Option<AgentKind> {
    token.as_deref().and_then(AgentKind::from_slug)
}

const COLUMNS: &str = "prompt_id, scope, scope_key, harness, template, created_at, updated_at";

fn row_to_override(row: &rusqlite::Row<'_>) -> rusqlite::Result<PromptOverride> {
    let scope: String = row.get(1)?;
    let scope_key: Option<String> = row.get(2)?;
    let harness: Option<String> = row.get(3)?;
    Ok(PromptOverride {
        prompt_id: row.get(0)?,
        scope: OverrideScope::from_row(&scope, scope_key),
        harness: harness_from_token(harness),
        template: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

/// Every override row, feature and global, in insertion order.
pub fn load_all(conn: &Connection) -> Result<Vec<PromptOverride>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM prompt_overrides ORDER BY created_at, rowid"
    ))?;
    let rows = stmt
        .query_map([], row_to_override)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Every override for one prompt id, feature and global.
pub fn load_for_prompt(conn: &Connection, prompt_id: &str) -> Result<Vec<PromptOverride>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM prompt_overrides
         WHERE prompt_id = ?1 ORDER BY created_at, rowid"
    ))?;
    let rows = stmt
        .query_map(params![prompt_id], row_to_override)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// One override by full identity, or `None`.
pub fn load_one(
    conn: &Connection,
    prompt_id: &str,
    scope: &OverrideScope,
    harness: Option<&AgentKind>,
) -> Result<Option<PromptOverride>> {
    let row = conn
        .query_row(
            &format!(
                "SELECT {COLUMNS} FROM prompt_overrides
                 WHERE prompt_id = ?1 AND scope = ?2
                   AND COALESCE(scope_key, '') = ?3
                   AND COALESCE(harness, '') = ?4"
            ),
            params![
                prompt_id,
                scope.as_db_str(),
                scope.key().unwrap_or(""),
                harness.map(harness_token).unwrap_or(""),
            ],
            row_to_override,
        )
        .optional()?;
    Ok(row)
}

/// Insert or replace the override at `(prompt_id, scope, harness)`. The
/// template is stored verbatim; `created_at` is preserved on update.
pub fn upsert(
    conn: &Connection,
    prompt_id: &str,
    scope: &OverrideScope,
    harness: Option<&AgentKind>,
    template: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO prompt_overrides
            (prompt_id, scope, scope_key, harness, template, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'), datetime('now'))
         ON CONFLICT (prompt_id, scope, COALESCE(scope_key, ''), COALESCE(harness, ''))
         DO UPDATE SET template = excluded.template, updated_at = excluded.updated_at",
        params![
            prompt_id,
            scope.as_db_str(),
            scope.key(),
            harness.map(harness_token),
            template,
        ],
    )?;
    Ok(())
}

/// Delete the override at `(prompt_id, scope, harness)`. Returns whether a row
/// was removed.
pub fn delete(
    conn: &Connection,
    prompt_id: &str,
    scope: &OverrideScope,
    harness: Option<&AgentKind>,
) -> Result<bool> {
    let removed = conn.execute(
        "DELETE FROM prompt_overrides
         WHERE prompt_id = ?1 AND scope = ?2
           AND COALESCE(scope_key, '') = ?3
           AND COALESCE(harness, '') = ?4",
        params![
            prompt_id,
            scope.as_db_str(),
            scope.key().unwrap_or(""),
            harness.map(harness_token).unwrap_or(""),
        ],
    )?;
    Ok(removed > 0)
}

/// Drop every feature-scope override for a checkout — wired into feature
/// deletion so an abandoned workdir path does not keep matching a future
/// feature that reuses it.
pub fn delete_for_workdir(conn: &Connection, workdir: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM prompt_overrides WHERE scope = 'feature' AND scope_key = ?1",
        params![workdir],
    )?;
    Ok(())
}

/// An in-memory view of the persisted overrides.
///
/// Mirrors how `db/todos.rs` is consumed: a DB is optional, and when there is
/// none the collection is simply empty and every write is a no-op that still
/// updates the in-memory copy, so the manager overlay works (unpersisted) and
/// the resolver falls straight through to the project/built-in layers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptOverrides {
    rows: Vec<PromptOverride>,
}

impl PromptOverrides {
    /// Load every row, or an empty set when there is no database.
    pub fn load(conn: Option<&Connection>) -> Result<Self> {
        let rows = match conn {
            Some(conn) => load_all(conn)?,
            None => Vec::new(),
        };
        Ok(Self { rows })
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn all(&self) -> &[PromptOverride] {
        &self.rows
    }

    fn position(
        &self,
        prompt_id: &str,
        scope: &OverrideScope,
        harness: Option<&AgentKind>,
    ) -> Option<usize> {
        self.rows.iter().position(|row| {
            row.prompt_id == prompt_id
                && &row.scope == scope
                && row.harness.as_ref() == harness
        })
    }

    /// The exact row at `(prompt_id, scope, harness)`, if present.
    pub fn get(
        &self,
        prompt_id: &str,
        scope: &OverrideScope,
        harness: Option<&AgentKind>,
    ) -> Option<&PromptOverride> {
        self.position(prompt_id, scope, harness)
            .map(|index| &self.rows[index])
    }

    /// The template that applies to `prompt_id` **at this scope** for
    /// `harness`: the per-harness row if one exists, else the shared row, else
    /// `None`. Cross-scope precedence is the caller's job.
    pub fn effective_at(
        &self,
        prompt_id: &str,
        scope: &OverrideScope,
        harness: &AgentKind,
    ) -> Option<&str> {
        self.get(prompt_id, scope, Some(harness))
            .or_else(|| self.get(prompt_id, scope, None))
            .map(|row| row.template.as_str())
    }

    /// Insert or update an override, writing through to the database when one
    /// is present. The in-memory copy is updated either way.
    pub fn set(
        &mut self,
        conn: Option<&Connection>,
        prompt_id: &str,
        scope: OverrideScope,
        harness: Option<AgentKind>,
        template: &str,
    ) -> Result<()> {
        if let Some(conn) = conn {
            upsert(conn, prompt_id, &scope, harness.as_ref(), template)?;
        }
        match self.position(prompt_id, &scope, harness.as_ref()) {
            Some(index) => {
                self.rows[index].template = template.to_string();
                self.rows[index].updated_at = now_marker();
            }
            None => self.rows.push(PromptOverride {
                prompt_id: prompt_id.to_string(),
                scope,
                harness,
                template: template.to_string(),
                created_at: now_marker(),
                updated_at: now_marker(),
            }),
        }
        Ok(())
    }

    /// Remove an override, writing through to the database when one is
    /// present. Returns whether anything was removed from the in-memory copy.
    pub fn remove(
        &mut self,
        conn: Option<&Connection>,
        prompt_id: &str,
        scope: &OverrideScope,
        harness: Option<&AgentKind>,
    ) -> Result<bool> {
        if let Some(conn) = conn {
            delete(conn, prompt_id, scope, harness)?;
        }
        Ok(match self.position(prompt_id, scope, harness) {
            Some(index) => {
                self.rows.remove(index);
                true
            }
            None => false,
        })
    }
}

/// A timestamp for in-memory rows that never touched SQLite. The DB path uses
/// `datetime('now')`; this only needs to be monotone-ish and comparable.
fn now_marker() -> String {
    chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        conn
    }

    fn feature(workdir: &str) -> OverrideScope {
        OverrideScope::Feature {
            workdir: workdir.to_string(),
        }
    }

    #[test]
    fn round_trips_a_global_shared_override() {
        let conn = conn();
        upsert(
            &conn,
            "pr_review.ai_review",
            &OverrideScope::Global,
            None,
            "my template {{annotated_diff}}",
        )
        .unwrap();

        let all = load_all(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].prompt_id, "pr_review.ai_review");
        assert_eq!(all[0].scope, OverrideScope::Global);
        assert_eq!(all[0].harness, None);
        assert_eq!(all[0].template, "my template {{annotated_diff}}");

        let one = load_one(&conn, "pr_review.ai_review", &OverrideScope::Global, None)
            .unwrap()
            .unwrap();
        assert_eq!(one, all[0]);
    }

    #[test]
    fn round_trips_a_feature_per_harness_override() {
        let conn = conn();
        upsert(
            &conn,
            "learning.answer",
            &feature("/repo/.worktrees/x"),
            Some(&AgentKind::Codex),
            "codex-only",
        )
        .unwrap();

        let one = load_one(
            &conn,
            "learning.answer",
            &feature("/repo/.worktrees/x"),
            Some(&AgentKind::Codex),
        )
        .unwrap()
        .unwrap();
        assert_eq!(one.scope, feature("/repo/.worktrees/x"));
        assert_eq!(one.harness, Some(AgentKind::Codex));
        assert_eq!(one.template, "codex-only");

        // A different harness at the same scope is a different row.
        assert!(
            load_one(
                &conn,
                "learning.answer",
                &feature("/repo/.worktrees/x"),
                Some(&AgentKind::Claude),
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn upsert_replaces_by_identity_and_keeps_created_at() {
        let conn = conn();
        upsert(&conn, "session.summary", &OverrideScope::Global, None, "v1").unwrap();
        let created = load_one(&conn, "session.summary", &OverrideScope::Global, None)
            .unwrap()
            .unwrap()
            .created_at;

        upsert(&conn, "session.summary", &OverrideScope::Global, None, "v2").unwrap();
        let after = load_one(&conn, "session.summary", &OverrideScope::Global, None)
            .unwrap()
            .unwrap();

        assert_eq!(load_all(&conn).unwrap().len(), 1, "no duplicate row");
        assert_eq!(after.template, "v2");
        assert_eq!(after.created_at, created, "created_at is preserved");
    }

    #[test]
    fn shared_and_per_harness_rows_coexist_at_one_scope() {
        let conn = conn();
        upsert(&conn, "review.walkthrough", &OverrideScope::Global, None, "shared").unwrap();
        upsert(
            &conn,
            "review.walkthrough",
            &OverrideScope::Global,
            Some(&AgentKind::Claude),
            "claude",
        )
        .unwrap();
        assert_eq!(load_for_prompt(&conn, "review.walkthrough").unwrap().len(), 2);
    }

    #[test]
    fn delete_reports_whether_a_row_went() {
        let conn = conn();
        upsert(&conn, "session.summary", &OverrideScope::Global, None, "x").unwrap();
        assert!(delete(&conn, "session.summary", &OverrideScope::Global, None).unwrap());
        assert!(!delete(&conn, "session.summary", &OverrideScope::Global, None).unwrap());
    }

    #[test]
    fn delete_for_workdir_only_touches_that_checkout() {
        let conn = conn();
        upsert(&conn, "learning.answer", &feature("/a"), None, "a").unwrap();
        upsert(&conn, "learning.answer", &feature("/b"), None, "b").unwrap();
        upsert(&conn, "learning.answer", &OverrideScope::Global, None, "g").unwrap();

        delete_for_workdir(&conn, "/a").unwrap();

        let remaining: Vec<_> = load_all(&conn).unwrap();
        assert_eq!(remaining.len(), 2);
        assert!(remaining.iter().all(|row| row.template != "a"));
    }

    #[test]
    fn in_memory_view_without_a_db_is_empty_and_write_through_is_a_noop() {
        let mut set = PromptOverrides::load(None).unwrap();
        assert!(set.is_empty());
        set.set(
            None,
            "session.summary",
            OverrideScope::Global,
            None,
            "session-only",
        )
        .unwrap();
        assert_eq!(
            set.effective_at("session.summary", &OverrideScope::Global, &AgentKind::Claude),
            Some("session-only"),
            "the edit is visible in memory even with no database"
        );
    }

    #[test]
    fn in_memory_view_write_through_persists_when_a_db_is_present() {
        let conn = conn();
        let mut set = PromptOverrides::load(Some(&conn)).unwrap();
        set.set(
            Some(&conn),
            "review.co_review",
            OverrideScope::Global,
            Some(AgentKind::Claude),
            "wrote through",
        )
        .unwrap();

        let reloaded = PromptOverrides::load(Some(&conn)).unwrap();
        assert_eq!(
            reloaded.effective_at("review.co_review", &OverrideScope::Global, &AgentKind::Claude),
            Some("wrote through")
        );
    }

    #[test]
    fn effective_at_prefers_the_per_harness_row_over_the_shared_one() {
        let conn = conn();
        let mut set = PromptOverrides::load(Some(&conn)).unwrap();
        set.set(Some(&conn), "learning.answer", OverrideScope::Global, None, "shared")
            .unwrap();
        set.set(
            Some(&conn),
            "learning.answer",
            OverrideScope::Global,
            Some(AgentKind::Pi),
            "pi-specific",
        )
        .unwrap();

        assert_eq!(
            set.effective_at("learning.answer", &OverrideScope::Global, &AgentKind::Pi),
            Some("pi-specific")
        );
        assert_eq!(
            set.effective_at("learning.answer", &OverrideScope::Global, &AgentKind::Codex),
            Some("shared"),
            "a harness with no specific row falls back to shared"
        );
    }

    #[test]
    fn unknown_harness_token_degrades_to_shared() {
        let conn = conn();
        conn.execute(
            "INSERT INTO prompt_overrides
                (prompt_id, scope, scope_key, harness, template, created_at, updated_at)
             VALUES ('x', 'global', NULL, 'gemini', 't', datetime('now'), datetime('now'))",
            [],
        )
        .unwrap();
        assert_eq!(load_all(&conn).unwrap()[0].harness, None);
    }
}
