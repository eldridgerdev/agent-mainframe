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
        (
            "Replace unbounded debug.log file with capped DB table",
            MIGRATION_003,
        ),
        (
            "Replace per-file session-status with DB table",
            MIGRATION_004,
        ),
        (
            "Add file_mtime_nanos to session_status for cache invalidation",
            MIGRATION_005,
        ),
        (
            "Add incremental parse state to token usage cache",
            MIGRATION_006,
        ),
        ("Add prompt_templates table for the prompt library", MIGRATION_007),
        (
            "Cache normalized PR reviews keyed by PR# + head SHA",
            MIGRATION_008,
        ),
        (
            "Persist local PR comment triage state (fixing/done/…)",
            MIGRATION_009,
        ),
        (
            "Re-key PR comment triage by PR# + comment id (survive head-SHA changes)",
            MIGRATION_010,
        ),
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

const MIGRATION_004: &str = "
CREATE TABLE IF NOT EXISTS session_status (
    session_id  TEXT PRIMARY KEY,
    feature_id  TEXT NOT NULL REFERENCES features(id) ON DELETE CASCADE,
    status_text TEXT NOT NULL DEFAULT '',
    updated_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_session_status_feature
    ON session_status(feature_id);
";

const MIGRATION_005: &str = "
ALTER TABLE session_status ADD COLUMN file_mtime_nanos INTEGER;
";

const MIGRATION_006: &str = "
ALTER TABLE token_usage_cache ADD COLUMN parse_state TEXT;
";

const MIGRATION_007: &str = "
CREATE TABLE IF NOT EXISTS prompt_templates (
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL,
    description  TEXT,
    body         TEXT NOT NULL,
    tags         TEXT NOT NULL DEFAULT '[]',
    placeholders TEXT NOT NULL DEFAULT '[]',
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    sort_order   INTEGER NOT NULL DEFAULT 0
);
";

const MIGRATION_008: &str = "
CREATE TABLE IF NOT EXISTS pr_review_cache (
    pr_number  INTEGER NOT NULL,
    head_sha   TEXT NOT NULL,
    json       TEXT NOT NULL,
    fetched_at TEXT NOT NULL,
    PRIMARY KEY (pr_number, head_sha)
);
CREATE INDEX IF NOT EXISTS idx_pr_review_cache_fetched
    ON pr_review_cache(fetched_at);
";

const MIGRATION_009: &str = "
CREATE TABLE IF NOT EXISTS pr_comment_triage (
    pr_number  INTEGER NOT NULL,
    comment_id INTEGER NOT NULL,
    head_sha   TEXT NOT NULL,
    state      TEXT NOT NULL,
    note       TEXT,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (pr_number, comment_id, head_sha)
);
CREATE INDEX IF NOT EXISTS idx_pr_comment_triage_updated
    ON pr_comment_triage(updated_at);
";

// Triage is local, per-comment state (done / skipped / fixing). The original
// schema keyed it by `PR# + comment id + head SHA`, which meant a push that
// moved the PR head silently dropped every mark on the next re-resolve. A
// comment's GitHub id is stable across commits, so re-key triage on
// `PR# + comment id` and keep `head_sha` only as an informational record of the
// last SHA a mark was set under. Collapse any per-SHA duplicates from the old
// schema, keeping the most recently updated row per comment (SQLite's
// bare-column-with-MAX() rule pulls the rest of that row's fields).
const MIGRATION_010: &str = "
CREATE TABLE pr_comment_triage_rekeyed (
    pr_number  INTEGER NOT NULL,
    comment_id INTEGER NOT NULL,
    head_sha   TEXT NOT NULL,
    state      TEXT NOT NULL,
    note       TEXT,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (pr_number, comment_id)
);
INSERT INTO pr_comment_triage_rekeyed
    (pr_number, comment_id, head_sha, state, note, updated_at)
SELECT pr_number, comment_id, head_sha, state, note, MAX(updated_at)
FROM pr_comment_triage
GROUP BY pr_number, comment_id;
DROP TABLE pr_comment_triage;
ALTER TABLE pr_comment_triage_rekeyed RENAME TO pr_comment_triage;
CREATE INDEX IF NOT EXISTS idx_pr_comment_triage_updated
    ON pr_comment_triage(updated_at);
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

#[cfg(test)]
mod tests {
    use rusqlite::{Connection, params};

    /// Migration 010 re-keys triage on `PR# + comment id`: rows that the old
    /// (head-SHA-keyed) schema recorded once per SHA collapse to a single row
    /// per comment, keeping the most recently updated state.
    #[test]
    fn migration_010_collapses_per_sha_triage_rows() {
        let conn = Connection::open_in_memory().unwrap();
        // Stand up the v009 schema and seed it before 010 runs.
        conn.execute_batch(super::MIGRATION_009).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL, description TEXT NOT NULL);
             INSERT INTO schema_version VALUES (9, datetime('now'), 'seed');",
        )
        .unwrap();

        let insert = "INSERT INTO pr_comment_triage
            (pr_number, comment_id, head_sha, state, note, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)";
        // Same comment marked under two head SHAs: the newer mark should win.
        conn.execute(
            insert,
            params![7, 1, "old", "fixing", Option::<&str>::None, "2026-01-01 00:00:00"],
        )
        .unwrap();
        conn.execute(
            insert,
            params![7, 1, "new", "done", Option::<&str>::None, "2026-02-01 00:00:00"],
        )
        .unwrap();
        // A distinct comment is preserved as its own row.
        conn.execute(
            insert,
            params![7, 2, "old", "skipped", Some("nope"), "2026-01-15 00:00:00"],
        )
        .unwrap();

        super::run(&conn).unwrap();

        // Comment 1 collapsed to the newer "done"; comment 2 survived.
        let mut stmt = conn
            .prepare("SELECT comment_id, state, head_sha FROM pr_comment_triage ORDER BY comment_id")
            .unwrap();
        let rows: Vec<(i64, String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            rows,
            vec![
                (1, "done".to_string(), "new".to_string()),
                (2, "skipped".to_string(), "old".to_string()),
            ]
        );

        // The re-keyed table rejects a second row for an existing comment.
        let dup = conn.execute(
            insert,
            params![7, 1, "newer", "fixing", Option::<&str>::None, "2026-03-01 00:00:00"],
        );
        assert!(dup.is_err(), "PK should now be (pr_number, comment_id)");
    }
}
