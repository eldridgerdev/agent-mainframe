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
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use sha2::{Digest, Sha256};

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
///
/// The max-then-insert pair runs inside `BEGIN IMMEDIATE`/`COMMIT` (mirroring
/// `store::save`), which takes SQLite's write lock before the `SELECT MAX` so
/// a second process can't read the same stale max while this one is still
/// between the two statements — without it, two concurrent inserts could
/// compute identical `sort_order` values.
pub fn insert(conn: &Connection, template: &PromptTemplate) -> Result<()> {
    conn.execute_batch("BEGIN IMMEDIATE;")?;
    match do_insert(conn, template) {
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

fn do_insert(conn: &Connection, template: &PromptTemplate) -> Result<()> {
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

/// Raw-column revision for delayed authoring. Sort order is deliberately absent:
/// unrelated insertions/reorders must not make an unchanged row uneditable.
pub(crate) struct Snapshot {
    pub template: PromptTemplate,
    pub revision: String,
    raw_tags: String,
    raw_placeholders: String,
}

pub(crate) enum Mutation {
    Edit {
        name: String,
        body: String,
        tags: Vec<String>,
    },
    Delete,
    Duplicate,
}

pub(crate) fn snapshot(conn: &Connection, id: &str) -> Result<Option<Snapshot>> {
    let row = conn
        .query_row(
            "SELECT id,name,description,body,tags,placeholders,created_at,updated_at
         FROM prompt_templates WHERE id=?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                ))
            },
        )
        .optional()?;
    let Some(raw) = row else {
        return Ok(None);
    };
    let revision = format!("{:x}", Sha256::digest(serde_json::to_vec(&raw)?));
    // Browsing tolerates old broken rows; authoring must never replace their
    // metadata with the browser's defaults or silently repair invalid dates.
    let metadata: serde_json::Value = serde_json::from_str(&raw.5)?;
    for field in metadata
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Invalid placeholder metadata"))?
    {
        let mut kind = field.clone();
        let object = kind
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("Invalid placeholder metadata"))?;
        object
            .entry("kind")
            .or_insert_with(|| serde_json::Value::String("text".into()));
        // Flattened serde defaults can hide an invalid kind or its values.
        // Parse the enum directly while retaining the original raw document.
        serde_json::from_value::<crate::prompt_library::PlaceholderKind>(kind)?;
    }
    let template = PromptTemplate {
        id: raw.0,
        name: raw.1,
        description: raw.2,
        body: raw.3,
        tags: serde_json::from_str(&raw.4)?,
        placeholders: serde_json::from_str(&raw.5)?,
        created_at: raw.6.parse()?,
        updated_at: raw.7.parse()?,
    };
    Ok(Some(Snapshot {
        template,
        revision,
        raw_tags: raw.4,
        raw_placeholders: raw.5,
    }))
}

/// Read + compare + mutate while owning SQLite's writer lock. None means the
/// exact persisted row was deleted or edited elsewhere; no write took place.
pub(crate) fn mutate(
    conn: &Connection,
    id: &str,
    expected: &str,
    mutation: Mutation,
) -> Result<Option<String>> {
    let transaction = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let Some(current) = snapshot(&transaction, id)? else {
        return Ok(None);
    };
    if current.revision != expected {
        return Ok(None);
    }
    let result_id = match mutation {
        Mutation::Edit { name, body, tags } => {
            transaction.execute(
                "UPDATE prompt_templates SET name=?2,body=?3,tags=?4,updated_at=?5 WHERE id=?1",
                params![
                    id,
                    name,
                    body,
                    serde_json::to_string(&tags)?,
                    dt_to_str(&Utc::now())
                ],
            )?;
            id.to_string()
        }
        Mutation::Delete => {
            transaction.execute("DELETE FROM prompt_templates WHERE id=?1", [id])?;
            id.to_string()
        }
        Mutation::Duplicate => {
            let copy = current.template.user_copy();
            let order: i64 = transaction.query_row(
                "SELECT COALESCE(MAX(sort_order),-1)+1 FROM prompt_templates",
                [],
                |row| row.get(0),
            )?;
            transaction.execute(
                "INSERT INTO prompt_templates(id,name,description,body,tags,placeholders,created_at,updated_at,sort_order)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![copy.id,copy.name,copy.description,copy.body,current.raw_tags,current.raw_placeholders,
                    dt_to_str(&copy.created_at),dt_to_str(&copy.updated_at),order],
            )?;
            copy.id
        }
    };
    transaction.commit()?;
    Ok(Some(result_id))
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
    fn checked_edits_and_copies_preserve_raw_metadata_and_ignore_reordering() {
        let (_tmp, db) = open_temp_db();
        let original = PromptTemplate::new("Original".into(), "Use {{area}}".into());
        let sibling = PromptTemplate::new("Sibling".into(), "Kept".into());
        insert(&db.conn, &original).unwrap();
        insert(&db.conn, &sibling).unwrap();
        let raw = r#"[ {"key":"area","kind":"text","default":"api","future_format":{"width":80}}, {"key":"unused","kind":"select","options":["a","b"]} ]"#;
        db.conn.execute("UPDATE prompt_templates SET description='Original description',placeholders=?2 WHERE id=?1", params![original.id, raw]).unwrap();
        let before = snapshot(&db.conn, &original.id).unwrap().unwrap();
        let sibling_revision = snapshot(&db.conn, &sibling.id).unwrap().unwrap().revision;
        db.conn
            .execute("UPDATE prompt_templates SET sort_order=sort_order+10", [])
            .unwrap();
        let copy_id = mutate(
            &db.conn,
            &original.id,
            &before.revision,
            Mutation::Duplicate,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            snapshot(&db.conn, &copy_id)
                .unwrap()
                .unwrap()
                .raw_placeholders,
            raw
        );
        assert_eq!(
            snapshot(&db.conn, &copy_id).unwrap().unwrap().template.name,
            "Original (copy)"
        );
        mutate(
            &db.conn,
            &original.id,
            &before.revision,
            Mutation::Edit {
                name: "Edited".into(),
                body: "Changed".into(),
                tags: vec!["tag".into()],
            },
        )
        .unwrap()
        .unwrap();
        let after = snapshot(&db.conn, &original.id).unwrap().unwrap();
        assert_eq!(after.raw_placeholders, raw);
        assert_eq!(after.template.description, before.template.description);
        assert_eq!(after.template.created_at, before.template.created_at);
        assert_eq!(
            snapshot(&db.conn, &sibling.id).unwrap().unwrap().revision,
            sibling_revision
        );
        assert!(
            mutate(&db.conn, &original.id, &before.revision, Mutation::Delete)
                .unwrap()
                .is_none()
        );
        assert!(
            mutate(
                &db.conn,
                &original.id,
                &before.revision,
                Mutation::Duplicate
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn writer_lock_external_changes_and_failed_mutation_never_clobber_a_row() {
        let (tmp, db) = open_temp_db();
        let original = PromptTemplate::new("Original".into(), "Kept".into());
        insert(&db.conn, &original).unwrap();
        let before = snapshot(&db.conn, &original.id).unwrap().unwrap();
        let sibling = Connection::open(tmp.path()).unwrap();
        db.conn.busy_timeout(std::time::Duration::ZERO).unwrap();
        sibling.execute_batch("BEGIN IMMEDIATE").unwrap();
        assert!(mutate(&db.conn, &original.id, &before.revision, Mutation::Delete).is_err());
        sibling
            .execute(
                "UPDATE prompt_templates SET body='Other process' WHERE id=?1",
                [&original.id],
            )
            .unwrap();
        sibling.execute_batch("COMMIT").unwrap();
        assert!(
            mutate(&db.conn, &original.id, &before.revision, Mutation::Delete)
                .unwrap()
                .is_none()
        );
        let fresh = snapshot(&db.conn, &original.id).unwrap().unwrap();
        db.conn.execute_batch("CREATE TRIGGER reject_prompt_edit BEFORE UPDATE ON prompt_templates BEGIN SELECT RAISE(ABORT,'write refused'); END;").unwrap();
        assert!(
            mutate(
                &db.conn,
                &original.id,
                &fresh.revision,
                Mutation::Edit {
                    name: "Lost".into(),
                    body: "Lost".into(),
                    tags: vec![]
                }
            )
            .is_err()
        );
        assert_eq!(
            snapshot(&db.conn, &original.id).unwrap().unwrap().revision,
            fresh.revision
        );
        assert!(db.conn.is_autocommit());
        db.conn
            .execute_batch("DROP TRIGGER reject_prompt_edit")
            .unwrap();
        mutate(&db.conn, &original.id, &fresh.revision, Mutation::Delete)
            .unwrap()
            .unwrap();
        assert!(
            mutate(&db.conn, &original.id, &fresh.revision, Mutation::Delete)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn authoring_refuses_malformed_or_unsupported_metadata_without_replacing_defaults() {
        let (_tmp, db) = open_temp_db();
        let original = PromptTemplate::new("Original".into(), "Kept".into());
        insert(&db.conn, &original).unwrap();
        for raw in [
            "not json",
            r#"[{"key":"area","kind":"future_kind"}]"#,
            r#"{"key":"area"}"#,
            r#"[{"key":"area","kind":"select","options":42}]"#,
            r#"[{"key":"area","default":42}]"#,
        ] {
            db.conn
                .execute(
                    "UPDATE prompt_templates SET placeholders=?2 WHERE id=?1",
                    params![original.id, raw],
                )
                .unwrap();
            assert!(snapshot(&db.conn, &original.id).is_err());
            assert!(mutate(&db.conn, &original.id, "any revision", Mutation::Delete).is_err());
            let retained: String = db
                .conn
                .query_row(
                    "SELECT placeholders FROM prompt_templates WHERE id=?1",
                    [&original.id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(retained, raw);
            assert!(db.conn.is_autocommit());
        }
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
