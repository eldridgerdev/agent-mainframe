use anyhow::Result;
use rusqlite::Connection;

pub(super) fn run(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version     INTEGER PRIMARY KEY,
            applied_at  TEXT NOT NULL,
            description TEXT NOT NULL
        );",
    )?;

    let version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    let migrations: &[(&str, &str)] = &[
        (
            "Initial schema: projects, features, sessions, bookmarks",
            MIGRATION_001,
        ),
        ("Persist token usage cache across restarts", MIGRATION_002),
        ("Replace unbounded debug.log file with capped DB table", MIGRATION_003),
    ];

    for (i, (desc, sql)) in migrations.iter().enumerate() {
        let target = (i + 1) as i64;
        if version < target {
            conn.execute_batch(sql)?;
            conn.execute(
                "INSERT INTO schema_version (version, applied_at, description)
                 VALUES (?1, datetime('now'), ?2)",
                rusqlite::params![target, desc],
            )?;
        }
    }

    Ok(())
}

const MIGRATION_002: &str = "
CREATE TABLE IF NOT EXISTS token_usage_cache (
    source_provider   TEXT NOT NULL,
    source_id         TEXT NOT NULL,
    signature         INTEGER,
    has_usage         INTEGER NOT NULL DEFAULT 0,
    input_tokens      INTEGER NOT NULL DEFAULT 0,
    output_tokens     INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
    reasoning_tokens  INTEGER NOT NULL DEFAULT 0,
    total_tokens      INTEGER NOT NULL DEFAULT 0,
    updated_at        TEXT NOT NULL,
    PRIMARY KEY (source_provider, source_id)
);
CREATE INDEX IF NOT EXISTS idx_token_cache_updated
    ON token_usage_cache(updated_at);
";

const MIGRATION_003: &str = "
CREATE TABLE IF NOT EXISTS debug_log (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    ts      TEXT NOT NULL,
    level   TEXT NOT NULL,
    context TEXT NOT NULL,
    message TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_debug_log_ts ON debug_log(ts DESC);
CREATE TRIGGER IF NOT EXISTS debug_log_cap
AFTER INSERT ON debug_log
BEGIN
    DELETE FROM debug_log
    WHERE id <= (
        SELECT id FROM debug_log ORDER BY id DESC LIMIT 1 OFFSET 10000
    );
END;
";

const MIGRATION_001: &str = "
CREATE TABLE IF NOT EXISTS store_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS projects (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    repo            TEXT NOT NULL,
    collapsed       INTEGER NOT NULL DEFAULT 0,
    preferred_agent TEXT NOT NULL DEFAULT 'claude',
    is_git          INTEGER NOT NULL DEFAULT 1,
    created_at      TEXT NOT NULL,
    sort_order      INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS features (
    id                 TEXT PRIMARY KEY,
    project_id         TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name               TEXT NOT NULL,
    branch             TEXT NOT NULL,
    workdir            TEXT NOT NULL,
    is_worktree        INTEGER NOT NULL DEFAULT 0,
    tmux_session       TEXT NOT NULL DEFAULT '',
    mode               TEXT NOT NULL DEFAULT 'vibeless',
    review             INTEGER NOT NULL DEFAULT 0,
    plan_mode          INTEGER NOT NULL DEFAULT 0,
    agent              TEXT NOT NULL DEFAULT 'claude',
    enable_chrome      INTEGER NOT NULL DEFAULT 0,
    status             TEXT NOT NULL DEFAULT 'stopped',
    summary            TEXT,
    summary_updated_at TEXT,
    nickname           TEXT,
    collapsed          INTEGER NOT NULL DEFAULT 1,
    created_at         TEXT NOT NULL,
    last_accessed      TEXT NOT NULL,
    ready              INTEGER NOT NULL DEFAULT 0,
    sort_order         INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_features_project
    ON features(project_id);

CREATE TABLE IF NOT EXISTS feature_sessions (
    id                       TEXT PRIMARY KEY,
    feature_id               TEXT NOT NULL REFERENCES features(id) ON DELETE CASCADE,
    kind                     TEXT NOT NULL,
    label                    TEXT NOT NULL DEFAULT '',
    tmux_window              TEXT NOT NULL DEFAULT '',
    claude_session_id        TEXT,
    token_usage_source       TEXT,
    token_usage_source_match TEXT,
    created_at               TEXT NOT NULL,
    command                  TEXT,
    on_stop                  TEXT,
    pre_check                TEXT,
    sort_order               INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_sessions_feature
    ON feature_sessions(feature_id);
CREATE INDEX IF NOT EXISTS idx_sessions_claude_id
    ON feature_sessions(claude_session_id)
    WHERE claude_session_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS session_bookmarks (
    project_id TEXT NOT NULL,
    feature_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    PRIMARY KEY (project_id, feature_id, session_id)
);
";
