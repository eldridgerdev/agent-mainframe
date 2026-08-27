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
        (
            "Record which learning Q&A a deep dive re-ran, apart from its thread parent",
            MIGRATION_021,
        ),
        (
            "Pin an agent reply draft to the session that generated it",
            MIGRATION_022,
        ),
        (
            "Link a TODO to the feature a plan-mode run created for it",
            MIGRATION_023,
        ),
        (
            "Mark a TODO as actively being worked, apart from its session link",
            MIGRATION_024,
        ),
        (
            "Scope TODO lists to a worktree, a project, or the machine",
            MIGRATION_025,
        ),
        (
            "Cache a branch's terminal (merged/closed) PR state, keyed by repo + branch",
            MIGRATION_026,
        ),
        (
            "Persist a manually selected plan path per feature",
            MIGRATION_027,
        ),
        (
            "Replace TODO completion flags with a three-state status and agent association",
            MIGRATION_028,
        ),
        (
            "Persist TODO-menu launch references on agent sessions",
            MIGRATION_029,
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

// Which row a deep dive re-ran, kept apart from `parent_qa_id`.
//
// A deep dive is stored under the answer it checks so the pair renders
// together, but it does not *continue* that answer — it replaces it. Ancestor
// traversal has to tell the two apart or a follow-up on a deep dive carries the
// shallow answer (the one that may have invented files) back into the prompt.
// `run_mode` cannot stand in for it: `effective_for` records every Codex row as
// a deep dive, follow-ups included.
//
// No FK: the row it names is always this row's `parent_qa_id` too, so the
// existing self-referencing cascade already takes the pair away together.
// Existing rows default to NULL — "an ordinary follow-up" is the safe reading,
// since it only ever adds context rather than dropping it.
const MIGRATION_021: &str = "
ALTER TABLE learning_qa
    ADD COLUMN deep_dive_of_qa_id TEXT;
";

// Which agent session produced a reply draft, recorded when the fix is injected
// rather than re-derived when the reply dialog opens.
//
// The disclosure posted with an unedited AI draft names a harness, a model and a
// usage estimate. Reading those off the pane's *current* fix target attributes
// the draft to whatever session happens to be selected now: re-opening PR Triage
// resets the target to the default, and removing the session leaves nothing to
// read at all, so a draft written by one agent could be posted under another's
// name — or lose its disclosure entirely.
//
// One JSON column rather than four: it is written and read whole, only rows with
// an agent draft ever have one, and the usage baseline inside it is a struct the
// token tracker owns. NULL is the honest reading for a draft begun before this
// column existed (or with no resolvable session): the metadata is then reported
// as unavailable instead of guessed.
const MIGRATION_022: &str = "
ALTER TABLE pr_comment_reply_drafts
    ADD COLUMN provenance TEXT;
";

/// A TODO can point at a feature that plan mode created for it, separately from
/// `spawned_session_id`, which points at a session inside the TODO's *host*
/// feature. The two are different destinations and a TODO can accumulate both,
/// so this is a new column rather than a reuse of that one.
///
/// Plain TEXT with no FK, like every other id in the todo tables: `store::save`
/// full-replaces `features`, which would cascade-wipe these rows. A feature
/// deleted out from under a TODO is repaired explicitly in `app/feature_ops.rs`.
const MIGRATION_023: &str = "
ALTER TABLE todos
    ADD COLUMN linked_feature_id TEXT;
";

/// A TODO that is actively being worked, recorded rather than derived.
///
/// `spawned_session_id` cannot stand in for it: a session link survives the
/// work being abandoned, is absent for a TODO the user started by hand, and
/// points at a session that may be reused for something else. "Implement next"
/// needs to skip what is already underway *and* be told when it is no longer
/// underway, so the state has to be writable on its own.
///
/// Existing rows default to 0. "Not started" is the safe reading: the worst it
/// costs is offering a TODO whose work is already in flight, which the
/// already-linked prompt then catches, whereas defaulting to 1 would hide every
/// pre-existing TODO from the scan with no way to discover why.
const MIGRATION_024: &str = "
ALTER TABLE todos
    ADD COLUMN in_progress INTEGER NOT NULL DEFAULT 0;
";

/// Give a TODO list a **scope**: the worktree it belongs to, the project it
/// belongs to, or nothing at all (the machine-wide global list).
///
/// This is a table rebuild rather than a set of `ALTER TABLE`s because two of
/// the three changes cannot be expressed any other way in SQLite: dropping the
/// `project_id UNIQUE` constraint (a project now owns one project-scoped list
/// *and* one list per worktree) and relaxing `project_id` / `feature_id` to
/// nullable (the global list has neither).
///
/// The rebuild runs with `foreign_keys` off, and that is load-bearing in both
/// directions. `DROP TABLE todo_lists` with foreign keys on would fire
/// `todos`' `ON DELETE CASCADE` and take every TODO in the database with it;
/// and `ALTER TABLE ... RENAME TO` with them on would rewrite `todos`' own
/// `REFERENCES todo_lists(id)` clause to point at the temporary name. With
/// them off, neither happens and `todos` ends up referencing the rebuilt table
/// under its original name.
///
/// Every existing row is backfilled to `scope = 'project'` with its
/// `carry_over` scratchpad, host feature, and id all preserved — an upgrade
/// leaves the lists people already have exactly where they were, and the new
/// worktree lists start empty.
///
/// The three partial unique indexes are what the old single UNIQUE used to be:
/// one project-scoped list per project, one worktree list per
/// (project, workdir), and exactly one global list on the machine.
const MIGRATION_025: &str = "
PRAGMA foreign_keys=OFF;

CREATE TABLE todo_lists_new (
    id          TEXT PRIMARY KEY,
    project_id  TEXT,
    feature_id  TEXT,
    scope       TEXT NOT NULL DEFAULT 'project',
    workdir     TEXT,
    carry_over  TEXT,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

INSERT INTO todo_lists_new
    (id, project_id, feature_id, scope, workdir, carry_over, created_at, updated_at)
SELECT id, project_id, feature_id, 'project', NULL, carry_over, created_at, updated_at
FROM todo_lists;

DROP TABLE todo_lists;
ALTER TABLE todo_lists_new RENAME TO todo_lists;

CREATE UNIQUE INDEX idx_todo_lists_project
    ON todo_lists(project_id) WHERE scope = 'project';
CREATE UNIQUE INDEX idx_todo_lists_worktree
    ON todo_lists(project_id, workdir) WHERE scope = 'worktree';
CREATE UNIQUE INDEX idx_todo_lists_global
    ON todo_lists(scope) WHERE scope = 'global';

PRAGMA foreign_keys=ON;
";

/// A branch's terminal PR state never goes stale — merged and closed never
/// revert — so it is keyed by `(repo, branch)` rather than by feature id:
/// that identity outlives any one feature/worktree pointing at the branch,
/// and a row is looked up again if a deleted feature's branch is ever reused.
/// No foreign key to `todo_lists`/features on purpose, for the same reason.
const MIGRATION_026: &str = "
CREATE TABLE IF NOT EXISTS pr_terminal_state (
    repo       TEXT NOT NULL,
    branch     TEXT NOT NULL,
    pr_number  INTEGER NOT NULL,
    state      TEXT NOT NULL,
    at         TEXT NOT NULL,
    PRIMARY KEY (repo, branch)
);
";

const MIGRATION_027: &str = "
ALTER TABLE features ADD COLUMN selected_plan_path TEXT;
";

/// Replace the two independent lifecycle flags with one exhaustive status and
/// give the TODO-specific agent link an explicit, harness-neutral name.
///
/// Existing completed work stays completed and keeps its session association.
/// Every incomplete row becomes not started with no association, including a
/// row that happened to have the old `in_progress` bit or a historical spawned
/// session. This is the settled migration rule: AMF must not infer that old
/// links still represent active TODO-specific launches.
const MIGRATION_028: &str = "
ALTER TABLE todos
    ADD COLUMN status TEXT NOT NULL DEFAULT 'not_started'
        CHECK (status IN ('not_started', 'in_progress', 'completed'));
ALTER TABLE todos
    ADD COLUMN agent_session_id TEXT;

UPDATE todos
SET status = CASE WHEN done != 0 THEN 'completed' ELSE 'not_started' END,
    agent_session_id = CASE WHEN done != 0 THEN spawned_session_id ELSE NULL END;
";

/// A TODO reference belongs to an AMF agent session, rather than to the TODO's
/// lifecycle/work-state link.  Existing sessions predate this explicit launch
/// provenance and therefore remain unreferenced.
///
/// No index on `todo_id`: sessions are always loaded by `feature_id` and the
/// reverse lookup (clearing references for a deleted TODO) walks the in-memory
/// store, so an index would be dead weight.
const MIGRATION_029: &str = "
ALTER TABLE feature_sessions ADD COLUMN todo_id TEXT;
ALTER TABLE feature_sessions
    ADD COLUMN todo_launched_from_menu INTEGER NOT NULL DEFAULT 0;
";

#[cfg(test)]
mod tests {
    use rusqlite::{Connection, params};

    /// The tables a DB last touched around v018 actually has: 001's base schema,
    /// the todo tables 011 built, and the reply-draft table 013/014 built.
    /// Fixtures that seed a version this high have to stand up everything later
    /// migrations alter — 022 alters `pr_comment_reply_drafts` and 023 alters
    /// `todos`.
    fn seed_pre_learning_schema(conn: &Connection) {
        conn.execute_batch(super::MIGRATION_001).unwrap();
        conn.execute_batch(super::MIGRATION_011).unwrap();
        conn.execute_batch(super::MIGRATION_013).unwrap();
        conn.execute_batch(super::MIGRATION_014).unwrap();
    }

    /// A DB last touched before Learning Mode existed (schema version 18)
    /// upgrades in place: only 019 replays, and it lands the two new tables.
    #[test]
    fn migration_019_upgrades_a_pre_learning_database() {
        let conn = Connection::open_in_memory().unwrap();
        seed_pre_learning_schema(&conn);
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
        assert_eq!(version, 29);
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
        seed_pre_learning_schema(&conn);
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

    /// Migration 021 adds `deep_dive_of_qa_id` to rows written before it
    /// existed. They come back NULL — "an ordinary follow-up" — so ancestor
    /// traversal over old threads behaves exactly as it did before.
    #[test]
    fn migration_021_leaves_existing_qa_rows_without_a_rerun_origin() {
        let conn = Connection::open_in_memory().unwrap();
        seed_pre_learning_schema(&conn);
        conn.execute_batch(super::MIGRATION_019).unwrap();
        conn.execute_batch(super::MIGRATION_020).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL, description TEXT NOT NULL);
             INSERT INTO schema_version VALUES (20, datetime('now'), 'seed');",
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

        let origin: Option<String> = conn
            .query_row(
                "SELECT deep_dive_of_qa_id FROM learning_qa WHERE id = 'q1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(origin, None);
    }

    /// A brand-new DB arrives at the same place in one pass.
    #[test]
    fn fresh_database_lands_at_the_latest_version() {
        let conn = Connection::open_in_memory().unwrap();
        super::run(&conn).unwrap();
        let version: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 29);
    }

    #[test]
    fn migration_029_leaves_existing_sessions_without_todo_provenance() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(super::MIGRATION_001).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL, description TEXT NOT NULL);
             INSERT INTO schema_version VALUES (28, datetime('now'), 'seed');
             INSERT INTO projects (id, name, repo, created_at)
                VALUES ('p1', 'project', '/repo', datetime('now'));
             INSERT INTO features (id, project_id, name, branch, workdir, created_at, last_accessed)
                VALUES ('f1', 'p1', 'feature', 'main', '/repo', datetime('now'), datetime('now'));
             INSERT INTO feature_sessions (id, feature_id, kind, created_at)
                VALUES ('s1', 'f1', 'claude', datetime('now'));",
        )
        .unwrap();

        super::run(&conn).unwrap();

        let reference: (Option<String>, i64) = conn
            .query_row(
                "SELECT todo_id, todo_launched_from_menu
                 FROM feature_sessions WHERE id = 's1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(reference, (None, 0));
    }

    /// Migration 023 adds `linked_feature_id` to TODOs written before it
    /// existed. They come back NULL — "never planned into a feature" — which is
    /// what every pre-023 TODO actually is, and leaves `spawned_session_id`
    /// (the other, older link) untouched.
    #[test]
    fn migration_023_leaves_existing_todos_unlinked_but_keeps_their_session() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(super::MIGRATION_001).unwrap();
        conn.execute_batch(super::MIGRATION_011).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL, description TEXT NOT NULL);
             INSERT INTO schema_version VALUES (22, datetime('now'), 'seed');",
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO todo_lists (id, project_id, feature_id, created_at, updated_at)
             VALUES ('l1', 'p1', 'f1', datetime('now'), datetime('now'));
             INSERT INTO todos
                (id, list_id, title, spawned_session_id, created_at, updated_at)
             VALUES ('t1', 'l1', 'ship it', 'sess-1',
                     datetime('now'), datetime('now'));",
        )
        .unwrap();

        super::run(&conn).unwrap();

        let (linked, session): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT linked_feature_id, spawned_session_id FROM todos WHERE id = 't1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(linked, None, "an existing TODO has no planned feature");
        assert_eq!(
            session.as_deref(),
            Some("sess-1"),
            "the older link survives"
        );
    }

    /// Migration 024 adds `in_progress` to TODOs written before it existed.
    /// They come back false — "nobody has started this" — which keeps every
    /// pre-024 TODO eligible for "implement next" rather than hiding it.
    #[test]
    fn migration_024_leaves_existing_todos_not_in_progress() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(super::MIGRATION_001).unwrap();
        conn.execute_batch(super::MIGRATION_011).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL, description TEXT NOT NULL);
             INSERT INTO schema_version VALUES (23, datetime('now'), 'seed');",
        )
        .unwrap();
        // The row has to look like a pre-024 row, which 023 already reached.
        conn.execute_batch(super::MIGRATION_023).unwrap();
        conn.execute_batch(
            "INSERT INTO todo_lists (id, project_id, feature_id, created_at, updated_at)
             VALUES ('l1', 'p1', 'f1', datetime('now'), datetime('now'));
             INSERT INTO todos
                (id, list_id, title, done, spawned_session_id, created_at, updated_at)
             VALUES ('t1', 'l1', 'ship it', 0, 'sess-1',
                     datetime('now'), datetime('now'));",
        )
        .unwrap();

        super::run(&conn).unwrap();

        let (in_progress, session): (i64, Option<String>) = conn
            .query_row(
                "SELECT in_progress, spawned_session_id FROM todos WHERE id = 't1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(in_progress, 0);
        // The older link is untouched: a pre-existing session is still the
        // TODO's session, it just isn't evidence that work is underway.
        assert_eq!(session.as_deref(), Some("sess-1"));
    }

    #[test]
    fn migration_028_preserves_todo_data_and_normalizes_work_state() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(super::MIGRATION_001).unwrap();
        conn.execute_batch(super::MIGRATION_011).unwrap();
        conn.execute_batch(super::MIGRATION_023).unwrap();
        conn.execute_batch(super::MIGRATION_024).unwrap();
        conn.execute_batch(super::MIGRATION_025).unwrap();
        conn.execute_batch(super::MIGRATION_026).unwrap();
        conn.execute_batch(super::MIGRATION_027).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL, description TEXT NOT NULL);
             INSERT INTO schema_version VALUES (27, datetime('now'), 'seed');

             INSERT INTO todo_lists
                (id, project_id, feature_id, scope, workdir, carry_over,
                 created_at, updated_at)
             VALUES
                ('lp', 'p1', 'f1', 'project', NULL, 'project scratchpad',
                 '2025-01-01', '2025-01-02'),
                ('lw', 'p1', 'fw', 'worktree', '/repo/.worktrees/w', 'worktree notes',
                 '2025-02-01', '2025-02-02'),
                ('lg', NULL, NULL, 'global', NULL, 'global notes',
                 '2025-03-01', '2025-03-02');

             INSERT INTO todos
                (id, list_id, title, body, priority, done, sort_order,
                 spawned_session_id, linked_feature_id, in_progress,
                 created_at, updated_at)
             VALUES
                ('open', 'lw', 'Open item', 'keep body', 'high', 0, 7,
                 'stale-session', 'linked-feature', 1,
                 '2025-04-01', '2025-04-02'),
                ('done', 'lp', 'Done item', 'done body', 'low', 1, 3,
                 'completed-session', NULL, 0,
                 '2025-05-01', '2025-05-02');",
        )
        .unwrap();

        super::run(&conn).unwrap();

        let open: (String, Option<String>, String, String, i64, String, String) = conn
            .query_row(
                "SELECT status, agent_session_id, body, priority, sort_order,
                        created_at, updated_at
                 FROM todos WHERE id = 'open'",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(open.0, "not_started");
        assert_eq!(open.1, None, "incomplete rows lose historical links");
        assert_eq!(
            (&open.2, &open.3, open.4, &open.5, &open.6),
            (
                &"keep body".to_string(),
                &"high".to_string(),
                7,
                &"2025-04-01".to_string(),
                &"2025-04-02".to_string()
            )
        );

        let completed: (String, Option<String>) = conn
            .query_row(
                "SELECT status, agent_session_id FROM todos WHERE id = 'done'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(completed.0, "completed");
        assert_eq!(completed.1.as_deref(), Some("completed-session"));

        type ListSnapshot = (String, Option<String>, Option<String>, Option<String>);
        let lists: Vec<ListSnapshot> = {
            let mut stmt = conn
                .prepare(
                    "SELECT scope, project_id, workdir, carry_over
                     FROM todo_lists ORDER BY id",
                )
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        assert_eq!(lists.len(), 3);
        assert!(lists.contains(&(
            "worktree".to_string(),
            Some("p1".to_string()),
            Some("/repo/.worktrees/w".to_string()),
            Some("worktree notes".to_string())
        )));
        assert!(lists.contains(&(
            "project".to_string(),
            Some("p1".to_string()),
            None,
            Some("project scratchpad".to_string())
        )));
        assert!(lists.contains(&(
            "global".to_string(),
            None,
            None,
            Some("global notes".to_string())
        )));
    }

    /// Migration 022 adds `provenance` to reply drafts written before it
    /// existed. They come back NULL, and the reply then discloses AI authorship
    /// with unknown details rather than borrowing another session's.
    #[test]
    fn migration_022_leaves_existing_reply_drafts_without_provenance() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(super::MIGRATION_001).unwrap();
        // 023 replays after 022 here and alters `todos`, so it has to exist.
        conn.execute_batch(super::MIGRATION_011).unwrap();
        conn.execute_batch(super::MIGRATION_013).unwrap();
        conn.execute_batch(super::MIGRATION_014).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL, description TEXT NOT NULL);
             INSERT INTO schema_version VALUES (21, datetime('now'), 'seed');",
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO pr_comment_reply_drafts
                (pr_number, comment_id, request_id, body, updated_at, base_head_sha)
             VALUES (7, 11, 'req', 'Fixed the guard.', datetime('now'), 'sha');",
        )
        .unwrap();

        super::run(&conn).unwrap();

        let provenance: Option<String> = conn
            .query_row(
                "SELECT provenance FROM pr_comment_reply_drafts WHERE comment_id = 11",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(provenance, None);
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
        assert_eq!(rows, 29);
    }

    /// Features written before selected-plan persistence existed acquire a
    /// nullable column. Their meaning is unchanged: no manual plan has been
    /// selected yet.
    #[test]
    fn migration_027_leaves_existing_features_without_a_selected_plan() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(super::MIGRATION_001).unwrap();
        conn.execute_batch(super::MIGRATION_011).unwrap();
        conn.execute_batch(super::MIGRATION_015).unwrap();
        conn.execute_batch(super::MIGRATION_023).unwrap();
        conn.execute_batch(super::MIGRATION_024).unwrap();
        conn.execute_batch(super::MIGRATION_025).unwrap();
        conn.execute_batch(super::MIGRATION_026).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_version (version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL, description TEXT NOT NULL);
             INSERT INTO schema_version VALUES (26, datetime('now'), 'seed');
             INSERT INTO projects
                (id, name, repo, created_at)
             VALUES ('proj-1', 'project', '/tmp/project', datetime('now'));
             INSERT INTO features
                (id, project_id, name, branch, workdir, status,
                 created_at, last_accessed)
             VALUES ('feat-1', 'proj-1', 'feature', 'feature/plan',
                     '/tmp/project/.worktrees/feature', 'stopped',
                     datetime('now'), datetime('now'));",
        )
        .unwrap();

        super::run(&conn).unwrap();

        let selected_plan_path: Option<String> = conn
            .query_row(
                "SELECT selected_plan_path FROM features WHERE id = 'feat-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(selected_plan_path, None);

        let version: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, 29);
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
