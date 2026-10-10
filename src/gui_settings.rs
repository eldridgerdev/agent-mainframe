//! Scoped desktop edits of global dormancy settings. Preserve the rest of the
//! JSON document and refuse edits based on an older file.
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::app::AppConfig;
use crate::gui_contract::{GuiError, GuiHandle, GuiResult};

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

fn read(path: &Path) -> GuiResult<(serde_json::Value, DormancySettingsView)> {
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
        return Err(anyhow::anyhow!("Global config must contain a JSON object").into());
    }
    // Validate the whole known schema instead of overwriting a broken config.
    let config: AppConfig = serde_json::from_value(value.clone()).map_err(anyhow::Error::from)?;
    let settings = DormancySettings {
        idle_minutes: config.dormant_idle_minutes,
        unattended_hours: config.dormant_last_accessed_hours,
    };
    validate(&settings)?;
    let revision = bytes.map_or_else(
        || "missing".to_string(),
        |bytes| format!("{:x}", Sha256::digest(bytes)),
    );
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
    let dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Config directory is missing"))?;
    std::fs::create_dir_all(dir).map_err(anyhow::Error::from)?;
    // Serialize desktop writers across processes. Other config writers do not
    // participate; the revision also catches their edits made before this read.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("json.lock"))
        .map_err(anyhow::Error::from)?;
    lock.lock().map_err(anyhow::Error::from)?;
    let (mut value, current) = read(path)?;
    if current.revision != revision {
        return Err(GuiError::conflict(
            "Global config changed. Reload settings before saving.",
        ));
    }
    value["dormant_idle_minutes"] = settings.idle_minutes.into();
    value["dormant_last_accessed_hours"] = settings.unattended_hours.into();
    let mut temp = tempfile::NamedTempFile::new_in(dir).map_err(anyhow::Error::from)?;
    if let Ok(metadata) = std::fs::metadata(path) {
        temp.as_file()
            .set_permissions(metadata.permissions())
            .map_err(anyhow::Error::from)?;
    }
    serde_json::to_writer_pretty(&mut temp, &value).map_err(anyhow::Error::from)?;
    temp.flush().map_err(anyhow::Error::from)?;
    temp.persist(path)
        .map_err(|error| anyhow::Error::new(error.error))?;
    // Return what was written, without a second fallible read after commit.
    let bytes = serde_json::to_vec_pretty(&value).map_err(anyhow::Error::from)?;
    Ok(DormancySettingsView {
        settings,
        revision: format!("{:x}", Sha256::digest(bytes)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
