use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::PathBuf;

use crate::project::{
    AgentKind, CURRENT_PROJECT_STORE_VERSION, Feature, FeatureSession, Project, ProjectStatus,
    ProjectStore, SessionBookmark, SessionKind, TodoSessionReference, TokenUsageSourceMatch,
    VibeMode,
};
use crate::token_tracking::TokenUsageSource;

// ── enum ↔ str helpers ───────────────────────────────────────

pub(super) fn agent_to_str(a: &AgentKind) -> &'static str {
    match a {
        AgentKind::Claude => "claude",
        AgentKind::Opencode => "opencode",
        AgentKind::Codex => "codex",
        AgentKind::Pi => "pi",
    }
}

pub(super) fn agent_from_str(s: &str) -> AgentKind {
    match s {
        "opencode" => AgentKind::Opencode,
        "codex" => AgentKind::Codex,
        "pi" => AgentKind::Pi,
        _ => AgentKind::Claude,
    }
}

fn mode_to_str(m: &VibeMode) -> &'static str {
    match m {
        VibeMode::Vibeless => "vibeless",
        VibeMode::Vibe => "vibe",
        VibeMode::SuperVibe => "supervibe",
    }
}

fn mode_from_str(s: &str) -> VibeMode {
    match s {
        "vibe" => VibeMode::Vibe,
        "supervibe" => VibeMode::SuperVibe,
        _ => VibeMode::Vibeless,
    }
}

fn status_to_str(s: &ProjectStatus) -> &'static str {
    match s {
        ProjectStatus::Active => "active",
        ProjectStatus::Idle => "idle",
        ProjectStatus::Stopped => "stopped",
    }
}

fn status_from_str(s: &str) -> ProjectStatus {
    match s {
        "active" => ProjectStatus::Active,
        "idle" => ProjectStatus::Idle,
        _ => ProjectStatus::Stopped,
    }
}

fn kind_to_str(k: &SessionKind) -> &'static str {
    match k {
        SessionKind::Claude => "claude",
        SessionKind::Opencode => "opencode",
        SessionKind::Codex => "codex",
        SessionKind::Pi => "pi",
        SessionKind::Terminal => "terminal",
        SessionKind::Nvim => "nvim",
        SessionKind::Vscode => "vscode",
        SessionKind::Custom => "custom",
        SessionKind::Todos => "todos",
    }
}

fn kind_from_str(s: &str) -> SessionKind {
    match s {
        "opencode" => SessionKind::Opencode,
        "codex" => SessionKind::Codex,
        "pi" => SessionKind::Pi,
        "terminal" => SessionKind::Terminal,
        "nvim" => SessionKind::Nvim,
        "vscode" => SessionKind::Vscode,
        "custom" => SessionKind::Custom,
        "todos" => SessionKind::Todos,
        _ => SessionKind::Claude,
    }
}

fn match_to_str(m: &TokenUsageSourceMatch) -> &'static str {
    match m {
        TokenUsageSourceMatch::Exact => "exact",
        TokenUsageSourceMatch::Inferred => "inferred",
    }
}

fn match_from_str(s: &str) -> TokenUsageSourceMatch {
    match s {
        "inferred" => TokenUsageSourceMatch::Inferred,
        _ => TokenUsageSourceMatch::Exact,
    }
}

fn dt_to_str(dt: &DateTime<Utc>) -> String {
    dt.to_rfc3339()
}

fn dt_from_str(s: &str) -> DateTime<Utc> {
    s.parse().unwrap_or_else(|_| Utc::now())
}

fn source_to_json(source: &TokenUsageSource) -> String {
    serde_json::to_string(source).unwrap_or_else(|_| "null".to_string())
}

fn source_from_json(s: &str) -> Option<TokenUsageSource> {
    serde_json::from_str(s).ok()
}

// ── load ─────────────────────────────────────────────────────

pub fn load(conn: &Connection) -> Result<ProjectStore> {
    let available_harnesses: Vec<AgentKind> = conn
        .query_row(
            "SELECT value FROM store_meta WHERE key = 'available_harnesses'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .map(|v| v.iter().map(|s| agent_from_str(s)).collect())
        .unwrap_or_default();

    let extra: std::collections::HashMap<String, serde_json::Value> = conn
        .query_row(
            "SELECT value FROM store_meta WHERE key = 'extra'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let mut bookmark_stmt =
        conn.prepare("SELECT project_id, feature_id, session_id FROM session_bookmarks")?;
    let session_bookmarks: Vec<SessionBookmark> = bookmark_stmt
        .query_map([], |row| {
            Ok(SessionBookmark {
                project_id: row.get(0)?,
                feature_id: row.get(1)?,
                session_id: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    // Templates are global, frequently mutated, and shared across every
    // concurrently running AMF process, so they're read/written through
    // `db::prompt_templates` directly rather than as part of this
    // full-replace store — see that module's doc comment.
    let prompt_templates = super::prompt_templates::load(conn)?;

    let mut proj_stmt = conn.prepare(
        "SELECT id, name, repo, collapsed, preferred_agent, is_git, created_at
         FROM projects ORDER BY sort_order ASC, rowid ASC",
    )?;
    let project_ids: Vec<(String, String, String, bool, String, bool, String)> = proj_stmt
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut projects = Vec::new();
    for (id, name, repo, collapsed, preferred_agent, is_git, created_at) in project_ids {
        let features = load_features(conn, &id)?;
        projects.push(Project {
            id,
            name,
            repo: PathBuf::from(repo),
            collapsed,
            features,
            created_at: dt_from_str(&created_at),
            preferred_agent: agent_from_str(&preferred_agent),
            is_git,
        });
    }

    Ok(ProjectStore {
        version: CURRENT_PROJECT_STORE_VERSION,
        projects,
        session_bookmarks,
        available_harnesses,
        prompt_templates,
        extra,
    })
}

/// [`load`] plus the `store_version` it was read at, as one consistent
/// snapshot: reading them as two separate statements without a shared
/// transaction could interleave with a concurrent [`save_checked`] and pair
/// this load's data with a version that does not actually describe it.
/// Every application-level load that will later save through
/// [`save_checked`] needs this, not [`load`] plus a separate
/// [`current_version`] call.
pub fn load_versioned(conn: &Connection) -> Result<(ProjectStore, u64)> {
    conn.execute_batch("BEGIN DEFERRED;")?;
    let result = (|| -> Result<(ProjectStore, u64)> {
        let store = load(conn)?;
        let version = current_version(conn)?;
        Ok((store, version))
    })();
    // Read-only transaction: nothing to keep even on success, just release
    // the snapshot. Roll back either way rather than distinguish the
    // success path, since there is no write to preserve.
    let _ = conn.execute_batch("ROLLBACK;");
    result
}

/// One `features` row in SELECT column order (see `load_features`): id, name,
/// branch, workdir, is_worktree, tmux_session, mode, review, plan_mode, agent,
/// enable_chrome, status, summary, summary_updated_at, nickname, collapsed,
/// created_at, last_accessed, ready, triage_source, selected_plan_path,
/// review_source, issue_source.
type FeatureRow = (
    String,
    String,
    String,
    String,
    bool,
    String,
    String,
    bool,
    bool,
    String,
    bool,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    bool,
    String,
    String,
    bool,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn load_features(conn: &Connection, project_id: &str) -> Result<Vec<Feature>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, branch, workdir, is_worktree, tmux_session,
                mode, review, plan_mode, agent, enable_chrome, status,
                summary, summary_updated_at, nickname, collapsed,
                created_at, last_accessed, ready, triage_source,
                selected_plan_path, review_source, issue_source
         FROM features WHERE project_id = ?1
         ORDER BY sort_order ASC, rowid ASC",
    )?;

    let rows: Vec<FeatureRow> = stmt
        .query_map(params![project_id], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
                row.get(10)?,
                row.get(11)?,
                row.get(12)?,
                row.get(13)?,
                row.get(14)?,
                row.get(15)?,
                row.get(16)?,
                row.get(17)?,
                row.get(18)?,
                row.get(19)?,
                row.get(20)?,
                row.get(21)?,
                row.get(22)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut features = Vec::new();
    for (
        feat_id,
        feat_name,
        branch,
        workdir,
        is_worktree,
        tmux_session,
        mode_str,
        review,
        plan_mode,
        agent_str,
        enable_chrome,
        status_str,
        summary,
        summary_updated_at_str,
        nickname,
        feat_collapsed,
        feat_created_at,
        last_accessed,
        ready,
        triage_source_json,
        selected_plan_path,
        review_source_json,
        issue_source_json,
    ) in rows
    {
        let sessions = load_sessions(conn, &feat_id)?;
        features.push(Feature {
            id: feat_id,
            name: feat_name,
            branch,
            workdir: PathBuf::from(workdir),
            is_worktree,
            tmux_session,
            sessions,
            collapsed: feat_collapsed,
            mode: mode_from_str(&mode_str),
            review,
            plan_mode,
            agent: agent_from_str(&agent_str),
            enable_chrome,
            remote_control: false, // not yet in DB schema; default false
            pending_worktree_script: false,
            ready,
            status: status_from_str(&status_str),
            created_at: dt_from_str(&feat_created_at),
            last_accessed: dt_from_str(&last_accessed),
            summary,
            summary_updated_at: summary_updated_at_str.as_deref().map(dt_from_str),
            nickname,
            selected_plan_path: selected_plan_path.map(PathBuf::from),
            // A malformed blob (hand-edited DB, or a row written by a future
            // schema) degrades to "not a triage feature" rather than failing
            // the whole store load.
            triage_source: triage_source_json
                .as_deref()
                .and_then(|json| serde_json::from_str(json).ok()),
            // Same degradation rule as `triage_source`: a malformed blob reads
            // as "not a companion review feature" rather than failing the load.
            review_source: review_source_json
                .as_deref()
                .and_then(|json| serde_json::from_str(json).ok()),
            issue_source: issue_source_json
                .as_deref()
                .and_then(|json| serde_json::from_str(json).ok()),
        });
    }
    Ok(features)
}

fn load_sessions(conn: &Connection, feature_id: &str) -> Result<Vec<FeatureSession>> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, label, tmux_window, claude_session_id,
                token_usage_source, token_usage_source_match,
                created_at, command, on_stop, pre_check,
                todo_id, todo_launched_from_menu
         FROM feature_sessions WHERE feature_id = ?1
         ORDER BY sort_order ASC, rowid ASC",
    )?;

    let sessions = stmt
        .query_map(params![feature_id], |row| {
            Ok(FeatureSession {
                id: row.get(0)?,
                kind: kind_from_str(&row.get::<_, String>(1)?),
                label: row.get(2)?,
                tmux_window: row.get(3)?,
                claude_session_id: row.get(4)?,
                todo_reference: row.get::<_, Option<String>>(11)?.map(|todo_id| {
                    TodoSessionReference {
                        todo_id,
                        launched_from_todo_menu: row.get::<_, i64>(12).unwrap_or(0) != 0,
                    }
                }),
                token_usage_source: row
                    .get::<_, Option<String>>(5)?
                    .as_deref()
                    .and_then(source_from_json),
                token_usage_source_match: row
                    .get::<_, Option<String>>(6)?
                    .as_deref()
                    .map(match_from_str),
                created_at: dt_from_str(&row.get::<_, String>(7)?),
                command: row.get(8)?,
                on_stop: row.get(9)?,
                pre_check: row.get(10)?,
                status_text: None,
                token_usage: None,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(sessions)
}

// ── save ─────────────────────────────────────────────────────

/// The full-replace save's cross-process safety net (`AMF_PLAN.md` Task 5,
/// "Establish cross-process coordination"). `save` below deletes and
/// reinserts every row on every call: two processes (the TUI and the GUI,
/// or two AMF instances) each holding their own in-memory `ProjectStore`
/// would otherwise silently clobber each other's concurrent writes — the
/// second save wins in full, discarding whatever the first one added, with
/// no error and no trace. `store_meta`'s `store_version` key turns that into
/// a detectable optimistic-concurrency conflict: every save increments it,
/// and `save_checked` refuses to proceed when the caller's expected version
/// doesn't match what's actually on disk.
pub(super) fn current_version(conn: &Connection) -> Result<u64> {
    Ok(conn
        .query_row(
            "SELECT value FROM store_meta WHERE key = 'store_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .and_then(|s| s.parse().ok())
        .unwrap_or(0))
}

/// Outcome of a version-checked save. `Conflict` carries the version that
/// was actually on disk so the caller can decide how far it drifted (today,
/// every caller just reloads and reports rather than inspecting this, but a
/// large gap is a stronger signal than a one-behind race).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveOutcome {
    Saved { new_version: u64 },
    Conflict { current_version: u64 },
}

/// Save without a version check: only for call sites with no concurrent
/// writer to race against by construction (seeding/merging a legacy store at
/// `AmfDb::open_or_seed` time, before anything holds a loaded version to
/// check against). Ordinary application saves must go through
/// [`save_checked`] instead — see its doc comment.
pub fn save(conn: &Connection, store: &ProjectStore) -> Result<()> {
    conn.execute_batch("BEGIN IMMEDIATE;")?;
    match do_save(conn, store) {
        Ok(()) => {
            let next = current_version(conn)?.wrapping_add(1);
            conn.execute(
                "INSERT OR REPLACE INTO store_meta (key, value) VALUES ('store_version', ?1)",
                params![next.to_string()],
            )?;
            conn.execute_batch("COMMIT;")?;
            Ok(())
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK;");
            Err(e)
        }
    }
}

/// Save `store`, but only if the on-disk version still matches
/// `expected_version` — i.e. nothing else has saved since the caller last
/// loaded. `BEGIN IMMEDIATE` takes SQLite's write lock before the version
/// check runs, so the check-then-write is atomic against another process
/// doing the same thing concurrently, not just against interleaving within
/// one process.
pub fn save_checked(
    conn: &Connection,
    store: &ProjectStore,
    expected_version: u64,
) -> Result<SaveOutcome> {
    conn.execute_batch("BEGIN IMMEDIATE;")?;
    let on_disk = match current_version(conn) {
        Ok(v) => v,
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK;");
            return Err(e);
        }
    };
    if on_disk != expected_version {
        conn.execute_batch("ROLLBACK;")?;
        return Ok(SaveOutcome::Conflict {
            current_version: on_disk,
        });
    }
    match do_save(conn, store) {
        Ok(()) => {
            let next = on_disk.wrapping_add(1);
            conn.execute(
                "INSERT OR REPLACE INTO store_meta (key, value) VALUES ('store_version', ?1)",
                params![next.to_string()],
            )?;
            conn.execute_batch("COMMIT;")?;
            Ok(SaveOutcome::Saved { new_version: next })
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK;");
            Err(e)
        }
    }
}

fn do_save(conn: &Connection, store: &ProjectStore) -> Result<()> {
    // Full replace: CASCADE deletes features → sessions. `prompt_templates`
    // is deliberately excluded — it's persisted independently through
    // `db::prompt_templates`, see that module's doc comment for why.
    conn.execute_batch("DELETE FROM session_bookmarks; DELETE FROM projects;")?;

    let harnesses_json = serde_json::to_string(
        &store
            .available_harnesses
            .iter()
            .map(agent_to_str)
            .collect::<Vec<_>>(),
    )?;
    let extra_json = serde_json::to_string(&store.extra)?;

    conn.execute(
        "INSERT OR REPLACE INTO store_meta (key, value)
         VALUES ('available_harnesses', ?1)",
        params![harnesses_json],
    )?;
    conn.execute(
        "INSERT OR REPLACE INTO store_meta (key, value) VALUES ('extra', ?1)",
        params![extra_json],
    )?;

    for bookmark in &store.session_bookmarks {
        conn.execute(
            "INSERT OR IGNORE INTO session_bookmarks (project_id, feature_id, session_id)
             VALUES (?1, ?2, ?3)",
            params![
                bookmark.project_id,
                bookmark.feature_id,
                bookmark.session_id
            ],
        )?;
    }

    for (pi, project) in store.projects.iter().enumerate() {
        conn.execute(
            "INSERT INTO projects
             (id, name, repo, collapsed, preferred_agent, is_git, created_at, sort_order)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                project.id,
                project.name,
                project.repo.to_string_lossy(),
                project.collapsed as i32,
                agent_to_str(&project.preferred_agent),
                project.is_git as i32,
                dt_to_str(&project.created_at),
                pi as i64,
            ],
        )?;

        for (fi, feature) in project.features.iter().enumerate() {
            if feature.pending_worktree_script {
                continue;
            }
            conn.execute(
                "INSERT INTO features (
                    id, project_id, name, branch, workdir, is_worktree,
                    tmux_session, mode, review, plan_mode, agent, enable_chrome,
                    status, summary, summary_updated_at, nickname, collapsed,
                    created_at, last_accessed, ready, sort_order, triage_source,
                    selected_plan_path, review_source, issue_source
                ) VALUES (
                    ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25
                )",
                params![
                    feature.id,
                    project.id,
                    feature.name,
                    feature.branch,
                    feature.workdir.to_string_lossy(),
                    feature.is_worktree as i32,
                    feature.tmux_session,
                    mode_to_str(&feature.mode),
                    feature.review as i32,
                    feature.plan_mode as i32,
                    agent_to_str(&feature.agent),
                    feature.enable_chrome as i32,
                    status_to_str(&feature.status),
                    feature.summary,
                    feature.summary_updated_at.as_ref().map(dt_to_str),
                    feature.nickname,
                    feature.collapsed as i32,
                    dt_to_str(&feature.created_at),
                    dt_to_str(&feature.last_accessed),
                    feature.ready as i32,
                    fi as i64,
                    feature
                        .triage_source
                        .as_ref()
                        .and_then(|link| serde_json::to_string(link).ok()),
                    feature
                        .selected_plan_path
                        .as_ref()
                        .map(|path| path.to_string_lossy()),
                    feature
                        .review_source
                        .as_ref()
                        .and_then(|link| serde_json::to_string(link).ok()),
                    feature
                        .issue_source
                        .as_ref()
                        .and_then(|link| serde_json::to_string(link).ok()),
                ],
            )?;

            for (si, session) in feature.sessions.iter().enumerate() {
                conn.execute(
                    "INSERT INTO feature_sessions (
                        id, feature_id, kind, label, tmux_window,
                        claude_session_id, token_usage_source,
                        token_usage_source_match, created_at,
                        command, on_stop, pre_check, todo_id,
                        todo_launched_from_menu, sort_order
                    ) VALUES (
                        ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15
                    )",
                    params![
                        session.id,
                        feature.id,
                        kind_to_str(&session.kind),
                        session.label,
                        session.tmux_window,
                        session.claude_session_id,
                        session.token_usage_source.as_ref().map(source_to_json),
                        session.token_usage_source_match.as_ref().map(match_to_str),
                        dt_to_str(&session.created_at),
                        session.command,
                        session.on_stop,
                        session.pre_check,
                        session
                            .todo_reference
                            .as_ref()
                            .map(|reference| &reference.todo_id),
                        session
                            .todo_reference
                            .as_ref()
                            .is_some_and(|reference| reference.launched_from_todo_menu)
                            as i32,
                        si as i64,
                    ],
                )?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::AmfDb;
    use crate::project::{Feature, FeatureSession, Project, ProjectStore, SessionKind, VibeMode};
    use std::collections::HashMap;
    use tempfile::NamedTempFile;

    fn empty_store() -> ProjectStore {
        ProjectStore {
            version: CURRENT_PROJECT_STORE_VERSION,
            projects: Vec::new(),
            session_bookmarks: Vec::new(),
            available_harnesses: Vec::new(),
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        }
    }

    fn open_temp_db() -> (NamedTempFile, AmfDb) {
        let tmp = NamedTempFile::new().unwrap();
        let db = AmfDb::open(tmp.path()).unwrap();
        (tmp, db)
    }

    #[test]
    fn empty_store_roundtrip() {
        let (_tmp, db) = open_temp_db();
        let store = empty_store();
        db.save_store(&store).unwrap();
        let loaded = db.load_store().unwrap();
        assert_eq!(loaded.projects.len(), 0);
        assert_eq!(loaded.session_bookmarks.len(), 0);
        assert_eq!(loaded.prompt_templates.len(), 0);
    }

    /// `prompt_templates` is deliberately excluded from the full-replace
    /// save path (see `db::prompt_templates`'s doc comment): a store whose
    /// in-memory `prompt_templates` disagrees with the table must not touch
    /// it either way. Round-trip coverage for the templates table itself
    /// lives in `db::prompt_templates`'s own tests.
    #[test]
    fn save_store_does_not_touch_prompt_templates_table() {
        use crate::prompt_library::PromptTemplate;

        let (_tmp, db) = open_temp_db();

        let on_disk = PromptTemplate::new("On disk".to_string(), "Body".to_string());
        db.insert_prompt_template(&on_disk).unwrap();

        // A store snapshot that disagrees with the table (empty here, but
        // any mismatch would do) must not overwrite it.
        let store = empty_store();
        db.save_store(&store).unwrap();

        let loaded = db.load_prompt_templates().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "On disk");
    }

    #[test]
    fn project_with_features_and_sessions_roundtrip() {
        let (_tmp, db) = open_temp_db();

        let session = FeatureSession {
            id: "sess-1".to_string(),
            kind: SessionKind::Claude,
            label: "Claude 1".to_string(),
            tmux_window: "claude".to_string(),
            claude_session_id: Some("claude-abc123".to_string()),
            todo_reference: Some(TodoSessionReference {
                todo_id: "todo-123".to_string(),
                launched_from_todo_menu: true,
            }),
            token_usage_source: None,
            token_usage_source_match: None,
            created_at: Utc::now(),
            command: None,
            on_stop: None,
            pre_check: None,
            status_text: None,
            token_usage: None,
        };

        let feature = Feature {
            id: "feat-1".to_string(),
            name: "my-feature".to_string(),
            branch: "feature/my-feature".to_string(),
            workdir: PathBuf::from("/tmp/repo/.worktrees/my-feature"),
            is_worktree: true,
            tmux_session: "amf-my-feature".to_string(),
            sessions: vec![session],
            collapsed: false,
            mode: VibeMode::Vibe,
            review: true,
            plan_mode: false,
            agent: crate::project::AgentKind::Claude,
            enable_chrome: false,
            remote_control: false,
            pending_worktree_script: false,
            ready: true,
            status: crate::project::ProjectStatus::Idle,
            created_at: Utc::now(),
            last_accessed: Utc::now(),
            summary: Some("did some stuff".to_string()),
            summary_updated_at: Some(Utc::now()),
            nickname: Some("myf".to_string()),
            selected_plan_path: Some(PathBuf::from(
                "/tmp/repo/.worktrees/my-feature/docs/current-plan.md",
            )),
            // A companion PR-triage feature: the link that survives a restart
            // and lets PR Triage find and reuse this feature for the same PR.
            triage_source: Some(crate::project::TriageSource {
                pr_number: 42,
                source_feature_id: "feat-source".to_string(),
                pr_branch: "feature/my-feature".to_string(),
                base_sha: "abc123".to_string(),
            }),
            // A companion review feature's link back to the feature its final
            // review ran from — the parallel of `triage_source` for the "New
            // feature…" review destination.
            review_source: Some(crate::project::ReviewSource {
                source_feature_id: "feat-source".to_string(),
                target_branch: "feature/my-feature".to_string(),
                base_sha: "def456".to_string(),
            }),
            issue_source: Some(crate::project::IssueSource {
                host: "github.com".to_string(),
                owner: "acme".to_string(),
                repository: "widget".to_string(),
                number: 73,
                comment_status: crate::project::IssueCommentStatus::Posted,
            }),
        };

        let project = Project {
            id: "proj-1".to_string(),
            name: "my-project".to_string(),
            repo: PathBuf::from("/tmp/repo"),
            collapsed: false,
            features: vec![feature],
            created_at: Utc::now(),
            preferred_agent: crate::project::AgentKind::Claude,
            is_git: true,
        };

        let mut store = empty_store();
        store.projects.push(project);

        db.save_store(&store).unwrap();
        let loaded = db.load_store().unwrap();

        assert_eq!(loaded.projects.len(), 1);
        let lp = &loaded.projects[0];
        assert_eq!(lp.name, "my-project");
        assert_eq!(lp.repo, PathBuf::from("/tmp/repo"));

        assert_eq!(lp.features.len(), 1);
        let lf = &lp.features[0];
        assert_eq!(lf.name, "my-feature");
        assert_eq!(lf.mode, VibeMode::Vibe);
        assert!(lf.review);
        assert!(lf.ready);
        assert_eq!(lf.summary, Some("did some stuff".to_string()));
        assert_eq!(lf.nickname, Some("myf".to_string()));
        assert_eq!(
            lf.selected_plan_path,
            Some(PathBuf::from(
                "/tmp/repo/.worktrees/my-feature/docs/current-plan.md"
            )),
            "the feature's manual plan selection must survive a save/load round trip"
        );
        assert_eq!(
            lf.triage_source,
            Some(crate::project::TriageSource {
                pr_number: 42,
                source_feature_id: "feat-source".to_string(),
                pr_branch: "feature/my-feature".to_string(),
                base_sha: "abc123".to_string(),
            }),
            "the PR/source-feature link must survive a save/load round trip"
        );
        assert_eq!(
            lf.review_source,
            Some(crate::project::ReviewSource {
                source_feature_id: "feat-source".to_string(),
                target_branch: "feature/my-feature".to_string(),
                base_sha: "def456".to_string(),
            }),
            "the companion review feature's source link must survive a save/load round trip"
        );
        assert_eq!(
            lf.issue_source,
            Some(crate::project::IssueSource {
                host: "github.com".to_string(),
                owner: "acme".to_string(),
                repository: "widget".to_string(),
                number: 73,
                comment_status: crate::project::IssueCommentStatus::Posted,
            }),
            "the source issue link must survive without disturbing other associations"
        );

        assert_eq!(lf.sessions.len(), 1);
        let ls = &lf.sessions[0];
        assert_eq!(ls.kind, SessionKind::Claude);
        assert_eq!(ls.claude_session_id, Some("claude-abc123".to_string()));
        assert_eq!(
            ls.todo_reference,
            Some(TodoSessionReference {
                todo_id: "todo-123".to_string(),
                launched_from_todo_menu: true,
            })
        );
        assert!(ls.status_text.is_none()); // transient — never persisted
    }

    #[test]
    fn sort_order_preserved() {
        let (_tmp, db) = open_temp_db();

        let mut store = empty_store();
        for name in ["alpha", "beta", "gamma"] {
            store.projects.push(Project {
                id: format!("proj-{name}"),
                name: name.to_string(),
                repo: PathBuf::from(format!("/tmp/{name}")),
                collapsed: false,
                features: Vec::new(),
                created_at: Utc::now(),
                preferred_agent: crate::project::AgentKind::Claude,
                is_git: true,
            });
        }

        db.save_store(&store).unwrap();
        let loaded = db.load_store().unwrap();

        let names: Vec<&str> = loaded.projects.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn save_is_atomic_full_replace() {
        let (_tmp, db) = open_temp_db();

        let mut store = empty_store();
        store.projects.push(Project {
            id: "proj-old".to_string(),
            name: "old-project".to_string(),
            repo: PathBuf::from("/tmp/old"),
            collapsed: false,
            features: Vec::new(),
            created_at: Utc::now(),
            preferred_agent: crate::project::AgentKind::Claude,
            is_git: true,
        });
        db.save_store(&store).unwrap();

        // Replace with a completely different store.
        let mut store2 = empty_store();
        store2.projects.push(Project {
            id: "proj-new".to_string(),
            name: "new-project".to_string(),
            repo: PathBuf::from("/tmp/new"),
            collapsed: false,
            features: Vec::new(),
            created_at: Utc::now(),
            preferred_agent: crate::project::AgentKind::Claude,
            is_git: true,
        });
        db.save_store(&store2).unwrap();

        let loaded = db.load_store().unwrap();
        assert_eq!(loaded.projects.len(), 1);
        assert_eq!(loaded.projects[0].name, "new-project");
    }

    #[test]
    fn pending_worktree_script_not_persisted() {
        let (_tmp, db) = open_temp_db();

        let mut store = empty_store();
        store.projects.push(Project {
            id: "proj-1".to_string(),
            name: "my-project".to_string(),
            repo: PathBuf::from("/tmp/repo"),
            collapsed: false,
            features: vec![
                Feature {
                    id: "feat-keep".to_string(),
                    name: "keep".to_string(),
                    branch: "keep".to_string(),
                    workdir: PathBuf::from("/tmp/repo"),
                    is_worktree: false,
                    tmux_session: "amf-keep".to_string(),
                    sessions: Vec::new(),
                    collapsed: true,
                    mode: VibeMode::default(),
                    review: false,
                    plan_mode: false,
                    agent: crate::project::AgentKind::Claude,
                    enable_chrome: false,
                    remote_control: false,
                    pending_worktree_script: false,
                    ready: false,
                    status: crate::project::ProjectStatus::Stopped,
                    created_at: Utc::now(),
                    last_accessed: Utc::now(),
                    summary: None,
                    summary_updated_at: None,
                    nickname: None,
                    selected_plan_path: None,
                    triage_source: None,
                    review_source: None,
                    issue_source: None,
                },
                Feature {
                    id: "feat-skip".to_string(),
                    name: "skip".to_string(),
                    branch: "skip".to_string(),
                    workdir: PathBuf::from("/tmp/repo/.worktrees/skip"),
                    is_worktree: true,
                    tmux_session: "amf-skip".to_string(),
                    sessions: Vec::new(),
                    collapsed: true,
                    mode: VibeMode::default(),
                    review: false,
                    plan_mode: false,
                    agent: crate::project::AgentKind::Claude,
                    enable_chrome: false,
                    remote_control: false,
                    pending_worktree_script: true, // should be excluded
                    ready: false,
                    status: crate::project::ProjectStatus::Stopped,
                    created_at: Utc::now(),
                    last_accessed: Utc::now(),
                    summary: None,
                    summary_updated_at: None,
                    nickname: None,
                    selected_plan_path: None,
                    triage_source: None,
                    review_source: None,
                    issue_source: None,
                },
            ],
            created_at: Utc::now(),
            preferred_agent: crate::project::AgentKind::Claude,
            is_git: true,
        });

        db.save_store(&store).unwrap();
        let loaded = db.load_store().unwrap();

        assert_eq!(loaded.projects[0].features.len(), 1);
        assert_eq!(loaded.projects[0].features[0].name, "keep");
    }

    // ── cross-process coordination (AMF_PLAN.md Task 5) ─────────────

    #[test]
    fn fresh_database_has_version_zero() {
        let (_tmp, db) = open_temp_db();
        assert_eq!(current_version(&db.conn).unwrap(), 0);
    }

    #[test]
    fn save_checked_succeeds_and_advances_the_version_when_expectation_matches() {
        let (_tmp, db) = open_temp_db();
        let store = empty_store();

        let outcome = save_checked(&db.conn, &store, 0).unwrap();

        assert_eq!(outcome, SaveOutcome::Saved { new_version: 1 });
        assert_eq!(current_version(&db.conn).unwrap(), 1);
    }

    #[test]
    fn save_checked_reports_a_conflict_without_writing_when_expectation_is_stale() {
        let (_tmp, db) = open_temp_db();
        let mut store = empty_store();
        store.projects.push(Project {
            id: "proj-first".to_string(),
            name: "first-writer".to_string(),
            repo: PathBuf::from("/tmp/first"),
            collapsed: false,
            features: Vec::new(),
            created_at: Utc::now(),
            preferred_agent: crate::project::AgentKind::Claude,
            is_git: true,
        });
        // A first writer's save, establishing version 1.
        save_checked(&db.conn, &store, 0).unwrap();

        // A second writer, still expecting version 0 (as if it had loaded
        // before the first writer's save landed), tries to save something
        // else entirely.
        let mut stale_store = empty_store();
        stale_store.projects.push(Project {
            id: "proj-second".to_string(),
            name: "second-writer".to_string(),
            repo: PathBuf::from("/tmp/second"),
            collapsed: false,
            features: Vec::new(),
            created_at: Utc::now(),
            preferred_agent: crate::project::AgentKind::Claude,
            is_git: true,
        });
        let outcome = save_checked(&db.conn, &stale_store, 0).unwrap();

        assert_eq!(outcome, SaveOutcome::Conflict { current_version: 1 });
        // The rejected save must not have touched the table: the first
        // writer's data is exactly what full-replace `save` would otherwise
        // have silently discarded.
        let loaded = load(&db.conn).unwrap();
        assert_eq!(loaded.projects.len(), 1);
        assert_eq!(loaded.projects[0].name, "first-writer");
        assert_eq!(current_version(&db.conn).unwrap(), 1);
    }

    #[test]
    fn load_versioned_pairs_the_store_with_the_version_it_was_read_at() {
        let (_tmp, db) = open_temp_db();
        let store = empty_store();
        save_checked(&db.conn, &store, 0).unwrap();
        save_checked(&db.conn, &store, 1).unwrap();

        let (_loaded, version) = load_versioned(&db.conn).unwrap();

        assert_eq!(version, 2);
    }

    #[test]
    fn unconditional_save_also_advances_the_version() {
        // `save` (no expected-version check) is still the seed/merge path's
        // save at `AmfDb::open_or_seed` time; it must keep incrementing the
        // same counter `save_checked` reads; otherwise the very first
        // application-level save after a fresh seed would see a version
        // that does not match what is actually on disk.
        let (_tmp, db) = open_temp_db();
        let store = empty_store();

        save(&db.conn, &store).unwrap();

        assert_eq!(current_version(&db.conn).unwrap(), 1);
    }
}
