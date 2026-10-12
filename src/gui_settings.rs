//! Scoped desktop edits of global settings. Preserve the rest of the
//! JSON document and refuse edits based on an older file.
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::app::AppConfig;
use crate::gui_contract::{GuiError, GuiHandle, GuiResult};
use crate::project::AgentKind;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DormancySettings {
    pub idle_minutes: u64,
    pub unattended_hours: u64,
}

#[derive(Debug, Serialize)]
pub struct DormancySettingsView {
    pub settings: DormancySettings,
    pub revision: String,
}

/// Read the original JSON object and raw-byte revision, without choosing a schema.
/// Missing files are empty objects with the distinct `missing` revision.
pub(crate) fn read_json_document(path: &Path) -> GuiResult<(serde_json::Value, String)> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(anyhow::Error::new(error).into()),
    };
    let value: serde_json::Value = match &bytes {
        Some(bytes) => serde_json::from_slice(bytes).map_err(anyhow::Error::from)?,
        None => serde_json::json!({}),
    };
    if !value.is_object() {
        return Err(anyhow::anyhow!("Config must contain a JSON object").into());
    }
    let revision = bytes.map_or_else(
        || "missing".to_string(),
        |bytes| format!("{:x}", Sha256::digest(bytes)),
    );
    Ok((value, revision))
}

pub(crate) fn read_document(path: &Path) -> GuiResult<(serde_json::Value, AppConfig, String)> {
    let (value, revision) = read_json_document(path)?;
    // Validate the whole known schema instead of overwriting a broken config.
    let config: AppConfig = serde_json::from_value(value.clone()).map_err(anyhow::Error::from)?;
    validate(&DormancySettings {
        idle_minutes: config.dormant_idle_minutes,
        unattended_hours: config.dormant_last_accessed_hours,
    })?;
    Ok((value, config, revision))
}

fn read(path: &Path) -> GuiResult<(serde_json::Value, DormancySettingsView)> {
    let (value, config, revision) = read_document(path)?;
    let settings = DormancySettings {
        idle_minutes: config.dormant_idle_minutes,
        unattended_hours: config.dormant_last_accessed_hours,
    };
    Ok((value, DormancySettingsView { settings, revision }))
}

fn validate(settings: &DormancySettings) -> GuiResult<()> {
    if settings.idle_minutes > 9_007_199_254_740_991 || settings.unattended_hours > u64::MAX / 3600
    {
        return Err(anyhow::anyhow!("Dormancy thresholds are too large").into());
    }
    Ok(())
}

fn apply(gui: &mut GuiHandle, settings: &DormancySettings) {
    let config = &mut gui.app_for_workflow().config;
    config.dormant_idle_minutes = settings.idle_minutes;
    config.dormant_last_accessed_hours = settings.unattended_hours;
}

pub fn load() -> GuiResult<DormancySettingsView> {
    Ok(read(&crate::project::amf_config_dir().join("config.json"))?.1)
}

/// Refresh just these settings, leaving other runtime settings untouched.
pub(crate) fn refresh(gui: &mut GuiHandle) -> GuiResult<()> {
    // Test Apps deliberately have no global config or filesystem side effects.
    if !gui.app_for_workflow().store_path.as_os_str().is_empty() {
        refresh_at(gui, &crate::project::amf_config_dir().join("config.json"))?;
    }
    Ok(())
}

fn refresh_at(gui: &mut GuiHandle, path: &Path) -> GuiResult<()> {
    apply(gui, &read(path)?.1.settings);
    Ok(())
}

pub fn save(
    gui: &mut GuiHandle,
    revision: &str,
    settings: DormancySettings,
) -> GuiResult<DormancySettingsView> {
    save_and_apply(
        gui,
        &crate::project::amf_config_dir().join("config.json"),
        revision,
        settings,
    )
}

fn save_and_apply(
    gui: &mut GuiHandle,
    path: &Path,
    revision: &str,
    settings: DormancySettings,
) -> GuiResult<DormancySettingsView> {
    let view = save_at(path, revision, settings)?;
    apply(gui, &view.settings);
    Ok(view)
}

fn save_at(
    path: &Path,
    revision: &str,
    settings: DormancySettings,
) -> GuiResult<DormancySettingsView> {
    validate(&settings)?;
    let revision = update_document(path, revision, |value| {
        value["dormant_idle_minutes"] = settings.idle_minutes.into();
        value["dormant_last_accessed_hours"] = settings.unattended_hours.into();
    })?;
    Ok(DormancySettingsView { settings, revision })
}

fn update_document(
    path: &Path,
    revision: &str,
    update: impl FnOnce(&mut serde_json::Value),
) -> GuiResult<String> {
    let _lock = lock_document(path)?;
    let (mut value, _, current_revision) = read_document(path)?;
    if current_revision != revision {
        return Err(GuiError::conflict(
            "Global config changed. Reload settings before saving.",
        ));
    }
    update(&mut value);
    commit_document(path, &value)
}

/// Hold this guard through the freshness check and atomic commit. Other config
/// writers do not participate; callers must also compare original revisions.
pub(crate) fn lock_document(path: &Path) -> GuiResult<std::fs::File> {
    let dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Config directory is missing"))?;
    std::fs::create_dir_all(dir).map_err(anyhow::Error::from)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("json.lock"))
        .map_err(anyhow::Error::from)?;
    lock.lock().map_err(anyhow::Error::from)?;
    Ok(lock)
}

/// Atomically replace a scoped JSON document, preserving existing permissions.
/// The caller owns the writer lock, schema validation and revision comparison.
pub(crate) fn commit_document(path: &Path, value: &serde_json::Value) -> GuiResult<String> {
    let dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Config directory is missing"))?;
    let mut temp = tempfile::NamedTempFile::new_in(dir).map_err(anyhow::Error::from)?;
    if let Ok(metadata) = std::fs::metadata(path) {
        temp.as_file()
            .set_permissions(metadata.permissions())
            .map_err(anyhow::Error::from)?;
    }
    serde_json::to_writer_pretty(&mut temp, value).map_err(anyhow::Error::from)?;
    temp.flush().map_err(anyhow::Error::from)?;
    temp.persist(path)
        .map_err(|error| anyhow::Error::new(error.error))?;
    let bytes = serde_json::to_vec_pretty(value).map_err(anyhow::Error::from)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

/// Global harness availability, before project overrides and installed-harness filtering.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentAvailability {
    pub allowed_agents: Vec<AgentKind>,
}

#[derive(Debug, Serialize)]
pub struct AgentAvailabilityView {
    pub settings: AgentAvailability,
    pub revision: String,
}

fn agents_read(path: &Path) -> GuiResult<AgentAvailabilityView> {
    let (_, config, revision) = read_document(path)?;
    Ok(AgentAvailabilityView {
        settings: AgentAvailability {
            allowed_agents: config.extension.allowed_agents(),
        },
        revision,
    })
}

pub fn agents_load() -> GuiResult<AgentAvailabilityView> {
    agents_read(&crate::project::amf_config_dir().join("config.json"))
}

pub fn agents_save(
    gui: &mut GuiHandle,
    revision: &str,
    settings: AgentAvailability,
) -> GuiResult<AgentAvailabilityView> {
    let view = agents_save_at(
        &crate::project::amf_config_dir().join("config.json"),
        revision,
        settings,
    )?;
    gui.app_for_workflow().config.extension.allowed_agents =
        Some(view.settings.allowed_agents.clone());
    Ok(view)
}

fn agents_save_at(
    path: &Path,
    revision: &str,
    settings: AgentAvailability,
) -> GuiResult<AgentAvailabilityView> {
    if settings.allowed_agents.is_empty()
        || AgentKind::ALL
            .iter()
            .filter(|agent| settings.allowed_agents.contains(agent))
            .count()
            != settings.allowed_agents.len()
    {
        return Err(anyhow::anyhow!("Select at least one agent, without duplicate choices").into());
    }
    let revision = update_document(path, revision, |value| {
        if value.get("extension").is_none() {
            value["extension"] = serde_json::json!({});
        }
        value["extension"]["allowed_agents"] = serde_json::json!(settings.allowed_agents);
    })?;
    Ok(AgentAvailabilityView { settings, revision })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_settings_preserve_nested_config_and_share_freshness_with_dormancy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"future":true,"extension":{"custom_sessions":[],"future_nested":{"answer":42}},"dormant_idle_minutes":17}"#).unwrap();
        let initial = agents_read(&path).unwrap();
        assert_eq!(initial.settings.allowed_agents, AgentKind::ALL);
        let saved = agents_save_at(
            &path,
            &initial.revision,
            AgentAvailability {
                allowed_agents: vec![AgentKind::Codex, AgentKind::Pi],
            },
        )
        .unwrap();
        let (value, _, _) = read_document(&path).unwrap();
        assert_eq!(value["future"], true);
        assert_eq!(value["extension"]["future_nested"]["answer"], 42);
        assert_eq!(value["dormant_idle_minutes"], 17);
        assert_eq!(agents_read(&path).unwrap().revision, saved.revision);
        assert_eq!(agents_read(&path).unwrap().settings, saved.settings);
        let dormant = read(&path).unwrap().1;
        save_at(
            &path,
            &dormant.revision,
            DormancySettings {
                idle_minutes: 20,
                unattended_hours: 5,
            },
        )
        .unwrap();
        assert!(agents_save_at(&path, &saved.revision, saved.settings).is_err());
        assert_eq!(
            agents_read(&path).unwrap().settings.allowed_agents,
            vec![AgentKind::Codex, AgentKind::Pi]
        );
        let current = agents_read(&path).unwrap();
        agents_save_at(
            &path,
            &current.revision,
            AgentAvailability {
                allowed_agents: vec![AgentKind::Claude],
            },
        )
        .unwrap();
        assert!(save_at(&path, &dormant.revision, dormant.settings).is_err());
        assert_eq!(read(&path).unwrap().1.settings.idle_minutes, 20);
    }

    #[test]
    fn agent_settings_reject_invalid_configs_and_empty_or_duplicate_choices() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let initial = agents_read(&path).unwrap();
        assert!(!path.exists());
        for allowed_agents in [vec![], vec![AgentKind::Codex, AgentKind::Codex]] {
            assert!(
                agents_save_at(
                    &path,
                    &initial.revision,
                    AgentAvailability { allowed_agents }
                )
                .is_err()
            );
            assert!(!path.exists());
        }
        for text in [
            "{",
            "[]",
            r#"{"extension":null}"#,
            r#"{"extension":{"allowed_agents":["unknown"]}}"#,
            r#"{"max_concurrent_agents":"invalid"}"#,
        ] {
            std::fs::write(&path, text).unwrap();
            assert!(agents_read(&path).is_err());
            assert!(
                agents_save_at(
                    &path,
                    "missing",
                    AgentAvailability {
                        allowed_agents: vec![AgentKind::Codex]
                    }
                )
                .is_err()
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        }
        std::fs::write(&path, r#"{"extension":{"allowed_agents":[]}}"#).unwrap();
        assert_eq!(
            agents_read(&path).unwrap().settings.allowed_agents,
            AgentKind::ALL
        );
    }

    #[test]
    fn saved_agent_choices_reach_gui_session_options_and_refuse_disallowed_launches() {
        use crate::app::App;
        use crate::extension::set_test_global_extension_config;
        use crate::gui_contract::FeatureTarget;
        use crate::project::{
            Feature, Project, ProjectStatus, ProjectStore, SessionKind, VibeMode,
        };
        use crate::traits::{MockTmuxOps, MockWorktreeOps};
        use std::sync::Mutex;
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
        let mut feature = Feature::new_for_project(
            "demo",
            "feature".into(),
            "feature".into(),
            root.path().into(),
            false,
            VibeMode::default(),
            false,
            false,
            AgentKind::Claude,
            false,
            false,
        );
        feature.status = ProjectStatus::Idle;
        let target = FeatureTarget {
            project_id: project.id.clone(),
            feature_id: feature.id.clone(),
        };
        project.features.push(feature);
        let mut store = ProjectStore::empty();
        store.projects.push(project);
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists().return_const(true);
        let app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
        let gui = Mutex::new(GuiHandle::from_app(app));
        let before = crate::gui_sessions::new_session_options(&gui, &target).unwrap();
        assert!(
            before
                .builtin
                .iter()
                .any(|option| option.kind == SessionKind::Claude)
        );
        let initial = agents_read(&path).unwrap();
        agents_save_at(
            &path,
            &initial.revision,
            AgentAvailability {
                allowed_agents: vec![AgentKind::Codex, AgentKind::Pi],
            },
        )
        .unwrap();
        // The test seam supplies the committed global file without changing process-wide XDG paths.
        set_test_global_extension_config(Some(read_document(&path).unwrap().1.extension));
        let after = crate::gui_sessions::new_session_options(&gui, &target).unwrap();
        let agents = after
            .builtin
            .iter()
            .filter(|option| {
                matches!(
                    option.kind,
                    SessionKind::Claude
                        | SessionKind::Codex
                        | SessionKind::Opencode
                        | SessionKind::Pi
                )
            })
            .map(|option| option.kind.clone())
            .collect::<Vec<_>>();
        assert_eq!(agents, vec![SessionKind::Codex, SessionKind::Pi]);
        assert!(
            gui.lock()
                .unwrap()
                .add_session(target.clone(), SessionKind::Claude, None, false)
                .is_err()
        );
        std::fs::write(
            root.path().join("amf.json"),
            r#"{"allowed_agents":["claude"]}"#,
        )
        .unwrap();
        let project_override = crate::gui_sessions::new_session_options(&gui, &target).unwrap();
        assert!(
            project_override
                .builtin
                .iter()
                .any(|option| option.kind == SessionKind::Claude)
        );
        assert!(
            !project_override
                .builtin
                .iter()
                .any(|option| option.kind == SessionKind::Codex)
        );
    }

    #[test]
    fn agent_settings_permissions_and_project_precedence_survive_save() {
        use crate::extension::merge_project_extension_config;
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, "{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let initial = agents_read(&path).unwrap();
        agents_save_at(
            &path,
            &initial.revision,
            AgentAvailability {
                allowed_agents: vec![AgentKind::Codex],
            },
        )
        .unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let (_, config, _) = read_document(&path).unwrap();
        assert_eq!(
            merge_project_extension_config(&config.extension, dir.path()).allowed_agents(),
            vec![AgentKind::Codex]
        );
        std::fs::write(dir.path().join("amf.json"), r#"{"allowed_agents":["pi"]}"#).unwrap();
        assert_eq!(
            merge_project_extension_config(&config.extension, dir.path()).allowed_agents(),
            vec![AgentKind::Pi]
        );
    }

    #[test]
    fn preserves_unknown_settings_and_refuses_external_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{"future":{"key":true},"max_concurrent_agents":9}"#,
        )
        .unwrap();
        let initial = read(&path).unwrap().1;
        let saved = save_at(
            &path,
            &initial.revision,
            DormancySettings {
                idle_minutes: 15,
                unattended_hours: 0,
            },
        )
        .unwrap();
        let (value, loaded) = read(&path).unwrap();
        assert_eq!(loaded.revision, saved.revision);
        assert_eq!(loaded.settings, saved.settings);
        assert_eq!(value["future"]["key"], true);
        assert_eq!(value["max_concurrent_agents"], 9);
        assert!(save_at(&path, &initial.revision, initial.settings).is_err());
        std::fs::write(&path, "{}").unwrap();
        assert!(save_at(&path, &saved.revision, saved.settings).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{}");
    }

    #[test]
    fn save_applies_only_after_commit_and_preserves_other_runtime_settings() {
        use crate::app::App;
        use crate::project::ProjectStore;
        use crate::traits::{MockTmuxOps, MockWorktreeOps};
        let app = App::new_for_test(
            ProjectStore::empty(),
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        let mut gui = GuiHandle::from_app(app);
        gui.app_for_workflow().config.max_concurrent_agents = 11;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let initial = read(&path).unwrap().1;
        let settings = DormancySettings {
            idle_minutes: 15,
            unattended_hours: 0,
        };
        save_and_apply(&mut gui, &path, &initial.revision, settings.clone()).unwrap();
        let config = &gui.app_for_workflow().config;
        assert_eq!(config.dormant_idle_minutes, 15);
        assert!(config.dormant_thresholds().is_none());
        assert_eq!(config.max_concurrent_agents, 11);
        assert!(
            save_and_apply(
                &mut gui,
                &path,
                &initial.revision,
                DormancySettings {
                    idle_minutes: 30,
                    unattended_hours: 4
                }
            )
            .is_err()
        );
        assert_eq!(gui.app_for_workflow().config.dormant_idle_minutes, 15);
        // An unreadable destination also leaves the live settings untouched.
        assert!(save_and_apply(&mut gui, dir.path(), "missing", settings).is_err());
        assert_eq!(gui.app_for_workflow().config.dormant_idle_minutes, 15);
        std::fs::write(
            &path,
            r#"{"dormant_idle_minutes":30,"dormant_last_accessed_hours":2}"#,
        )
        .unwrap();
        refresh_at(&mut gui, &path).unwrap();
        let config = &gui.app_for_workflow().config;
        assert_eq!(config.dormant_idle_minutes, 30);
        assert_eq!(config.dormant_last_accessed_hours, 2);
        assert_eq!(config.max_concurrent_agents, 11);
        std::fs::write(&path, "broken").unwrap();
        assert!(refresh_at(&mut gui, &path).is_err());
        assert_eq!(gui.app_for_workflow().config.dormant_idle_minutes, 30);
    }

    #[test]
    fn missing_defaults_invalid_files_and_overflow() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let initial = read(&path).unwrap().1;
        assert_eq!(initial.settings.idle_minutes, 60);
        assert!(!path.exists());
        save_at(&path, &initial.revision, initial.settings).unwrap();
        for text in [
            "{",
            "[]",
            r#"{"dormant_idle_minutes":-1}"#,
            r#"{"dormant_last_accessed_hours":18446744073709551615}"#,
        ] {
            std::fs::write(&path, text).unwrap();
            assert!(read(&path).is_err());
            assert!(
                save_at(
                    &path,
                    "missing",
                    DormancySettings {
                        idle_minutes: 1,
                        unattended_hours: 1
                    }
                )
                .is_err()
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        }
        assert!(
            validate(&DormancySettings {
                idle_minutes: u64::MAX,
                unattended_hours: 1
            })
            .is_err()
        );
    }
}
