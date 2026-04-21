use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::PathBuf;

use crate::project::{
    AgentKind, Feature, FeatureSession, Project, ProjectStatus, ProjectStore, SessionBookmark,
    SessionKind, TokenUsageSourceMatch, VibeMode, CURRENT_PROJECT_STORE_VERSION,
};
use crate::token_tracking::TokenUsageSource;

// ── enum ↔ str helpers ───────────────────────────────────────

fn agent_to_str(a: &AgentKind) -> &'static str {
    match a {
        AgentKind::Claude => "claude",
        AgentKind::Opencode => "opencode",
        AgentKind::Codex => "codex",
        AgentKind::Pi => "pi",
    }
}

fn agent_from_str(s: &str) -> AgentKind {
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

    let mut bookmark_stmt = conn.prepare(
        "SELECT project_id, feature_id, session_id FROM session_bookmarks",
    )?;
    let session_bookmarks: Vec<SessionBookmark> = bookmark_stmt
        .query_map([], |row| {
            Ok(SessionBookmark {
                project_id: row.get(0)?,
                feature_id: row.get(1)?,
                session_id: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

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
        extra,
    })
}

fn load_features(conn: &Connection, project_id: &str) -> Result<Vec<Feature>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, branch, workdir, is_worktree, tmux_session,
                mode, review, plan_mode, agent, enable_chrome, status,
                summary, summary_updated_at, nickname, collapsed,
                created_at, last_accessed, ready
         FROM features WHERE project_id = ?1
         ORDER BY sort_order ASC, rowid ASC",
    )?;

    let rows: Vec<(
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
    )> = stmt
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
            pending_worktree_script: false,
            ready,
            status: status_from_str(&status_str),
            created_at: dt_from_str(&feat_created_at),
            last_accessed: dt_from_str(&last_accessed),
            summary,
            summary_updated_at: summary_updated_at_str.as_deref().map(dt_from_str),
            nickname,
        });
    }
    Ok(features)
}

fn load_sessions(conn: &Connection, feature_id: &str) -> Result<Vec<FeatureSession>> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, label, tmux_window, claude_session_id,
                token_usage_source, token_usage_source_match,
                created_at, command, on_stop, pre_check
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
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(sessions)
}

// ── save ─────────────────────────────────────────────────────

pub fn save(conn: &Connection, store: &ProjectStore) -> Result<()> {
    conn.execute_batch("BEGIN IMMEDIATE;")?;
    match do_save(conn, store) {
        Ok(()) => {
            conn.execute_batch("COMMIT;")?;
            Ok(())
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK;");
            Err(e)
        }
    }
}

fn do_save(conn: &Connection, store: &ProjectStore) -> Result<()> {
    // Full replace: CASCADE deletes features → sessions.
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
            params![bookmark.project_id, bookmark.feature_id, bookmark.session_id],
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
                    created_at, last_accessed, ready, sort_order
                ) VALUES (
                    ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21
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
                ],
            )?;

            for (si, session) in feature.sessions.iter().enumerate() {
                conn.execute(
                    "INSERT INTO feature_sessions (
                        id, feature_id, kind, label, tmux_window,
                        claude_session_id, token_usage_source,
                        token_usage_source_match, created_at,
                        command, on_stop, pre_check, sort_order
                    ) VALUES (
                        ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13
                    )",
                    params![
                        session.id,
                        feature.id,
                        kind_to_str(&session.kind),
                        session.label,
                        session.tmux_window,
                        session.claude_session_id,
                        session.token_usage_source.as_ref().map(source_to_json),
                        session
                            .token_usage_source_match
                            .as_ref()
                            .map(match_to_str),
                        dt_to_str(&session.created_at),
                        session.command,
                        session.on_stop,
                        session.pre_check,
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
            token_usage_source: None,
            token_usage_source_match: None,
            created_at: Utc::now(),
            command: None,
            on_stop: None,
            pre_check: None,
            status_text: None,
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
            pending_worktree_script: false,
            ready: true,
            status: crate::project::ProjectStatus::Idle,
            created_at: Utc::now(),
            last_accessed: Utc::now(),
            summary: Some("did some stuff".to_string()),
            summary_updated_at: Some(Utc::now()),
            nickname: Some("myf".to_string()),
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

        assert_eq!(lf.sessions.len(), 1);
        let ls = &lf.sessions[0];
        assert_eq!(ls.kind, SessionKind::Claude);
        assert_eq!(ls.claude_session_id, Some("claude-abc123".to_string()));
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
                    pending_worktree_script: false,
                    ready: false,
                    status: crate::project::ProjectStatus::Stopped,
                    created_at: Utc::now(),
                    last_accessed: Utc::now(),
                    summary: None,
                    summary_updated_at: None,
                    nickname: None,
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
                    pending_worktree_script: true, // should be excluded
                    ready: false,
                    status: crate::project::ProjectStatus::Stopped,
                    created_at: Utc::now(),
                    last_accessed: Utc::now(),
                    summary: None,
                    summary_updated_at: None,
                    nickname: None,
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
}
