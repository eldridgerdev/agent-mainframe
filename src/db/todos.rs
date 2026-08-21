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
use uuid::Uuid;

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
    pub done: bool,
    pub sort_order: i64,
    /// `FeatureSession.id` of an agent launched for this item, if any. Always
    /// a session inside the list's *host* feature.
    pub spawned_session_id: Option<String>,
    /// `Feature.id` of a feature plan mode created for this item, if any. A
    /// different destination from [`Self::spawned_session_id`], and a TODO can
    /// carry both.
    pub linked_feature_id: Option<String>,
    /// Set while this item is actively being worked, so "implement next" skips
    /// it. Written when a session is spawned for it and cleared when the item
    /// is completed, when its session goes away, or by hand — none of which
    /// `spawned_session_id` alone can express.
    pub in_progress: bool,
    pub created_at: String,
    pub updated_at: String,
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

/// Load every item in `list_id`, sorted by done (open first), then priority,
/// then manual `sort_order`.
pub fn list_todos(conn: &Connection, list_id: &str) -> Result<Vec<Todo>> {
    let mut stmt = conn.prepare(
        "SELECT id, list_id, title, body, priority, done, sort_order,
                spawned_session_id, linked_feature_id, in_progress,
                created_at, updated_at
         FROM todos WHERE list_id = ?1
         ORDER BY done ASC, sort_order ASC",
    )?;
    let rows = stmt.query_map(params![list_id], |row| {
        let priority: String = row.get(4)?;
        let done: i64 = row.get(5)?;
        let in_progress: i64 = row.get(9)?;
        Ok(Todo {
            id: row.get(0)?,
            list_id: row.get(1)?,
            title: row.get(2)?,
            body: row.get(3)?,
            priority: TodoPriority::from_db_str(&priority),
            done: done != 0,
            sort_order: row.get(6)?,
            spawned_session_id: row.get(7)?,
            linked_feature_id: row.get(8)?,
            in_progress: in_progress != 0,
            created_at: row.get(10)?,
            updated_at: row.get(11)?,
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
            (id, list_id, title, body, priority, done, sort_order,
             spawned_session_id, linked_feature_id, in_progress,
             created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, NULL, NULL, 0,
                 datetime('now'), datetime('now'))",
        params![id, list_id, title, body, priority.as_db_str(), next_order],
    )?;
    Ok(Todo {
        id,
        list_id: list_id.to_string(),
        title: title.to_string(),
        body: body.map(str::to_string),
        priority,
        done: false,
        sort_order: next_order,
        spawned_session_id: None,
        linked_feature_id: None,
        in_progress: false,
        created_at: String::new(),
        updated_at: String::new(),
    })
}

/// Persist the mutable fields of an existing TODO (everything except
/// `created_at`). `updated_at` is bumped automatically.
pub fn update_todo(conn: &Connection, todo: &Todo) -> Result<()> {
    conn.execute(
        "UPDATE todos SET
            list_id = ?2, title = ?3, body = ?4, priority = ?5, done = ?6,
            sort_order = ?7, spawned_session_id = ?8, linked_feature_id = ?9,
            in_progress = ?10, updated_at = datetime('now')
         WHERE id = ?1",
        params![
            todo.id,
            todo.list_id,
            todo.title,
            todo.body,
            todo.priority.as_db_str(),
            todo.done as i64,
            todo.sort_order,
            todo.spawned_session_id,
            todo.linked_feature_id,
            todo.in_progress as i64,
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
pub fn set_spawned_session(conn: &Connection, todo_id: &str, session_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE todos SET spawned_session_id = ?2, updated_at = datetime('now')
         WHERE id = ?1",
        params![todo_id, session_id],
    )?;
    Ok(())
}

/// Set or clear a TODO's in-progress flag.
///
/// Targeted for the same reason as [`set_spawned_session`], and for one more:
/// the flag is written from surfaces where the TODOs overlay is not open (the
/// dashboard's "implement next"), so there is no in-memory row to write back
/// through [`update_todo`].
pub fn set_in_progress(conn: &Connection, todo_id: &str, in_progress: bool) -> Result<()> {
    conn.execute(
        "UPDATE todos SET in_progress = ?2, updated_at = datetime('now')
         WHERE id = ?1",
        params![todo_id, in_progress as i64],
    )?;
    Ok(())
}

/// Drop a TODO's link to the session spawned for it, when that session is gone.
pub fn clear_spawned_session(conn: &Connection, todo_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE todos SET spawned_session_id = NULL, updated_at = datetime('now')
         WHERE id = ?1",
        params![todo_id],
    )?;
    Ok(())
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
/// The copy is deliberately *unstarted*: `spawned_session_id`, the feature
/// link, and `in_progress` are all dropped. Carrying them would leave two rows
/// in two panes each claiming the same session, and "implement next" would
/// then hold both in reserve for work only one of them describes.
pub fn copy_todo(conn: &Connection, todo_id: &str, target_list_id: &str) -> Result<Option<Todo>> {
    let source = conn
        .query_row(
            "SELECT title, body, priority, done FROM todos WHERE id = ?1",
            params![todo_id],
            |row| {
                let priority: String = row.get(2)?;
                let done: i64 = row.get(3)?;
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    TodoPriority::from_db_str(&priority),
                    done != 0,
                ))
            },
        )
        .optional()?;
    let Some((title, body, priority, done)) = source else {
        return Ok(None);
    };

    let next_order = next_sort_order(conn, target_list_id)?;
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO todos
            (id, list_id, title, body, priority, done, sort_order,
             spawned_session_id, linked_feature_id, in_progress,
             created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL, 0,
                 datetime('now'), datetime('now'))",
        params![
            id,
            target_list_id,
            title,
            body,
            priority.as_db_str(),
            done as i64,
            next_order
        ],
    )?;
    Ok(Some(Todo {
        id,
        list_id: target_list_id.to_string(),
        title,
        body,
        priority,
        done,
        sort_order: next_order,
        spawned_session_id: None,
        linked_feature_id: None,
        in_progress: false,
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
        todo.done = true;
        todo.priority = TodoPriority::Low;
        todo.spawned_session_id = Some("sess-7".to_string());
        db.update_todo(&todo).unwrap();

        let loaded = db.todos(&list.id).unwrap();
        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].done);
        assert_eq!(loaded[0].priority, TodoPriority::Low);
        assert_eq!(loaded[0].body.as_deref(), Some("cover edge cases"));
        assert_eq!(loaded[0].spawned_session_id.as_deref(), Some("sess-7"));

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
        moving.spawned_session_id = Some("sess-1".to_string());
        moving.linked_feature_id = Some("feat-planned".to_string());
        moving.in_progress = true;
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
        assert_eq!(moved.spawned_session_id.as_deref(), Some("sess-1"));
        assert_eq!(moved.linked_feature_id.as_deref(), Some("feat-planned"));
        assert!(moved.in_progress);
        assert_eq!(moved.body.as_deref(), Some("notes"));
        assert_eq!(moved.priority, TodoPriority::High);
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
        original.spawned_session_id = Some("sess-1".to_string());
        original.linked_feature_id = Some("feat-planned".to_string());
        original.in_progress = true;
        db.update_todo(&original).unwrap();

        let copy = db.copy_todo(&original.id, &dst.id).unwrap().unwrap();

        // The original is untouched, links and all.
        let kept = &db.todos(&src.id).unwrap()[0];
        assert_eq!(kept.id, original.id);
        assert_eq!(kept.spawned_session_id.as_deref(), Some("sess-1"));
        assert!(kept.in_progress);

        assert_ne!(copy.id, original.id);
        let landed = &db.todos(&dst.id).unwrap()[0];
        assert_eq!(landed.title, "share me");
        assert_eq!(landed.body.as_deref(), Some("why"));
        assert_eq!(landed.priority, TodoPriority::Low);
        assert!(landed.spawned_session_id.is_none());
        assert!(landed.linked_feature_id.is_none());
        assert!(!landed.in_progress);
    }

    #[test]
    fn copying_a_todo_that_is_gone_reports_it_rather_than_inventing_one() {
        let (_tmp, db) = open_temp_db();
        let dst = db.create_todo_list(&TodoScope::Global, None).unwrap();
        assert!(db.copy_todo("no-such-todo", &dst.id).unwrap().is_none());
        assert!(db.todos(&dst.id).unwrap().is_empty());
    }

    /// The in-progress flag round-trips, and the two targeted writers can set
    /// and clear it without going through a whole-row update.
    #[test]
    fn in_progress_roundtrips_and_has_targeted_writers() {
        let (_tmp, db) = open_temp_db();
        let list = db
            .create_todo_list(&project("proj-1"), Some("feat-1"))
            .unwrap();
        let mut todo = db
            .add_todo(&list.id, "Ship it", None, TodoPriority::High)
            .unwrap();
        // A new TODO is nobody's work in progress.
        assert!(!todo.in_progress);
        assert!(!db.todos(&list.id).unwrap()[0].in_progress);

        todo.in_progress = true;
        db.update_todo(&todo).unwrap();
        assert!(db.todos(&list.id).unwrap()[0].in_progress);

        db.set_todo_in_progress(&todo.id, false).unwrap();
        assert!(!db.todos(&list.id).unwrap()[0].in_progress);

        // Clearing a dead session link is its own write, leaving everything
        // else on the row alone.
        db.set_todo_spawned_session(&todo.id, "sess-1").unwrap();
        db.set_todo_in_progress(&todo.id, true).unwrap();
        db.clear_todo_spawned_session(&todo.id).unwrap();
        let loaded = db.todos(&list.id).unwrap();
        assert!(loaded[0].spawned_session_id.is_none());
        assert_eq!(loaded[0].title, "Ship it");
        assert!(loaded[0].in_progress);
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
        a.done = true;
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
        todo.spawned_session_id = Some("sess-1".to_string());
        todo.linked_feature_id = Some("feat-new".to_string());
        db.update_todo(&todo).unwrap();

        let loaded = &db.todos(&list.id).unwrap()[0];
        assert_eq!(loaded.spawned_session_id.as_deref(), Some("sess-1"));
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
        a.spawned_session_id = Some("sess-1".to_string());
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
        assert_eq!(a.spawned_session_id.as_deref(), Some("sess-1"));
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
