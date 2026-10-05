//! Read-only desktop prompt library. Source merging, search and substitution
//! are shared with the TUI; delivery returns an unsent frontend draft only.
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};

use crate::app::session_ops::agent_for_session_kind;
use crate::gui_contract::{GuiError, GuiHandle, GuiResult, SessionTarget};
use crate::project::ProjectStatus;
pub use crate::prompt_library::PlaceholderKind;
use crate::prompt_library::{
    placeholder_default, prompt_filter_score, render_template, resolve_placeholders,
};

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LibraryScope {
    #[default]
    Global,
    Project {
        project_id: String,
    },
    Feature {
        project_id: String,
        feature_id: String,
    },
}

#[derive(Debug, Serialize)]
pub struct LibrarySlot {
    pub key: String,
    pub label: String,
    #[serde(flatten)]
    pub kind: PlaceholderKind,
    pub required: bool,
    pub initial_value: String,
}

#[derive(Debug, Serialize)]
pub struct LibraryEntry {
    /// Content and checkout identity, excluding generated config IDs/dates.
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub body: String,
    pub tags: Vec<String>,
    pub source: &'static str,
    pub slots: Vec<LibrarySlot>,
}

#[derive(Debug, Serialize)]
pub struct LibraryTarget {
    pub target: SessionTarget,
    pub label: String,
    pub stopped: bool,
}

#[derive(Debug, Serialize)]
pub struct LibraryView {
    pub entries: Vec<LibraryEntry>,
    pub targets: Vec<LibraryTarget>,
}

#[derive(Debug, Deserialize)]
pub struct ResolvePrompt {
    pub scope: LibraryScope,
    pub entry_key: String,
    pub values: Vec<(String, String)>,
    /// None previews locally. Some validates a destination for draft insertion.
    pub target: Option<SessionTarget>,
}

pub fn load(gui: &mut GuiHandle, scope: &LibraryScope, query: &str) -> GuiResult<LibraryView> {
    gui.refresh_snapshot()?;
    let app = gui.app_for_workflow();
    let (pi, fi) = match scope {
        LibraryScope::Global => (None, None),
        LibraryScope::Project { project_id } => {
            let pi = app
                .store
                .projects
                .iter()
                .position(|p| &p.id == project_id)
                .ok_or_else(|| {
                    GuiError::not_found("Project was deleted; choose another library scope")
                })?;
            (Some(pi), None)
        }
        LibraryScope::Feature {
            project_id,
            feature_id,
        } => {
            let (pi, fi) = app
                .store
                .locate_feature_by_id(Some(project_id), feature_id)
                .ok_or_else(|| {
                    GuiError::not_found("Feature was deleted; choose another library scope")
                })?;
            (Some(pi), Some(fi))
        }
    };
    let repo = pi.map(|pi| &app.store.projects[pi].repo);
    let workdir = pi
        .zip(fi)
        .map(|(pi, fi)| &app.store.projects[pi].features[fi].workdir);
    let mut scored = Vec::new();
    for (index, entry) in app.prompt_library_for_scope(pi, fi).into_iter().enumerate() {
        let template = entry.template;
        let Some(score) =
            prompt_filter_score(&template.name, &template.body, &template.tags, query)
        else {
            continue;
        };
        let slots = resolve_placeholders(&template);
        let mut hash = DefaultHasher::new();
        // Config entries without IDs/timestamps are reconstructed on every read.
        // Hash only authored content; include checkout paths to refuse reassignment.
        serde_json::to_string(&(
            scope,
            repo,
            workdir,
            entry.source.label(),
            index,
            &template.name,
            &template.description,
            &template.body,
            &template.tags,
            &slots,
            (entry.source == crate::prompt_library::PromptSource::User).then_some(&template.id),
        ))
        .map_err(|error| GuiError::from(anyhow::Error::from(error)))?
        .hash(&mut hash);
        scored.push((
            score,
            LibraryEntry {
                key: format!("{:016x}", hash.finish()),
                name: template.name,
                description: template.description,
                body: template.body,
                tags: template.tags,
                source: entry.source.label(),
                slots: slots
                    .into_iter()
                    .map(|slot| LibrarySlot {
                        label: slot.display_label().to_owned(),
                        initial_value: placeholder_default(&slot),
                        key: slot.key,
                        kind: slot.kind,
                        required: slot.required,
                    })
                    .collect(),
            },
        ));
    }
    scored.sort_by_key(|(score, _)| *score);
    let mut targets = Vec::new();
    for project in &app.store.projects {
        let allowed = app.allowed_agents_for_repo(&project.repo);
        for feature in &project.features {
            for session in &feature.sessions {
                if agent_for_session_kind(&session.kind)
                    .is_some_and(|agent| allowed.contains(&agent))
                {
                    targets.push(LibraryTarget {
                        target: SessionTarget {
                            project_id: project.id.clone(),
                            feature_id: feature.id.clone(),
                            session_id: session.id.clone(),
                        },
                        label: format!("{} / {} / {}", project.name, feature.name, session.label),
                        stopped: feature.status == ProjectStatus::Stopped || session.stopped,
                    });
                }
            }
        }
    }
    Ok(LibraryView {
        entries: scored.into_iter().map(|(_, entry)| entry).collect(),
        targets,
    })
}

pub fn resolve(gui: &mut GuiHandle, request: ResolvePrompt) -> GuiResult<String> {
    let view = load(gui, &request.scope, "")?;
    let entry = view.entries.iter().find(|entry| entry.key == request.entry_key)
        .ok_or_else(|| GuiError::conflict("This template or its checkout changed or was deleted. Refresh and select it again."))?;
    if let Some(target) = &request.target {
        if !view.targets.iter().any(|candidate| {
            candidate.target.project_id == target.project_id
                && candidate.target.feature_id == target.feature_id
                && candidate.target.session_id == target.session_id
        }) {
            return Err(GuiError::not_found(
                "That agent session was removed or is no longer allowed. Choose another target.",
            ));
        }
        for slot in &entry.slots {
            let value = request
                .values
                .iter()
                .find(|(key, _)| key == &slot.key)
                .map(|(_, value)| value.as_str())
                .unwrap_or("");
            if slot.required && value.trim().is_empty() {
                return Err(GuiError::conflict(format!("{} is required", slot.label)));
            }
        }
    }
    for slot in &entry.slots {
        if let PlaceholderKind::Select { options } = &slot.kind {
            let value = request
                .values
                .iter()
                .find(|(key, _)| key == &slot.key)
                .map(|(_, value)| value.as_str())
                .unwrap_or("");
            if !(options.iter().any(|option| option == value)
                || options.is_empty() && value.is_empty())
            {
                return Err(GuiError::conflict(format!(
                    "Choose a configured option for {}",
                    slot.label
                )));
            }
        }
    }
    let text = render_template(&entry.body, &request.values);
    if request.target.is_some() && text.trim().is_empty() {
        return Err(GuiError::conflict("The resolved prompt is empty"));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, AppMode};
    use crate::db::AmfDb;
    use crate::gui_contract::GuiErrorKind;
    use crate::project::{AgentKind, Feature, Project, ProjectStore, SessionKind, VibeMode};
    use crate::prompt_library::PromptTemplate;
    use crate::traits::{MockTmuxOps, MockWorktreeOps};

    fn fixture() -> (tempfile::TempDir, GuiHandle, LibraryScope, SessionTarget) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let workdir = dir.path().join("worktree");
        std::fs::create_dir(&repo).unwrap();
        std::fs::create_dir(&workdir).unwrap();
        let mut project = Project::new("Project".into(), repo, true, AgentKind::Claude);
        let mut feature = Feature::new_for_project(
            "Project",
            "Feature".into(),
            "feature".into(),
            workdir,
            true,
            VibeMode::default(),
            false,
            false,
            AgentKind::Claude,
            false,
            false,
        );
        let session_id = feature.add_session(SessionKind::Claude).id.clone();
        for kind in [
            SessionKind::Codex,
            SessionKind::Opencode,
            SessionKind::Pi,
            SessionKind::Terminal,
            SessionKind::Nvim,
        ] {
            feature.add_session(kind);
        }
        let target = SessionTarget {
            project_id: project.id.clone(),
            feature_id: feature.id.clone(),
            session_id,
        };
        let scope = LibraryScope::Feature {
            project_id: project.id.clone(),
            feature_id: feature.id.clone(),
        };
        project.features.push(feature);
        let mut store = ProjectStore::empty();
        store.projects.push(project);
        let db = AmfDb::open(&dir.path().join("amf.db")).unwrap();
        db.save_store(&store).unwrap();
        let (_, version) = db.load_store_versioned().unwrap();
        let mut app = App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.db = Some(db);
        app.store_version = Some(version);
        app.config.extension.prompt_templates =
            vec![PromptTemplate::new("Same".into(), "global".into())];
        (dir, GuiHandle::from_app(app), scope, target)
    }

    fn insert_user(gui: &mut GuiHandle, body: &str) -> PromptTemplate {
        let mut template = PromptTemplate::new("Same".into(), body.into());
        template.tags = vec!["bug".into()];
        gui.app_for_workflow()
            .db
            .as_ref()
            .unwrap()
            .insert_prompt_template(&template)
            .unwrap();
        template
    }

    fn request(
        scope: &LibraryScope,
        entry: &LibraryEntry,
        target: Option<SessionTarget>,
    ) -> ResolvePrompt {
        ResolvePrompt {
            scope: scope.clone(),
            entry_key: entry.key.clone(),
            values: entry
                .slots
                .iter()
                .map(|slot| (slot.key.clone(), slot.initial_value.clone()))
                .collect(),
            target,
        }
    }

    #[test]
    fn merges_all_sources_without_hiding_same_names_and_uses_shared_search() {
        let (dir, mut gui, scope, _) = fixture();
        insert_user(&mut gui, "user");
        std::fs::write(
            dir.path().join("repo/amf.json"),
            r#"{"prompt_templates":[{"name":"Same","body":"project"}]}"#,
        )
        .unwrap();
        std::fs::create_dir(dir.path().join("worktree/.amf")).unwrap();
        std::fs::write(
            dir.path().join("worktree/.amf/config.json"),
            r#"{"prompt_templates":[{"name":"Same","body":"worktree"}]}"#,
        )
        .unwrap();
        let view = load(&mut gui, &scope, "").unwrap();
        assert_eq!(
            view.entries.iter().map(|e| e.source).collect::<Vec<_>>(),
            ["User", "Worktree", "Project", "Global"]
        );
        assert_eq!(load(&mut gui, &scope, "#bug").unwrap().entries.len(), 1);
        assert_eq!(
            load(&mut gui, &scope, "worktree").unwrap().entries[0].source,
            "Worktree"
        );
        assert_eq!(
            load(&mut gui, &LibraryScope::Global, "")
                .unwrap()
                .entries
                .len(),
            2
        );
        let project_id = gui.snapshot().projects[0].id.clone();
        assert_eq!(
            load(&mut gui, &LibraryScope::Project { project_id }, "")
                .unwrap()
                .entries
                .len(),
            3
        );
    }

    #[test]
    fn anonymous_config_identity_is_stable_but_authored_changes_and_removal_are_refused() {
        let (dir, mut gui, scope, target) = fixture();
        let path = dir.path().join("worktree/amf.json");
        std::fs::write(
            &path,
            r#"{"prompt_templates":[{"name":"Config","body":"hello"}]}"#,
        )
        .unwrap();
        let view = load(&mut gui, &scope, "").unwrap();
        let req = request(&scope, &view.entries[0], Some(target.clone()));
        assert_eq!(resolve(&mut gui, req).unwrap(), "hello");
        let req = request(&scope, &view.entries[0], Some(target.clone()));
        std::fs::write(
            &path,
            r#"{"prompt_templates":[{"name":"Config","body":"changed"}]}"#,
        )
        .unwrap();
        assert_eq!(
            resolve(&mut gui, req).unwrap_err().kind,
            GuiErrorKind::Conflict
        );
        let view = load(&mut gui, &scope, "").unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(resolve(&mut gui, request(&scope, &view.entries[0], Some(target))).is_err());
    }

    #[test]
    fn resolves_defaults_explicit_multiline_and_inline_choices_without_changing_mode_or_sending() {
        let (dir, mut gui, scope, target) = fixture();
        std::fs::write(dir.path().join("worktree/amf.json"), r#"{"prompt_templates":[{"name":"Fields","body":"{{ name }} {{name}} {{notes}} {{env: dev|prod}} {{rust|go}}", "placeholders":[{"key":"name","label":"Your name","kind":"text","default":"Ada","required":true},{"key":"notes","kind":"multi_line","default":"one\ntwo"},{"key":"absent","kind":"text","default":"unused"}]}]}"#).unwrap();
        let view = load(&mut gui, &scope, "").unwrap();
        let entry = &view.entries[0];
        assert_eq!(entry.slots.len(), 4);
        assert_eq!(entry.slots[0].label, "Your name");
        assert_eq!(entry.slots[3].label, "Choose an option");
        let version = gui
            .app_for_workflow()
            .db
            .as_ref()
            .unwrap()
            .current_store_version()
            .unwrap();
        assert_eq!(
            resolve(&mut gui, request(&scope, entry, Some(target))).unwrap(),
            "Ada Ada one\ntwo dev rust"
        );
        assert!(matches!(gui.app_for_workflow().mode, AppMode::Normal));
        assert_eq!(
            gui.app_for_workflow()
                .db
                .as_ref()
                .unwrap()
                .current_store_version()
                .unwrap(),
            version
        );
        // Mock tmux/worktree expect no calls: preparing a prompt is read-only.
    }

    #[test]
    fn preview_allows_empty_required_fields_but_insertion_and_invalid_selects_are_refused() {
        let (_, mut gui, scope, target) = fixture();
        let mut template = insert_user(&mut gui, "{{name}} {{dev|prod}}");
        template.placeholders = vec![crate::prompt_library::PromptPlaceholder {
            key: "name".into(),
            label: None,
            kind: PlaceholderKind::default(),
            required: true,
        }];
        gui.app_for_workflow()
            .db
            .as_ref()
            .unwrap()
            .update_prompt_template(&template)
            .unwrap();
        let view = load(&mut gui, &scope, "").unwrap();
        let entry = &view.entries[0];
        assert_eq!(
            resolve(&mut gui, request(&scope, entry, None)).unwrap(),
            " dev"
        );
        assert_eq!(
            resolve(&mut gui, request(&scope, entry, Some(target)))
                .unwrap_err()
                .kind,
            GuiErrorKind::Conflict
        );
        let mut invalid = request(&scope, entry, None);
        invalid.values[1].1 = "arbitrary".into();
        assert!(resolve(&mut gui, invalid).is_err());
    }

    #[test]
    fn reads_external_template_edits_and_refuses_deleted_templates() {
        let (dir, mut gui, scope, target) = fixture();
        let mut template = insert_user(&mut gui, "old");
        let before = load(&mut gui, &scope, "").unwrap();
        let external = AmfDb::open(&dir.path().join("amf.db")).unwrap();
        template.body = "new".into();
        external.update_prompt_template(&template).unwrap();
        assert_eq!(load(&mut gui, &scope, "").unwrap().entries[0].body, "new");
        assert!(
            resolve(
                &mut gui,
                request(&scope, &before.entries[0], Some(target.clone()))
            )
            .is_err()
        );
        let current = load(&mut gui, &scope, "").unwrap();
        external.delete_prompt_template(&template.id).unwrap();
        assert!(resolve(&mut gui, request(&scope, &current.entries[0], Some(target))).is_err());
    }

    #[test]
    fn targets_all_four_allowed_agents_including_stopped_sessions_and_rechecks_restrictions() {
        let (dir, mut gui, scope, target) = fixture();
        insert_user(&mut gui, "hello");
        let view = load(&mut gui, &scope, "").unwrap();
        assert_eq!(view.targets.len(), 4);
        assert!(view.targets.iter().all(|target| target.stopped));
        std::fs::write(
            dir.path().join("repo/amf.json"),
            r#"{"allowed_agents":["codex"]}"#,
        )
        .unwrap();
        assert_eq!(load(&mut gui, &scope, "").unwrap().targets.len(), 1);
        assert_eq!(
            resolve(&mut gui, request(&scope, &view.entries[0], Some(target)))
                .unwrap_err()
                .kind,
            GuiErrorKind::NotFound
        );
    }

    #[test]
    fn external_session_deletion_and_checkout_reassignment_refuse_old_requests() {
        let (dir, mut gui, scope, target) = fixture();
        insert_user(&mut gui, "hello");
        let view = load(&mut gui, &scope, "").unwrap();
        let external = AmfDb::open(&dir.path().join("amf.db")).unwrap();
        let mut store = external.load_store().unwrap();
        store.projects[0].features[0]
            .sessions
            .retain(|session| session.id != target.session_id);
        external.save_store(&store).unwrap();
        assert_eq!(
            resolve(&mut gui, request(&scope, &view.entries[0], Some(target)))
                .unwrap_err()
                .kind,
            GuiErrorKind::NotFound
        );
        store.projects[0].features[0].workdir = dir.path().join("different");
        external.save_store(&store).unwrap();
        assert_eq!(
            resolve(&mut gui, request(&scope, &view.entries[0], None))
                .unwrap_err()
                .kind,
            GuiErrorKind::Conflict
        );
        store.projects[0].features.clear();
        external.save_store(&store).unwrap();
        assert_eq!(
            load(&mut gui, &scope, "").unwrap_err().kind,
            GuiErrorKind::NotFound
        );
    }
}
