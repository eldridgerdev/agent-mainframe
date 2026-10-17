use super::*;
use crate::gui_contract::GuiErrorKind;
use crate::gui_prompts::tests::fixture;
use crate::prompt_library::{PlaceholderKind, PromptPlaceholder};

fn draft(snapshot: &UserPromptEditor) -> UserPromptDraft {
    UserPromptDraft {
        id: Some(snapshot.id.clone()),
        revision: Some(snapshot.revision.clone()),
        name: snapshot.name.clone(),
        body: snapshot.body.clone(),
        tags: snapshot.tags.clone(),
    }
}
fn add(gui: &GuiHandle) -> PromptTemplate {
    let mut template = PromptTemplate::new("Repair".into(), "Fix {{area}}".into());
    template.description = Some("Keep this description".into());
    template.placeholders = vec![
        PromptPlaceholder {
            key: "area".into(),
            label: Some("Subsystem".into()),
            kind: PlaceholderKind::Select {
                options: vec!["api".into(), "db".into()],
            },
            required: true,
        },
        PromptPlaceholder {
            key: "unused".into(),
            label: Some("Retained for later".into()),
            kind: PlaceholderKind::MultiLine {
                default: Some("notes".into()),
            },
            required: false,
        },
    ];
    gui.db().unwrap().insert_prompt_template(&template).unwrap();
    template
}
#[test]
fn user_edit_preserves_metadata_and_siblings_then_rejects_stale_save_and_delete() {
    let (_dir, mut gui, scope, _) = fixture();
    let template = add(&gui);
    let sibling = PromptTemplate::new("Sibling".into(), "Untouched".into());
    gui.db().unwrap().insert_prompt_template(&sibling).unwrap();
    let view = load(&mut gui, &scope, "").unwrap();
    let key = &view
        .entries
        .iter()
        .find(|entry| entry.user_id.as_deref() == Some(&template.id))
        .unwrap()
        .key;
    let snapshot = edit(&mut gui, &scope, key).unwrap();
    let mut changed = draft(&snapshot);
    changed.name = " Edited ".into();
    changed.body = " Revised {{area}} ".into();
    changed.tags = "#bug Frontend, BUG".into();
    save(&mut gui, changed).unwrap();
    let rows = gui.db().unwrap().load_prompt_templates().unwrap();
    let edited = rows.iter().find(|row| row.id == template.id).unwrap();
    assert_eq!(edited.name, "Edited");
    assert_eq!(edited.body, "Revised {{area}}");
    assert_eq!(edited.tags, ["bug", "Frontend"]);
    assert_eq!(edited.description, template.description);
    assert_eq!(edited.placeholders, template.placeholders);
    assert_eq!(edited.created_at, template.created_at);
    assert_eq!(
        rows.iter().find(|row| row.id == sibling.id).unwrap(),
        &sibling
    );
    assert_eq!(
        save(&mut gui, draft(&snapshot)).unwrap_err().kind,
        GuiErrorKind::Conflict
    );
    assert_eq!(
        delete(&mut gui, &snapshot.id, &snapshot.revision)
            .unwrap_err()
            .kind,
        GuiErrorKind::Conflict
    );
    let fresh = reload(&gui, &template.id).unwrap();
    delete(&mut gui, &fresh.id, &fresh.revision).unwrap();
    assert!(reload(&gui, &template.id).is_err());
    assert!(save(&mut gui, draft(&fresh)).is_err());
    assert_eq!(
        gui.db().unwrap().load_prompt_templates().unwrap(),
        [sibling]
    );
}
#[test]
fn create_uses_shared_validation_and_duplicate_user_retains_all_definitions_with_new_identity() {
    let (_dir, mut gui, scope, _) = fixture();
    let template = add(&gui);
    let key = load(&mut gui, &scope, "Repair").unwrap().entries[0]
        .key
        .clone();
    let copy_id = duplicate(&mut gui, &scope, &key).unwrap();
    let copy = gui
        .db()
        .unwrap()
        .load_prompt_templates()
        .unwrap()
        .into_iter()
        .find(|row| row.id == copy_id)
        .unwrap();
    assert_eq!(copy.name, "Repair (copy)");
    assert_ne!(copy.id, template.id);
    assert_eq!(copy.placeholders, template.placeholders);
    assert_eq!(copy.description, template.description);
    assert_eq!(copy.created_at, copy.updated_at);
    assert!(copy.created_at >= template.created_at);
    let count = gui.db().unwrap().load_prompt_templates().unwrap().len();
    for (name, body) in [(" ", "text"), ("Name", " \n")] {
        assert!(
            save(
                &mut gui,
                UserPromptDraft {
                    id: None,
                    revision: None,
                    name: name.into(),
                    body: body.into(),
                    tags: String::new()
                }
            )
            .is_err()
        );
    }
    assert_eq!(
        gui.db().unwrap().load_prompt_templates().unwrap().len(),
        count
    );
    let id = save(
        &mut gui,
        UserPromptDraft {
            id: None,
            revision: None,
            name: " Composer ".into(),
            body: " Keep Unicode: café 世界\n{{task}} ".into(),
            tags: "#a a B".into(),
        },
    )
    .unwrap();
    let row = reload(&gui, &id).unwrap();
    assert_eq!(row.body, "Keep Unicode: café 世界\n{{task}}");
    assert_eq!(row.tags, "a, B");
}
#[test]
fn declarative_duplicate_preserves_unused_definitions_and_never_edits_its_source() {
    let (dir, mut gui, scope, _) = fixture();
    let path = dir.path().join("worktree/amf.json");
    let bytes = r#"{"prompt_templates":[{"name":"Configured","body":"Use {{area}}","description":"Read-only source","tags":["config"],"placeholders":[{"key":"area","kind":"select","options":["api","db"],"required":true},{"key":"unused","kind":"multi_line","default":"Keep me"}]}]}"#;
    std::fs::write(&path, bytes).unwrap();
    let view = load(&mut gui, &scope, "").unwrap();
    let entry = view
        .entries
        .iter()
        .find(|entry| entry.source == "Worktree")
        .unwrap();
    assert!(edit(&mut gui, &scope, &entry.key).is_err());
    let id = duplicate(&mut gui, &scope, &entry.key).unwrap();
    let copied = gui
        .db()
        .unwrap()
        .load_prompt_templates()
        .unwrap()
        .into_iter()
        .find(|row| row.id == id)
        .unwrap();
    assert_eq!(copied.name, "Configured (copy)");
    assert_eq!(copied.description.as_deref(), Some("Read-only source"));
    assert_eq!(copied.placeholders.len(), 2);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), bytes);
    let old_key = entry.key.clone();
    let changed = bytes.replace("Keep me", "Changed unused definition");
    std::fs::write(&path, changed).unwrap();
    assert!(duplicate(&mut gui, &scope, &old_key).is_err());
    std::fs::remove_file(&path).unwrap();
    assert!(duplicate(&mut gui, &scope, &old_key).is_err());
}
#[test]
fn unknown_declarative_metadata_and_deleted_scope_refuse_without_new_user_rows() {
    let (dir, mut gui, scope, _) = fixture();
    let path = dir.path().join("repo/amf.json");
    for raw in [
        r#"{"prompt_templates":[{"name":"Future","body":"Safe","future_metadata":42}]}"#,
        r#"{"prompt_templates":[{"name":"Future","body":"{{unused}}","placeholders":[{"key":"unused","kind":"text","future_format":"secret"}]}]}"#,
    ] {
        std::fs::write(&path, raw).unwrap();
        let view = load(&mut gui, &scope, "").unwrap();
        let entry = view
            .entries
            .iter()
            .find(|entry| entry.source == "Project")
            .unwrap();
        assert_eq!(
            duplicate(&mut gui, &scope, &entry.key).unwrap_err().kind,
            GuiErrorKind::Conflict
        );
        assert!(
            gui.db()
                .unwrap()
                .load_prompt_templates()
                .unwrap()
                .is_empty()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
    }
    let key = load(&mut gui, &scope, "").unwrap().entries[0].key.clone();
    gui.app_for_workflow().store.projects.clear();
    assert!(duplicate(&mut gui, &scope, &key).is_err());
}

#[test]
fn source_reordering_preserves_selection_and_duplicate_never_writes_the_source() {
    let (dir, mut gui, scope, _) = fixture();
    let path = dir.path().join("repo/amf.json");
    let first = serde_json::json!({"name":"First","body":"First body","placeholders":[{"key":"unused","kind":"text","default":"retained"}]});
    let other = serde_json::json!({"name":"Other","body":"Other body"});
    std::fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({"prompt_templates":[first,other]})).unwrap(),
    )
    .unwrap();
    let key = load(&mut gui, &scope, "First").unwrap().entries[0]
        .key
        .clone();
    let reordered =
        serde_json::to_vec(&serde_json::json!({"prompt_templates":[other,first]})).unwrap();
    std::fs::write(&path, &reordered).unwrap();
    let copy_id = duplicate(&mut gui, &scope, &key).unwrap();
    assert_eq!(reload(&gui, &copy_id).unwrap().name, "First (copy)");
    assert_eq!(reload(&gui, &copy_id).unwrap().configured_fields, 1);
    assert_eq!(std::fs::read(&path).unwrap(), reordered);
    let changed = serde_json::to_vec(&serde_json::json!({"prompt_templates":[other,{"name":"First","body":"First body","placeholders":[{"key":"unused","kind":"text","default":"external change"}]}]})).unwrap();
    std::fs::write(&path, &changed).unwrap();
    assert!(duplicate(&mut gui, &scope, &key).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), changed);
    assert_eq!(gui.db().unwrap().load_prompt_templates().unwrap().len(), 1);
}
