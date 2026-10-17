use super::*;

fn local(dir: &Path, value: Value) -> Document {
    std::fs::write(dir.join("amf.json"), serde_json::to_vec(&value).unwrap()).unwrap();
    Document::read(Location::Local(dir.to_path_buf())).unwrap()
}
fn source(raw: Value) -> ExportSource {
    ExportSource {
        revision: revision(&raw).unwrap(),
        raw,
        file: None,
    }
}

#[test]
fn edit_is_one_raw_preserving_rename_and_refuses_name_collisions_and_stale_files() {
    let dir = tempfile::tempdir().unwrap();
    let row = json!({"name":"Old","body":"Fix {{area}}","description":"Keep","id":"stable-id","created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-02T00:00:00Z","tags":["bug"],"future":{"format":3},"placeholders":[{"key":"unused","kind":"text","default":"retained","future_width":80}]});
    let sibling = json!({"name":"Sibling","body":"Kept","opaque":[1,2]});
    let document = local(
        dir.path(),
        json!({"unknown_root":{"a":1},"prompt_templates":[row,sibling]}),
    );
    let edit = Edit::read(document.clone(), 0).unwrap();
    assert!(edit.commit("Sibling", "Changed", "tag", || Ok(())).is_err());
    assert_eq!(
        Document::read(document.location.clone()).unwrap().value,
        document.value
    );
    let fresh = edit
        .commit(
            " Renamed ",
            " Revised {{area}} ",
            "#bug BUG invoice",
            || Ok(()),
        )
        .unwrap();
    let mut expected = row.clone();
    expected["name"] = "Renamed".into();
    expected["body"] = "Revised {{area}}".into();
    expected["tags"] = json!(["bug", "invoice"]);
    assert_eq!(fresh.raw, expected);
    assert_eq!(fresh.document.rows().unwrap()[1], sibling);
    assert_eq!(fresh.document.value["unknown_root"], json!({"a":1}));
    assert!(edit.commit("Lost", "Lost", "", || Ok(())).is_err());
    assert!(
        edit.reload().is_err(),
        "renamed rows need explicit reselection"
    );
    assert_eq!(fresh.reload().unwrap().raw, expected);
}

#[test]
fn legacy_migration_preserves_bytes_mode_and_refuses_canonical_arrival() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".amf")).unwrap();
    let legacy = dir.path().join(".amf/config.json");
    let original=br#"{ "opaque": true, "prompt_templates": [{"name":"Legacy","body":"Kept","description":"metadata"}] }"#;
    std::fs::write(&legacy, original).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&legacy, std::fs::Permissions::from_mode(0o640)).unwrap();
    }
    let document = Document::read(Location::Local(dir.path().into())).unwrap();
    let edit = Edit::read(document, 0).unwrap();
    std::fs::write(dir.path().join("amf.json"), "{}").unwrap();
    assert!(edit.commit("New", "Body", "", || Ok(())).is_err());
    assert_eq!(std::fs::read(&legacy).unwrap(), original);
    std::fs::remove_file(dir.path().join("amf.json")).unwrap();
    let edit = Edit::read(
        Document::read(Location::Local(dir.path().into())).unwrap(),
        0,
    )
    .unwrap();
    let fresh = edit.commit("New", "Body", "", || Ok(())).unwrap();
    assert_eq!(fresh.document.active_path, dir.path().join("amf.json"));
    assert_eq!(std::fs::read(&legacy).unwrap(), original);
    assert_eq!(fresh.document.value["opaque"], true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(dir.path().join("amf.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o640
        );
    }
}

#[test]
fn malformed_known_structures_are_refused_without_replacing_any_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("amf.json");
    for raw in [
        "{",
        "null",
        r#"{"prompt_templates":{}}"#,
        r#"{"prompt_templates":[{"name":"Broken","body":"safe","placeholders":[{"key":"x","kind":"future"}]}]}"#,
        r#"{"prompt_templates":[{"name":"Broken","body":"safe","placeholders":[{"key":"x","kind":"select","options":42}]}]}"#,
    ] {
        std::fs::write(&path, raw).unwrap();
        assert!(Document::read(Location::Local(dir.path().into())).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
    }
    let path = dir.path().join("global.json");
    for raw in [
        r#"{"extension":null}"#,
        r#"{"extension":{"prompt_templates":42}}"#,
        r#"{"remote_control_default":"bad"}"#,
    ] {
        std::fs::write(&path, raw).unwrap();
        assert!(Document::read(Location::Global(path.clone())).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
    }
}

#[test]
fn export_requires_replacement_and_keeps_source_metadata_and_destination_opaque_fields() {
    let dir = tempfile::tempdir().unwrap();
    let source = json!({"name":"Review","body":"New {{area}}","description":"Source description","source_extra":{"format":4},"placeholders":[{"key":"unused","kind":"multi_line","default":"Kept","opaque":true}]});
    let sibling = json!({"name":"Sibling","body":"Unchanged","extras":42});
    let document = local(
        dir.path(),
        json!({"outer":true,"prompt_templates":[{"name":"Review","body":"Old","tags":["old"],"destination_extra":[1,2]},sibling]}),
    );
    let plan = Export::prepare(super::tests::source(source.clone()), document.clone()).unwrap();
    assert!(plan.replacing);
    assert!(!plan.no_op);
    assert!(plan.commit(false, || Ok(())).is_err());
    assert_eq!(
        Document::read(document.location.clone()).unwrap().binding,
        document.binding
    );
    let result = plan.commit(true, || Ok(())).unwrap();
    let mut expected = source;
    expected["destination_extra"] = json!([1, 2]);
    assert_eq!(result.rows().unwrap(), [expected, sibling]);
    assert_eq!(result.value["outer"], true);
    assert!(
        Export::prepare(
            super::tests::source(json!({"name":"Sibling","body":"Unchanged","extras":42})),
            result
        )
        .unwrap()
        .no_op
    );
}

#[test]
fn source_and_destination_changes_context_failure_and_self_export_never_write() {
    let dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let document = local(
        dir.path(),
        json!({"prompt_templates":[{"name":"Template","body":"Original"}]}),
    );
    let from = ExportSource::from_file(Edit::read(document.clone(), 0).unwrap()).unwrap();
    let self_plan = Export::prepare(from.clone(), document.clone()).unwrap();
    assert!(self_plan.no_op);
    assert_eq!(
        self_plan.commit(false, || Ok(())).unwrap().binding,
        document.binding
    );
    let destination = local(target.path(), json!({"unknown":true}));
    let plan = Export::prepare(from, destination.clone()).unwrap();
    assert!(
        plan.commit(false, || anyhow::bail!("Project was retargeted"))
            .is_err()
    );
    assert_eq!(
        Document::read(destination.location.clone())
            .unwrap()
            .binding,
        destination.binding
    );
    std::fs::write(
        dir.path().join("amf.json"),
        r#"{"prompt_templates":[{"name":"Template","body":"External"}]}"#,
    )
    .unwrap();
    assert!(plan.commit(false, || Ok(())).is_err());
    let from =
        ExportSource::from_file(Edit::read(Document::read(document.location).unwrap(), 0).unwrap())
            .unwrap();
    let plan = Export::prepare(from, destination.clone()).unwrap();
    std::fs::write(target.path().join("amf.json"), r#"{"external":42}"#).unwrap();
    assert!(plan.commit(false, || Ok(())).is_err());
    assert_eq!(
        std::fs::read_to_string(target.path().join("amf.json")).unwrap(),
        r#"{"external":42}"#
    );
}

#[test]
fn global_edits_patch_only_the_template_and_unknown_nested_settings_survive() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let value = json!({"unknown_global":42,"extension":{"unknown_extension":{"keep":true},"prompt_templates":[{"name":"Global","body":"Old","metadata":42}]}});
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    let edit = Edit::read(Document::read(Location::Global(path)).unwrap(), 0).unwrap();
    let fresh = edit.commit("Global", "New", "tag", || Ok(())).unwrap();
    assert_eq!(fresh.document.value["unknown_global"], 42);
    assert_eq!(
        fresh.document.value["extension"]["unknown_extension"],
        json!({"keep":true})
    );
    assert_eq!(fresh.raw["metadata"], 42);
}

#[test]
fn replaced_checkout_with_missing_config_changes_the_destination_binding() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let original = Document::read(Location::Local(repo.clone())).unwrap();
    std::fs::rename(&repo, dir.path().join("old-repo")).unwrap();
    std::fs::create_dir(&repo).unwrap();
    let plan = Export::prepare(source(json!({"name":"New","body":"Text"})), original).unwrap();
    assert!(plan.commit(false, || Ok(())).is_err());
    assert!(!repo.join("amf.json").exists());
}

#[test]
#[cfg(unix)]
fn alias_exports_deduplicate_locks_and_symlink_edits_keep_the_link() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let value = json!({"prompt_templates":[{"name":"Same","body":"Text","unknown":true}]});
    let document = local(&repo, value);
    let alias = dir.path().join("alias");
    symlink(&repo, &alias).unwrap();
    let src = ExportSource::from_file(
        Edit::read(Document::read(Location::Local(alias)).unwrap(), 0).unwrap(),
    )
    .unwrap();
    let plan = Export::prepare(src, document).unwrap();
    assert!(plan.no_op);
    plan.commit(false, || Ok(())).unwrap();
    let target = repo.join("actual.json");
    std::fs::rename(repo.join("amf.json"), &target).unwrap();
    symlink(&target, repo.join("amf.json")).unwrap();
    let edit = Edit::read(Document::read(Location::Local(repo.clone())).unwrap(), 0).unwrap();
    let result = edit.commit("Same", "Edited", "", || Ok(())).unwrap();
    assert!(repo.join("amf.json").is_symlink());
    assert_eq!(result.raw["body"], "Edited");
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(target).unwrap()).unwrap()["prompt_templates"]
            [0]["unknown"],
        true
    );
}

#[cfg(unix)]
#[test]
fn same_stem_symlink_target_takes_derived_lock_only_once() {
    use std::os::unix::fs::symlink;
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("amf.backup");
    std::fs::write(
        &target,
        serde_json::to_vec(
            &serde_json::json!({"prompt_templates":[{"name":"Original","body":"body"}]}),
        )
        .unwrap(),
    )
    .unwrap();
    symlink("amf.backup", directory.path().join("amf.json")).unwrap();
    let edit = Edit::read(
        Document::read(Location::Local(directory.path().into())).unwrap(),
        0,
    )
    .unwrap();
    edit.commit("Changed", "body", "", || Ok(())).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(target).unwrap()).unwrap();
    assert_eq!(value["prompt_templates"][0]["name"], "Changed");
}
