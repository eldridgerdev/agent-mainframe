//! SQLite persistence for scoped TODO lists.
//!
//! A [`TodoList`] belongs to one [`TodoScope`]: a **worktree** (keyed by the
//! workdir path a feature checks out), a **project**, or the machine-wide
//! **global** list that belongs to no project at all. Three partial unique
//! indexes (see `MIGRATION_025`) keep each of those a singleton within its
//! scope. A worktree- or project-scoped list also records a host feature
//! (`feature_id`); the global list has none. Each list owns an ordered set of
//! [`Todo`] items. See `docs/backlog/feature-todos-plan.md`.
//!
//! Unlike most domain data, todo lists live *outside* the `ProjectStore` JSON
//! blob and the store's full-replace save path, so they survive ordinary store
//! saves. Their `project_id` / `feature_id` are plain TEXT (no FK to
//! projects/features) for exactly that reason — cleanup on project/feature
//! deletion is handled explicitly by the functions here.
//!
//! The persistence layer landed ahead of its UI consumers (see the Feature
//! TODOs plan, Epics 2–6), so the API is allowed to be unused for now.
#![allow(dead_code)]

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Persisted lifecycle state of a [`Todo`].
///
/// This replaces the invalid combinations made possible by the historical
/// `done` and `in_progress` flags with one exhaustive value. The snake-case
/// names are shared by SQLite and serde so exports and database rows describe
/// the same state.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    #[default]
    NotStarted,
    InProgress,
    Completed,
}

impl TodoStatus {
    pub fn as_db_str(self) -> &'static str {
        match self {
            TodoStatus::NotStarted => "not_started",
            TodoStatus::InProgress => "in_progress",
            TodoStatus::Completed => "completed",
        }
    }

    /// Parse a stored status token; unknown values degrade to not started.
    ///
    /// That fallback keeps a damaged or future row visible and eligible for
    /// an explicit user decision instead of silently presenting it as done.
    pub fn from_db_str(s: &str) -> Self {
        match s {
            "in_progress" => TodoStatus::InProgress,
            "completed" => TodoStatus::Completed,
            _ => TodoStatus::NotStarted,
        }
    }

    /// The next state selected by the existing manual toggle action.
    pub fn next_manual(self) -> Self {
        match self {
            TodoStatus::NotStarted => TodoStatus::InProgress,
            TodoStatus::InProgress => TodoStatus::Completed,
            TodoStatus::Completed => TodoStatus::NotStarted,
        }
    }

    pub fn is_not_started(self) -> bool {
        self == TodoStatus::NotStarted
    }

    pub fn is_in_progress(self) -> bool {
        self == TodoStatus::InProgress
    }

    pub fn is_completed(self) -> bool {
        self == TodoStatus::Completed
    }
}

/// The status and optional agent-session association that must be persisted
/// together for a TODO.
///
/// `agent_session_id` is a [`crate::project::FeatureSession`] id, rather than
/// a tmux name or harness-specific identifier. It is stable across AMF
/// restarts and works for every supported agent harness.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoWorkState {
    pub status: TodoStatus,
    pub agent_session_id: Option<String>,
}

impl TodoWorkState {
    /// Apply the manual three-state cycle. The association is deliberately
    /// retained: it remains useful for jumping back to prior work, and only a
    /// failed launch or reconciliation of a missing session invalidates it.
    pub fn cycle_manually(&mut self) {
        self.status = self.status.next_manual();
    }

    /// Reserve a not-started TODO for a TODO-specific launch.
    ///
    /// Returns `false` without changing anything when the TODO is already in
    /// progress or completed. A prior association is cleared because the new
    /// launch, once created, becomes the association for this reservation.
    pub fn reserve_launch(&mut self) -> bool {
        if self.status != TodoStatus::NotStarted {
            return false;
        }
        self.status = TodoStatus::InProgress;
        self.agent_session_id = None;
        true
    }

    /// Record the session produced by a successful reserved launch.
    ///
    /// Association is accepted only while the TODO is reserved/in progress,
    /// preventing a late launch result from attaching itself after a manual
    /// status change.
    pub fn associate_session(&mut self, session_id: impl Into<String>) -> bool {
        if self.status != TodoStatus::InProgress {
            return false;
        }
        self.agent_session_id = Some(session_id.into());
        true
    }

    /// Undo a failed agent creation or prompt delivery.
    pub fn rollback_launch(&mut self) {
        self.status = TodoStatus::NotStarted;
        self.agent_session_id = None;
    }

    /// Clear a stale session reference without changing the TODO's status.
    pub fn clear_missing_session(&mut self) {
        self.agent_session_id = None;
    }
}

/// Priority of a [`Todo`], persisted as a short token (`high`/`med`/`low`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoPriority {
    High,
    Med,
    Low,
}

impl TodoPriority {
    pub fn as_db_str(self) -> &'static str {
        match self {
            TodoPriority::High => "high",
            TodoPriority::Med => "med",
            TodoPriority::Low => "low",
        }
    }

    /// Parse a stored priority token; unknown values degrade to [`Med`].
    pub fn from_db_str(s: &str) -> Self {
        match s {
            "high" => TodoPriority::High,
            "low" => TodoPriority::Low,
            _ => TodoPriority::Med,
        }
    }

    /// Rank used for sorting: lower comes first (High = 0).
    pub fn rank(self) -> u8 {
        match self {
            TodoPriority::High => 0,
            TodoPriority::Med => 1,
            TodoPriority::Low => 2,
        }
    }
}

/// Which list a [`TodoList`] is: the worktree it belongs to, the project it
/// belongs to, or the machine-wide list that belongs to neither.
///
/// The variants are ordered the way ties between them resolve — narrowest
/// first — and [`Self::rank`] is that order made explicit for sorting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TodoScope {
    /// One list per checkout. Keyed by **workdir path** rather than feature id
    /// so the list belongs to the working tree the TODOs were written about,
    /// not to whichever feature row happens to point at it.
    Worktree { project_id: String, workdir: String },
    /// One list per project — the only scope that existed before
    /// `MIGRATION_025`, and the scope every pre-existing list keeps.
    Project { project_id: String },
    /// A single list for the whole machine, belonging to no project.
    Global,
}

impl TodoScope {
    /// The `todo_lists.scope` token.
    pub fn as_db_str(&self) -> &'static str {
        match self {
            TodoScope::Worktree { .. } => "worktree",
            TodoScope::Project { .. } => "project",
            TodoScope::Global => "global",
        }
    }

    pub fn project_id(&self) -> Option<&str> {
        match self {
            TodoScope::Worktree { project_id, .. } | TodoScope::Project { project_id } => {
                Some(project_id)
            }
            TodoScope::Global => None,
        }
    }

    pub fn workdir(&self) -> Option<&str> {
        match self {
            TodoScope::Worktree { workdir, .. } => Some(workdir),
            _ => None,
        }
    }

    /// Tie-break order across scopes: worktree (0) beats project (1) beats
    /// global (2) at equal priority.
    pub fn rank(&self) -> u8 {
        match self {
            TodoScope::Worktree { .. } => 0,
            TodoScope::Project { .. } => 1,
            TodoScope::Global => 2,
        }
    }

    /// Rebuild a scope from the three stored columns. An unrecognized token
    /// reads as `project` — the scope every row had before `MIGRATION_025`,
    /// and the only one that stays meaningful when the other columns are
    /// missing.
    fn from_row(scope: &str, project_id: Option<String>, workdir: Option<String>) -> Self {
        match (scope, project_id, workdir) {
            ("worktree", Some(project_id), Some(workdir)) => TodoScope::Worktree {
                project_id,
                workdir,
            },
            ("global", _, _) => TodoScope::Global,
            (_, Some(project_id), _) => TodoScope::Project { project_id },
            // A project-scoped row with no project is not addressable by any
            // scope; treating it as global is the only reading that keeps it
            // loadable rather than silently dropping the user's items.
            (_, None, _) => TodoScope::Global,
        }
    }
}

/// A TODO list within one [`TodoScope`], hosted by one feature.
#[derive(Debug, Clone)]
pub struct TodoList {
    pub id: String,
    /// What this list is a list *for*. Carries the project id and workdir, so
    /// it is the whole key: nothing else on the row identifies the list.
    pub scope: TodoScope,
    /// The feature that hosts the list. `None` for the global list, which
    /// belongs to no project and so to no feature.
    pub feature_id: Option<String>,
    /// "Left off here" carry-over banner note. Presented as the list's
    /// free-form *scratchpad*; the column keeps its original name.
    pub carry_over: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// A single TODO item belonging to a [`TodoList`].
#[derive(Debug, Clone)]
pub struct Todo {
    pub id: String,
    pub list_id: String,
    pub title: String,
    pub body: Option<String>,
    pub priority: TodoPriority,
    pub sort_order: i64,
    pub work: TodoWorkState,
    /// `Feature.id` of a feature plan mode created for this item, if any. A
    /// different destination from [`TodoWorkState::agent_session_id`], and a TODO can
    /// carry both.
    pub linked_feature_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// A TODO resolved by its stable identity together with the list that
/// currently owns it.  This deliberately derives scope at read time: moving a
/// TODO changes its list, not its identity or any session references to it.
#[derive(Debug, Clone)]
pub struct ResolvedTodo {
    pub todo: Todo,
    pub list: TodoList,
}

impl Todo {
    /// Automatic TODO spawning is reserved exclusively for untouched work.
    pub fn is_eligible_for_automatic_spawn(&self) -> bool {
        self.work.status.is_not_started()
    }
}

const LIST_COLUMNS: &str =
    "id, project_id, feature_id, scope, workdir, carry_over, created_at, updated_at";

fn row_to_list(row: &rusqlite::Row<'_>) -> rusqlite::Result<TodoList> {
    let project_id: Option<String> = row.get(1)?;
    let scope: String = row.get(3)?;
    let workdir: Option<String> = row.get(4)?;
    Ok(TodoList {
        id: row.get(0)?,
        scope: TodoScope::from_row(&scope, project_id, workdir),
        feature_id: row.get(2)?,
        carry_over: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

/// Load the TODO list for `scope`, or `None` if that scope has none yet.
///
/// Each branch matches on exactly the columns its partial unique index covers,
/// so the lookup and the constraint that makes it a singleton stay the same
/// question asked twice.
pub fn load_list(conn: &Connection, scope: &TodoScope) -> Result<Option<TodoList>> {
    let row = match scope {
        TodoScope::Worktree {
            project_id,
            workdir,
        } => conn
            .query_row(
                &format!(
                    "SELECT {LIST_COLUMNS} FROM todo_lists
                     WHERE scope = 'worktree' AND project_id = ?1 AND workdir = ?2"
                ),
                params![project_id, workdir],
                row_to_list,
            )
            .optional()?,
        TodoScope::Project { project_id } => conn
            .query_row(
                &format!(
                    "SELECT {LIST_COLUMNS} FROM todo_lists
                     WHERE scope = 'project' AND project_id = ?1"
                ),
                params![project_id],
                row_to_list,
            )
            .optional()?,
        TodoScope::Global => conn
            .query_row(
                &format!("SELECT {LIST_COLUMNS} FROM todo_lists WHERE scope = 'global'"),
                [],
                row_to_list,
            )
            .optional()?,
    };
    Ok(row)
}

/// Load a list by its own id, whatever scope it is in. Used when acting on a
/// list the caller already has a handle to (a move target, a re-home).
pub fn load_list_by_id(conn: &Connection, list_id: &str) -> Result<Option<TodoList>> {
    let row = conn
        .query_row(
            &format!("SELECT {LIST_COLUMNS} FROM todo_lists WHERE id = ?1"),
            params![list_id],
            row_to_list,
        )
        .optional()?;
    Ok(row)
}

/// Create the TODO list for `scope`, hosted by `feature_id`, and return it.
/// Fails if a list already exists in that scope — the partial unique indexes
/// from `MIGRATION_025` are what reject it.
pub fn create_list(
    conn: &Connection,
    scope: &TodoScope,
    feature_id: Option<&str>,
) -> Result<TodoList> {
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO todo_lists
            (id, project_id, feature_id, scope, workdir, carry_over, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, NULL, datetime('now'), datetime('now'))",
        params![
            id,
            scope.project_id(),
            feature_id,
            scope.as_db_str(),
            scope.workdir(),
        ],
    )?;
    load_list(conn, scope)?
        .ok_or_else(|| anyhow::anyhow!("todo list vanished immediately after insert"))
}

/// Return the scope's existing TODO list, creating one under `feature_id` if
/// none exists yet. This is the lazy creation a worktree pane does on first
/// open and the global pane does on first reveal.
pub fn load_or_create_list(
    conn: &Connection,
    scope: &TodoScope,
    feature_id: Option<&str>,
) -> Result<TodoList> {
    match load_list(conn, scope)? {
        Some(list) => Ok(list),
        None => create_list(conn, scope, feature_id),
    }
}

/// Update the carry-over "left off here" note on a list.
pub fn set_carry_over(conn: &Connection, list_id: &str, carry_over: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE todo_lists SET carry_over = ?2, updated_at = datetime('now') WHERE id = ?1",
        params![list_id, carry_over],
    )?;
    Ok(())
}

/// Reassign the list's host feature (used when re-homing after the host
/// feature is deleted).
pub fn set_host_feature(conn: &Connection, list_id: &str, feature_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE todo_lists SET feature_id = ?2, updated_at = datetime('now') WHERE id = ?1",
        params![list_id, feature_id],
    )?;
    Ok(())
}

/// Delete a TODO list and (via cascade) all of its items.
pub fn delete_list(conn: &Connection, list_id: &str) -> Result<()> {
    conn.execute("DELETE FROM todo_lists WHERE id = ?1", params![list_id])?;
    Ok(())
}

/// Delete **every** list belonging to `project_id` — its project-scoped list
/// and each of its worktree lists — along with their items. Called when a
/// project is deleted, since there is no FK cascade from `projects`. The
/// global list belongs to no project and is never touched here.
pub fn delete_lists_for_project(conn: &Connection, project_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM todo_lists WHERE project_id = ?1",
        params![project_id],
    )?;
    Ok(())
}

/// Delete the worktree-scoped list at `workdir`, if any. Called after a
/// feature's TODOs have been dispositioned on delete.
pub fn delete_worktree_list(conn: &Connection, project_id: &str, workdir: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM todo_lists
         WHERE scope = 'worktree' AND project_id = ?1 AND workdir = ?2",
        params![project_id, workdir],
    )?;
    Ok(())
}

/// Load a single TODO by id, regardless of which list currently holds it.
///
/// Unlike [`list_todos`], this needs no `list_id` — a caller that only has a
/// TODO's id (e.g. one captured before a `move`/`copy` could have relocated
/// it) can still resolve the row.
pub fn find_todo_by_id(conn: &Connection, todo_id: &str) -> Result<Option<Todo>> {
    let mut stmt = conn.prepare(
        "SELECT id, list_id, title, body, priority, sort_order,
                status, agent_session_id, linked_feature_id,
                created_at, updated_at
         FROM todos WHERE id = ?1",
    )?;
    let mut rows = stmt.query_map(params![todo_id], |row| {
        let priority: String = row.get(4)?;
        let status: String = row.get(6)?;
        Ok(Todo {
            id: row.get(0)?,
            list_id: row.get(1)?,
            title: row.get(2)?,
            body: row.get(3)?,
            priority: TodoPriority::from_db_str(&priority),
            sort_order: row.get(5)?,
            work: TodoWorkState {
                status: TodoStatus::from_db_str(&status),
                agent_session_id: row.get(7)?,
            },
            linked_feature_id: row.get(8)?,
            created_at: row.get(9)?,
            updated_at: row.get(10)?,
        })
    })?;
    rows.next().transpose().map_err(Into::into)
}

/// Resolve a TODO and its current scope without requiring the caller to retain
/// its old list id.  A missing list is treated like a missing TODO rather than
/// exposing a partial, stale reference.
pub fn resolve_todo_by_id(conn: &Connection, todo_id: &str) -> Result<Option<ResolvedTodo>> {
    let Some(todo) = find_todo_by_id(conn, todo_id)? else {
        return Ok(None);
    };
    let Some(list) = load_list_by_id(conn, &todo.list_id)? else {
        return Ok(None);
    };
    Ok(Some(ResolvedTodo { todo, list }))
}

/// Load every item in `list_id`, sorted by status (completed last), then
/// then manual `sort_order`.
pub fn list_todos(conn: &Connection, list_id: &str) -> Result<Vec<Todo>> {
    let mut stmt = conn.prepare(
        "SELECT id, list_id, title, body, priority, sort_order,
                status, agent_session_id, linked_feature_id,
                created_at, updated_at
         FROM todos WHERE list_id = ?1
         ORDER BY (status = 'completed') ASC, sort_order ASC",
    )?;
    let rows = stmt.query_map(params![list_id], |row| {
        let priority: String = row.get(4)?;
        let status: String = row.get(6)?;
        Ok(Todo {
            id: row.get(0)?,
            list_id: row.get(1)?,
            title: row.get(2)?,
            body: row.get(3)?,
            priority: TodoPriority::from_db_str(&priority),
            sort_order: row.get(5)?,
            work: TodoWorkState {
                status: TodoStatus::from_db_str(&status),
                agent_session_id: row.get(7)?,
            },
            linked_feature_id: row.get(8)?,
            created_at: row.get(9)?,
            updated_at: row.get(10)?,
        })
    })?;

    let mut todos = Vec::new();
    for row in rows {
        todos.push(row?);
    }
    Ok(todos)
}

/// Insert a new TODO at the end of `list_id` (highest `sort_order` + 1) and
/// return it.
pub fn add_todo(
    conn: &Connection,
    list_id: &str,
    title: &str,
    body: Option<&str>,
    priority: TodoPriority,
) -> Result<Todo> {
    let next_order = next_sort_order(conn, list_id)?;
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO todos
            (id, list_id, title, body, priority, sort_order, status,
             agent_session_id, linked_feature_id,
             created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'not_started', NULL, NULL,
                 datetime('now'), datetime('now'))",
        params![id, list_id, title, body, priority.as_db_str(), next_order],
    )?;
    Ok(Todo {
        id,
        list_id: list_id.to_string(),
        title: title.to_string(),
        body: body.map(str::to_string),
        priority,
        sort_order: next_order,
        work: TodoWorkState::default(),
        linked_feature_id: None,
        created_at: String::new(),
        updated_at: String::new(),
    })
}

/// Persist the mutable fields of an existing TODO (everything except
/// `created_at`). `updated_at` is bumped automatically.
pub fn update_todo(conn: &Connection, todo: &Todo) -> Result<()> {
    conn.execute(
        "UPDATE todos SET
            list_id = ?2, title = ?3, body = ?4, priority = ?5,
            sort_order = ?6, status = ?7, agent_session_id = ?8,
            linked_feature_id = ?9, updated_at = datetime('now')
         WHERE id = ?1",
        params![
            todo.id,
            todo.list_id,
            todo.title,
            todo.body,
            todo.priority.as_db_str(),
            todo.sort_order,
            todo.work.status.as_db_str(),
            todo.work.agent_session_id,
            todo.linked_feature_id,
        ],
    )?;
    Ok(())
}

/// Delete a single TODO by id. Any session it spawned is left untouched.
pub fn delete_todo(conn: &Connection, todo_id: &str) -> Result<()> {
    conn.execute("DELETE FROM todos WHERE id = ?1", params![todo_id])?;
    Ok(())
}

/// Point a TODO at the feature a plan run created for it.
///
/// A targeted write rather than [`update_todo`]: by the time a plan is
/// accepted the TODOs overlay is gone, so there is no in-memory row to write
/// back — and a stale one reconstructed from before the interview would undo
/// whatever else changed meanwhile.
pub fn set_linked_feature(conn: &Connection, todo_id: &str, feature_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE todos SET linked_feature_id = ?2, updated_at = datetime('now')
         WHERE id = ?1",
        params![todo_id, feature_id],
    )?;
    Ok(())
}

/// Point a TODO at the session spawned for it. Targeted for the same reason as
/// [`set_linked_feature`].
pub fn set_agent_session(conn: &Connection, todo_id: &str, session_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE todos SET agent_session_id = ?2, updated_at = datetime('now')
         WHERE id = ?1",
        params![todo_id, session_id],
    )?;
    Ok(())
}

/// Persist status and association as one work-state transition.
pub fn set_work_state(conn: &Connection, todo_id: &str, work: &TodoWorkState) -> Result<()> {
    conn.execute(
        "UPDATE todos SET status = ?2, agent_session_id = ?3,
                          updated_at = datetime('now')
         WHERE id = ?1",
        params![todo_id, work.status.as_db_str(), work.agent_session_id],
    )?;
    Ok(())
}

/// Drop a TODO's link to the session spawned for it, when that session is gone.
pub fn clear_agent_session(conn: &Connection, todo_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE todos SET agent_session_id = NULL, updated_at = datetime('now')
         WHERE id = ?1",
        params![todo_id],
    )?;
    Ok(())
}

/// All persisted TODO-to-agent associations, for startup/session
/// reconciliation against the authoritative feature-session store.
pub fn agent_session_associations(conn: &Connection) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT id, agent_session_id FROM todos
         WHERE agent_session_id IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Drop one TODO's link to the feature planned for it, when that feature is
/// gone.
///
/// The by-`todo_id` counterpart to [`clear_linked_feature`], and targeted for
/// the same reason as [`clear_spawned_session`]: the self-healing jump runs
/// from the dashboard too, where the TODOs overlay is not open and there is no
/// in-memory row to write back through [`update_todo`].
pub fn clear_linked_feature_for_todo(conn: &Connection, todo_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE todos SET linked_feature_id = NULL, updated_at = datetime('now')
         WHERE id = ?1",
        params![todo_id],
    )?;
    Ok(())
}

/// Drop any TODO's link to `feature_id`, across every list.
///
/// Called when a feature is deleted: `linked_feature_id` has no FK (see
/// MIGRATION_023), so nothing else would clear it, and a TODO left pointing at
/// a feature that no longer exists would offer a jump that cannot land. The
/// TODO itself is kept — the work it describes outlived the feature.
pub fn clear_linked_feature(conn: &Connection, feature_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE todos SET linked_feature_id = NULL, updated_at = datetime('now')
         WHERE linked_feature_id = ?1",
        params![feature_id],
    )?;
    Ok(())
}

/// The `sort_order` a new item appended to `list_id` should take.
fn next_sort_order(conn: &Connection, list_id: &str) -> Result<i64> {
    let next: i64 = conn.query_row(
        "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM todos WHERE list_id = ?1",
        params![list_id],
        |row| row.get(0),
    )?;
    Ok(next)
}

/// Move a TODO into another list, appending it at the destination's end.
///
/// Both links ride along: it is the same item of work, and the session or
/// feature someone already started for it is still the work in flight. What
/// changes is only which list it is filed under, so `sort_order` is
/// recomputed against the destination — keeping the source's number would drop
/// the item into the middle of a list it has never been in.
pub fn move_todo(conn: &Connection, todo_id: &str, target_list_id: &str) -> Result<()> {
    let next_order = next_sort_order(conn, target_list_id)?;
    conn.execute(
        "UPDATE todos SET list_id = ?2, sort_order = ?3, updated_at = datetime('now')
         WHERE id = ?1",
        params![todo_id, target_list_id, next_order],
    )?;
    Ok(())
}

/// Copy a TODO into another list, appending it at the destination's end, and
/// return the new item.
///
/// The copy is deliberately *unstarted*: the agent association, feature link,
/// and in-progress state are all dropped. Carrying them would leave two rows
/// in two panes each claiming the same session, and "implement next" would
/// then hold both in reserve for work only one of them describes.
pub fn copy_todo(conn: &Connection, todo_id: &str, target_list_id: &str) -> Result<Option<Todo>> {
    let source = conn
        .query_row(
            "SELECT title, body, priority, status FROM todos WHERE id = ?1",
            params![todo_id],
            |row| {
                let priority: String = row.get(2)?;
                let status: String = row.get(3)?;
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    TodoPriority::from_db_str(&priority),
                    TodoStatus::from_db_str(&status),
                ))
            },
        )
        .optional()?;
    let Some((title, body, priority, source_status)) = source else {
        return Ok(None);
    };

    let next_order = next_sort_order(conn, target_list_id)?;
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO todos
            (id, list_id, title, body, priority, sort_order, status,
             agent_session_id, linked_feature_id,
             created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL,
                 datetime('now'), datetime('now'))",
        params![
            id,
            target_list_id,
            title,
            body,
            priority.as_db_str(),
            next_order,
            if source_status == TodoStatus::Completed {
                TodoStatus::Completed.as_db_str()
            } else {
                TodoStatus::NotStarted.as_db_str()
            }
        ],
    )?;
    Ok(Some(Todo {
        id,
        list_id: target_list_id.to_string(),
        title,
        body,
        priority,
        sort_order: next_order,
        work: TodoWorkState {
            status: if source_status == TodoStatus::Completed {
                TodoStatus::Completed
            } else {
                TodoStatus::NotStarted
            },
            agent_session_id: None,
        },
        linked_feature_id: None,
        created_at: String::new(),
        updated_at: String::new(),
    }))
}

/// Persist a manual ordering: `ordered_ids` are written back as `sort_order`
/// 0, 1, 2, … in the given sequence.
pub fn reorder_todos(conn: &Connection, ordered_ids: &[String]) -> Result<()> {
    for (idx, id) in ordered_ids.iter().enumerate() {
        conn.execute(
            "UPDATE todos SET sort_order = ?2, updated_at = datetime('now') WHERE id = ?1",
            params![id, idx as i64],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::AmfDb;
    use tempfile::NamedTempFile;

    #[test]
    fn todo_status_db_and_serde_representations_are_stable() {
        let cases = [
            (TodoStatus::NotStarted, "not_started"),
            (TodoStatus::InProgress, "in_progress"),
            (TodoStatus::Completed, "completed"),
        ];

        for (status, token) in cases {
            assert_eq!(status.as_db_str(), token);
            assert_eq!(TodoStatus::from_db_str(token), status);
            assert_eq!(
                serde_json::to_string(&status).unwrap(),
                format!("\"{token}\"")
            );
            assert_eq!(
                serde_json::from_str::<TodoStatus>(&format!("\"{token}\"")).unwrap(),
                status
            );
        }
        assert_eq!(
            TodoStatus::from_db_str("future_state"),
            TodoStatus::NotStarted
        );
    }

    #[test]
    fn todo_status_manual_cycle_visits_each_state_in_order() {
        let mut state = TodoWorkState::default();
        state.cycle_manually();
        assert_eq!(state.status, TodoStatus::InProgress);
        state.cycle_manually();
        assert_eq!(state.status, TodoStatus::Completed);
        state.cycle_manually();
        assert_eq!(state.status, TodoStatus::NotStarted);
    }

    #[test]
    fn todo_launch_transitions_keep_status_and_association_consistent() {
        let mut state = TodoWorkState {
            status: TodoStatus::NotStarted,
            agent_session_id: Some("old-session".to_string()),
        };

        assert!(state.reserve_launch());
        assert_eq!(state.status, TodoStatus::InProgress);
        assert!(state.agent_session_id.is_none());
        assert!(
            !state.reserve_launch(),
            "an in-progress TODO cannot be reserved twice"
        );
        assert!(state.associate_session("new-session"));
        assert_eq!(state.agent_session_id.as_deref(), Some("new-session"));

        state.clear_missing_session();
        assert_eq!(state.status, TodoStatus::InProgress);
        assert!(state.agent_session_id.is_none());

        state.rollback_launch();
        assert_eq!(state, TodoWorkState::default());
    }

    #[test]
    fn manual_cycle_retains_association_and_late_association_is_rejected() {
        let mut state = TodoWorkState {
            status: TodoStatus::InProgress,
            agent_session_id: Some("session-1".to_string()),
        };
        state.cycle_manually();
        assert_eq!(state.status, TodoStatus::Completed);
        assert_eq!(state.agent_session_id.as_deref(), Some("session-1"));
        assert!(!state.associate_session("late-session"));
        assert_eq!(state.agent_session_id.as_deref(), Some("session-1"));
    }

    fn open_temp_db() -> (NamedTempFile, AmfDb) {
        let tmp = NamedTempFile::new().unwrap();
        let db = AmfDb::open(tmp.path()).unwrap();
        (tmp, db)
    }

    fn project(id: &str) -> TodoScope {
        TodoScope::Project {
            project_id: id.to_string(),
        }
    }

    fn worktree(id: &str, workdir: &str) -> TodoScope {
        TodoScope::Worktree {
            project_id: id.to_string(),
            workdir: workdir.to_string(),
        }
    }

    #[test]
    fn create_and_load_list() {
        let (_tmp, db) = open_temp_db();

        assert!(db.todo_list(&project("proj-1")).unwrap().is_none());

        let list = db
            .create_todo_list(&project("proj-1"), Some("feat-1"))
            .unwrap();
        assert_eq!(list.scope, project("proj-1"));
        assert_eq!(list.feature_id.as_deref(), Some("feat-1"));
        assert!(list.carry_over.is_none());

        let loaded = db.todo_list(&project("proj-1")).unwrap().unwrap();
        assert_eq!(loaded.id, list.id);
    }

    #[test]
    fn one_list_per_project() {
        let (_tmp, db) = open_temp_db();
        db.create_todo_list(&project("proj-1"), Some("feat-1"))
            .unwrap();
        // The partial unique index must reject a second project-scoped list.
        assert!(
            db.create_todo_list(&project("proj-1"), Some("feat-2"))
                .is_err()
        );
    }

    /// The three scopes are three different lists, and the worktree scope is a
    /// singleton per workdir rather than per project.
    #[test]
    fn scopes_are_independent_singletons() {
        let (_tmp, db) = open_temp_db();

        let proj = db
            .create_todo_list(&project("proj-1"), Some("feat-1"))
            .unwrap();
        let wt_a = db
            .create_todo_list(&worktree("proj-1", "/repo/.worktrees/a"), Some("feat-a"))
            .unwrap();
        let wt_b = db
            .create_todo_list(&worktree("proj-1", "/repo/.worktrees/b"), Some("feat-b"))
            .unwrap();
        let global = db.create_todo_list(&TodoScope::Global, None).unwrap();

        let ids = [&proj.id, &wt_a.id, &wt_b.id, &global.id];
        for (i, a) in ids.iter().enumerate() {
            for b in ids.iter().skip(i + 1) {
                assert_ne!(a, b, "each scope gets its own list");
            }
        }
        assert!(global.feature_id.is_none(), "the global list has no host");

        // Second list in the same scope is rejected in every scope.
        assert!(
            db.create_todo_list(&worktree("proj-1", "/repo/.worktrees/a"), Some("feat-a"))
                .is_err()
        );
        assert!(db.create_todo_list(&TodoScope::Global, None).is_err());

        // Another project's worktree at a different path is fine.
        assert!(
            db.create_todo_list(&worktree("proj-2", "/other/.worktrees/a"), Some("feat-z"))
                .is_ok()
        );
    }

    #[test]
    fn load_or_create_is_idempotent() {
        let (_tmp, db) = open_temp_db();
        let a = db
            .load_or_create_todo_list(&project("proj-1"), Some("feat-1"))
            .unwrap();
        let b = db
            .load_or_create_todo_list(&project("proj-1"), Some("feat-9"))
            .unwrap();
        assert_eq!(a.id, b.id);
        // Host feature is preserved from the first create, not overwritten.
        assert_eq!(b.feature_id.as_deref(), Some("feat-1"));
    }

    #[test]
    fn todo_crud_roundtrip() {
        let (_tmp, db) = open_temp_db();
        let list = db
            .create_todo_list(&project("proj-1"), Some("feat-1"))
            .unwrap();

        let mut todo = db
            .add_todo(
                &list.id,
                "Write tests",
                Some("cover edge cases"),
                TodoPriority::High,
            )
            .unwrap();
        assert_eq!(todo.sort_order, 0);

        // Mutate and persist.
        todo.work.status = TodoStatus::Completed;
        todo.priority = TodoPriority::Low;
        todo.work.agent_session_id = Some("sess-7".to_string());
        db.update_todo(&todo).unwrap();

        let loaded = db.todos(&list.id).unwrap();
        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].work.status.is_completed());
        assert_eq!(loaded[0].priority, TodoPriority::Low);
        assert_eq!(loaded[0].body.as_deref(), Some("cover edge cases"));
        assert_eq!(loaded[0].work.agent_session_id.as_deref(), Some("sess-7"));

        db.delete_todo(&todo.id).unwrap();
        assert!(db.todos(&list.id).unwrap().is_empty());
    }

    #[test]
    fn add_todo_increments_sort_order() {
        let (_tmp, db) = open_temp_db();
        let list = db
            .create_todo_list(&project("proj-1"), Some("feat-1"))
            .unwrap();
        let a = db.add_todo(&list.id, "a", None, TodoPriority::Med).unwrap();
        let b = db.add_todo(&list.id, "b", None, TodoPriority::Med).unwrap();
        let c = db.add_todo(&list.id, "c", None, TodoPriority::Med).unwrap();
        assert_eq!((a.sort_order, b.sort_order, c.sort_order), (0, 1, 2));
    }

    /// A move re-files the same work: it lands at the end of the destination
    /// and keeps whatever was already started for it.
    #[test]
    fn move_appends_to_the_destination_and_keeps_the_links() {
        let (_tmp, db) = open_temp_db();
        let src = db
            .create_todo_list(&worktree("proj-1", "/wt/a"), Some("feat-1"))
            .unwrap();
        let dst = db
            .create_todo_list(&project("proj-1"), Some("feat-1"))
            .unwrap();
        db.add_todo(&dst.id, "already here", None, TodoPriority::Med)
            .unwrap();

        let mut moving = db
            .add_todo(&src.id, "port me", Some("notes"), TodoPriority::High)
            .unwrap();
        moving.work.agent_session_id = Some("sess-1".to_string());
        moving.linked_feature_id = Some("feat-planned".to_string());
        moving.work.status = TodoStatus::InProgress;
        db.update_todo(&moving).unwrap();

        db.move_todo(&moving.id, &dst.id).unwrap();

        assert!(db.todos(&src.id).unwrap().is_empty(), "it left the source");
        let landed = db.todos(&dst.id).unwrap();
        assert_eq!(landed.len(), 2);
        let moved = landed.iter().find(|t| t.title == "port me").unwrap();
        assert_eq!(
            moved.sort_order, 1,
            "appended, not overlapping the sitting item"
        );
        assert_eq!(moved.work.agent_session_id.as_deref(), Some("sess-1"));
        assert_eq!(moved.linked_feature_id.as_deref(), Some("feat-planned"));
        assert!(moved.work.status.is_in_progress());
        assert_eq!(moved.body.as_deref(), Some("notes"));
        assert_eq!(moved.priority, TodoPriority::High);
    }

    /// A caller that only kept a TODO's id (not the list it started in) must
    /// still be able to resolve it after a move — the scenario a plan
    /// interview hits between generating a plan and the user accepting it.
    #[test]
    fn find_by_id_locates_a_todo_after_it_moved_to_another_list() {
        let (_tmp, db) = open_temp_db();
        let src = db
            .create_todo_list(&worktree("proj-1", "/wt/a"), Some("feat-1"))
            .unwrap();
        let dst = db.create_todo_list(&TodoScope::Global, None).unwrap();

        let todo = db
            .add_todo(&src.id, "port me", None, TodoPriority::Med)
            .unwrap();
        db.move_todo(&todo.id, &dst.id).unwrap();

        let found = db
            .resolve_todo_by_id(&todo.id)
            .unwrap()
            .expect("resolved by id alone, without knowing it moved");
        assert_eq!(found.todo.list_id, dst.id);
        assert_eq!(found.todo.title, "port me");
        assert_eq!(found.list.scope, TodoScope::Global);

        assert!(db.find_todo_by_id("no-such-id").unwrap().is_none());
    }

    /// A copy is a second, *unstarted* item: two panes must never both claim
    /// the same session.
    #[test]
    fn copy_leaves_the_original_and_clears_what_was_started() {
        let (_tmp, db) = open_temp_db();
        let src = db
            .create_todo_list(&worktree("proj-1", "/wt/a"), Some("feat-1"))
            .unwrap();
        let dst = db.create_todo_list(&TodoScope::Global, None).unwrap();

        let mut original = db
            .add_todo(&src.id, "share me", Some("why"), TodoPriority::Low)
            .unwrap();
        original.work.agent_session_id = Some("sess-1".to_string());
        original.linked_feature_id = Some("feat-planned".to_string());
        original.work.status = TodoStatus::InProgress;
        db.update_todo(&original).unwrap();

        let copy = db.copy_todo(&original.id, &dst.id).unwrap().unwrap();

        // The original is untouched, links and all.
        let kept = &db.todos(&src.id).unwrap()[0];
        assert_eq!(kept.id, original.id);
        assert_eq!(kept.work.agent_session_id.as_deref(), Some("sess-1"));
        assert!(kept.work.status.is_in_progress());

        assert_ne!(copy.id, original.id);
        let landed = &db.todos(&dst.id).unwrap()[0];
        assert_eq!(landed.title, "share me");
        assert_eq!(landed.body.as_deref(), Some("why"));
        assert_eq!(landed.priority, TodoPriority::Low);
        assert!(landed.work.agent_session_id.is_none());
        assert!(landed.linked_feature_id.is_none());
        assert!(landed.work.status.is_not_started());
    }

    #[test]
    fn copying_a_todo_that_is_gone_reports_it_rather_than_inventing_one() {
        let (_tmp, db) = open_temp_db();
        let dst = db.create_todo_list(&TodoScope::Global, None).unwrap();
        assert!(db.copy_todo("no-such-todo", &dst.id).unwrap().is_none());
        assert!(db.todos(&dst.id).unwrap().is_empty());
    }

    /// Work state round-trips and the targeted writer updates status and
    /// association together without a whole-row update.
    #[test]
    fn work_state_roundtrips_and_has_targeted_writers() {
        let (_tmp, db) = open_temp_db();
        let list = db
            .create_todo_list(&project("proj-1"), Some("feat-1"))
            .unwrap();
        let mut todo = db
            .add_todo(&list.id, "Ship it", None, TodoPriority::High)
            .unwrap();
        // A new TODO is nobody's work in progress.
        assert!(todo.work.status.is_not_started());
        assert!(db.todos(&list.id).unwrap()[0].work.status.is_not_started());

        todo.work.status = TodoStatus::InProgress;
        db.update_todo(&todo).unwrap();
        assert!(db.todos(&list.id).unwrap()[0].work.status.is_in_progress());

        let completed = TodoWorkState {
            status: TodoStatus::Completed,
            agent_session_id: Some("sess-1".to_string()),
        };
        db.set_todo_work_state(&todo.id, &completed).unwrap();
        assert_eq!(db.todos(&list.id).unwrap()[0].work, completed);

        // Clearing a dead session link is its own write, leaving everything
        // else on the row alone.
        db.clear_todo_agent_session(&todo.id).unwrap();
        let loaded = db.todos(&list.id).unwrap();
        assert!(loaded[0].work.agent_session_id.is_none());
        assert_eq!(loaded[0].title, "Ship it");
        assert!(loaded[0].work.status.is_completed());
    }

    #[test]
    fn todo_status_and_agent_association_survive_database_restart() {
        let tmp = NamedTempFile::new().unwrap();
        let (list_id, todo_id) = {
            let db = AmfDb::open(tmp.path()).unwrap();
            let list = db
                .create_todo_list(&project("proj-1"), Some("feat-1"))
                .unwrap();
            let mut todo = db
                .add_todo(&list.id, "Keep working", None, TodoPriority::High)
                .unwrap();
            todo.work = TodoWorkState {
                status: TodoStatus::InProgress,
                agent_session_id: Some("session-42".to_string()),
            };
            db.update_todo(&todo).unwrap();
            (list.id, todo.id)
        };

        let reopened = AmfDb::open(tmp.path()).unwrap();
        let loaded = reopened
            .todos(&list_id)
            .unwrap()
            .into_iter()
            .find(|todo| todo.id == todo_id)
            .unwrap();
        assert_eq!(
            loaded.work,
            TodoWorkState {
                status: TodoStatus::InProgress,
                agent_session_id: Some("session-42".to_string()),
            }
        );
    }

    #[test]
    fn reorder_persists_new_order() {
        let (_tmp, db) = open_temp_db();
        let list = db
            .create_todo_list(&project("proj-1"), Some("feat-1"))
            .unwrap();
        let a = db.add_todo(&list.id, "a", None, TodoPriority::Med).unwrap();
        let b = db.add_todo(&list.id, "b", None, TodoPriority::Med).unwrap();
        let c = db.add_todo(&list.id, "c", None, TodoPriority::Med).unwrap();

        db.reorder_todos(&[c.id.clone(), a.id.clone(), b.id.clone()])
            .unwrap();

        let order: Vec<String> = db
            .todos(&list.id)
            .unwrap()
            .into_iter()
            .map(|t| t.title)
            .collect();
        assert_eq!(order, vec!["c", "a", "b"]);
    }

    #[test]
    fn open_items_sort_before_done() {
        let (_tmp, db) = open_temp_db();
        let list = db
            .create_todo_list(&project("proj-1"), Some("feat-1"))
            .unwrap();
        let mut a = db.add_todo(&list.id, "a", None, TodoPriority::Med).unwrap();
        let _b = db.add_todo(&list.id, "b", None, TodoPriority::Med).unwrap();
        a.work.status = TodoStatus::Completed;
        db.update_todo(&a).unwrap();

        let titles: Vec<String> = db
            .todos(&list.id)
            .unwrap()
            .into_iter()
            .map(|t| t.title)
            .collect();
        // Open "b" comes before done "a" despite lower sort_order on "a".
        assert_eq!(titles, vec!["b", "a"]);
    }

    #[test]
    fn carry_over_and_host_feature_update() {
        let (_tmp, db) = open_temp_db();
        let list = db
            .create_todo_list(&project("proj-1"), Some("feat-1"))
            .unwrap();

        db.set_todo_carry_over(&list.id, Some("finishing the parser"))
            .unwrap();
        db.set_todo_list_host_feature(&list.id, "feat-2").unwrap();

        let loaded = db.todo_list(&project("proj-1")).unwrap().unwrap();
        assert_eq!(loaded.carry_over.as_deref(), Some("finishing the parser"));
        assert_eq!(loaded.feature_id.as_deref(), Some("feat-2"));
    }

    #[test]
    fn linked_feature_survives_a_roundtrip_and_is_independent_of_the_session_link() {
        let (_tmp, db) = open_temp_db();
        let list = db
            .create_todo_list(&project("proj-1"), Some("feat-1"))
            .unwrap();
        let mut todo = db
            .add_todo(&list.id, "plan it", None, TodoPriority::Med)
            .unwrap();
        assert!(todo.linked_feature_id.is_none());

        // Both links can be held at once: they point at different things.
        todo.work.agent_session_id = Some("sess-1".to_string());
        todo.linked_feature_id = Some("feat-new".to_string());
        db.update_todo(&todo).unwrap();

        let loaded = &db.todos(&list.id).unwrap()[0];
        assert_eq!(loaded.work.agent_session_id.as_deref(), Some("sess-1"));
        assert_eq!(loaded.linked_feature_id.as_deref(), Some("feat-new"));
    }

    #[test]
    fn clearing_a_deleted_features_link_keeps_the_todo_and_its_session_link() {
        let (_tmp, db) = open_temp_db();
        let list = db
            .create_todo_list(&project("proj-1"), Some("feat-1"))
            .unwrap();
        let mut a = db.add_todo(&list.id, "a", None, TodoPriority::Med).unwrap();
        let mut b = db.add_todo(&list.id, "b", None, TodoPriority::Med).unwrap();
        a.linked_feature_id = Some("feat-gone".to_string());
        a.work.agent_session_id = Some("sess-1".to_string());
        b.linked_feature_id = Some("feat-kept".to_string());
        db.update_todo(&a).unwrap();
        db.update_todo(&b).unwrap();

        db.clear_todo_linked_feature("feat-gone").unwrap();

        let loaded = db.todos(&list.id).unwrap();
        assert_eq!(loaded.len(), 2, "the TODOs outlive the feature");
        let a = loaded.iter().find(|t| t.title == "a").unwrap();
        let b = loaded.iter().find(|t| t.title == "b").unwrap();
        assert!(a.linked_feature_id.is_none());
        // Only the feature link is dropped; the session link is a separate one.
        assert_eq!(a.work.agent_session_id.as_deref(), Some("sess-1"));
        assert_eq!(b.linked_feature_id.as_deref(), Some("feat-kept"));
    }

    #[test]
    fn delete_list_cascades_to_todos() {
        let (_tmp, db) = open_temp_db();
        let list = db
            .create_todo_list(&project("proj-1"), Some("feat-1"))
            .unwrap();
        db.add_todo(&list.id, "a", None, TodoPriority::Med).unwrap();

        db.delete_todo_lists_for_project("proj-1").unwrap();
        assert!(db.todo_list(&project("proj-1")).unwrap().is_none());
        // Cascade removed the items too.
        assert!(db.todos(&list.id).unwrap().is_empty());
    }

    /// Deleting a project takes its worktree lists with it and leaves the
    /// machine-wide list — which belongs to no project — alone.
    #[test]
    fn deleting_a_project_takes_its_worktree_lists_but_not_the_global_one() {
        let (_tmp, db) = open_temp_db();
        let proj = db
            .create_todo_list(&project("proj-1"), Some("feat-1"))
            .unwrap();
        let wt = db
            .create_todo_list(&worktree("proj-1", "/wt/a"), Some("feat-a"))
            .unwrap();
        let global = db.create_todo_list(&TodoScope::Global, None).unwrap();
        db.add_todo(&global.id, "keep me", None, TodoPriority::Med)
            .unwrap();

        db.delete_todo_lists_for_project("proj-1").unwrap();

        assert!(db.todo_list_by_id(&proj.id).unwrap().is_none());
        assert!(db.todo_list_by_id(&wt.id).unwrap().is_none());
        assert!(db.todo_list(&TodoScope::Global).unwrap().is_some());
        assert_eq!(db.todos(&global.id).unwrap().len(), 1);
    }

    /// The worktree list can be dropped on its own once its items have been
    /// dispositioned, without disturbing the project's own list.
    #[test]
    fn deleting_one_worktree_list_leaves_the_project_list() {
        let (_tmp, db) = open_temp_db();
        let proj = db
            .create_todo_list(&project("proj-1"), Some("feat-1"))
            .unwrap();
        db.create_todo_list(&worktree("proj-1", "/wt/a"), Some("feat-a"))
            .unwrap();

        db.delete_worktree_todo_list("proj-1", "/wt/a").unwrap();

        assert!(
            db.todo_list(&worktree("proj-1", "/wt/a"))
                .unwrap()
                .is_none()
        );
        assert!(db.todo_list_by_id(&proj.id).unwrap().is_some());
    }
}
