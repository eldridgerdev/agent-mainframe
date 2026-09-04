//! SQLite persistence for user-created prompt-library templates.
//!
//! Unlike most domain data, this table lives *outside* the `ProjectStore`
//! full-replace save path in `db::store::do_save`. Prompt templates are
//! global, not scoped to a project or feature, and it is normal to run AMF
//! as several concurrent processes — one per checkout — all pointed at the
//! same `~/.config/amf/amf.db`. Each process holds its own in-memory
//! `ProjectStore` snapshot; a generic `App::save()` fires often (status
//! sync, feature start/stop, session changes) and previously rewrote the
//! *entire* `prompt_templates` table from that snapshot every time. Any
//! process whose snapshot predated a sibling's edit would silently erase it
//! on its next unrelated save. The targeted insert/update/delete here touch
//! only the row they mean to change, so a save from one process can never
//! discard another's.

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};

use crate::prompt_library::{PromptPlaceholder, PromptTemplate};

fn dt_to_str(dt: &DateTime<Utc>) -> String {
    dt.to_rfc3339()
}

fn dt_from_str(s: &str) -> DateTime<Utc> {
    s.parse().unwrap_or_else(|_| Utc::now())
}

pub fn load(conn: &Connection) -> Result<Vec<PromptTemplate>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, description, body, tags, placeholders, created_at, updated_at
         FROM prompt_templates ORDER BY sort_order ASC, rowid ASC",
    )?;

    let templates = stmt
        .query_map([], |row| {
            let tags_json: String = row.get(4)?;
            let placeholders_json: String = row.get(5)?;
            Ok(PromptTemplate {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                body: row.get(3)?,
                tags: serde_json::from_str::<Vec<String>>(&tags_json).unwrap_or_default(),
                placeholders: serde_json::from_str::<Vec<PromptPlaceholder>>(&placeholders_json)
                    .unwrap_or_default(),
                created_at: dt_from_str(&row.get::<_, String>(6)?),
                updated_at: dt_from_str(&row.get::<_, String>(7)?),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(templates)
}

/// Insert a new template, appending it after the current highest
/// `sort_order` so it lands at the end of the list regardless of what any
/// other process has added concurrently.
pub fn insert(conn: &Connection, template: &PromptTemplate) -> Result<()> {
    let next_sort_order: i64 = conn.query_row(
        "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM prompt_templates",
        [],
        |row| row.get(0),
    )?;
    let tags_json = serde_json::to_string(&template.tags)?;
    let placeholders_json = serde_json::to_string(&template.placeholders)?;
    conn.execute(
        "INSERT INTO prompt_templates (
            id, name, description, body, tags, placeholders,
            created_at, updated_at, sort_order
        ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            template.id,
            template.name,
            template.description,
            template.body,
            tags_json,
            placeholders_json,
            dt_to_str(&template.created_at),
            dt_to_str(&template.updated_at),
            next_sort_order,
        ],
    )?;
    Ok(())
}

/// Update an existing template's editable fields, matched by id. Leaves
/// `sort_order` and `created_at` untouched.
pub fn update(conn: &Connection, template: &PromptTemplate) -> Result<()> {
    let tags_json = serde_json::to_string(&template.tags)?;
    let placeholders_json = serde_json::to_string(&template.placeholders)?;
    conn.execute(
        "UPDATE prompt_templates SET
            name = ?2, description = ?3, body = ?4, tags = ?5,
            placeholders = ?6, updated_at = ?7
         WHERE id = ?1",
        params![
            template.id,
            template.name,
            template.description,
            template.body,
            tags_json,
            placeholders_json,
            dt_to_str(&template.updated_at),
        ],
    )?;
    Ok(())
}

pub fn delete(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM prompt_templates WHERE id = ?1", params![id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::AmfDb;
    use crate::prompt_library::PlaceholderKind;
    use tempfile::NamedTempFile;

    fn open_temp_db() -> (NamedTempFile, AmfDb) {
        let tmp = NamedTempFile::new().unwrap();
        let db = AmfDb::open(tmp.path()).unwrap();
        (tmp, db)
    }

    #[test]
    fn insert_then_load_preserves_order_and_fields() {
        let (_tmp, db) = open_temp_db();

        let mut first = PromptTemplate::new("Fix bug".to_string(), "Fix {{area}}".to_string());
        first.id = "tpl-1".to_string();
        first.description = Some("a description".to_string());
        first.tags = vec!["dev".to_string(), "fix".to_string()];
        first.placeholders = vec![PromptPlaceholder {
            key: "area".to_string(),
            label: Some("Area".to_string()),
            kind: PlaceholderKind::Text { default: None },
            required: true,
        }];

        let mut second = PromptTemplate::new("Review".to_string(), "Review the diff".to_string());
        second.id = "tpl-2".to_string();

        db.insert_prompt_template(&first).unwrap();
        db.insert_prompt_template(&second).unwrap();

        let loaded = db.load_prompt_templates().unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].id, "tpl-1");
        assert_eq!(loaded[1].id, "tpl-2");

        let lt = &loaded[0];
        assert_eq!(lt.name, "Fix bug");
        assert_eq!(lt.body, "Fix {{area}}");
        assert_eq!(lt.description, Some("a description".to_string()));
        assert_eq!(lt.tags, vec!["dev".to_string(), "fix".to_string()]);
        assert_eq!(lt.placeholders, first.placeholders);
    }

    #[test]
    fn update_changes_only_the_matching_row() {
        let (_tmp, db) = open_temp_db();

        let mut first = PromptTemplate::new("First".to_string(), "Body one".to_string());
        first.id = "tpl-1".to_string();
        let second = PromptTemplate::new("Second".to_string(), "Body two".to_string());
        db.insert_prompt_template(&first).unwrap();
        db.insert_prompt_template(&second).unwrap();

        first.name = "First (edited)".to_string();
        first.body = "Body one edited".to_string();
        db.update_prompt_template(&first).unwrap();

        let loaded = db.load_prompt_templates().unwrap();
        assert_eq!(loaded.len(), 2);
        let edited = loaded.iter().find(|t| t.id == "tpl-1").unwrap();
        assert_eq!(edited.name, "First (edited)");
        assert_eq!(edited.body, "Body one edited");
        let untouched = loaded.iter().find(|t| t.id == second.id).unwrap();
        assert_eq!(untouched.name, "Second");
    }

    #[test]
    fn delete_removes_only_that_template() {
        let (_tmp, db) = open_temp_db();

        let first = PromptTemplate::new("First".to_string(), "Body one".to_string());
        let second = PromptTemplate::new("Second".to_string(), "Body two".to_string());
        db.insert_prompt_template(&first).unwrap();
        db.insert_prompt_template(&second).unwrap();

        db.delete_prompt_template(&first.id).unwrap();

        let loaded = db.load_prompt_templates().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, second.id);
    }

    /// The whole point of moving this out of the `ProjectStore` full-replace
    /// path: a generic `save_store` from a process whose in-memory snapshot
    /// predates a template inserted by this connection must not erase it.
    #[test]
    fn generic_store_save_does_not_clobber_templates_inserted_out_of_band() {
        use crate::project::ProjectStore;
        use std::collections::HashMap;

        let (_tmp, db) = open_temp_db();

        let template = PromptTemplate::new("Survivor".to_string(), "Body".to_string());
        db.insert_prompt_template(&template).unwrap();

        // A sibling process's stale, empty-templates snapshot.
        let stale_store = ProjectStore {
            version: crate::project::CURRENT_PROJECT_STORE_VERSION,
            projects: Vec::new(),
            session_bookmarks: Vec::new(),
            available_harnesses: Vec::new(),
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        };
        db.save_store(&stale_store).unwrap();

        let loaded = db.load_prompt_templates().unwrap();
        assert_eq!(loaded.len(), 1, "concurrent save clobbered the template");
        assert_eq!(loaded[0].name, "Survivor");
    }
}
