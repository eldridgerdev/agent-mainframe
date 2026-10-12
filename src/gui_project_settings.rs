//! Project-scoped desktop agent availability. The TUI and later GUI creation
//! forms resolve the same project extension; existing sessions never change.
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::extension::{self, ExtensionConfig};
use crate::gui_contract::{GuiError, GuiHandle, GuiResult};
use crate::gui_settings::{
    commit_document_preserving, lock_document, read_json_document, validate_agents,
};
use crate::project::AgentKind;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ProjectAgentAvailability {
    /// None removes the project override and inherits the global choices.
    pub allowed_agents: Option<Vec<AgentKind>>,
}

#[derive(Debug, Serialize)]
pub struct ProjectAgentAvailabilityView {
    pub project_id: String,
    pub project_name: String,
    pub repo: String,
    pub source_path: Option<String>,
    pub config_path: String,
    pub inherited_agents: Vec<AgentKind>,
    /// Creation choices include the shared engine's installed-harness filtering.
    pub effective_agents: Vec<AgentKind>,
    pub settings: ProjectAgentAvailability,
    pub revision: String,
}

struct Document {
    view: ProjectAgentAvailabilityView,
    value: serde_json::Value,
    canonical: PathBuf,
    permissions_from: PathBuf,
    repo_identity: String,
    canonical_revision: String,
    legacy_revision: String,
}

fn path_string(path: &Path) -> GuiResult<String> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| GuiError::conflict("This project path is not valid UTF-8"))
}

fn repository_identity(path: &Path) -> GuiResult<String> {
    let metadata = path.metadata().map_err(anyhow::Error::from)?;
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        format!("{}:{}", metadata.dev(), metadata.ino())
    };
    #[cfg(not(unix))]
    let identity = format!("{:?}", metadata.created().ok());
    serde_json::to_string(&(path_string(path)?, identity))
        .map_err(anyhow::Error::from)
        .map_err(Into::into)
}

fn raw_revision(path: &Path) -> GuiResult<String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(format!("{:x}", Sha256::digest(bytes))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok("missing".into()),
        Err(error) => Err(anyhow::Error::new(error).into()),
    }
}

impl Document {
    fn revision(&self) -> GuiResult<String> {
        let bytes = serde_json::to_vec(&(
            &self.view.project_id,
            &self.view.repo,
            &self.repo_identity,
            &self.canonical_revision,
            &self.legacy_revision,
            &self.view.inherited_agents,
        ))
        .map_err(anyhow::Error::from)?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
}

fn read(gui: &mut GuiHandle, project_id: &str) -> GuiResult<Document> {
    gui.refresh_store()?;
    let project = gui
        .app_for_workflow()
        .store
        .projects
        .iter()
        .find(|project| project.id == project_id)
        .ok_or_else(|| GuiError::not_found("Project was deleted. Choose another project."))?;
    let repo = project.repo.clone();
    let project_name = project.name.clone();
    let physical_repo = repo.canonicalize().map_err(anyhow::Error::from)?;
    if !physical_repo.is_dir() {
        return Err(GuiError::conflict(
            "The project repository is no longer a directory",
        ));
    }
    let canonical = extension::project_config_path(&physical_repo);
    let legacy = extension::legacy_project_config_path(&physical_repo);
    let canonical_revision = raw_revision(&canonical)?;
    let legacy_revision = raw_revision(&legacy)?;
    let source = if canonical_revision != "missing" {
        Some(canonical.clone())
    } else if legacy_revision != "missing" {
        Some(legacy)
    } else {
        None
    };
    let permissions_from = source.clone().unwrap_or_else(|| canonical.clone());
    let (value, parsed_revision) = read_json_document(&permissions_from)?;
    let active_revision = if canonical_revision != "missing" {
        &canonical_revision
    } else {
        &legacy_revision
    };
    if &parsed_revision != active_revision {
        return Err(GuiError::conflict(
            "Project config changed while loading. Reload settings.",
        ));
    }
    // Validate every known project field, preserving unknown fields in value.
    // Never replace an invalid effective config with an empty/default document.
    let config: ExtensionConfig =
        serde_json::from_value(value.clone()).map_err(anyhow::Error::from)?;
    let mut document = Document {
        view: ProjectAgentAvailabilityView {
            project_id: project_id.to_string(),
            project_name,
            repo: path_string(&repo)?,
            source_path: source.as_deref().map(path_string).transpose()?,
            config_path: path_string(&canonical)?,
            inherited_agents: extension::load_global_extension_config().allowed_agents(),
            effective_agents: gui.app_for_workflow().allowed_agents_for_repo(&repo),
            settings: ProjectAgentAvailability {
                // The shared engine interprets an explicit empty legacy list as
                // all agents. Show all checked, keeping the override explicit.
                allowed_agents: config
                    .allowed_agents
                    .as_ref()
                    .map(|_| config.allowed_agents()),
            },
            revision: String::new(),
        },
        value,
        canonical,
        permissions_from,
        repo_identity: repository_identity(&physical_repo)?,
        canonical_revision,
        legacy_revision,
    };
    document.view.revision = document.revision()?;
    Ok(document)
}

pub fn load(gui: &mut GuiHandle, project_id: &str) -> GuiResult<ProjectAgentAvailabilityView> {
    Ok(read(gui, project_id)?.view)
}

pub fn save(
    gui: &mut GuiHandle,
    project_id: &str,
    revision: &str,
    settings: ProjectAgentAvailability,
) -> GuiResult<ProjectAgentAvailabilityView> {
    if let Some(agents) = &settings.allowed_agents {
        validate_agents(agents)?;
    }
    let initial = read(gui, project_id)?;
    if initial.view.revision != revision {
        return Err(GuiError::conflict(
            "Project or agent settings changed. Reload settings before saving.",
        ));
    }
    // Canonical lock serializes desktop writers even when reading legacy config.
    let _lock = lock_document(&initial.canonical)?;
    // Refresh stable project identity again after waiting for the writer lock.
    let mut document = read(gui, project_id)?;
    if document.view.revision != revision || document.canonical != initial.canonical {
        return Err(GuiError::conflict(
            "Project or agent settings changed. Reload settings before saving.",
        ));
    }
    match &settings.allowed_agents {
        Some(agents) => document.value["allowed_agents"] = serde_json::json!(agents),
        None => {
            document
                .value
                .as_object_mut()
                .expect("validated JSON object")
                .remove("allowed_agents");
        }
    }
    document.canonical_revision = commit_document_preserving(
        &document.canonical,
        &document.value,
        &document.permissions_from,
    )?;
    // Do not delete or rewrite legacy config as incidental cleanup. Canonical
    // precedence makes this new document effective for both interfaces.
    document.view.source_path = Some(document.view.config_path.clone());
    document.view.settings = settings;
    document.view.effective_agents = gui
        .app_for_workflow()
        .allowed_agents_for_repo(document.canonical.parent().expect("project repository"));
    document.view.revision = document.revision()?;
    // New-feature/session operations read the shared extension fresh by repo.
    // No active session, runtime mode, or unrelated/global setting is mutated.
    Ok(document.view)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::db::AmfDb;
    use crate::extension::set_test_global_extension_config;
    use crate::gui_contract::{FeatureTarget, GuiErrorKind};
    use crate::project::{Feature, Project, ProjectStatus, ProjectStore, SessionKind, VibeMode};
    use crate::traits::{MockTmuxOps, MockWorktreeOps};
    use std::sync::Mutex;

    struct Fixture {
        root: tempfile::TempDir,
        gui: Mutex<GuiHandle>,
        target: FeatureTarget,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            set_test_global_extension_config(None);
        }
    }
    impl Fixture {
        fn repo(&self) -> PathBuf {
            self.root.path().join("repo")
        }
        fn load(&self) -> ProjectAgentAvailabilityView {
            load(&mut self.gui.lock().unwrap(), &self.target.project_id).unwrap()
        }
        fn save(
            &self,
            revision: &str,
            agents: Option<Vec<AgentKind>>,
        ) -> GuiResult<ProjectAgentAvailabilityView> {
            save(
                &mut self.gui.lock().unwrap(),
                &self.target.project_id,
                revision,
                ProjectAgentAvailability {
                    allowed_agents: agents,
                },
            )
        }
        fn options(&self) -> Vec<SessionKind> {
            crate::gui_sessions::new_session_options(&self.gui, &self.target)
                .unwrap()
                .builtin
                .into_iter()
                .map(|option| option.kind)
                .filter(|kind| kind.is_agent_harness())
                .collect()
        }
    }
    fn fixture() -> Fixture {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let mut project = Project::new("Project".into(), repo.clone(), true, AgentKind::Claude);
        let mut feature = Feature::new_for_project(
            "Project",
            "Feature".into(),
            "feature".into(),
            repo,
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
        let db = AmfDb::open(&root.path().join("amf.db")).unwrap();
        db.save_store(&store).unwrap();
        let (_, version) = db.load_store_versioned().unwrap();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists().return_const(true);
        let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
        app.db = Some(db);
        app.store_version = Some(version);
        set_test_global_extension_config(Some(ExtensionConfig {
            allowed_agents: Some(vec![AgentKind::Pi]),
            ..Default::default()
        }));
        Fixture {
            root,
            gui: Mutex::new(GuiHandle::from_app(app)),
            target,
        }
    }

    #[test]
    fn saved_override_and_clear_reach_shared_session_choices_without_changing_existing_sessions() {
        let fx = fixture();
        let path = fx.repo().join("amf.json");
        let original = serde_json::json!({"future":{"nested":[1,2]},"custom_sessions":[],"allowed_agents":["claude"]});
        std::fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
        let before =
            fx.gui.lock().unwrap().app_for_workflow().store.projects[0].features[0].clone();
        assert_eq!(fx.options(), vec![SessionKind::Claude]);
        let view = fx.load();
        assert_eq!(view.inherited_agents, vec![AgentKind::Pi]);
        let saved = fx
            .save(&view.revision, Some(vec![AgentKind::Codex]))
            .unwrap();
        assert_ne!(saved.revision, view.revision);
        assert_eq!(fx.options(), vec![SessionKind::Codex]);
        assert!(
            fx.gui
                .lock()
                .unwrap()
                .add_session(fx.target.clone(), SessionKind::Claude, None, false)
                .is_err()
        );
        let cleared = fx.save(&saved.revision, None).unwrap();
        assert_eq!(cleared.settings.allowed_agents, None);
        assert_eq!(fx.options(), vec![SessionKind::Pi]);
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            raw,
            serde_json::json!({"future":{"nested":[1,2]},"custom_sessions":[]})
        );
        let mut gui = fx.gui.lock().unwrap();
        let app = gui.app_for_workflow();
        assert_eq!(
            serde_json::to_value(&app.store.projects[0].features[0]).unwrap(),
            serde_json::to_value(before).unwrap()
        );
        assert_eq!(
            extension::load_global_extension_config().allowed_agents(),
            vec![AgentKind::Pi]
        );
    }

    #[test]
    fn effective_creation_choices_use_installed_filter_without_losing_configured_choices() {
        let fx = fixture();
        std::fs::write(
            fx.repo().join("amf.json"),
            r#"{"allowed_agents":["codex","pi"]}"#,
        )
        .unwrap();
        let db = AmfDb::open(&fx.root.path().join("amf.db")).unwrap();
        let mut external = db.load_store().unwrap();
        external.available_harnesses = vec![AgentKind::Codex];
        db.save_store(&external).unwrap();
        let view = fx.load();
        assert_eq!(
            view.settings.allowed_agents,
            Some(vec![AgentKind::Codex, AgentKind::Pi])
        );
        assert_eq!(view.effective_agents, vec![AgentKind::Codex]);
        assert_eq!(fx.options(), vec![SessionKind::Codex]);
        let saved = fx.save(&view.revision, Some(vec![AgentKind::Pi])).unwrap();
        assert_eq!(saved.settings.allowed_agents, Some(vec![AgentKind::Pi]));
        assert!(saved.effective_agents.is_empty());
        assert!(fx.options().is_empty());
    }

    #[test]
    fn external_document_or_global_choice_changes_refuse_without_rewriting() {
        let fx = fixture();
        let view = fx.load();
        let path = fx.repo().join("amf.json");
        let external = r#"{"future":"external"}"#;
        std::fs::write(&path, external).unwrap();
        assert_eq!(
            fx.save(&view.revision, Some(vec![AgentKind::Codex]))
                .unwrap_err()
                .kind,
            GuiErrorKind::Conflict
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), external);
        let view = fx.load();
        set_test_global_extension_config(Some(ExtensionConfig {
            allowed_agents: Some(vec![AgentKind::Claude]),
            ..Default::default()
        }));
        assert_eq!(
            fx.save(&view.revision, None).unwrap_err().kind,
            GuiErrorKind::Conflict
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), external);
        assert!(
            !path.with_extension("json.lock").exists(),
            "refuse known stale revision before mutation"
        );
    }

    #[test]
    fn legacy_effective_object_is_copied_to_canonical_without_touching_legacy() {
        let fx = fixture();
        let legacy = extension::legacy_project_config_path(&fx.repo());
        std::fs::create_dir(legacy.parent().unwrap()).unwrap();
        let original = r#"{"allowed_agents":[],"future":{"keep":true},"keybindings":{}}"#;
        std::fs::write(&legacy, original).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&legacy, std::fs::Permissions::from_mode(0o640)).unwrap();
        }
        let view = fx.load();
        assert_eq!(view.settings.allowed_agents, Some(AgentKind::ALL.to_vec()));
        assert_eq!(view.source_path.as_deref(), legacy.to_str());
        let saved = fx
            .save(&view.revision, Some(vec![AgentKind::Codex]))
            .unwrap();
        assert_eq!(
            saved.source_path.as_deref(),
            fx.repo().join("amf.json").to_str()
        );
        assert_eq!(std::fs::read_to_string(&legacy).unwrap(), original);
        let canonical = fx.repo().join("amf.json");
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&canonical).unwrap()).unwrap();
        assert_eq!(
            raw,
            serde_json::json!({"allowed_agents":["codex"],"future":{"keep":true},"keybindings":{}})
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                canonical.metadata().unwrap().permissions().mode() & 0o777,
                0o640
            );
        }
        assert_eq!(fx.options(), vec![SessionKind::Codex]);
    }

    #[test]
    fn canonical_arrival_and_legacy_edits_refuse_stale_legacy_based_saves() {
        let fx = fixture();
        let legacy = extension::legacy_project_config_path(&fx.repo());
        std::fs::create_dir(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, r#"{"allowed_agents":["claude"]}"#).unwrap();
        let view = fx.load();
        std::fs::write(&legacy, r#"{"allowed_agents":["pi"]}"#).unwrap();
        assert_eq!(
            fx.save(&view.revision, None).unwrap_err().kind,
            GuiErrorKind::Conflict
        );
        let view = fx.load();
        let canonical = fx.repo().join("amf.json");
        std::fs::write(&canonical, r#"{"future":"canonical arrival"}"#).unwrap();
        assert_eq!(
            fx.save(&view.revision, None).unwrap_err().kind,
            GuiErrorKind::Conflict
        );
        assert_eq!(
            std::fs::read_to_string(&canonical).unwrap(),
            r#"{"future":"canonical arrival"}"#
        );
        assert_eq!(
            std::fs::read_to_string(&legacy).unwrap(),
            r#"{"allowed_agents":["pi"]}"#
        );
    }

    #[test]
    fn invalid_effective_schema_or_choices_never_replace_a_project_document() {
        let fx = fixture();
        let view = fx.load();
        assert_eq!(view.settings.allowed_agents, None);
        for agents in [vec![], vec![AgentKind::Codex, AgentKind::Codex]] {
            assert!(fx.save(&view.revision, Some(agents)).is_err());
            assert!(!fx.repo().join("amf.json").exists());
        }
        let path = fx.repo().join("amf.json");
        for raw in [
            "[1]",
            "{broken",
            r#"{"allowed_agents":["future"]}"#,
            r#"{"custom_sessions":"invalid"}"#,
        ] {
            std::fs::write(&path, raw).unwrap();
            assert!(load(&mut fx.gui.lock().unwrap(), &fx.target.project_id).is_err());
            assert!(fx.save(&view.revision, None).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
        }
        std::fs::remove_file(&path).unwrap();
        let legacy = extension::legacy_project_config_path(&fx.repo());
        std::fs::create_dir(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, "{broken").unwrap();
        assert!(fx.save(&view.revision, None).is_err());
        assert!(!path.exists());
        // Canonical precedence also means an inactive malformed legacy file is
        // retained without preventing edits to a valid effective document.
        std::fs::write(&path, r#"{"future":"canonical"}"#).unwrap();
        let view = fx.load();
        fx.save(&view.revision, Some(vec![AgentKind::Codex]))
            .unwrap();
        assert_eq!(std::fs::read_to_string(&legacy).unwrap(), "{broken");
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            raw,
            serde_json::json!({"future":"canonical","allowed_agents":["codex"]})
        );
    }

    #[test]
    fn externally_retargeted_or_deleted_project_is_refused_before_any_new_path_write() {
        let fx = fixture();
        let view = fx.load();
        let other_repo = fx.root.path().join("other");
        std::fs::create_dir(&other_repo).unwrap();
        let db = AmfDb::open(&fx.root.path().join("amf.db")).unwrap();
        let mut external = db.load_store().unwrap();
        external.projects[0].repo = other_repo.clone();
        db.save_store(&external).unwrap();
        assert_eq!(
            fx.save(&view.revision, Some(vec![AgentKind::Codex]))
                .unwrap_err()
                .kind,
            GuiErrorKind::Conflict
        );
        assert!(!other_repo.join("amf.json").exists());
        assert!(!other_repo.join("amf.json.lock").exists());
        assert!(!fx.repo().join("amf.json").exists());
        let view = fx.load();
        external.projects.clear();
        db.save_store(&external).unwrap();
        assert_eq!(
            fx.save(&view.revision, None).unwrap_err().kind,
            GuiErrorKind::NotFound
        );
    }

    #[cfg(unix)]
    #[test]
    fn replacement_repository_at_the_same_path_is_a_stale_target() {
        let fx = fixture();
        let view = fx.load();
        std::fs::rename(fx.repo(), fx.root.path().join("original-repo")).unwrap();
        std::fs::create_dir(fx.repo()).unwrap();
        assert_eq!(
            fx.save(&view.revision, Some(vec![AgentKind::Pi]))
                .unwrap_err()
                .kind,
            GuiErrorKind::Conflict
        );
        assert!(!fx.repo().join("amf.json.lock").exists());
    }
}
