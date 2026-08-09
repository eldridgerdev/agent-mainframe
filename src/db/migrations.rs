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
        (
            "Add prompt_templates table for the prompt library",
            MIGRATION_007,
        ),
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
        (
            "Add todo_lists + todos tables for per-project TODO lists",
            MIGRATION_011,
        ),
        (
            "Cache AI PR-review findings keyed by PR# + head SHA (split from pr_review_cache)",
            MIGRATION_012,
        ),
        (
            "Persist agent-written PR comment reply drafts",
            MIGRATION_013,
        ),
        (
            "Track the pre-fix PR head for accurate reply commit references",
            MIGRATION_014,
        ),
        (
            "Link a companion PR-triage feature back to its PR and source feature",
            MIGRATION_015,
        ),
        (
            "Persist plan-interview drafts and accepted transcripts per feature",
            MIGRATION_016,
        ),
        (
            "Track editors AMF launched per feature so they can be reclaimed on stop",
            MIGRATION_017,
        ),
        (
            "Record the editor process's own start time to survive PID recycling",
            MIGRATION_018,
        ),
        (
            "Add learning_sessions + learning_qa tables for Learning Mode",
            MIGRATION_019,
        ),
        (
            "Record whether a learning Q&A's captured selection is a diff excerpt",
            MIGRATION_020,
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

// NOTE: `todo_lists.project_id` / `feature_id` are deliberately plain TEXT
// columns with NO foreign-key reference to projects/features. `store::save`
// does a full `DELETE FROM projects` replace on every save, which would
// cascade-wipe these rows if they were FK-bound. Cleanup on project/feature
// deletion is therefore handled explicitly (see `db/todos.rs`). The
// `todos.list_id -> todo_lists.id` cascade is safe because `todo_lists` is
// never touched by the store full-replace.
const MIGRATION_011: &str = "
CREATE TABLE IF NOT EXISTS todo_lists (
    id          TEXT PRIMARY KEY,
    project_id  TEXT NOT NULL UNIQUE,
    feature_id  TEXT NOT NULL,
    carry_over  TEXT,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS todos (
    id                 TEXT PRIMARY KEY,
    list_id            TEXT NOT NULL REFERENCES todo_lists(id) ON DELETE CASCADE,
    title              TEXT NOT NULL,
    body               TEXT,
    priority           TEXT NOT NULL DEFAULT 'med',
    done               INTEGER NOT NULL DEFAULT 0,
    sort_order         INTEGER NOT NULL DEFAULT 0,
    spawned_session_id TEXT,
    created_at         TEXT NOT NULL,
    updated_at         TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_todos_list ON todos(list_id);
";

// AI-review findings used to ride inside `pr_review_cache` (as `ai_generated`/
// `ai_published` flags on ordinary `PrComment`s plus a `last_ai_review` field on
// `PrReview`). Split into its own table when AI review became its own workflow
// (see the plan doc's "does AI review belong in this pane" open question) —
// AI-review storage no longer needs to fit the comment-triage row shape at all.
const MIGRATION_012: &str = "
CREATE TABLE IF NOT EXISTS ai_review_cache (
    pr_number  INTEGER NOT NULL,
    head_sha   TEXT NOT NULL,
    json       TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (pr_number, head_sha)
);
CREATE INDEX IF NOT EXISTS idx_ai_review_cache_updated
    ON ai_review_cache(updated_at);
";

// A fix prompt carries a fresh request id for each comment. The agent sends its
// proposed reviewer-facing reply back through AMF's IPC command with that id;
// updating only the matching row prevents a late response from an older fix
// attempt from replacing the current draft.
const MIGRATION_013: &str = "
CREATE TABLE IF NOT EXISTS pr_comment_reply_drafts (
    pr_number  INTEGER NOT NULL,
    comment_id INTEGER NOT NULL,
    request_id TEXT NOT NULL,
    body       TEXT,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (pr_number, comment_id)
);
CREATE INDEX IF NOT EXISTS idx_pr_comment_reply_drafts_updated
    ON pr_comment_reply_drafts(updated_at);
";

const MIGRATION_014: &str = "
ALTER TABLE pr_comment_reply_drafts
    ADD COLUMN base_head_sha TEXT NOT NULL DEFAULT '';
";

/// A companion triage feature (PR Triage's `New feature…` fix target) can't be
/// tied back to its PR by branch — it deliberately sits on its own branch so
/// git can check it out alongside the source worktree. The link is stored as a
/// JSON blob rather than four columns: it's read and written whole, and only a
/// small minority of features ever have one.
const MIGRATION_015: &str = "
ALTER TABLE features ADD COLUMN triage_source TEXT;
";

/// A feature keeps at most one in-progress interview draft and one accepted
/// transcript, so the key is `(feature_id, stage)` rather than `feature_id`
/// alone: re-running the interview on a feature that already has an accepted
/// plan must be able to save progress without destroying the plan it is
/// revising.
///
/// `feature_id` is a plain TEXT column with NO foreign key, for the same reason
/// as `todo_lists` (see MIGRATION_011): `store::save` full-replaces `features`
/// on every save, which would cascade-wipe these rows. Cleanup on feature
/// deletion is handled explicitly in `db/plan_interviews.rs`.
///
/// `questions` and `answers` are JSON arrays rather than a child table. They
/// are only ever read and written whole, one interview at a time, and the
/// question shape (`PlanQuestion`, including config-authored select options and
/// the AI round each generated question came from) already has a serde
/// representation worth reusing.
const MIGRATION_016: &str = "
CREATE TABLE IF NOT EXISTS plan_interviews (
    feature_id          TEXT NOT NULL,
    stage               TEXT NOT NULL,
    feature_name        TEXT NOT NULL,
    brief               TEXT NOT NULL,
    questions           TEXT NOT NULL,
    answers             TEXT NOT NULL,
    plan                TEXT,
    ai_rounds_completed INTEGER NOT NULL DEFAULT 0,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL,
    PRIMARY KEY (feature_id, stage)
);

CREATE INDEX IF NOT EXISTS idx_plan_interviews_updated
    ON plan_interviews(updated_at);
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

/// Editors AMF launched for a feature, so stopping the feature can reclaim
/// them (and the language servers they run) instead of leaving multi-GiB
/// processes behind.
///
/// `feature_id` is a plain TEXT column with NO foreign key, for the same reason
/// as `todo_lists` (see MIGRATION_011): `store::save` full-replaces `features`
/// on every save, which would cascade-wipe these rows. Cleanup on feature
/// deletion is explicit, in `db/editors.rs`.
///
/// `dedicated` records whether AMF opened a window it owns (VS Code launched
/// with `--new-window`) or merely handed a path to an instance that was already
/// running. Only a dedicated launch may ever be killed.
///
/// `command` is the argv AMF spawned, kept as the identity check at kill time:
/// PIDs are recycled, and a bare liveness check would eventually signal an
/// unrelated process. `started_at` is AMF's own launch timestamp, used to
/// report age and to break ties in the report.
const MIGRATION_017: &str = "
CREATE TABLE IF NOT EXISTS launched_editors (
    id            TEXT PRIMARY KEY,
    feature_id    TEXT NOT NULL,
    session_id    TEXT,
    kind          TEXT NOT NULL,
    pid           INTEGER NOT NULL,
    worktree_path TEXT NOT NULL,
    dedicated     INTEGER NOT NULL DEFAULT 0,
    command       TEXT NOT NULL DEFAULT '',
    started_at    TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_launched_editors_feature
    ON launched_editors(feature_id);
";

/// The argv identity check of `MIGRATION_017` is not enough on its own: argv is
/// reproducible, so a recycled PID belonging to a *user-opened* window on the
/// same worktree passes it. `proc_started_at` records when the attributed
/// process itself started (`ps -o lstart=`), which the next holder of that PID
/// cannot match. Empty for rows written before this migration and for launches
/// whose owner was never resolved — both fall back to the argv check alone.
const MIGRATION_018: &str = "
ALTER TABLE launched_editors ADD COLUMN proc_started_at TEXT NOT NULL DEFAULT '';
";

// Learning Mode's Q&A history (see `docs/backlog/learning-mode-plan.md`).
// Shaped after MIGRATION_011's todo tables: `project_id` / `feature_id` are
// plain TEXT with no FK to projects/features (those live in the store tables
// and are rewritten wholesale on save), so cleanup on project deletion is
// explicit — see `learning::delete_sessions_for_project`.
//
// `project_id` is deliberately *not* UNIQUE: whether a project has one
// learning session or many is still open (the plan's "learning-session
// lifecycle" question), and v1's one-per-project behaviour is enforced in
// `load_or_create_session` rather than baked into the schema.
//
// `parent_qa_id` self-references so a follow-up thread dies with the question
// it hangs off, matching the session→Q&A cascade.
const MIGRATION_019: &str = "
CREATE TABLE IF NOT EXISTS learning_sessions (
    id              TEXT PRIMARY KEY,
    project_id      TEXT NOT NULL,
    feature_id      TEXT NOT NULL,
    title           TEXT NOT NULL DEFAULT '',
    harness         TEXT NOT NULL DEFAULT 'claude',
    level           TEXT NOT NULL DEFAULT 'newcomer',
    onboarding_seen INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_learning_sessions_project
    ON learning_sessions(project_id);

CREATE TABLE IF NOT EXISTS learning_qa (
    id                  TEXT PRIMARY KEY,
    learning_session_id TEXT NOT NULL
                        REFERENCES learning_sessions(id) ON DELETE CASCADE,
    parent_qa_id        TEXT REFERENCES learning_qa(id) ON DELETE CASCADE,
    file_path           TEXT,
    anchor_kind         TEXT NOT NULL DEFAULT 'file',
    line_start          INTEGER,
    line_end            INTEGER,
    selection_text      TEXT NOT NULL DEFAULT '',
    question            TEXT NOT NULL,
    intent              TEXT NOT NULL DEFAULT 'explain',
    level               TEXT NOT NULL DEFAULT 'newcomer',
    answer              TEXT,
    harness             TEXT NOT NULL DEFAULT 'claude',
    run_mode            TEXT NOT NULL DEFAULT 'no_tools',
    status              TEXT NOT NULL DEFAULT 'pending',
    error               TEXT,
    todo_id             TEXT,
    spawned_session_id  TEXT,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_learning_qa_session
    ON learning_qa(learning_session_id);
CREATE INDEX IF NOT EXISTS idx_learning_qa_parent
    ON learning_qa(parent_qa_id);
";

// Whether a row's captured `selection_text` is a unified-diff excerpt rather
// than plain source.
//
// It cannot be re-derived from the row: a line anchor looks identical whether
// it came from the repo tree or from a diff, and the browse scope that would
// have told them apart isn't stored. Without it a follow-up asked after the
// user browsed elsewhere would present its parent's diff excerpt as ordinary
// numbered source (or the reverse), which is exactly the confusion the `+`/`-`
// markers exist to prevent. Existing rows default to 0: plain source is the
// safer wrong answer, since a marker-free block read as a diff would be.
const MIGRATION_020: &str = "
ALTER TABLE learning_qa
    ADD COLUMN selection_is_diff INTEGER NOT NULL DEFAULT 0;
";

#[cfg(test)]
mod tests {
    use rusqlite::{Connection, params};

    /// A DB last touched before Learning Mode existed (schema version 18)
    /// upgrades in place: only 019 replays, and it lands the two new tables.
    #[test]
    fn migration_019_upgrades_a_pre_learning_database() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(super::MIGRATION_001).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL, description TEXT NOT NULL);
             INSERT INTO schema_version VALUES (18, datetime('now'), 'seed');",
        )
        .unwrap();

        super::run(&conn).unwrap();

        let version: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        // `run` doesn't stop at 019 — it carries on through every later
        // migration, so the DB lands at the newest version, not at 19.
        assert_eq!(version, 20);
        for table in ["learning_sessions", "learning_qa"] {
            let found: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    params![table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(found, 1, "{table} should exist after migration 019");
        }
    }

    /// Migration 020 adds `selection_is_diff` to rows written before it
    /// existed. They default to 0 — plain source — which is the safe wrong
    /// answer: a marker-free block presented as a diff would confuse the agent,
    /// whereas a diff presented as source is what those rows already got.
    #[test]
    fn migration_020_backfills_existing_qa_rows_as_plain_source() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(super::MIGRATION_001).unwrap();
        conn.execute_batch(super::MIGRATION_019).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL, description TEXT NOT NULL);
             INSERT INTO schema_version VALUES (19, datetime('now'), 'seed');",
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO learning_sessions
                (id, project_id, feature_id, title, created_at, updated_at)
             VALUES ('s1', 'p1', 'f1', 'amf', datetime('now'), datetime('now'));
             INSERT INTO learning_qa
                (id, learning_session_id, question, created_at, updated_at)
             VALUES ('q1', 's1', 'What is this?', datetime('now'), datetime('now'));",
        )
        .unwrap();

        super::run(&conn).unwrap();

        let is_diff: i64 = conn
            .query_row(
                "SELECT selection_is_diff FROM learning_qa WHERE id = 'q1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(is_diff, 0, "an existing row is treated as plain source");
    }

    /// A brand-new DB arrives at the same place in one pass.
    #[test]
    fn fresh_database_lands_at_the_latest_version() {
        let conn = Connection::open_in_memory().unwrap();
        super::run(&conn).unwrap();
        let version: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 20);
    }

    /// Replaying `run` over an already-migrated DB is a no-op, so a rollback to
    /// an older AMF and back doesn't duplicate or drop anything.
    #[test]
    fn migrations_are_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        super::run(&conn).unwrap();
        super::run(&conn).unwrap();
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 20);
    }

    /// Migration 010 re-keys triage on `PR# + comment id`: rows that the old
    /// (head-SHA-keyed) schema recorded once per SHA collapse to a single row
    /// per comment, keeping the most recently updated state.
    #[test]
    fn migration_010_collapses_per_sha_triage_rows() {
        let conn = Connection::open_in_memory().unwrap();
        // Stand up the v009 schema and seed it before 010 runs. The base
        // schema comes along too: later migrations (015 onwards) alter tables
        // 001 created, and this DB is replayed through all of them.
        conn.execute_batch(super::MIGRATION_001).unwrap();
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
            params![
                7,
                1,
                "old",
                "fixing",
                Option::<&str>::None,
                "2026-01-01 00:00:00"
            ],
        )
        .unwrap();
        conn.execute(
            insert,
            params![
                7,
                1,
                "new",
                "done",
                Option::<&str>::None,
                "2026-02-01 00:00:00"
            ],
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
            .prepare(
                "SELECT comment_id, state, head_sha FROM pr_comment_triage ORDER BY comment_id",
            )
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
            params![
                7,
                1,
                "newer",
                "fixing",
                Option::<&str>::None,
                "2026-03-01 00:00:00"
            ],
        );
        assert!(dup.is_err(), "PK should now be (pr_number, comment_id)");
    }
}
