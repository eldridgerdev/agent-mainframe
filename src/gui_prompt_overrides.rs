//! Desktop manager for editable headless-prompt overrides (the TUI's
//! `AppMode::PromptOverrides`, dashboard `E`).
//!
//! Registry metadata, layered resolution and both persistence stores are the
//! shared ones: `crate::prompts` (feature → project → global → built-in),
//! `amf.db`'s `prompt_overrides` table for feature/global scope and the repo's
//! `amf.json` `prompt_overrides` key for project scope. Nothing here touches
//! `App::mode`, so the manager can sit on top of an open plan interview or
//! Final Review and its pre-call notice.
//!
//! Every prompt row carries a `revision` fingerprint of the overrides stored
//! for it in the chosen context (and of that context's checkout paths). A save
//! or clear must present the revision it was based on; a write made by the TUI,
//! another window or a hand edit of `amf.json` in the meantime is refused and
//! the client must reload before trying again.
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::app::{App, AppMode};
use crate::db::prompt_overrides::{OverrideScope, PromptOverrides};
use crate::gui_contract::{GuiError, GuiHandle, GuiResult};
use crate::project::AgentKind;
use crate::prompts::project::ProjectPromptOverrides;
use crate::prompts::{PromptId, PromptLayers, PromptSource, resolve_template_layered};

/// Which checkout the manager edits. A feature context offers all three
/// scopes, a project context project + global, and the global context only
/// the global scope.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OverrideContext {
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

/// A layer of the resolution order. `BuiltIn` only ever appears as a row's
/// source; it is never a save or clear target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OverrideLayer {
    Feature,
    Project,
    Global,
    BuiltIn,
}

impl From<PromptSource> for OverrideLayer {
    fn from(source: PromptSource) -> Self {
        match source {
            PromptSource::Feature => Self::Feature,
            PromptSource::Project => Self::Project,
            PromptSource::Global => Self::Global,
            PromptSource::BuiltIn => Self::BuiltIn,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ScopeOption {
    pub scope: OverrideLayer,
    pub label: &'static str,
    pub available: bool,
    /// Why the scope cannot be used here, when it can't.
    pub reason: Option<String>,
}

/// One stored override in the current context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoredOverride {
    pub scope: OverrideLayer,
    /// `None` is the shared template that applies to every harness.
    pub harness: Option<AgentKind>,
    pub template: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct OverrideRow {
    pub id: &'static str,
    pub title: &'static str,
    pub summary: &'static str,
    pub placeholders: Vec<&'static str>,
    /// The layer that supplies the effective template for the view harness.
    pub source: OverrideLayer,
    /// `Some(harness)` when that layer's per-harness template won over its
    /// shared one; `None` for a shared override or the built-in default.
    pub source_harness: Option<AgentKind>,
    pub effective_template: String,
    pub default_template: &'static str,
    pub stored: Vec<StoredOverride>,
    pub revision: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct OverridesView {
    pub context: OverrideContext,
    pub context_label: String,
    pub repo: Option<String>,
    pub workdir: Option<String>,
    /// The harness whose effective templates the rows show.
    pub harness: AgentKind,
    pub scopes: Vec<ScopeOption>,
    /// Set when the repo config cannot be parsed: its project overrides are
    /// ignored by every headless call, and project saves are refused.
    pub project_config_error: Option<String>,
    pub rows: Vec<OverrideRow>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SaveOverride {
    pub context: OverrideContext,
    pub prompt_id: String,
    pub scope: OverrideLayer,
    pub harness: Option<AgentKind>,
    pub template: String,
    pub revision: String,
    /// The harness the returned view resolves for.
    pub view_harness: Option<AgentKind>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClearOverride {
    pub context: OverrideContext,
    pub prompt_id: String,
    pub scope: OverrideLayer,
    pub harness: Option<AgentKind>,
    pub revision: String,
    pub view_harness: Option<AgentKind>,
}

/// What the "Edit prompt" link of a pending pre-call notice opens.
#[derive(Debug, Clone, Serialize)]
pub struct PrecallOverrideTarget {
    pub prompt_id: &'static str,
    pub harness: AgentKind,
    pub context: OverrideContext,
    /// Set when the call's feature or project couldn't be found and the
    /// manager opens on Global, which may not be what the call resolves.
    pub context_note: Option<String>,
}

struct Resolved {
    label: String,
    repo: Option<PathBuf>,
    workdir: Option<PathBuf>,
    harness: AgentKind,
}

fn resolve_context(app: &App, context: &OverrideContext) -> GuiResult<Resolved> {
    match context {
        OverrideContext::Global => Ok(Resolved {
            label: "Global (all projects)".into(),
            repo: None,
            workdir: None,
            harness: AgentKind::default(),
        }),
        OverrideContext::Project { project_id } => {
            let project = app
                .store
                .projects
                .iter()
                .find(|project| &project.id == project_id)
                .ok_or_else(|| {
                    GuiError::not_found("Project was deleted; choose another override context")
                })?;
            Ok(Resolved {
                label: project.name.clone(),
                repo: Some(project.repo.clone()),
                workdir: None,
                harness: AgentKind::default(),
            })
        }
        OverrideContext::Feature {
            project_id,
            feature_id,
        } => {
            let (pi, fi) = app
                .store
                .locate_feature_by_id(Some(project_id), feature_id)
                .ok_or_else(|| {
                    GuiError::not_found("Feature was deleted; choose another override context")
                })?;
            let project = &app.store.projects[pi];
            let feature = &project.features[fi];
            Ok(Resolved {
                label: format!("{} / {}", project.name, feature.name),
                repo: Some(project.repo.clone()),
                workdir: Some(feature.workdir.clone()),
                harness: feature.agent.clone(),
            })
        }
    }
}

fn path_string(path: &Path) -> GuiResult<String> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| GuiError::conflict("This checkout path is not valid UTF-8"))
}

fn feature_scope(resolved: &Resolved) -> GuiResult<Option<OverrideScope>> {
    resolved
        .workdir
        .as_deref()
        .map(|workdir| path_string(workdir).map(|workdir| OverrideScope::Feature { workdir }))
        .transpose()
}

/// Stored overrides for one prompt in this context, in resolution order:
/// feature, project, global; shared before per-harness within a scope.
fn stored_for(
    id: PromptId,
    db: Option<&PromptOverrides>,
    project: &ProjectPromptOverrides,
    feature: Option<&OverrideScope>,
) -> Vec<StoredOverride> {
    let mut stored = Vec::new();
    let db_scope = |scope: &OverrideScope, layer: OverrideLayer, out: &mut Vec<_>| {
        let Some(db) = db else { return };
        let harnesses = std::iter::once(None).chain(AgentKind::ALL.iter().map(Some));
        for harness in harnesses {
            if let Some(row) = db.get(id.as_str(), scope, harness) {
                out.push(StoredOverride {
                    scope: layer,
                    harness: harness.cloned(),
                    template: row.template.clone(),
                });
            }
        }
    };
    if let Some(scope) = feature {
        db_scope(scope, OverrideLayer::Feature, &mut stored);
    }
    if let Some(entry) = project.get(id.as_str()) {
        if let Some(template) = &entry.template {
            stored.push(StoredOverride {
                scope: OverrideLayer::Project,
                harness: None,
                template: template.clone(),
            });
        }
        for harness in AgentKind::ALL {
            if let Some(template) = entry.harnesses.get(harness.slug()) {
                stored.push(StoredOverride {
                    scope: OverrideLayer::Project,
                    harness: Some(harness),
                    template: template.clone(),
                });
            }
        }
    }
    db_scope(&OverrideScope::Global, OverrideLayer::Global, &mut stored);
    stored
}

fn build_view(
    app: &App,
    context: &OverrideContext,
    harness: Option<AgentKind>,
) -> GuiResult<OverridesView> {
    let resolved = resolve_context(app, context)?;
    let harness = harness.unwrap_or_else(|| resolved.harness.clone());
    let db = app
        .db
        .as_ref()
        .map(|db| db.load_prompt_overrides())
        .transpose()
        .map_err(GuiError::from)?;
    let (project, project_config_error) = match resolved.repo.as_deref() {
        // The same (empty) map headless calls fall back to on a bad config.
        Some(repo) => match crate::prompts::project::load_strict(repo) {
            Ok(map) => (map, None),
            Err(error) => (ProjectPromptOverrides::new(), Some(error.to_string())),
        },
        None => (ProjectPromptOverrides::new(), None),
    };
    let feature = feature_scope(&resolved)?;
    let workdir = resolved.workdir.as_deref().map(path_string).transpose()?;
    let repo = resolved.repo.as_deref().map(path_string).transpose()?;
    let layers = PromptLayers {
        feature_workdir: workdir.as_deref(),
        db: db.as_ref(),
        project: Some(&project),
    };
    let rows = PromptId::ALL
        .into_iter()
        .map(|id| {
            let spec = id.spec();
            let (effective, source) = resolve_template_layered(id, &harness, &layers);
            let stored = stored_for(id, db.as_ref(), &project, feature.as_ref());
            let source = OverrideLayer::from(source);
            let source_harness = stored
                .iter()
                .any(|slot| slot.scope == source && slot.harness.as_ref() == Some(&harness))
                .then(|| harness.clone());
            // Serializing the stored slots is infallible: strings and enums.
            let identity = serde_json::to_string(&(
                &repo,
                &workdir,
                db.is_some(),
                &project_config_error,
                &stored,
            ))
            .unwrap_or_default();
            let mut hasher = DefaultHasher::new();
            identity.hash(&mut hasher);
            OverrideRow {
                id: id.as_str(),
                title: spec.title,
                summary: spec.summary,
                placeholders: spec.placeholders.to_vec(),
                source,
                source_harness,
                effective_template: effective.into_owned(),
                default_template: spec.default_template_for(&harness),
                stored,
                revision: format!("{:016x}", hasher.finish()),
            }
        })
        .collect();
    let no_db = || Some("No AMF database is open".to_string());
    let scopes = vec![
        ScopeOption {
            scope: OverrideLayer::Feature,
            label: "This feature",
            available: feature.is_some() && db.is_some(),
            reason: if feature.is_none() {
                Some("Choose a feature context to save a feature override".into())
            } else if db.is_none() {
                no_db()
            } else {
                None
            },
        },
        ScopeOption {
            scope: OverrideLayer::Project,
            label: "This project (amf.json)",
            available: repo.is_some() && project_config_error.is_none(),
            reason: if repo.is_none() {
                Some("Choose a project or feature context to save a project override".into())
            } else {
                project_config_error.clone()
            },
        },
        ScopeOption {
            scope: OverrideLayer::Global,
            label: "Global (all projects)",
            available: db.is_some(),
            reason: db.is_none().then(no_db).flatten(),
        },
    ];
    Ok(OverridesView {
        context: context.clone(),
        context_label: resolved.label,
        repo,
        workdir,
        harness,
        scopes,
        project_config_error,
        rows,
    })
}

pub fn load(
    gui: &mut GuiHandle,
    context: &OverrideContext,
    harness: Option<AgentKind>,
) -> GuiResult<OverridesView> {
    gui.refresh_snapshot()?;
    build_view(gui.app_for_workflow(), context, harness)
}

/// Re-resolve the context and refuse a write based on an out-of-date view.
fn checked_row(
    app: &App,
    context: &OverrideContext,
    prompt_id: &str,
    scope: OverrideLayer,
    revision: &str,
) -> GuiResult<(PromptId, OverridesView)> {
    let id = PromptId::from_key(prompt_id)
        .ok_or_else(|| GuiError::not_found(format!("Unknown prompt {prompt_id}")))?;
    let view = build_view(app, context, None)?;
    let row = view
        .rows
        .iter()
        .find(|row| row.id == id.as_str())
        .expect("every registry prompt has a row");
    if row.revision != revision {
        return Err(GuiError::conflict(
            "This prompt's overrides or checkout changed outside this window. \
             Reload to review the current version; your draft is kept.",
        ));
    }
    let option = view
        .scopes
        .iter()
        .find(|option| option.scope == scope)
        .ok_or_else(|| GuiError::conflict("The built-in default cannot be edited"))?;
    if !option.available {
        return Err(GuiError::conflict(option.reason.clone().unwrap_or_else(
            || format!("{} is not available here", option.label),
        )));
    }
    Ok((id, view))
}

pub fn save(gui: &mut GuiHandle, request: SaveOverride) -> GuiResult<OverridesView> {
    gui.refresh_snapshot()?;
    let app = gui.app_for_workflow();
    let (id, _) = checked_row(
        app,
        &request.context,
        &request.prompt_id,
        request.scope,
        &request.revision,
    )?;
    if request.template.trim().is_empty() {
        return Err(GuiError::conflict(
            "The template is empty. Clear the override instead to fall back to the next layer.",
        ));
    }
    write(
        app,
        &request.context,
        id,
        request.scope,
        request.harness.as_ref(),
        Some(&request.template),
    )?;
    build_view(app, &request.context, request.view_harness)
}

pub fn clear(gui: &mut GuiHandle, request: ClearOverride) -> GuiResult<OverridesView> {
    gui.refresh_snapshot()?;
    let app = gui.app_for_workflow();
    let (id, view) = checked_row(
        app,
        &request.context,
        &request.prompt_id,
        request.scope,
        &request.revision,
    )?;
    let exists = view
        .rows
        .iter()
        .find(|row| row.id == id.as_str())
        .is_some_and(|row| {
            row.stored
                .iter()
                .any(|slot| slot.scope == request.scope && slot.harness == request.harness)
        });
    if !exists {
        return Err(GuiError::conflict(
            "That override no longer exists. Reload.",
        ));
    }
    write(
        app,
        &request.context,
        id,
        request.scope,
        request.harness.as_ref(),
        None,
    )?;
    build_view(app, &request.context, request.view_harness)
}

/// Upsert (`Some`) or delete (`None`) exactly one override slot.
fn write(
    app: &App,
    context: &OverrideContext,
    id: PromptId,
    scope: OverrideLayer,
    harness: Option<&AgentKind>,
    template: Option<&str>,
) -> GuiResult<()> {
    let resolved = resolve_context(app, context)?;
    let db_scope = match scope {
        OverrideLayer::Feature => feature_scope(&resolved)?,
        OverrideLayer::Global => Some(OverrideScope::Global),
        OverrideLayer::Project => {
            let repo = resolved
                .repo
                .as_deref()
                .ok_or_else(|| GuiError::conflict("No project repository in this context"))?;
            return crate::prompts::project::update_in_repo(repo, |overrides| {
                let entry = overrides.entry(id.as_str().to_string()).or_default();
                let template = template.map(str::to_string);
                match harness {
                    Some(harness) => entry.set_harness(harness, template),
                    None => entry.set_shared(template),
                }
            })
            .map_err(GuiError::from);
        }
        OverrideLayer::BuiltIn => None,
    };
    let (Some(scope), Some(db)) = (db_scope, app.db.as_ref()) else {
        return Err(GuiError::conflict("That scope is not available here"));
    };
    match template {
        Some(template) => db.upsert_prompt_override(id.as_str(), &scope, harness, template)?,
        None => {
            db.delete_prompt_override(id.as_str(), &scope, harness)?;
        }
    }
    Ok(())
}

/// The prompt, harness and context of the pending pre-call notice, for its
/// "Edit prompt" link. Continuing that call re-resolves its prompt, so a save
/// made in the manager applies to it.
pub fn precall_target(gui: &mut GuiHandle) -> GuiResult<PrecallOverrideTarget> {
    let app = gui.app_for_workflow();
    let AppMode::PromptPrecall(pending) = &app.mode else {
        return Err(GuiError::conflict("There is no pending AI call to edit"));
    };
    let (workdir, project_name) = match pending.prior_mode.as_ref() {
        AppMode::PlanInterview(state) => (
            Some(&state.workdir),
            state
                .pending_launch
                .as_ref()
                .map(|launch| &launch.project_name),
        ),
        AppMode::DiffViewer(state) => (Some(&state.workdir), None),
        _ => (None, None),
    };
    let feature = workdir.and_then(|workdir| {
        app.store.projects.iter().find_map(|project| {
            project
                .features
                .iter()
                .find(|feature| &feature.workdir == workdir)
                .map(|feature| OverrideContext::Feature {
                    project_id: project.id.clone(),
                    feature_id: feature.id.clone(),
                })
        })
    });
    // A creation-time interview has no feature yet: its project still applies.
    // That launch names its project, and the launch itself resolves it by that
    // name (the store's key; creating a duplicate is refused), so use exactly
    // that lookup. Otherwise a workdir that is a project's own checkout.
    let project = || {
        project_name
            .map(|name| app.store.find_project(name))
            .unwrap_or_else(|| {
                workdir.and_then(|workdir| {
                    app.store
                        .projects
                        .iter()
                        .find(|project| &project.repo == workdir)
                })
            })
            .map(|project| OverrideContext::Project {
                project_id: project.id.clone(),
            })
    };
    let (context, context_note) = match feature.or_else(project) {
        Some(context) => (context, None),
        None => (
            OverrideContext::Global,
            Some(match (project_name, workdir) {
                (Some(name), _) => format!(
                    "Project \"{name}\" for this call was not found, so this opened on Global. \
                     Choose the right context before editing."
                ),
                (None, Some(workdir)) => format!(
                    "No AMF feature or project matches {}, so this opened on Global. \
                     Feature and project overrides for this call may differ.",
                    workdir.display()
                ),
                (None, None) => "AMF couldn't tell which feature or project this call runs in, so \
                     this opened on Global. Feature and project overrides for this call may \
                     differ."
                    .into(),
            }),
        ),
    };
    Ok(PrecallOverrideTarget {
        prompt_id: pending.prompt_id.as_str(),
        harness: pending.harness.clone(),
        context,
        context_note,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::db::AmfDb;
    use crate::gui_contract::GuiErrorKind;
    use crate::project::{Feature, Project, ProjectStore, VibeMode};
    use crate::traits::{MockTmuxOps, MockWorktreeOps};

    struct Fixture {
        dir: tempfile::TempDir,
        gui: GuiHandle,
        feature: OverrideContext,
        project: OverrideContext,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let workdir = dir.path().join("worktree");
        std::fs::create_dir(&repo).unwrap();
        std::fs::create_dir(&workdir).unwrap();
        let mut project = Project::new("Project".into(), repo, true, AgentKind::Claude);
        let feature = Feature::new_for_project(
            "Project",
            "Feature".into(),
            "feature".into(),
            workdir,
            true,
            VibeMode::default(),
            false,
            false,
            AgentKind::Codex,
            false,
            false,
        );
        let feature_context = OverrideContext::Feature {
            project_id: project.id.clone(),
            feature_id: feature.id.clone(),
        };
        let project_context = OverrideContext::Project {
            project_id: project.id.clone(),
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
        Fixture {
            dir,
            gui: GuiHandle::from_app(app),
            feature: feature_context,
            project: project_context,
        }
    }

    fn row(view: &OverridesView, id: PromptId) -> &OverrideRow {
        view.rows.iter().find(|row| row.id == id.as_str()).unwrap()
    }

    fn save_req(
        context: &OverrideContext,
        view: &OverridesView,
        id: PromptId,
        scope: OverrideLayer,
        harness: Option<AgentKind>,
        template: &str,
    ) -> SaveOverride {
        SaveOverride {
            context: context.clone(),
            prompt_id: id.as_str().into(),
            scope,
            harness,
            template: template.into(),
            revision: row(view, id).revision.clone(),
            view_harness: Some(view.harness.clone()),
        }
    }

    fn resolved_by_call_site(fx: &mut Fixture, id: PromptId, harness: &AgentKind) -> String {
        let repo = fx.dir.path().join("repo");
        let workdir = fx.dir.path().join("worktree");
        fx.gui
            .app_for_workflow()
            .resolve_headless_template(id, harness, &repo, &workdir)
            .0
    }

    #[test]
    fn lists_every_registry_prompt_with_scopes_offered_by_context() {
        let mut fx = fixture();
        let view = load(&mut fx.gui, &fx.feature.clone(), None).unwrap();
        assert_eq!(view.rows.len(), PromptId::ALL.len());
        assert_eq!(
            view.harness,
            AgentKind::Codex,
            "feature agent is the default"
        );
        let walkthrough = row(&view, PromptId::ReviewWalkthrough);
        assert_eq!(walkthrough.source, OverrideLayer::BuiltIn);
        assert_eq!(walkthrough.placeholders, ["file_path", "patch"]);
        assert_eq!(walkthrough.effective_template, walkthrough.default_template);
        assert!(view.scopes.iter().all(|scope| scope.available));

        let project = load(&mut fx.gui, &fx.project.clone(), None).unwrap();
        let available: Vec<_> = project
            .scopes
            .iter()
            .filter(|scope| scope.available)
            .map(|scope| scope.scope)
            .collect();
        assert_eq!(available, [OverrideLayer::Project, OverrideLayer::Global]);
        let global = load(&mut fx.gui, &OverrideContext::Global, None).unwrap();
        assert!(
            global.scopes[1]
                .reason
                .as_deref()
                .unwrap()
                .contains("project")
        );
        assert!(!global.scopes[0].available && global.scopes[2].available);
    }

    #[test]
    fn saves_each_scope_and_harness_where_call_sites_resolve_them() {
        let mut fx = fixture();
        let ctx = fx.feature.clone();
        let id = PromptId::SessionSummary;
        for (scope, harness, text) in [
            (OverrideLayer::Global, None, "GLOBAL {{recent_lines}}"),
            (OverrideLayer::Project, Some(AgentKind::Pi), "PROJECT pi"),
            (OverrideLayer::Project, None, "PROJECT shared"),
            (
                OverrideLayer::Feature,
                Some(AgentKind::Codex),
                "FEATURE codex",
            ),
        ] {
            let view = load(&mut fx.gui, &ctx, None).unwrap();
            save(&mut fx.gui, save_req(&ctx, &view, id, scope, harness, text)).unwrap();
        }
        assert_eq!(
            resolved_by_call_site(&mut fx, id, &AgentKind::Codex),
            "FEATURE codex"
        );
        assert_eq!(
            resolved_by_call_site(&mut fx, id, &AgentKind::Pi),
            "PROJECT pi"
        );
        assert_eq!(
            resolved_by_call_site(&mut fx, id, &AgentKind::Claude),
            "PROJECT shared"
        );

        let view = load(&mut fx.gui, &ctx, None).unwrap();
        let summary = row(&view, id);
        assert_eq!(summary.source, OverrideLayer::Feature);
        assert_eq!(summary.source_harness, Some(AgentKind::Codex));
        assert_eq!(
            summary
                .stored
                .iter()
                .map(|slot| (slot.scope, slot.harness.clone()))
                .collect::<Vec<_>>(),
            [
                (OverrideLayer::Feature, Some(AgentKind::Codex)),
                (OverrideLayer::Project, None),
                (OverrideLayer::Project, Some(AgentKind::Pi)),
                (OverrideLayer::Global, None),
            ]
        );
        let claude = load(&mut fx.gui, &ctx, Some(AgentKind::Claude)).unwrap();
        assert_eq!(row(&claude, id).source, OverrideLayer::Project);
        assert_eq!(row(&claude, id).source_harness, None);
        // The project key is in the committed amf.json, not in .amf/.
        let raw = std::fs::read_to_string(fx.dir.path().join("repo/amf.json")).unwrap();
        assert!(raw.contains("PROJECT pi"), "{raw}");
    }

    #[test]
    fn stale_saves_after_external_db_or_amf_json_edits_are_refused() {
        let mut fx = fixture();
        let ctx = fx.feature.clone();
        let id = PromptId::ReviewWalkthrough;
        let view = load(&mut fx.gui, &ctx, None).unwrap();

        // Another process writes a global override for the same prompt.
        fx.gui
            .app_for_workflow()
            .db
            .as_ref()
            .unwrap()
            .upsert_prompt_override(id.as_str(), &OverrideScope::Global, None, "TUI {{patch}}")
            .unwrap();
        let stale = save(
            &mut fx.gui,
            save_req(&ctx, &view, id, OverrideLayer::Feature, None, "mine"),
        )
        .unwrap_err();
        assert_eq!(stale.kind, GuiErrorKind::Conflict);
        assert!(stale.message.contains("Reload"));

        // A hand edit of amf.json for the same prompt is refused the same way.
        let view = load(&mut fx.gui, &ctx, None).unwrap();
        std::fs::write(
            fx.dir.path().join("repo/amf.json"),
            r#"{"prompt_overrides":{"review.walkthrough":{"template":"hand"}}}"#,
        )
        .unwrap();
        let req = save_req(&ctx, &view, id, OverrideLayer::Global, None, "mine");
        assert_eq!(
            save(&mut fx.gui, req).unwrap_err().kind,
            GuiErrorKind::Conflict
        );
        // An unrelated prompt's revision is untouched.
        let other = PromptId::SessionSummary;
        save(
            &mut fx.gui,
            save_req(&ctx, &view, other, OverrideLayer::Global, None, "ok"),
        )
        .unwrap();

        // After reloading, the same draft saves against the current version.
        let view = load(&mut fx.gui, &ctx, None).unwrap();
        save(
            &mut fx.gui,
            save_req(&ctx, &view, id, OverrideLayer::Global, None, "mine"),
        )
        .unwrap();
        assert_eq!(
            resolved_by_call_site(&mut fx, id, &AgentKind::Claude),
            "hand"
        );
    }

    #[test]
    fn malformed_amf_json_is_reported_and_never_overwritten() {
        let mut fx = fixture();
        let path = fx.dir.path().join("repo/amf.json");
        std::fs::write(&path, "{ broken").unwrap();
        let ctx = fx.project.clone();
        let view = load(&mut fx.gui, &ctx, None).unwrap();
        assert!(
            view.project_config_error
                .as_deref()
                .unwrap()
                .contains("not valid JSON")
        );
        assert!(!view.scopes[1].available);
        let id = PromptId::SessionSummary;
        let error = save(
            &mut fx.gui,
            save_req(&ctx, &view, id, OverrideLayer::Project, None, "x"),
        )
        .unwrap_err();
        assert!(
            error.message.contains("not valid JSON"),
            "{}",
            error.message
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ broken");
        // Global saves still work in a project whose config is broken.
        save(
            &mut fx.gui,
            save_req(&ctx, &view, id, OverrideLayer::Global, None, "global"),
        )
        .unwrap();
    }

    #[test]
    fn clear_removes_exactly_one_slot_and_refuses_stale_or_missing_ones() {
        let mut fx = fixture();
        let ctx = fx.feature.clone();
        let id = PromptId::LearningAnswer;
        let view = load(&mut fx.gui, &ctx, None).unwrap();
        save(
            &mut fx.gui,
            save_req(&ctx, &view, id, OverrideLayer::Project, None, "shared"),
        )
        .unwrap();
        let view = load(&mut fx.gui, &ctx, None).unwrap();
        save(
            &mut fx.gui,
            save_req(
                &ctx,
                &view,
                id,
                OverrideLayer::Project,
                Some(AgentKind::Codex),
                "codex",
            ),
        )
        .unwrap();
        let clear_req = |view: &OverridesView, harness: Option<AgentKind>| ClearOverride {
            context: ctx.clone(),
            prompt_id: id.as_str().into(),
            scope: OverrideLayer::Project,
            harness,
            revision: row(view, id).revision.clone(),
            view_harness: None,
        };
        let view = load(&mut fx.gui, &ctx, None).unwrap();
        let after = clear(&mut fx.gui, clear_req(&view, Some(AgentKind::Codex))).unwrap();
        assert_eq!(row(&after, id).stored.len(), 1);
        assert_eq!(row(&after, id).effective_template, "shared");
        // The old revision is now stale.
        assert_eq!(
            clear(&mut fx.gui, clear_req(&view, None)).unwrap_err().kind,
            GuiErrorKind::Conflict
        );
        assert!(
            clear(&mut fx.gui, clear_req(&after, Some(AgentKind::Pi)))
                .unwrap_err()
                .message
                .contains("no longer exists")
        );
        let empty = clear(&mut fx.gui, clear_req(&after, None)).unwrap();
        assert_eq!(row(&empty, id).source, OverrideLayer::BuiltIn);
        let raw = std::fs::read_to_string(fx.dir.path().join("repo/amf.json")).unwrap();
        assert!(!raw.contains("prompt_overrides"), "{raw}");
    }

    #[test]
    fn deleted_features_reassigned_checkouts_and_empty_templates_are_refused() {
        let mut fx = fixture();
        let ctx = fx.feature.clone();
        let id = PromptId::ReviewCoReview;
        let view = load(&mut fx.gui, &ctx, None).unwrap();
        let empty = save(
            &mut fx.gui,
            save_req(&ctx, &view, id, OverrideLayer::Global, None, "  \n"),
        )
        .unwrap_err();
        assert!(empty.message.contains("empty"));

        // The feature's checkout moves (another process rewrote the store).
        {
            let app = fx.gui.app_for_workflow();
            let moved = fx.dir.path().join("moved");
            std::fs::create_dir(&moved).unwrap();
            app.store.projects[0].features[0].workdir = moved;
            let db = app.db.as_ref().unwrap();
            db.save_store(&app.store).unwrap();
        }
        let moved = save(
            &mut fx.gui,
            save_req(&ctx, &view, id, OverrideLayer::Feature, None, "x"),
        )
        .unwrap_err();
        assert_eq!(moved.kind, GuiErrorKind::Conflict);

        {
            let app = fx.gui.app_for_workflow();
            app.store.projects[0].features.clear();
            app.db.as_ref().unwrap().save_store(&app.store).unwrap();
        }
        assert_eq!(
            load(&mut fx.gui, &ctx, None).unwrap_err().kind,
            GuiErrorKind::NotFound
        );
    }

    #[test]
    fn precall_target_names_the_pending_prompt_and_its_feature() {
        use crate::app::precall::{PendingPrecall, PrecallAction};
        let mut fx = fixture();
        assert_eq!(
            precall_target(&mut fx.gui).unwrap_err().kind,
            GuiErrorKind::Conflict
        );
        let workdir = fx.dir.path().join("worktree");
        let app = fx.gui.app_for_workflow();
        let state = crate::app::PlanInterviewState::for_feature(
            "Feature".into(),
            "feature".into(),
            Vec::new(),
            workdir,
            AgentKind::Pi,
        );
        app.mode = AppMode::PromptPrecall(Box::new(PendingPrecall {
            action: PrecallAction::PlanRound,
            prompt_id: PromptId::PlanInterviewRound,
            harness: AgentKind::Pi,
            model: None,
            preview: String::new(),
            viewing: false,
            scroll: 0,
            prior_mode: Box::new(AppMode::PlanInterview(state)),
        }));
        let target = precall_target(&mut fx.gui).unwrap();
        assert_eq!(target.prompt_id, "plan_interview.round");
        assert_eq!(target.harness, AgentKind::Pi);
        assert_eq!(target.context, fx.feature);
        assert_eq!(target.context_note, None);
        // Opening the manager leaves the pending call untouched.
        load(&mut fx.gui, &target.context, Some(target.harness)).unwrap();
        assert!(matches!(
            fx.gui.app_for_workflow().mode,
            AppMode::PromptPrecall(_)
        ));
    }
    fn park_precall(fx: &mut Fixture, prior: AppMode) {
        use crate::app::precall::{PendingPrecall, PrecallAction};
        fx.gui.app_for_workflow().mode = AppMode::PromptPrecall(Box::new(PendingPrecall {
            action: PrecallAction::PlanRound,
            prompt_id: PromptId::PlanInterviewRound,
            harness: AgentKind::Pi,
            model: None,
            preview: String::new(),
            viewing: false,
            scroll: 0,
            prior_mode: Box::new(prior),
        }));
    }

    fn creation_interview(project_name: &str, workdir: PathBuf) -> AppMode {
        AppMode::PlanInterview(crate::app::PlanInterviewState::for_feature_creation(
            crate::app::PreparedFeatureLaunch {
                model_selection: None,
                project_name: project_name.into(),
                feature_name: None,
                branch: "planned".into(),
                workdir,
                is_worktree: true,
                mode: VibeMode::default(),
                review: false,
                plan_mode: true,
                quick_plan: false,
                agent: AgentKind::Pi,
                create_terminal: false,
                session_name: "Pi 1".into(),
                enable_chrome: false,
                remote_control: false,
                steering_enabled: false,
                hook_succeeded: None,
                startup_prompt: None,
                todo_origin: None,
                issue_source: None,
            },
            Vec::new(),
        ))
    }

    #[test]
    fn precall_target_finds_a_creation_interviews_project_the_way_its_launch_does() {
        let mut fx = fixture();
        // A second project whose checkout is the new worktree's path must not
        // win over the project the launch names.
        let new_worktree = fx.dir.path().join("planned");
        {
            let app = fx.gui.app_for_workflow();
            app.store.projects.push(Project::new(
                "Other".into(),
                new_worktree.clone(),
                true,
                AgentKind::Claude,
            ));
        }
        park_precall(&mut fx, creation_interview("Project", new_worktree));
        let target = precall_target(&mut fx.gui).unwrap();
        assert_eq!(target.context, fx.project);
        assert_eq!(target.context_note, None);
    }

    #[test]
    fn precall_target_says_why_it_falls_back_to_global() {
        let mut fx = fixture();
        let planned = fx.dir.path().join("planned");
        park_precall(&mut fx, creation_interview("Gone", planned));
        let target = precall_target(&mut fx.gui).unwrap();
        assert_eq!(target.context, OverrideContext::Global);
        let note = target.context_note.unwrap();
        assert!(
            note.contains("\"Gone\"") && note.contains("Global"),
            "{note}"
        );

        let elsewhere = fx.dir.path().join("elsewhere");
        let state = crate::app::PlanInterviewState::for_feature(
            "Feature".into(),
            "feature".into(),
            Vec::new(),
            elsewhere.clone(),
            AgentKind::Pi,
        );
        park_precall(&mut fx, AppMode::PlanInterview(state));
        let target = precall_target(&mut fx.gui).unwrap();
        assert_eq!(target.context, OverrideContext::Global);
        let note = target.context_note.unwrap();
        assert!(note.contains(&elsewhere.display().to_string()), "{note}");
    }
}
