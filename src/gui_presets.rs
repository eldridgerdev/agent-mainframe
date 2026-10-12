//! Global preset editing and project-effective creation choices.
use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

pub use crate::extension::FeaturePreset;
use crate::gui_contract::{GuiError, GuiHandle, GuiResult};
use crate::gui_settings::{commit_document, lock_document, read_document};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetRow {
    /// Identity in the revision being edited; survives renaming and reordering.
    pub original_index: Option<usize>,
    pub preset: FeaturePreset,
}

#[derive(Debug, Serialize)]
pub struct PresetsView {
    pub rows: Vec<PresetRow>,
    pub revision: String,
}

fn read(path: &Path) -> GuiResult<(serde_json::Value, PresetsView)> {
    let (value, config, revision) = read_document(path)?;
    let rows = config
        .extension
        .feature_presets
        .into_iter()
        .enumerate()
        .map(|(index, preset)| PresetRow {
            original_index: Some(index),
            preset,
        })
        .collect();
    Ok((value, PresetsView { rows, revision }))
}

pub fn load() -> GuiResult<PresetsView> {
    Ok(read(&crate::project::amf_config_dir().join("config.json"))?.1)
}

pub fn save(gui: &mut GuiHandle, revision: &str, rows: Vec<PresetRow>) -> GuiResult<PresetsView> {
    save_and_apply(
        gui,
        &crate::project::amf_config_dir().join("config.json"),
        revision,
        rows,
    )
}

fn save_and_apply(
    gui: &mut GuiHandle,
    path: &Path,
    revision: &str,
    rows: Vec<PresetRow>,
) -> GuiResult<PresetsView> {
    let view = save_at(path, revision, rows)?;
    gui.app_for_workflow().config.extension.feature_presets =
        view.rows.iter().map(|row| row.preset.clone()).collect();
    Ok(view)
}

fn save_at(path: &Path, revision: &str, mut rows: Vec<PresetRow>) -> GuiResult<PresetsView> {
    let _lock = lock_document(path)?;
    let (mut value, previous) = read(path)?;
    if previous.revision != revision {
        return Err(GuiError::conflict(
            "Global config changed. Reload presets before saving.",
        ));
    }
    let mut names = HashSet::new();
    let mut indices = HashSet::new();
    let mut updated = Vec::new();
    for row in &mut rows {
        row.preset.name = row.preset.name.trim().to_string();
        if row.preset.name.is_empty() || !names.insert(row.preset.name.clone()) {
            return Err(anyhow::anyhow!("Preset names must be nonempty and unique").into());
        }
        let mut raw = match row.original_index {
            Some(index) if index < previous.rows.len() && indices.insert(index) => {
                value["extension"]["feature_presets"][index].clone()
            }
            Some(_) => {
                return Err(GuiError::conflict(
                    "Preset identity changed. Reload presets before saving.",
                ));
            }
            None => serde_json::json!({}),
        };
        let fields = serde_json::to_value(&row.preset).map_err(anyhow::Error::from)?;
        for (key, field) in fields.as_object().expect("preset serializes as object") {
            raw[key] = field.clone();
        }
        updated.push(raw);
    }
    if value.get("extension").is_none() {
        value["extension"] = serde_json::json!({});
    }
    value["extension"]["feature_presets"] = updated.into();
    let revision = commit_document(path, &value)?;
    // Saved indices now identify the committed order, including newly added rows.
    for (index, row) in rows.iter_mut().enumerate() {
        row.original_index = Some(index);
    }
    Ok(PresetsView { rows, revision })
}

/// Project entries override globals by name, exactly as the TUI picker does.
/// Only currently allowed, installed harnesses are offered for creation.
pub fn creation_choices(gui: &mut GuiHandle, project_id: &str) -> GuiResult<Vec<FeaturePreset>> {
    gui.refresh_store()?;
    let app = gui.app_for_workflow();
    let repo = app
        .store
        .projects
        .iter()
        .find(|project| project.id == project_id)
        .ok_or_else(|| GuiError::not_found("Project was deleted; refresh and retry"))?
        .repo
        .clone();
    let agents = app.allowed_agents_for_repo(&repo);
    Ok(app
        .extension_for_repo(&repo)
        .allowed_feature_presets()
        .into_iter()
        .filter(|preset| agents.contains(&preset.agent))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppConfig;

    #[test]
    fn preserves_future_fields_through_rename_reorder_delete_and_add() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.json");
        std::fs::write(&path, r#"{"future":true,"extension":{"future_nested":42,"feature_presets":[{"name":"first","future_row":{"x":1}},{"name":"second","future_row":2},{"name":"deleted","future_row":3}]}}"#).unwrap();
        let mut rows = read(&path).unwrap().1.rows;
        rows.remove(2);
        rows.swap(0, 1);
        rows[1].preset.name = "renamed".into();
        rows[1].preset.branch_prefix = Some("feat".into());
        rows[1].preset.review = true;
        rows[1].preset.plan_mode = true;
        rows[1].preset.quick_plan = true;
        rows[1].preset.enable_chrome = true;
        rows[1].preset.remote_control = true;
        rows.push(PresetRow {
            original_index: None,
            preset: FeaturePreset {
                name: "new".into(),
                ..Default::default()
            },
        });
        let old = read(&path).unwrap().1;
        let saved = save_at(&path, &old.revision, rows).unwrap();
        let (raw, reopened) = read(&path).unwrap();
        assert_eq!(raw["future"], true);
        assert_eq!(raw["extension"]["future_nested"], 42);
        let presets = &raw["extension"]["feature_presets"];
        assert_eq!(presets[0]["future_row"], 2);
        assert_eq!(presets[1]["future_row"]["x"], 1);
        assert_eq!(presets[1]["name"], "renamed");
        assert_eq!(presets[1]["branch_prefix"], "feat");
        for key in [
            "review",
            "plan_mode",
            "quick_plan",
            "enable_chrome",
            "remote_control",
        ] {
            assert_eq!(presets[1][key], true);
        }
        assert_eq!(presets[2]["name"], "new");
        assert!(presets[2].get("future_row").is_none());
        assert_eq!(reopened.revision, saved.revision);
        assert_eq!(saved.rows[2].original_index, Some(2));
        assert!(save_at(&path, &old.revision, old.rows).is_err());
    }

    #[test]
    fn rejects_invalid_names_identity_and_broken_config_without_overwriting() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.json");
        let initial = read(&path).unwrap().1;
        for names in [vec!["  "], vec!["same", "same"]] {
            let rows = names
                .into_iter()
                .map(|name| PresetRow {
                    original_index: None,
                    preset: FeaturePreset {
                        name: name.into(),
                        ..Default::default()
                    },
                })
                .collect();
            assert!(save_at(&path, &initial.revision, rows).is_err());
            assert!(!path.exists());
        }
        for text in [
            "{",
            "[]",
            r#"{"extension":null}"#,
            r#"{"dormant_last_accessed_hours":18446744073709551615}"#,
            r#"{"extension":{"feature_presets":[{"agent":"unknown"}]}}"#,
        ] {
            std::fs::write(&path, text).unwrap();
            assert!(save_at(&path, "missing", vec![]).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        }
        std::fs::write(
            &path,
            r#"{"extension":{"feature_presets":[{"name":"one"}]}}"#,
        )
        .unwrap();
        let view = read(&path).unwrap().1;
        let mut row = view.rows[0].clone();
        row.original_index = Some(1);
        assert!(save_at(&path, &view.revision, vec![row]).is_err());
        let mut second = view.rows[0].clone();
        second.preset.name = "two".into();
        assert!(save_at(&path, &view.revision, vec![view.rows[0].clone(), second]).is_err());
    }

    #[test]
    fn save_and_creation_choices_leave_existing_features_and_other_runtime_config_untouched() {
        use crate::app::App;
        use crate::extension::set_test_global_extension_config;
        use crate::project::{AgentKind, Feature, Project, ProjectStore, VibeMode};
        use crate::traits::{MockTmuxOps, MockWorktreeOps};
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                set_test_global_extension_config(None);
            }
        }
        let _reset = Reset;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.json");
        let mut project = Project::new("demo".into(), root.path().into(), true, AgentKind::Claude);
        project.features.push(Feature::new_for_project(
            "demo",
            "existing".into(),
            "existing".into(),
            root.path().into(),
            false,
            VibeMode::Vibe,
            false,
            false,
            AgentKind::Claude,
            false,
            false,
        ));
        let id = project.id.clone();
        let before = serde_json::to_value(&project.features).unwrap();
        let mut store = ProjectStore::empty();
        store.projects.push(project);
        let mut gui = GuiHandle::from_app(App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        ));
        gui.app_for_workflow().config.max_concurrent_agents = 11;
        let initial = read(&path).unwrap().1;
        let saved = save_and_apply(
            &mut gui,
            &path,
            &initial.revision,
            vec![PresetRow {
                original_index: None,
                preset: FeaturePreset {
                    name: "saved".into(),
                    agent: AgentKind::Codex,
                    ..Default::default()
                },
            }],
        )
        .unwrap();
        assert_eq!(gui.app_for_workflow().config.max_concurrent_agents, 11);
        assert_eq!(
            gui.app_for_workflow().config.extension.feature_presets[0].name,
            "saved"
        );
        assert_eq!(
            serde_json::to_value(&gui.app_for_workflow().store.projects[0].features).unwrap(),
            before
        );
        assert!(save_and_apply(&mut gui, &path, &initial.revision, vec![]).is_err());
        assert_eq!(
            gui.app_for_workflow()
                .config
                .extension
                .feature_presets
                .len(),
            1
        );
        // Use the actual committed global configuration through the per-test seam.
        let config: AppConfig = serde_json::from_value(read(&path).unwrap().0).unwrap();
        set_test_global_extension_config(Some(config.extension));
        assert_eq!(creation_choices(&mut gui, &id).unwrap()[0].name, "saved");
        gui.app_for_workflow().store.available_harnesses = vec![AgentKind::Claude];
        assert!(creation_choices(&mut gui, &id).unwrap().is_empty());
        assert_eq!(saved.rows[0].original_index, Some(0));
    }

    #[test]
    fn project_override_and_availability_define_effective_choices() {
        use crate::extension::{ExtensionConfig, merge_project_extension_config};
        use crate::project::AgentKind;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.json");
        std::fs::write(&path, r#"{"extension":{"feature_presets":[{"name":"shared","agent":"claude"},{"name":"global","agent":"codex"},{"name":"hidden","agent":"pi"}]}}"#).unwrap();
        let view = read(&path).unwrap().1;
        save_at(&path, &view.revision, view.rows).unwrap();
        std::fs::write(root.path().join("amf.json"), r#"{"allowed_agents":["codex"],"feature_presets":[{"name":"shared","agent":"codex","review":true}]}"#).unwrap();
        let raw = read(&path).unwrap().0;
        let base: ExtensionConfig = serde_json::from_value(raw["extension"].clone()).unwrap();
        let effective =
            merge_project_extension_config(&base, root.path()).allowed_feature_presets();
        assert_eq!(
            effective
                .iter()
                .map(|preset| preset.name.as_str())
                .collect::<Vec<_>>(),
            vec!["shared", "global"]
        );
        assert_eq!(effective[0].agent, AgentKind::Codex);
        assert!(effective[0].review);
    }
}
