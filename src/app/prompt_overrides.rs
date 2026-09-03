//! The headless-prompt override manager overlay (Editable Headless Prompts).
//!
//! Lists every registry [`crate::prompts::PromptId`] with the scope its
//! effective template currently comes from, opens an inline [`TextEditor`] on
//! the effective template, and on save routes through a scope picker
//! (feature / project / global) and a harness picker (shared / one harness).
//!
//! Feature and global overrides persist in `amf.db`
//! (`crate::db::prompt_overrides`); project overrides persist in the repo's
//! `amf.json` (`ExtensionConfig::prompt_overrides`). Nothing here validates a
//! template — a dropped or unknown `{{token}}` is saved and rendered as-is.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::app::{
    App, AppMode, PromptOverrideEditState, PromptOverrideRow, PromptOverrideScope,
    PromptOverrideStep, PromptOverridesState, ViewState,
};
use crate::db::prompt_overrides::{OverrideScope, PromptOverrides};
use crate::editor::TextEditor;
use crate::project::AgentKind;
use crate::prompts::project::ProjectPromptOverrides;
use crate::prompts::{PromptId, PromptLayers, PromptSource, resolve_template_layered};

/// The repo root and feature workdir the manager acts against, derived from
/// the current dashboard selection. Both `None` means "global only".
struct OverrideContext {
    repo: Option<PathBuf>,
    workdir: Option<PathBuf>,
    /// The harness whose effective template the list summarises and the
    /// editor pre-fills — the selected feature's, else Claude.
    harness: AgentKind,
}

impl App {
    fn prompt_override_context(&self) -> OverrideContext {
        use crate::app::state::Selection;
        match &self.selection {
            Selection::Feature(pi, fi) | Selection::Session(pi, fi, _) => {
                let project = self.store.projects.get(*pi);
                let feature = project.and_then(|p| p.features.get(*fi));
                OverrideContext {
                    repo: project.map(|p| p.repo.clone()),
                    workdir: feature.map(|f| f.workdir.clone()),
                    harness: feature.map(|f| f.agent.clone()).unwrap_or_default(),
                }
            }
            Selection::Project(pi) => OverrideContext {
                repo: self.store.projects.get(*pi).map(|p| p.repo.clone()),
                workdir: None,
                harness: AgentKind::default(),
            },
        }
    }

    /// The DB override view (feature + global rows) and the project `amf.json`
    /// override map, loaded fresh.
    fn prompt_override_sources(
        &self,
        repo: Option<&Path>,
    ) -> (Option<PromptOverrides>, ProjectPromptOverrides) {
        let db = self
            .db
            .as_ref()
            .and_then(|db| db.load_prompt_overrides().ok());
        let project = repo
            .map(crate::prompts::project::load_from_repo)
            .unwrap_or_default();
        (db, project)
    }

    fn prompt_override_layers<'a>(
        db: &'a Option<PromptOverrides>,
        project: &'a ProjectPromptOverrides,
        workdir: Option<&'a Path>,
    ) -> PromptLayers<'a> {
        PromptLayers {
            feature_workdir: workdir.and_then(Path::to_str),
            db: db.as_ref(),
            project: Some(project),
        }
    }

    fn prompt_override_rows(&self, ctx: &OverrideContext) -> Vec<PromptOverrideRow> {
        let (db, project) = self.prompt_override_sources(ctx.repo.as_deref());
        let layers = Self::prompt_override_layers(&db, &project, ctx.workdir.as_deref());
        let feature_scope =
            ctx.workdir
                .as_deref()
                .and_then(Path::to_str)
                .map(|w| OverrideScope::Feature {
                    workdir: w.to_string(),
                });

        PromptId::ALL
            .into_iter()
            .map(|id| {
                let (_, source) = resolve_template_layered(id, &ctx.harness, &layers);
                let has_feature = feature_scope.as_ref().is_some_and(|scope| {
                    db.as_ref().is_some_and(|d| {
                        d.get(id.as_str(), scope, None).is_some()
                            || AgentKind::ALL
                                .iter()
                                .any(|h| d.get(id.as_str(), scope, Some(h)).is_some())
                    })
                });
                let has_global = db.as_ref().is_some_and(|d| {
                    d.get(id.as_str(), &OverrideScope::Global, None).is_some()
                        || AgentKind::ALL.iter().any(|h| {
                            d.get(id.as_str(), &OverrideScope::Global, Some(h))
                                .is_some()
                        })
                });
                let has_project = project
                    .get(id.as_str())
                    .is_some_and(|entry| !entry.is_empty());
                PromptOverrideRow {
                    id,
                    source,
                    has_feature,
                    has_project,
                    has_global,
                }
            })
            .collect()
    }

    /// Open the manager. `from_view` is the embedded session view to return to
    /// (leader command); `None` returns to the dashboard.
    pub fn open_prompt_overrides(&mut self, from_view: Option<ViewState>) {
        self.open_prompt_overrides_focused(from_view, None);
    }

    /// Open the manager with `focus` (if any) pre-selected — used by the
    /// pre-call notice's `e` to land the user on the prompt they were about
    /// to run.
    pub(crate) fn open_prompt_overrides_focused(
        &mut self,
        from_view: Option<ViewState>,
        focus: Option<PromptId>,
    ) {
        let ctx = self.prompt_override_context();
        let rows = self.prompt_override_rows(&ctx);
        let selected = focus
            .and_then(|id| rows.iter().position(|r| r.id == id))
            .unwrap_or(0);
        self.mode = AppMode::PromptOverrides(Box::new(PromptOverridesState {
            rows,
            selected,
            scroll: 0,
            edit: None,
            help_open: false,
            confirm_clear: false,
            from_view,
        }));
        self.message = None;
    }

    fn prompt_overrides_state(&mut self) -> Option<&mut PromptOverridesState> {
        match &mut self.mode {
            AppMode::PromptOverrides(state) => Some(state),
            _ => None,
        }
    }

    pub fn prompt_overrides_close(&mut self) {
        let from_view = match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::PromptOverrides(state) => state.from_view,
            other => {
                self.mode = other;
                return;
            }
        };
        // Opened from a pre-call notice's `e`? Return to the notice so the
        // user can continue the run with the override they just saved.
        if let Some(pending) = self.precall_return.take() {
            self.mode = AppMode::PromptPrecall(pending);
            return;
        }
        if let Some(view) = from_view {
            self.mode = AppMode::Viewing(view);
        }
    }

    pub fn prompt_overrides_select(&mut self, delta: isize) {
        if let Some(state) = self.prompt_overrides_state() {
            let n = state.rows.len();
            if n == 0 {
                return;
            }
            state.confirm_clear = false;
            let cur = state.selected as isize;
            state.selected = (cur + delta).rem_euclid(n as isize) as usize;
        }
    }

    /// Scroll the list so `selected` stays visible in a window of `height` rows.
    pub fn prompt_overrides_clamp_scroll(&mut self, height: usize) {
        if let Some(state) = self.prompt_overrides_state() {
            if state.selected < state.scroll {
                state.scroll = state.selected;
            } else if height > 0 && state.selected >= state.scroll + height {
                state.scroll = state.selected + 1 - height;
            }
        }
    }

    pub fn prompt_overrides_toggle_help(&mut self) {
        if let Some(state) = self.prompt_overrides_state() {
            state.help_open = !state.help_open;
        }
    }

    /// Open the editor on the selected row, pre-filled with its effective
    /// template for the context harness.
    pub fn prompt_overrides_start_edit(&mut self) {
        let ctx = self.prompt_override_context();
        let Some(id) = self
            .prompt_overrides_state()
            .and_then(|s| s.selected_row().map(|r| r.id))
        else {
            return;
        };
        let (db, project) = self.prompt_override_sources(ctx.repo.as_deref());
        let layers = Self::prompt_override_layers(&db, &project, ctx.workdir.as_deref());
        let (template, _) = resolve_template_layered(id, &ctx.harness, &layers);

        let mut scopes = Vec::new();
        if ctx.workdir.is_some() && self.db.is_some() {
            scopes.push(PromptOverrideScope::Feature);
        }
        if ctx.repo.is_some() {
            scopes.push(PromptOverrideScope::Project);
        }
        scopes.push(PromptOverrideScope::Global);

        let row = self
            .prompt_overrides_state()
            .map(|s| s.selected)
            .unwrap_or(0);
        if let Some(state) = self.prompt_overrides_state() {
            state.confirm_clear = false;
            state.edit = Some(PromptOverrideEditState {
                row,
                editor: TextEditor::new(template.into_owned()),
                step: PromptOverrideStep::Editing,
                scopes,
                scope_index: 0,
                harness_index: 0,
            });
        }
    }

    pub fn prompt_overrides_cancel_edit(&mut self) {
        if let Some(state) = self.prompt_overrides_state() {
            state.edit = None;
        }
        self.message = Some("Edit discarded".into());
    }

    pub fn prompt_overrides_editor_toggle_vim(&mut self) {
        if let Some(state) = self.prompt_overrides_state()
            && let Some(edit) = &mut state.edit
        {
            edit.editor.toggle_vim();
            let on = edit.editor.vim_mode().is_some();
            self.message = Some(if on {
                "Vim mode enabled".into()
            } else {
                "Vim mode disabled".into()
            });
        }
    }

    /// Editing → ScopePicker (or straight to save if only one scope + shared
    /// is possible — but we always show the pickers for clarity).
    pub fn prompt_overrides_editor_advance(&mut self) {
        if let Some(state) = self.prompt_overrides_state()
            && let Some(edit) = &mut state.edit
        {
            edit.step = match edit.step {
                PromptOverrideStep::Editing => PromptOverrideStep::ScopePicker,
                PromptOverrideStep::ScopePicker => PromptOverrideStep::HarnessPicker,
                PromptOverrideStep::HarnessPicker => PromptOverrideStep::HarnessPicker,
            };
        }
    }

    pub fn prompt_overrides_picker_back(&mut self) {
        if let Some(state) = self.prompt_overrides_state()
            && let Some(edit) = &mut state.edit
        {
            edit.step = match edit.step {
                PromptOverrideStep::HarnessPicker => PromptOverrideStep::ScopePicker,
                _ => PromptOverrideStep::Editing,
            };
        }
    }

    pub fn prompt_overrides_picker_move(&mut self, delta: isize) {
        if let Some(state) = self.prompt_overrides_state()
            && let Some(edit) = &mut state.edit
        {
            match edit.step {
                PromptOverrideStep::ScopePicker => {
                    let n = edit.scopes.len().max(1);
                    edit.scope_index =
                        (edit.scope_index as isize + delta).rem_euclid(n as isize) as usize;
                }
                PromptOverrideStep::HarnessPicker => {
                    // 0 = shared, then the four harnesses.
                    let n = 1 + AgentKind::ALL.len();
                    edit.harness_index =
                        (edit.harness_index as isize + delta).rem_euclid(n as isize) as usize;
                }
                PromptOverrideStep::Editing => {}
            }
        }
    }

    /// Persist the edited template to the chosen scope + harness, rebuild the
    /// rows, and return to the list.
    pub fn prompt_overrides_confirm_save(&mut self) -> Result<()> {
        let ctx = self.prompt_override_context();
        let Some((id, template, scope, harness)) =
            self.prompt_overrides_state().and_then(|state| {
                state.edit.as_ref().map(|edit| {
                    (
                        state.rows.get(edit.row).map(|r| r.id),
                        edit.editor.text().to_string(),
                        edit.scope(),
                        edit.harness(),
                    )
                })
            })
        else {
            return Ok(());
        };
        let Some(id) = id else { return Ok(()) };

        let outcome = match scope {
            PromptOverrideScope::Feature => self.save_db_override(
                id,
                ctx.workdir.as_deref(),
                harness.as_ref(),
                &template,
                true,
            ),
            PromptOverrideScope::Global => {
                self.save_db_override(id, None, harness.as_ref(), &template, false)
            }
            PromptOverrideScope::Project => {
                self.save_project_override(id, ctx.repo.as_deref(), harness.as_ref(), &template)
            }
        };

        match outcome {
            Ok(()) => {
                let harness_label = harness
                    .as_ref()
                    .map(|h| h.display_name())
                    .unwrap_or("all harnesses");
                self.message = Some(format!(
                    "Saved {} override for {} ({harness_label})",
                    scope.label().to_lowercase(),
                    id.as_str()
                ));
                if let Some(state) = self.prompt_overrides_state() {
                    state.edit = None;
                }
                self.prompt_overrides_reload();
            }
            Err(e) => {
                self.message = Some(format!("Couldn't save override: {e}"));
            }
        }
        Ok(())
    }

    fn save_db_override(
        &self,
        id: PromptId,
        workdir: Option<&Path>,
        harness: Option<&AgentKind>,
        template: &str,
        feature_scope: bool,
    ) -> Result<()> {
        let db = self.db.as_ref().ok_or_else(|| {
            anyhow::anyhow!("no database — feature and global overrides need one")
        })?;
        let scope = if feature_scope {
            let workdir = workdir
                .and_then(Path::to_str)
                .ok_or_else(|| anyhow::anyhow!("this feature has no workdir path"))?;
            OverrideScope::Feature {
                workdir: workdir.to_string(),
            }
        } else {
            OverrideScope::Global
        };
        db.upsert_prompt_override(id.as_str(), &scope, harness, template)
    }

    fn save_project_override(
        &self,
        id: PromptId,
        repo: Option<&Path>,
        harness: Option<&AgentKind>,
        template: &str,
    ) -> Result<()> {
        let repo = repo.ok_or_else(|| anyhow::anyhow!("no project repo in context"))?;
        let mut config = load_raw_project_config(repo);
        let entry = config
            .prompt_overrides
            .entry(id.as_str().to_string())
            .or_default();
        match harness {
            Some(h) => entry.set_harness(h, Some(template.to_string())),
            None => entry.set_shared(Some(template.to_string())),
        }
        crate::extension::save_project_extension_config(repo, &config)
    }

    /// Clear the selected row's effective override (`d`, `d` again to confirm).
    pub fn prompt_overrides_clear_selected(&mut self) -> Result<()> {
        let ctx = self.prompt_override_context();
        let Some((id, source)) = self
            .prompt_overrides_state()
            .and_then(|s| s.selected_row().map(|r| (r.id, r.source)))
        else {
            return Ok(());
        };

        if !source.is_override() {
            self.message = Some("No override on this prompt — it uses the built-in default".into());
            if let Some(state) = self.prompt_overrides_state() {
                state.confirm_clear = false;
            }
            return Ok(());
        }

        let confirmed = self
            .prompt_overrides_state()
            .is_some_and(|s| s.confirm_clear);
        if !confirmed {
            if let Some(state) = self.prompt_overrides_state() {
                state.confirm_clear = true;
            }
            self.message = Some(format!(
                "Clear the {} override for {}? Press d again to confirm.",
                source.label(),
                id.as_str()
            ));
            return Ok(());
        }

        let result = match source {
            PromptSource::Feature | PromptSource::Global => {
                self.clear_db_override(id, source, ctx.workdir.as_deref())
            }
            PromptSource::Project => self.clear_project_override(id, ctx.repo.as_deref()),
            PromptSource::BuiltIn => Ok(()),
        };

        match result {
            Ok(()) => {
                self.message = Some(format!(
                    "Cleared the {} override for {}",
                    source.label(),
                    id.as_str()
                ));
            }
            Err(e) => self.message = Some(format!("Couldn't clear override: {e}")),
        }
        if let Some(state) = self.prompt_overrides_state() {
            state.confirm_clear = false;
        }
        self.prompt_overrides_reload();
        Ok(())
    }

    fn clear_db_override(
        &self,
        id: PromptId,
        source: PromptSource,
        workdir: Option<&Path>,
    ) -> Result<()> {
        let db = self
            .db
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no database"))?;
        let scope = match source {
            PromptSource::Global => OverrideScope::Global,
            PromptSource::Feature => OverrideScope::Feature {
                workdir: workdir
                    .and_then(Path::to_str)
                    .ok_or_else(|| anyhow::anyhow!("no workdir"))?
                    .to_string(),
            },
            _ => return Ok(()),
        };
        // Drop both the shared row and every per-harness row for this prompt at
        // this scope — "clear the override" is unambiguous from the list.
        db.delete_prompt_override(id.as_str(), &scope, None)?;
        for h in AgentKind::ALL.iter() {
            db.delete_prompt_override(id.as_str(), &scope, Some(h))?;
        }
        Ok(())
    }

    fn clear_project_override(&self, id: PromptId, repo: Option<&Path>) -> Result<()> {
        let repo = repo.ok_or_else(|| anyhow::anyhow!("no project repo"))?;
        let mut config = load_raw_project_config(repo);
        config.prompt_overrides.remove(id.as_str());
        crate::extension::save_project_extension_config(repo, &config)
    }

    fn prompt_overrides_reload(&mut self) {
        let ctx = self.prompt_override_context();
        let rows = self.prompt_override_rows(&ctx);
        if let Some(state) = self.prompt_overrides_state() {
            let keep = state.selected.min(rows.len().saturating_sub(1));
            state.rows = rows;
            state.selected = keep;
        }
    }
}

/// Read `{repo}/amf.json` (or the legacy path) into an `ExtensionConfig`,
/// falling back to a default so a first override can be added to a repo with
/// no config file yet. Deliberately *not* merged with global config — the
/// manager edits this repo's file only.
fn load_raw_project_config(repo: &Path) -> crate::extension::ExtensionConfig {
    crate::extension::resolve_project_config_path(repo)
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}
