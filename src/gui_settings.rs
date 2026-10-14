//! Scoped desktop edits of global settings. Preserve the rest of the
//! JSON document and refuse edits based on an older file.
use std::path::Path;
use std::time::{Duration, SystemTime};

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
    let app = gui.app_for_workflow();
    // These are the file's values, so the saved copy takes them too: a later
    // settings save must not mistake them for its own unsaved change.
    for config in [&mut app.config, &mut app.saved_config] {
        config.dormant_idle_minutes = settings.idle_minutes;
        config.dormant_last_accessed_hours = settings.unattended_hours;
    }
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

/// Hold this guard through the freshness check and atomic commit. Startup
/// migration and TUI settings saves take the same lock for `config.json`, but
/// a hand edit takes none, so callers must also compare original revisions.
/// Gives up after about a second: a stuck holder fails the save with a reason
/// instead of hanging it.
pub(crate) fn lock_document(path: &Path) -> GuiResult<std::fs::File> {
    let dir = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Config directory is missing"))?;
    std::fs::create_dir_all(dir).map_err(anyhow::Error::from)?;
    Ok(crate::app::setup::lock_config(path)?)
}

/// Atomically replace a scoped JSON document, preserving existing permissions.
/// The caller owns the writer lock, schema validation and revision comparison.
pub(crate) fn commit_document(path: &Path, value: &serde_json::Value) -> GuiResult<String> {
    commit_document_preserving(path, value, path)
}

/// Preserve permissions from an effective legacy document when creating the
/// canonical file. The source is read for metadata only and is never modified.
/// A symlinked document is written through, leaving the link in place.
pub(crate) fn commit_document_preserving(
    path: &Path,
    value: &serde_json::Value,
    permissions_from: &Path,
) -> GuiResult<String> {
    crate::app::setup::write_json_document(path, value, permissions_from)?;
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

/// Only this process's copy is updated. An already open TUI keeps the list it
/// started with and writes its whole config back on its next settings save,
/// which is why the panel tells the user to restart it.
pub fn agents_save(
    gui: &mut GuiHandle,
    revision: &str,
    settings: AgentAvailability,
) -> GuiResult<AgentAvailabilityView> {
    agents_save_and_apply(
        gui,
        &crate::project::amf_config_dir().join("config.json"),
        revision,
        settings,
    )
}

fn agents_save_and_apply(
    gui: &mut GuiHandle,
    path: &Path,
    revision: &str,
    settings: AgentAvailability,
) -> GuiResult<AgentAvailabilityView> {
    let view = agents_save_at(path, revision, settings)?;
    let app = gui.app_for_workflow();
    // Written by this save, so not a pending change for a later one.
    for config in [&mut app.config, &mut app.saved_config] {
        config.extension.allowed_agents = Some(view.settings.allowed_agents.clone());
    }
    Ok(view)
}

/// The harnesses a creation form may offer: what creating a feature in this
/// project will accept (global choices, the project's own override, installed
/// harnesses), or the global choices alone when no project exists yet.
pub fn allowed_harnesses(
    gui: &mut GuiHandle,
    project_id: Option<&str>,
) -> GuiResult<Vec<AgentKind>> {
    gui.refresh_store()?;
    let app = gui.app_for_workflow();
    let Some(project_id) = project_id else {
        return Ok(app.globally_allowed_agents());
    };
    let project = app
        .store
        .projects
        .iter()
        .find(|project| project.id == project_id)
        .ok_or_else(|| GuiError::not_found(format!("Project '{project_id}' was not found")))?;
    Ok(app.allowed_agents_for_repo(&project.repo))
}

fn agents_save_at(
    path: &Path,
    revision: &str,
    settings: AgentAvailability,
) -> GuiResult<AgentAvailabilityView> {
    validate_agents(&settings.allowed_agents)?;
    // The order a later load reports, whatever order the request used.
    let settings = AgentAvailability {
        allowed_agents: AgentKind::allowed_list(Some(&settings.allowed_agents)),
    };
    let revision = update_document(path, revision, |value| {
        if value.get("extension").is_none() {
            value["extension"] = serde_json::json!({});
        }
        value["extension"]["allowed_agents"] = serde_json::json!(settings.allowed_agents);
    })?;
    Ok(AgentAvailabilityView { settings, revision })
}

pub(crate) fn validate_agents(agents: &[AgentKind]) -> GuiResult<()> {
    if agents.is_empty()
        || AgentKind::ALL
            .iter()
            .filter(|agent| agents.contains(agent))
            .count()
            != agents.len()
    {
        return Err(anyhow::anyhow!("Select at least one agent, without duplicate choices").into());
    }
    Ok(())
}

/// Strings retain the full u64 window limit across JavaScript IPC.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextSettingsInput {
    pub window_limit: String,
    pub warning_percent: String,
    pub critical_percent: String,
}

#[derive(Debug, Serialize)]
pub struct ContextSettingsView {
    pub settings: ContextSettingsInput,
    pub revision: String,
}

fn context_values(config: &AppConfig) -> ContextSettingsInput {
    ContextSettingsInput {
        window_limit: config
            .context_window_override
            .map(|value| value.to_string())
            .unwrap_or_default(),
        warning_percent: config.context_warning_percent.to_string(),
        critical_percent: config.context_critical_percent.to_string(),
    }
}

fn context_parse(
    input: &ContextSettingsInput,
) -> GuiResult<crate::context_tracking::ContextSettings> {
    crate::context_tracking::parse_context_settings(
        &input.window_limit,
        &input.warning_percent,
        &input.critical_percent,
    )
    .map_err(|error| anyhow::anyhow!(error).into())
}

/// The stored values as they are, valid or not: the dialog has to open on a
/// hand-edited `warning=90, critical=80` to be able to repair it. Save and
/// refresh validate through [`context_parse`].
fn context_read(path: &Path) -> GuiResult<ContextSettingsView> {
    let (_, config, revision) = read_document(path)?;
    Ok(ContextSettingsView {
        settings: context_values(&config),
        revision,
    })
}

pub fn context_load() -> GuiResult<ContextSettingsView> {
    context_read(&crate::project::amf_config_dir().join("config.json"))
}

fn context_apply(gui: &mut GuiHandle, settings: crate::context_tracking::ContextSettings) {
    let app = gui.app_for_workflow();
    // Every caller passes what the file holds, so the saved copy follows.
    app.saved_config.context_window_override = settings.window_override;
    app.saved_config.context_warning_percent = settings.warning_percent;
    app.saved_config.context_critical_percent = settings.critical_percent;
    app.apply_context_settings(settings);
}

/// What `stat` reported for the global config when context settings were last
/// read from it. The sidebar polls several times a second; this lets a poll
/// skip a file that has not changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConfigStamp {
    Missing,
    File { modified: SystemTime, len: u64 },
}

/// A stamp is trusted only once the file has been quiet this long. On a
/// filesystem with coarse timestamps, a same-length write in the tick of the
/// previous read would otherwise leave the stamp unchanged and go unseen.
const CONFIG_STAMP_SETTLE: Duration = Duration::from_secs(2);

/// `None` when the file cannot vouch for itself yet (just written, or not
/// stat-able), which makes the caller read it.
fn config_stamp(path: &Path) -> Option<ConfigStamp> {
    match std::fs::metadata(path) {
        Ok(metadata) => {
            let modified = metadata.modified().ok()?;
            modified
                .elapsed()
                .is_ok_and(|age| age >= CONFIG_STAMP_SETTLE)
                .then_some(ConfigStamp::File {
                    modified,
                    len: metadata.len(),
                })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(ConfigStamp::Missing),
        Err(_) => None,
    }
}

/// Refresh context policy before the GUI polls its own context collectors.
///
/// `seen` is the caller's record of the file as last read. Returns `false`
/// when the file is unchanged since then and was not read, so the outcome of
/// that earlier read (success or failure) still stands.
pub(crate) fn context_refresh(
    gui: &mut GuiHandle,
    seen: &mut Option<ConfigStamp>,
) -> GuiResult<bool> {
    // Test Apps deliberately have no global config or filesystem side effects.
    if gui.app_for_workflow().store_path.as_os_str().is_empty() {
        return Ok(false);
    }
    context_refresh_at(
        gui,
        &crate::project::amf_config_dir().join("config.json"),
        seen,
    )
}

fn context_refresh_at(
    gui: &mut GuiHandle,
    path: &Path,
    seen: &mut Option<ConfigStamp>,
) -> GuiResult<bool> {
    // Stamp before reading, so a write landing in between changes the next one.
    let stamp = config_stamp(path);
    if stamp.is_some() && stamp == *seen {
        return Ok(false);
    }
    *seen = stamp;
    let settings = context_parse(&context_read(path)?.settings)?;
    let config = &gui.app_for_workflow().config;
    let current = crate::context_tracking::ContextSettings {
        window_override: config.context_window_override,
        warning_percent: config.context_warning_percent,
        critical_percent: config.context_critical_percent,
    };
    // This desktop's own save lands here too, already applied.
    if settings != current {
        context_apply(gui, settings);
    }
    Ok(true)
}

pub fn context_save(
    gui: &mut GuiHandle,
    revision: &str,
    settings: ContextSettingsInput,
) -> GuiResult<ContextSettingsView> {
    context_save_at(
        gui,
        &crate::project::amf_config_dir().join("config.json"),
        revision,
        settings,
    )
}

fn context_save_at(
    gui: &mut GuiHandle,
    path: &Path,
    revision: &str,
    settings: ContextSettingsInput,
) -> GuiResult<ContextSettingsView> {
    let parsed = context_parse(&settings)?;
    let revision = update_document(path, revision, |value| {
        if let Some(limit) = parsed.window_override {
            value["context_window_override"] = limit.into();
        } else {
            value
                .as_object_mut()
                .expect("validated object")
                .remove("context_window_override");
        }
        value["context_warning_percent"] = parsed.warning_percent.into();
        value["context_critical_percent"] = parsed.critical_percent.into();
    })?;
    context_apply(gui, parsed);
    Ok(ContextSettingsView {
        settings: context_values(&gui.app_for_workflow().config),
        revision,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context_gui() -> GuiHandle {
        use crate::{
            app::App,
            project::ProjectStore,
            traits::{MockTmuxOps, MockWorktreeOps},
        };
        GuiHandle::from_app(App::new_for_test(
            ProjectStore::empty(),
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        ))
    }

    fn context_input(window: &str, warning: &str, critical: &str) -> ContextSettingsInput {
        ContextSettingsInput {
            window_limit: window.into(),
            warning_percent: warning.into(),
            critical_percent: critical.into(),
        }
    }

    #[test]
    fn context_settings_save_u64_exactly_clear_override_and_preserve_other_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"future":{"keep":true},"extension":{"allowed_agents":["pi"]},"dormant_idle_minutes":17}"#).unwrap();
        let initial = context_read(&path).unwrap();
        assert_eq!(initial.settings, context_input("", "70", "85"));
        let mut gui = context_gui();
        gui.app_for_workflow().config.max_concurrent_agents = 11;
        let saved = context_save_at(
            &mut gui,
            &path,
            &initial.revision,
            context_input("18446744073709551615", "1", "100"),
        )
        .unwrap();
        assert_eq!(context_read(&path).unwrap().settings, saved.settings);
        assert_eq!(
            gui.app_for_workflow().config.context_window_override,
            Some(u64::MAX)
        );
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["context_window_override"].as_u64(), Some(u64::MAX));
        assert_eq!(value["future"]["keep"], true);
        assert_eq!(value["extension"]["allowed_agents"][0], "pi");
        assert_eq!(value["dormant_idle_minutes"], 17);
        assert_eq!(gui.app_for_workflow().config.max_concurrent_agents, 11);
        let cleared = context_save_at(
            &mut gui,
            &path,
            &saved.revision,
            context_input("", "60", "90"),
        )
        .unwrap();
        assert_eq!(cleared.settings.window_limit, "");
        assert_eq!(gui.app_for_workflow().config.context_window_override, None);
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(value.get("context_window_override").is_none());
        assert!(
            context_save_at(
                &mut gui,
                &path,
                &saved.revision,
                context_input("200000", "50", "80")
            )
            .is_err()
        );
        assert_eq!(gui.app_for_workflow().config.context_window_override, None);
        assert_eq!(context_read(&path).unwrap().revision, cleared.revision);
    }

    #[test]
    fn context_settings_reject_invalid_input_or_files_without_changing_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut gui = context_gui();
        let initial = context_read(&path).unwrap();
        for input in [
            context_input("0", "70", "85"),
            context_input("18446744073709551616", "70", "85"),
            context_input("", "0", "85"),
            context_input("", "70", "101"),
            context_input("", "85", "85"),
            context_input("", "90", "80"),
        ] {
            assert!(context_save_at(&mut gui, &path, &initial.revision, input).is_err());
            assert!(!path.exists());
            assert_eq!(gui.app_for_workflow().config.context_warning_percent, 70);
        }
        for text in [
            "{",
            "[]",
            r#"{"context_warning_percent":"bad"}"#,
            r#"{"extension":null}"#,
        ] {
            std::fs::write(&path, text).unwrap();
            assert!(context_read(&path).is_err());
            assert!(
                context_save_at(
                    &mut gui,
                    &path,
                    &initial.revision,
                    context_input("", "60", "90")
                )
                .is_err()
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
            assert_eq!(gui.app_for_workflow().config.context_warning_percent, 70);
        }
    }

    #[test]
    fn invalid_stored_context_values_load_for_repair_but_never_reach_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut gui = context_gui();
        for (text, stored) in [
            (
                r#"{"context_window_override":0,"future":1}"#,
                context_input("0", "70", "85"),
            ),
            (
                r#"{"context_warning_percent":90,"context_critical_percent":80,"future":1}"#,
                context_input("", "90", "80"),
            ),
        ] {
            std::fs::write(&path, text).unwrap();
            let loaded = context_read(&path).unwrap();
            assert_eq!(loaded.settings, stored);
            assert!(context_refresh_at(&mut gui, &path, &mut None).is_err());
            assert_eq!(gui.app_for_workflow().config.context_warning_percent, 70);
            assert_eq!(gui.app_for_workflow().config.context_window_override, None);
            // Saving the stored values back unchanged is still refused.
            assert!(context_save_at(&mut gui, &path, &loaded.revision, stored).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
            // The revision from that load is what repairs the file.
            context_save_at(
                &mut gui,
                &path,
                &loaded.revision,
                context_input("", "60", "90"),
            )
            .unwrap();
            assert_eq!(
                context_read(&path).unwrap().settings,
                context_input("", "60", "90")
            );
            assert!(context_refresh_at(&mut gui, &path, &mut None).unwrap());
            let value: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(value["future"], 1);
            context_apply(
                &mut gui,
                context_parse(&context_input("", "70", "85")).unwrap(),
            );
        }
    }

    /// Backdate `path` so its stamp counts as settled.
    fn settle(path: &Path, seconds_ago: u64) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(seconds_ago))
            .unwrap();
    }

    #[test]
    fn context_refresh_reads_the_config_only_when_the_file_changed() {
        use crate::context_tracking::{
            ContextBand, ContextProvenance, ContextResetMetadata, ContextThresholds,
            ContextUsageSample, SessionContextState,
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut gui = context_gui();
        let mut seen = None;

        // A missing file is defaults, read once.
        assert!(context_refresh_at(&mut gui, &path, &mut seen).unwrap());
        assert_eq!(seen, Some(ConfigStamp::Missing));
        assert!(!context_refresh_at(&mut gui, &path, &mut seen).unwrap());

        // A file written moments ago is read every time until it settles.
        std::fs::write(&path, r#"{"context_warning_percent":60}"#).unwrap();
        assert!(context_refresh_at(&mut gui, &path, &mut seen).unwrap());
        assert_eq!(seen, None);
        assert!(context_refresh_at(&mut gui, &path, &mut seen).unwrap());
        assert_eq!(gui.app_for_workflow().config.context_warning_percent, 60);
        settle(&path, 60);
        assert!(context_refresh_at(&mut gui, &path, &mut seen).unwrap());
        assert!(!context_refresh_at(&mut gui, &path, &mut seen).unwrap());

        // An unchanged file is not read: nothing notices the runtime moving.
        gui.app_for_workflow().config.context_warning_percent = 65;
        assert!(!context_refresh_at(&mut gui, &path, &mut seen).unwrap());
        assert_eq!(gui.app_for_workflow().config.context_warning_percent, 65);

        // Same length, different time: read and applied.
        std::fs::write(&path, r#"{"context_warning_percent":50}"#).unwrap();
        settle(&path, 30);
        assert!(context_refresh_at(&mut gui, &path, &mut seen).unwrap());
        assert_eq!(gui.app_for_workflow().config.context_warning_percent, 50);

        // A read that finds the settings already in force leaves snapshots alone.
        let mut state = SessionContextState::default();
        state
            .accept_sample(
                ContextUsageSample {
                    used_tokens: 10,
                    context_limit: Some(1000),
                    provenance: ContextProvenance::Estimated,
                    sampled_at: chrono::Utc::now(),
                    checked_at: chrono::Utc::now(),
                    reset: ContextResetMetadata::default(),
                },
                ContextThresholds::default(),
            )
            .unwrap();
        state.snapshot.as_mut().unwrap().band = ContextBand::Critical;
        gui.app_for_workflow()
            .context_states
            .insert("session".into(), state);
        settle(&path, 20);
        assert!(context_refresh_at(&mut gui, &path, &mut seen).unwrap());
        assert_eq!(
            gui.app_for_workflow().context_states["session"]
                .snapshot
                .as_ref()
                .unwrap()
                .band,
            ContextBand::Critical
        );

        // A broken file fails once, then is not re-read until it changes.
        std::fs::write(&path, "broken").unwrap();
        settle(&path, 10);
        assert!(context_refresh_at(&mut gui, &path, &mut seen).is_err());
        assert!(!context_refresh_at(&mut gui, &path, &mut seen).unwrap());
        assert_eq!(gui.app_for_workflow().config.context_warning_percent, 50);
    }

    #[test]
    fn context_settings_apply_bands_mark_changed_limits_stale_and_refresh_external_edits() {
        use crate::context_tracking::{
            ContextBand, ContextFreshness, ContextProvenance, ContextResetMetadata,
            ContextThresholds, ContextUsageSample, SessionContextState,
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut gui = context_gui();
        for (id, provenance) in [
            ("session", ContextProvenance::Estimated),
            ("direct", ContextProvenance::Direct),
        ] {
            let mut state = SessionContextState::default();
            state
                .accept_sample(
                    ContextUsageSample {
                        used_tokens: 800,
                        context_limit: Some(1000),
                        provenance,
                        sampled_at: chrono::Utc::now(),
                        checked_at: chrono::Utc::now(),
                        reset: ContextResetMetadata::default(),
                    },
                    ContextThresholds::default(),
                )
                .unwrap();
            gui.app_for_workflow()
                .context_states
                .insert(id.into(), state);
        }
        let initial = context_read(&path).unwrap();
        let saved = context_save_at(
            &mut gui,
            &path,
            &initial.revision,
            context_input("", "50", "75"),
        )
        .unwrap();
        let snapshot = gui.app_for_workflow().context_states["session"]
            .snapshot
            .as_ref()
            .unwrap();
        assert_eq!(snapshot.band, ContextBand::Critical);
        assert_eq!(snapshot.freshness, ContextFreshness::Fresh);
        context_save_at(
            &mut gui,
            &path,
            &saved.revision,
            context_input("2000", "70", "90"),
        )
        .unwrap();
        let snapshot = gui.app_for_workflow().context_states["session"]
            .snapshot
            .as_ref()
            .unwrap();
        assert_eq!(snapshot.band, ContextBand::Warning);
        assert_eq!(snapshot.freshness, ContextFreshness::Stale);
        assert_eq!(snapshot.context_limit.get(), 1000);
        // Telemetry with its own limit is rebanded but stays fresh.
        let direct = gui.app_for_workflow().context_states["direct"]
            .snapshot
            .as_ref()
            .unwrap();
        assert_eq!(direct.band, ContextBand::Warning);
        assert_eq!(direct.freshness, ContextFreshness::Fresh);
        std::fs::write(&path, r#"{"context_window_override":3000,"context_warning_percent":85,"context_critical_percent":95}"#).unwrap();
        context_refresh_at(&mut gui, &path, &mut None).unwrap();
        assert_eq!(
            gui.app_for_workflow().config.context_window_override,
            Some(3000)
        );
        assert_eq!(
            gui.app_for_workflow().context_states["session"]
                .snapshot
                .as_ref()
                .unwrap()
                .band,
            ContextBand::Normal
        );
        std::fs::write(&path, "broken").unwrap();
        assert!(context_refresh_at(&mut gui, &path, &mut None).is_err());
        assert_eq!(
            gui.app_for_workflow().config.context_window_override,
            Some(3000)
        );
    }

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
        // Test threads are reused: start from a known global config rather
        // than whatever an earlier test left in the seam.
        set_test_global_extension_config(Some(crate::extension::ExtensionConfig::default()));
        let before = crate::gui_sessions::new_session_options(&gui, &target).unwrap();
        assert!(
            before
                .builtin
                .iter()
                .any(|option| option.kind == SessionKind::Claude)
        );
        assert_eq!(
            allowed_harnesses(&mut gui.lock().unwrap(), None).unwrap(),
            AgentKind::ALL
        );
        let initial = agents_read(&path).unwrap();
        agents_save_and_apply(
            &mut gui.lock().unwrap(),
            &path,
            &initial.revision,
            AgentAvailability {
                allowed_agents: vec![AgentKind::Codex, AgentKind::Pi],
            },
        )
        .unwrap();
        // The test seam supplies the committed global file without changing process-wide XDG paths.
        set_test_global_extension_config(Some(read_document(&path).unwrap().1.extension));
        // Creation forms are offered exactly what creation accepts.
        for project_id in [None, Some(target.project_id.as_str())] {
            assert_eq!(
                allowed_harnesses(&mut gui.lock().unwrap(), project_id).unwrap(),
                vec![AgentKind::Codex, AgentKind::Pi]
            );
        }
        assert!(allowed_harnesses(&mut gui.lock().unwrap(), Some("missing")).is_err());
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
        // The project's override reaches its own forms, not a new project's.
        assert_eq!(
            allowed_harnesses(&mut gui.lock().unwrap(), Some(&target.project_id)).unwrap(),
            vec![AgentKind::Claude]
        );
        assert_eq!(
            allowed_harnesses(&mut gui.lock().unwrap(), None).unwrap(),
            vec![AgentKind::Codex, AgentKind::Pi]
        );
    }

    #[test]
    fn agent_save_applies_only_after_commit_and_reports_the_reloaded_order() {
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
        let initial = agents_read(&path).unwrap();
        // The desktop form's order, which is not `AgentKind::ALL`'s.
        let requested = AgentAvailability {
            allowed_agents: vec![AgentKind::Codex, AgentKind::Opencode, AgentKind::Pi],
        };
        let saved =
            agents_save_and_apply(&mut gui, &path, &initial.revision, requested.clone()).unwrap();
        let reloaded = vec![AgentKind::Opencode, AgentKind::Codex, AgentKind::Pi];
        assert_eq!(saved.settings.allowed_agents, reloaded);
        assert_eq!(agents_read(&path).unwrap().settings, saved.settings);
        assert_eq!(agents_read(&path).unwrap().revision, saved.revision);
        let config = &gui.app_for_workflow().config;
        assert_eq!(config.extension.allowed_agents, Some(reloaded.clone()));
        assert_eq!(config.max_concurrent_agents, 11);
        // A stale revision, an invalid choice and an unreadable destination
        // all leave the live choices untouched.
        let claude = AgentAvailability {
            allowed_agents: vec![AgentKind::Claude],
        };
        assert!(agents_save_and_apply(&mut gui, &path, &initial.revision, claude.clone()).is_err());
        let empty = AgentAvailability {
            allowed_agents: vec![],
        };
        assert!(agents_save_and_apply(&mut gui, &path, &saved.revision, empty).is_err());
        let unreadable = dir.path().join("a-directory.json");
        std::fs::create_dir(&unreadable).unwrap();
        assert!(agents_save_and_apply(&mut gui, &unreadable, "missing", claude).is_err());
        assert_eq!(
            gui.app_for_workflow().config.extension.allowed_agents,
            Some(reloaded.clone())
        );
        assert_eq!(
            agents_read(&path).unwrap().settings.allowed_agents,
            reloaded
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

    #[cfg(unix)]
    #[test]
    fn a_symlinked_config_is_written_through_and_stays_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let dotfiles = dir.path().join("dotfiles");
        let home = dir.path().join("amf");
        std::fs::create_dir_all(&dotfiles).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let target = dotfiles.join("amf-config.json");
        let path = home.join("config.json");
        std::fs::write(&target, r#"{"future":true}"#).unwrap();
        std::os::unix::fs::symlink("../dotfiles/amf-config.json", &path).unwrap();
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
        assert!(std::fs::symlink_metadata(&path).unwrap().is_symlink());
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
        assert_eq!(written["dormant_idle_minutes"], 15);
        assert_eq!(written["future"], true);
        // The revision describes the file a reload will find behind the link.
        assert_eq!(read(&path).unwrap().1.revision, saved.revision);
    }

    #[test]
    fn a_held_lock_fails_the_save_instead_of_hanging_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"future":true}"#).unwrap();
        let initial = read(&path).unwrap().1;
        let holder = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path.with_extension("json.lock"))
            .unwrap();
        holder.lock().unwrap();
        let settings = DormancySettings {
            idle_minutes: 15,
            unattended_hours: 0,
        };
        let started = std::time::Instant::now();
        let error = save_at(&path, &initial.revision, settings.clone()).unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(error.message.contains("config.json.lock"), "{error:?}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            r#"{"future":true}"#
        );
        drop(holder);
        save_at(&path, &initial.revision, settings).unwrap();
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
