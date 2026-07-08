//! SQLite persistence for per-project TODO lists.
//!
//! Each project has at most one [`TodoList`] (enforced by a UNIQUE constraint
//! on `project_id`). The list is hosted by one of the project's features
//! (`feature_id`) and owns an ordered set of [`Todo`] items. See
//! `docs/backlog/feature-todos-plan.md`.
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

/// A per-project TODO list, hosted by one feature.
#[derive(Debug, Clone)]
pub struct TodoList {
    pub id: String,
    pub project_id: String,
    pub feature_id: String,
    /// "Left off here" carry-over banner note.
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
    /// `FeatureSession.id` of an agent launched for this item, if any.
    pub spawned_session_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Load the TODO list for `project_id`, or `None` if the project has none.
pub fn load_list(conn: &Connection, project_id: &str) -> Result<Option<TodoList>> {
    let row = conn
        .query_row(
            "SELECT id, project_id, feature_id, carry_over, created_at, updated_at
             FROM todo_lists WHERE project_id = ?1",
            params![project_id],
            |row| {
                Ok(TodoList {
                    id: row.get(0)?,
                    project_id: row.get(1)?,
                    feature_id: row.get(2)?,
                    carry_over: row.get(3)?,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                })
            },
        )
        .optional()?;
    Ok(row)
}

/// Create the project's TODO list under `feature_id`, returning it. Fails if a
/// list already exists for the project (one-list-per-project constraint).
pub fn create_list(conn: &Connection, project_id: &str, feature_id: &str) -> Result<TodoList> {
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO todo_lists (id, project_id, feature_id, carry_over, created_at, updated_at)
         VALUES (?1, ?2, ?3, NULL, datetime('now'), datetime('now'))",
        params![id, project_id, feature_id],
    )?;
    load_list(conn, project_id)?
        .ok_or_else(|| anyhow::anyhow!("todo list vanished immediately after insert"))
}

/// Return the project's existing TODO list, creating one under `feature_id` if
/// none exists yet.
pub fn load_or_create_list(
    conn: &Connection,
    project_id: &str,
    feature_id: &str,
) -> Result<TodoList> {
    match load_list(conn, project_id)? {
        Some(list) => Ok(list),
        None => create_list(conn, project_id, feature_id),
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

/// Delete the TODO list (and its items) for `project_id`, if any. Called when a
/// project is deleted, since there is no FK cascade from `projects`.
pub fn delete_list_for_project(conn: &Connection, project_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM todo_lists WHERE project_id = ?1",
        params![project_id],
    )?;
    Ok(())
}

/// Load every item in `list_id`, sorted by done (open first), then priority,
/// then manual `sort_order`.
pub fn list_todos(conn: &Connection, list_id: &str) -> Result<Vec<Todo>> {
    let mut stmt = conn.prepare(
        "SELECT id, list_id, title, body, priority, done, sort_order,
                spawned_session_id, created_at, updated_at
         FROM todos WHERE list_id = ?1
         ORDER BY done ASC, sort_order ASC",
    )?;
    let rows = stmt.query_map(params![list_id], |row| {
        let priority: String = row.get(4)?;
        let done: i64 = row.get(5)?;
        Ok(Todo {
            id: row.get(0)?,
            list_id: row.get(1)?,
            title: row.get(2)?,
            body: row.get(3)?,
            priority: TodoPriority::from_db_str(&priority),
            done: done != 0,
            sort_order: row.get(6)?,
            spawned_session_id: row.get(7)?,
            created_at: row.get(8)?,
            updated_at: row.get(9)?,
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
    let next_order: i64 = conn.query_row(
        "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM todos WHERE list_id = ?1",
        params![list_id],
        |row| row.get(0),
    )?;
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO todos
            (id, list_id, title, body, priority, done, sort_order,
             spawned_session_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, NULL, datetime('now'), datetime('now'))",
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
            sort_order = ?7, spawned_session_id = ?8, updated_at = datetime('now')
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
        ],
    )?;
    Ok(())
}

/// Delete a single TODO by id. Any session it spawned is left untouched.
pub fn delete_todo(conn: &Connection, todo_id: &str) -> Result<()> {
    conn.execute("DELETE FROM todos WHERE id = ?1", params![todo_id])?;
    Ok(())
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

    #[test]
    fn create_and_load_list() {
        let (_tmp, db) = open_temp_db();

        assert!(db.todo_list("proj-1").unwrap().is_none());

        let list = db.create_todo_list("proj-1", "feat-1").unwrap();
        assert_eq!(list.project_id, "proj-1");
        assert_eq!(list.feature_id, "feat-1");
        assert!(list.carry_over.is_none());

        let loaded = db.todo_list("proj-1").unwrap().unwrap();
        assert_eq!(loaded.id, list.id);
    }

    #[test]
    fn one_list_per_project() {
        let (_tmp, db) = open_temp_db();
        db.create_todo_list("proj-1", "feat-1").unwrap();
        // UNIQUE(project_id) must reject a second list.
        assert!(db.create_todo_list("proj-1", "feat-2").is_err());
    }

    #[test]
    fn load_or_create_is_idempotent() {
        let (_tmp, db) = open_temp_db();
        let a = db.load_or_create_todo_list("proj-1", "feat-1").unwrap();
        let b = db.load_or_create_todo_list("proj-1", "feat-9").unwrap();
        assert_eq!(a.id, b.id);
        // Host feature is preserved from the first create, not overwritten.
        assert_eq!(b.feature_id, "feat-1");
    }

    #[test]
    fn todo_crud_roundtrip() {
        let (_tmp, db) = open_temp_db();
        let list = db.create_todo_list("proj-1", "feat-1").unwrap();

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
        let list = db.create_todo_list("proj-1", "feat-1").unwrap();
        let a = db.add_todo(&list.id, "a", None, TodoPriority::Med).unwrap();
        let b = db.add_todo(&list.id, "b", None, TodoPriority::Med).unwrap();
        let c = db.add_todo(&list.id, "c", None, TodoPriority::Med).unwrap();
        assert_eq!((a.sort_order, b.sort_order, c.sort_order), (0, 1, 2));
    }

    #[test]
    fn reorder_persists_new_order() {
        let (_tmp, db) = open_temp_db();
        let list = db.create_todo_list("proj-1", "feat-1").unwrap();
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
        let list = db.create_todo_list("proj-1", "feat-1").unwrap();
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
        let list = db.create_todo_list("proj-1", "feat-1").unwrap();

        db.set_todo_carry_over(&list.id, Some("finishing the parser"))
            .unwrap();
        db.set_todo_list_host_feature(&list.id, "feat-2").unwrap();

        let loaded = db.todo_list("proj-1").unwrap().unwrap();
        assert_eq!(loaded.carry_over.as_deref(), Some("finishing the parser"));
        assert_eq!(loaded.feature_id, "feat-2");
    }

    #[test]
    fn delete_list_cascades_to_todos() {
        let (_tmp, db) = open_temp_db();
        let list = db.create_todo_list("proj-1", "feat-1").unwrap();
        db.add_todo(&list.id, "a", None, TodoPriority::Med).unwrap();

        db.delete_todo_list_for_project("proj-1").unwrap();
        assert!(db.todo_list("proj-1").unwrap().is_none());
        // Cascade removed the items too.
        assert!(db.todos(&list.id).unwrap().is_empty());
    }
}
