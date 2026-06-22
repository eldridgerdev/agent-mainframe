use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::Utc;

use super::*;
use crate::editor::TextEditor;
use crate::prompt_library::{
    PlaceholderKind, PromptPlaceholder, PromptSource, PromptTemplate, infer_placeholder_slots,
    render_template,
};

/// Where `export_selected_template` writes a user template.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptExportTarget {
    /// `~/.config/amf/config.json` (the `extension` block).
    Global,
    /// `{project.repo}/.amf/config.json` — the main repo root.
    Project,
    /// `{feature.workdir}/.amf/config.json` — the active worktree.
    /// Lets the user commit the template on a feature branch and
    /// promote it to the main repo via git later.
    Worktree,
}

impl App {
    // ── Picker: open / navigate / filter ─────────────────────────

    /// Build the merged, source-tagged template list and enter the
    /// prompt-library picker. Surfaces editable `User` templates from the
    /// SQLite store alongside read-only `Project` (`{repo}/.amf/config.json`)
    /// and `Global` (`~/.config/amf/config.json`) declarative templates.
    pub fn open_prompt_library(&mut self, from_view: Option<ViewState>) {
        self.rebuild_prompt_library(from_view, None);
    }

    /// Like `open_prompt_library` but positions the cursor on the entry
    /// matching `focus` (name + source) after the rebuild. Used by mutation
    /// paths so the newly added/modified entry is immediately visible.
    /// Falls back to position 0 when the focused entry isn't in the list
    /// (e.g. hidden by a higher-priority Worktree/Project entry with the
    /// same name).
    fn rebuild_prompt_library(
        &mut self,
        from_view: Option<ViewState>,
        focus: Option<(&str, PromptSource)>,
    ) {
        let project = self
            .resolve_library_repo(from_view.as_ref())
            .map(|repo| load_project_prompt_templates(&repo))
            .unwrap_or_default();

        // Load worktree templates separately so they get their own badge.
        let worktree_templates = self
            .resolve_worktree_dir(from_view.as_ref())
            .map(|dir| load_project_prompt_templates(&dir))
            .unwrap_or_default();

        // Always read global templates fresh from disk so that external edits
        // and `try_save_config` writes are immediately reflected without an
        // app restart. In tests (store_path empty) there is no real global
        // config file, so fall back to the in-memory value that the test seeds.
        let global_templates = if !self.store_path.as_os_str().is_empty() {
            let fresh = crate::extension::load_global_extension_config().prompt_templates;
            self.config.extension.prompt_templates = fresh.clone();
            fresh
        } else {
            self.config.extension.prompt_templates.clone()
        };

        let templates = merge_prompt_library_entries(
            &self.store.prompt_templates,
            &global_templates,
            &project,
            &worktree_templates,
        );

        let filtered: Vec<usize> = (0..templates.len()).collect();

        let selected = focus
            .and_then(|(name, source)| {
                filtered
                    .iter()
                    .position(|&idx| templates[idx].template.name == name && templates[idx].source == source)
            })
            .unwrap_or(0);

        self.mode = AppMode::PromptLibrary(PromptLibraryState {
            templates,
            filtered,
            query: String::new(),
            search_active: false,
            selected,
            confirm_delete: false,
            pending_export: false,
            from_view,
        });
        self.message = None;
    }

    pub fn prompt_library_select_next(&mut self) {
        if let AppMode::PromptLibrary(state) = &mut self.mode {
            if state.filtered.is_empty() {
                return;
            }
            state.confirm_delete = false;
            state.pending_export = false;
            state.selected = (state.selected + 1) % state.filtered.len();
        }
    }

    pub fn prompt_library_select_prev(&mut self) {
        if let AppMode::PromptLibrary(state) = &mut self.mode {
            if state.filtered.is_empty() {
                return;
            }
            state.confirm_delete = false;
            state.pending_export = false;
            state.selected = state
                .selected
                .checked_sub(1)
                .unwrap_or(state.filtered.len().saturating_sub(1));
        }
    }

    /// Recompute the filtered index list from the current query and
    /// clamp the selection. Fuzzy-matches against both name and body.
    pub fn prompt_library_filter(&mut self) {
        if let AppMode::PromptLibrary(state) = &mut self.mode {
            let mut scored: Vec<(usize, usize)> = state
                .templates
                .iter()
                .enumerate()
                .filter_map(|(idx, entry)| {
                    let name_score =
                        crate::app::util::fuzzy_match_score(&entry.template.name, &state.query);
                    let body_score =
                        crate::app::util::fuzzy_match_score(&entry.template.body, &state.query);
                    let best = match (name_score, body_score) {
                        (Some(a), Some(b)) => a.min(b),
                        (Some(a), None) => a,
                        (None, Some(b)) => b,
                        (None, None) => return None,
                    };
                    Some((idx, best))
                })
                .collect();

            scored.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
            state.filtered = scored.into_iter().map(|(idx, _)| idx).collect();
            if state.selected >= state.filtered.len() {
                state.selected = state.filtered.len().saturating_sub(1);
            }
            state.confirm_delete = false;
            state.pending_export = false;
        }
    }

    // ── Injection ────────────────────────────────────────────────

    /// Inject the selected template. Templates with no `{{slots}}` render
    /// and deliver verbatim; templates with slots enter the fill-in flow
    /// to collect a value for each slot first.
    pub fn inject_selected_template(&mut self) -> Result<()> {
        let (template, from_view) = match &self.mode {
            AppMode::PromptLibrary(state) => match state.selected_entry() {
                Some(entry) => (entry.template.clone(), state.from_view.clone()),
                None => {
                    self.message = Some("No template to inject".into());
                    return Ok(());
                }
            },
            _ => return Ok(()),
        };

        let placeholders = resolve_placeholders(&template);
        if placeholders.is_empty() {
            let rendered = render_template(&template.body, &[]);
            return self.deliver_prompt(rendered, from_view);
        }

        self.start_placeholder_fill(template, placeholders, from_view);
        Ok(())
    }

    /// Enter the fill-in flow on the first slot, seeding each field with its
    /// default value.
    fn start_placeholder_fill(
        &mut self,
        template: PromptTemplate,
        placeholders: Vec<PromptPlaceholder>,
        from_view: Option<ViewState>,
    ) {
        let values: Vec<String> = placeholders.iter().map(placeholder_default).collect();
        let mut state = PlaceholderFillState {
            template,
            placeholders,
            values,
            current: 0,
            input: TextEditor::new(String::new()),
            select_index: 0,
            from_view,
        };
        // Seed the first field's editor + select highlight.
        state.enter(0);
        self.mode = AppMode::PlaceholderFill(state);
        self.message = None;
    }

    /// Save the current field, then advance to the next slot — or submit
    /// when already on the last slot.
    pub fn placeholder_fill_next(&mut self) -> Result<()> {
        let at_last = match &mut self.mode {
            AppMode::PlaceholderFill(state) => {
                state.commit_current();
                state.current + 1 >= state.placeholders.len()
            }
            _ => return Ok(()),
        };
        if at_last {
            return self.submit_placeholder_fill();
        }
        if let AppMode::PlaceholderFill(state) = &mut self.mode {
            let next = state.current + 1;
            state.enter(next);
        }
        Ok(())
    }

    /// Save the current field, then step back to the previous slot.
    pub fn placeholder_fill_prev(&mut self) {
        if let AppMode::PlaceholderFill(state) = &mut self.mode {
            state.commit_current();
            if state.current > 0 {
                let prev = state.current - 1;
                state.enter(prev);
            }
        }
    }

    /// Substitute every slot value into the body and deliver the result.
    /// Empty `required` slots block delivery: the cursor jumps to the first
    /// such slot with a message.
    pub fn submit_placeholder_fill(&mut self) -> Result<()> {
        let (rendered, from_view) = match &mut self.mode {
            AppMode::PlaceholderFill(state) => {
                state.commit_current();

                let missing = state
                    .placeholders
                    .iter()
                    .zip(state.values.iter())
                    .position(|(p, v)| p.required && v.trim().is_empty());
                if let Some(missing) = missing {
                    state.enter(missing);
                    let label = placeholder_label(&state.placeholders[missing]).to_string();
                    self.message = Some(format!("\"{label}\" is required"));
                    return Ok(());
                }

                let pairs: Vec<(String, String)> = state
                    .placeholders
                    .iter()
                    .zip(state.values.iter())
                    .map(|(p, v)| (p.key.clone(), v.clone()))
                    .collect();
                (render_template(&state.template.body, &pairs), state.from_view.clone())
            }
            _ => return Ok(()),
        };
        self.deliver_prompt(rendered, from_view)
    }

    /// Abandon the fill-in flow and return to the picker it was launched from.
    pub fn cancel_placeholder_fill(&mut self) {
        let from_view = match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::PlaceholderFill(state) => state.from_view,
            other => {
                self.mode = other;
                return;
            }
        };
        self.open_prompt_library(from_view);
    }

    /// Shared delivery step. Seeds the compose box when compose
    /// interception is on (so the user reviews/edits before sending);
    /// otherwise pastes into the agent window with no trailing Enter, so
    /// nothing is sent automatically. With no session context, copies to
    /// the clipboard instead.
    pub fn deliver_prompt(&mut self, rendered: String, from_view: Option<ViewState>) -> Result<()> {
        let Some(view) = from_view else {
            match crate::app::util::copy_to_clipboard(&rendered) {
                Ok(()) => self.push_toast_success("Copied prompt to clipboard"),
                Err(e) => self.push_toast_warning(format!("Clipboard error: {e}")),
            }
            self.mode = AppMode::Normal;
            return Ok(());
        };

        if self.compose_intercept_active(&view) {
            self.mode = AppMode::Viewing(view);
            self.open_compose_seeded(rendered)?;
        } else {
            self.tmux.paste_text(&view.session, &view.window, &rendered)?;
            self.mode = AppMode::Viewing(view);
            self.push_toast_success("Pasted prompt (not sent)");
        }
        Ok(())
    }

    // ── CRUD ─────────────────────────────────────────────────────

    /// Open a blank editor for a new template, returning to the picker
    /// on save/cancel.
    pub fn start_new_prompt_template(&mut self) {
        let return_to = std::mem::replace(&mut self.mode, AppMode::Normal);
        self.mode = AppMode::PromptEditor(PromptEditorState {
            editing_id: None,
            editing_source: PromptSource::User,
            original_template: None,
            name: String::new(),
            name_field_active: true,
            editor: TextEditor::with_vim(String::new()),
            return_to: Box::new(return_to),
        });
    }

    /// Open the editor seeded with the selected template's name + body.
    pub fn start_edit_selected_template(&mut self) {
        let entry = match &self.mode {
            AppMode::PromptLibrary(state) => state.selected_entry().cloned(),
            _ => None,
        };
        let Some(entry) = entry else {
            self.message = Some("No template to edit".into());
            return;
        };

        let original_template = if entry.source.is_deletable() {
            None
        } else {
            Some(entry.template.clone())
        };
        let return_to = std::mem::replace(&mut self.mode, AppMode::Normal);
        self.mode = AppMode::PromptEditor(PromptEditorState {
            editing_id: Some(entry.template.id.clone()),
            editing_source: entry.source,
            original_template,
            name: entry.template.name.clone(),
            name_field_active: false,
            editor: TextEditor::with_vim(entry.template.body.clone()),
            return_to: Box::new(return_to),
        });
    }

    /// Capture the current compose buffer as a new template, opening the
    /// editor pre-seeded. The compose draft is preserved so the user
    /// lands back on their unsent text after saving.
    pub fn save_current_prompt_as_template(&mut self) {
        let text = match &self.mode {
            AppMode::Compose(state) => state.editor.text().trim().to_string(),
            _ => return,
        };
        if text.is_empty() {
            self.push_toast_warning("Nothing to save — compose box is empty");
            return;
        }

        // Closing compose stashes a draft and drops us back on the view;
        // capture that Viewing mode as the editor's return target.
        self.cancel_compose();
        let return_to = std::mem::replace(&mut self.mode, AppMode::Normal);
        self.mode = AppMode::PromptEditor(PromptEditorState {
            editing_id: None,
            editing_source: PromptSource::User,
            original_template: None,
            name: String::new(),
            name_field_active: true,
            editor: TextEditor::with_vim(text),
            return_to: Box::new(return_to),
        });
    }

    /// Validate and persist the editor's contents (insert or update),
    /// then return to wherever the editor was opened from.
    pub fn submit_prompt_editor(&mut self) -> Result<()> {
        let state = match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::PromptEditor(state) => state,
            other => {
                self.mode = other;
                return Ok(());
            }
        };

        let name = state.name.trim().to_string();
        let body = state.editor.text().trim().to_string();
        if name.is_empty() {
            self.message = Some("Name cannot be empty".into());
            self.mode = AppMode::PromptEditor(state);
            return Ok(());
        }
        if body.is_empty() {
            self.message = Some("Prompt body cannot be empty".into());
            self.mode = AppMode::PromptEditor(state);
            return Ok(());
        }

        // Extract from_view before consuming state (needed by config resolvers).
        let from_view: Option<ViewState> = match state.return_to.as_ref() {
            AppMode::PromptLibrary(picker) => picker.from_view.clone(),
            _ => None,
        };
        // Capture the final name before the match arms consume it (User arms move name).
        let focus_name = name.clone();

        match state.editing_source {
            PromptSource::User => {
                match &state.editing_id {
                    Some(id) => {
                        if let Some(template) = self
                            .store
                            .prompt_templates
                            .iter_mut()
                            .find(|t| &t.id == id)
                        {
                            template.name = name;
                            template.body = body;
                            template.updated_at = Utc::now();
                        }
                    }
                    None => {
                        self.store
                            .prompt_templates
                            .push(PromptTemplate::new(name, body));
                    }
                }
                self.save()?;
            }
            PromptSource::Global => {
                let orig = state.original_template.as_ref().expect("config edit always has original");
                let updated = PromptTemplate { name: name.clone(), body, updated_at: Utc::now(), ..orig.clone() };
                if orig.name != name {
                    self.config.extension.prompt_templates.retain(|t| t.name != orig.name);
                }
                upsert_template(&mut self.config.extension.prompt_templates, updated);
                if let Err(e) = self.try_save_config() {
                    self.push_toast_warning(format!("Failed to save global config: {e}"));
                    self.return_from_prompt_editor(*state.return_to);
                    return Ok(());
                }
            }
            PromptSource::Project => {
                let orig = state.original_template.as_ref().expect("config edit always has original");
                let updated = PromptTemplate { name: name.clone(), body, updated_at: Utc::now(), ..orig.clone() };
                let Some(repo) = self.resolve_export_repo(from_view.as_ref()) else {
                    self.push_toast_warning("No project repo — can't save");
                    self.return_from_prompt_editor(*state.return_to);
                    return Ok(());
                };
                if orig.name != name {
                    remove_template_from_config(&repo, &orig.name)?;
                }
                export_template_to_project_config(&repo, &updated)?;
            }
            PromptSource::Worktree => {
                let orig = state.original_template.as_ref().expect("config edit always has original");
                let updated = PromptTemplate { name: name.clone(), body, updated_at: Utc::now(), ..orig.clone() };
                let Some(workdir) = self.resolve_worktree_dir(from_view.as_ref()) else {
                    self.push_toast_warning("No worktree — can't save");
                    self.return_from_prompt_editor(*state.return_to);
                    return Ok(());
                };
                if orig.name != name {
                    remove_template_from_config(&workdir, &orig.name)?;
                }
                export_template_to_project_config(&workdir, &updated)?;
            }
        }
        let focus = (focus_name, state.editing_source);
        self.push_toast_success("Saved prompt");
        self.return_from_prompt_editor_focused(*state.return_to, Some(focus));
        Ok(())
    }

    pub fn cancel_prompt_editor(&mut self) {
        if let AppMode::PromptEditor(state) =
            std::mem::replace(&mut self.mode, AppMode::Normal)
        {
            self.return_from_prompt_editor(*state.return_to);
        }
    }

    /// Land back on the editor's origin. When returning to the picker,
    /// rebuild it so a new/edited/deleted template is reflected.
    fn return_from_prompt_editor(&mut self, return_to: AppMode) {
        self.return_from_prompt_editor_focused(return_to, None);
    }

    /// Like `return_from_prompt_editor` but scrolls the rebuilt picker to
    /// the entry with the given (name, source) so the user immediately sees
    /// the result of a create/edit.
    fn return_from_prompt_editor_focused(
        &mut self,
        return_to: AppMode,
        focus: Option<(String, PromptSource)>,
    ) {
        match return_to {
            AppMode::PromptLibrary(picker) => {
                let focus_ref = focus.as_ref().map(|(n, s)| (n.as_str(), *s));
                self.rebuild_prompt_library(picker.from_view, focus_ref);
            }
            other => self.mode = other,
        }
    }

    /// Delete the selected user template (two-step: the handler arms
    /// `confirm_delete` on the first `d`, this runs on the second).
    pub fn delete_selected_template(&mut self) -> Result<()> {
        let (entry, from_view) = match &self.mode {
            AppMode::PromptLibrary(state) => (state.selected_entry().cloned(), state.from_view.clone()),
            _ => return Ok(()),
        };
        let Some(entry) = entry else {
            return Ok(());
        };
        if !entry.source.is_deletable() {
            self.push_toast_warning("Config template can't be deleted here — remove it from the config file");
            return Ok(());
        }

        let id = entry.template.id.clone();
        self.store.prompt_templates.retain(|template| template.id != id);
        self.save()?;
        self.open_prompt_library(from_view);
        self.push_toast_success("Deleted prompt");
        Ok(())
    }

    /// Copy the selected template into the user library as an editable
    /// duplicate. Useful in phase 3 for read-only `Global` / `Project`
    /// templates; harmless for `User` ones.
    pub fn duplicate_selected_template_to_user(&mut self) -> Result<()> {
        let (entry, from_view) = match &self.mode {
            AppMode::PromptLibrary(state) => (state.selected_entry().cloned(), state.from_view.clone()),
            _ => return Ok(()),
        };
        let Some(entry) = entry else {
            self.message = Some("No template to duplicate".into());
            return Ok(());
        };

        let mut template = entry.template.clone();
        let now = Utc::now();
        template.id = uuid::Uuid::new_v4().to_string();
        template.name = format!("{} (copy)", template.name);
        template.created_at = now;
        template.updated_at = now;
        let copy_name = template.name.clone();
        self.store.prompt_templates.push(template);
        self.save()?;
        self.rebuild_prompt_library(from_view, Some((&copy_name, PromptSource::User)));
        self.push_toast_success("Duplicated to your library");
        Ok(())
    }

    // ── Export to declarative config ─────────────────────────────

    /// Export the selected template into the global or project
    /// `config.json` so it becomes a version-controllable, shareable
    /// entry. Existing config entries with the same name are replaced.
    pub fn export_selected_template(&mut self, target: PromptExportTarget) -> Result<()> {
        let (template, from_view) = match &self.mode {
            AppMode::PromptLibrary(state) => {
                (state.selected_entry().map(|e| e.template.clone()), state.from_view.clone())
            }
            _ => return Ok(()),
        };
        if let AppMode::PromptLibrary(state) = &mut self.mode {
            state.pending_export = false;
        }

        let Some(template) = template else {
            self.message = Some("No template to export".into());
            return Ok(());
        };

        let mut success: Option<(String, PromptSource)> = None;
        match target {
            PromptExportTarget::Global => {
                upsert_template(&mut self.config.extension.prompt_templates, template.clone());
                match self.try_save_config() {
                    Ok(()) => {
                        self.log_info("prompt-library", format!("exported \"{}\" to global config", template.name));
                        success = Some((
                            format!("Exported \"{}\" to global config", template.name),
                            PromptSource::Global,
                        ));
                    }
                    Err(e) => {
                        // Roll back the in-memory upsert so the list stays consistent.
                        self.config.extension.prompt_templates.retain(|t| t.name != template.name);
                        self.push_toast_warning(format!("Export failed: {e}"));
                        return Ok(());
                    }
                }
            }
            PromptExportTarget::Project => {
                let Some(repo) = self.resolve_export_repo(from_view.as_ref()) else {
                    self.push_toast_warning(
                        "No project to export to — open a session or run AMF in a repo",
                    );
                    return Ok(());
                };
                match export_template_to_project_config(&repo, &template) {
                    Ok(path) => {
                        let short = crate::app::util::shorten_path(&path);
                        self.log_info("prompt-library", format!("exported \"{}\" to {short}", template.name));
                        success = Some((
                            format!("Exported \"{}\" to {short}", template.name),
                            PromptSource::Project,
                        ));
                    }
                    Err(e) => self.push_toast_warning(format!("Export failed: {e}")),
                }
            }
            PromptExportTarget::Worktree => {
                let Some(workdir) = self.resolve_worktree_dir(from_view.as_ref()) else {
                    self.push_toast_warning(
                        "No worktree to export to — open a feature session first",
                    );
                    return Ok(());
                };
                match export_template_to_project_config(&workdir, &template) {
                    Ok(path) => {
                        let short = crate::app::util::shorten_path(&path);
                        self.log_info("prompt-library", format!("exported \"{}\" to {short}", template.name));
                        success = Some((
                            format!("Exported \"{}\" to {short}", template.name),
                            PromptSource::Worktree,
                        ));
                    }
                    Err(e) => self.push_toast_warning(format!("Export failed: {e}")),
                }
            }
        }
        if let Some((msg, source)) = success {
            self.push_toast_success(msg.clone());
            self.rebuild_prompt_library(from_view, Some((&template.name, source)));
            self.message = Some(msg);
        }
        Ok(())
    }

    /// Resolve the repo whose `.amf/config.json` project templates should
    /// appear in the picker: the viewed feature's project repo when opened
    /// from a session, else the currently selected project's repo. Never
    /// falls back to the working directory, so the picker only shows project
    /// templates when there is real project context (and unit tests stay
    /// deterministic). Used by both display and export so they always agree.
    fn resolve_library_repo(&self, from_view: Option<&ViewState>) -> Option<PathBuf> {
        if let Some(view) = from_view
            && let Some(project) = self
                .store
                .projects
                .iter()
                .find(|project| project.name == view.project_name)
        {
            return Some(project.repo.clone());
        }
        let pi = match &self.selection {
            Selection::Project(pi)
            | Selection::Feature(pi, _)
            | Selection::Session(pi, _, _) => *pi,
        };
        self.store.projects.get(pi).map(|project| project.repo.clone())
    }

    /// Resolve the repo to export a project template into. Delegates to
    /// `resolve_library_repo` so display and export always target the same
    /// main-repo root, regardless of whether AMF was launched from inside a
    /// worktree.
    fn resolve_export_repo(&self, from_view: Option<&ViewState>) -> Option<PathBuf> {
        self.resolve_library_repo(from_view)
    }

    /// Resolve the working directory for a worktree export: the active
    /// feature's `workdir` when opened from a session, else the selected
    /// feature's `workdir` on the dashboard. Returns `None` when no feature
    /// context is available.
    fn resolve_worktree_dir(&self, from_view: Option<&ViewState>) -> Option<PathBuf> {
        if let Some(view) = from_view {
            return self
                .store
                .projects
                .iter()
                .find(|p| p.name == view.project_name)
                .and_then(|p| p.features.iter().find(|f| f.name == view.feature_name))
                .map(|f| f.workdir.clone());
        }
        self.selected_feature().map(|(_, f)| f.workdir.clone())
    }
}

/// Read the `prompt_templates` declared in `{repo}/.amf/config.json`.
/// Returns an empty list when the file is absent or unparseable, mirroring
/// the tolerant loading in `merge_project_extension_config`.
fn load_project_prompt_templates(repo: &Path) -> Vec<PromptTemplate> {
    let path = repo.join(".amf").join("config.json");
    if !path.exists() {
        return Vec::new();
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<crate::extension::ExtensionConfig>(&s).ok())
        .map(|config| config.prompt_templates)
        .unwrap_or_default()
}

/// Merge the four template sources into one source-tagged list for the
/// picker: editable `User` entries first, then `Worktree`, then read-only
/// `Project`, then `Global`.
///
/// No cross-source deduplication is applied. Each source shows its entries
/// independently so the user can see exactly which scopes hold a copy of a
/// given name and manage them individually. (The AMF extension-config merge
/// hierarchy — Worktree > Project > Global — applies when *consuming* the
/// config for feature presets etc., but is wrong here: silently hiding an
/// exported template because a higher-priority copy exists makes the export
/// appear to have failed.)
fn merge_prompt_library_entries(
    user: &[PromptTemplate],
    global: &[PromptTemplate],
    project: &[PromptTemplate],
    worktree: &[PromptTemplate],
) -> Vec<PromptLibraryEntry> {
    let mut entries: Vec<PromptLibraryEntry> = Vec::new();
    for template in user {
        entries.push(PromptLibraryEntry { template: template.clone(), source: PromptSource::User });
    }
    for template in worktree {
        entries.push(PromptLibraryEntry { template: template.clone(), source: PromptSource::Worktree });
    }
    for template in project {
        entries.push(PromptLibraryEntry { template: template.clone(), source: PromptSource::Project });
    }
    for template in global {
        entries.push(PromptLibraryEntry { template: template.clone(), source: PromptSource::Global });
    }
    entries
}

/// Build the ordered list of slots to fill for a template: the distinct
/// `{{key}}` tokens that appear in the body, in first-seen order. Each key
/// resolves to its explicit `PromptPlaceholder` definition when the template
/// declares one (so config-authored label / kind / default / required apply),
/// otherwise a synthesized `Text` slot. Explicit placeholders whose key never
/// appears in the body are skipped — filling them would substitute nothing.
fn resolve_placeholders(template: &PromptTemplate) -> Vec<PromptPlaceholder> {
    infer_placeholder_slots(&template.body)
        .into_iter()
        .map(|slot| {
            // An explicit config-authored definition always wins.
            if let Some(explicit) = template.placeholders.iter().find(|p| p.key == slot.key) {
                return explicit.clone();
            }
            // A slot with inline options (`{{a|b}}` / `{{label: a|b}}`) becomes
            // a Select; a bare `{{key}}` becomes a free-text slot. A labelled
            // menu carries its label so the fill flow shows a heading.
            let kind = if slot.options.is_empty() {
                PlaceholderKind::Text { default: None }
            } else {
                PlaceholderKind::Select {
                    options: slot.options,
                }
            };
            PromptPlaceholder {
                key: slot.key,
                label: slot.label,
                kind,
                required: false,
            }
        })
        .collect()
}

/// The value a slot's field is seeded with. `Text` / `MultiLine` use their
/// configured default (empty when none); `Select` (phase 3) degrades to its
/// first option so a hand-authored config template still injects.
fn placeholder_default(p: &PromptPlaceholder) -> String {
    match &p.kind {
        PlaceholderKind::Text { default } | PlaceholderKind::MultiLine { default } => {
            default.clone().unwrap_or_default()
        }
        PlaceholderKind::Select { options } => options.first().cloned().unwrap_or_default(),
    }
}

/// The prompt shown for a slot in the fill-in flow: its explicit `label`, the
/// `key` for text slots, or a generic prompt for an unlabelled menu.
fn placeholder_label(p: &PromptPlaceholder) -> &str {
    p.display_label()
}

/// Insert or replace a template in a config list, matching by name.
fn upsert_template(list: &mut Vec<PromptTemplate>, template: PromptTemplate) {
    if let Some(existing) = list.iter_mut().find(|t| t.name == template.name) {
        *existing = template;
    } else {
        list.push(template);
    }
}

/// Write `template` into `{repo}/.amf/config.json`'s `prompt_templates`
/// array, replacing any same-name entry and preserving all other keys in
/// the file. Returns the path written.
fn export_template_to_project_config(repo: &Path, template: &PromptTemplate) -> Result<PathBuf> {
    let dir = repo.join(".amf");
    let path = dir.join("config.json");
    std::fs::create_dir_all(&dir)?;

    let mut root: serde_json::Value = if path.exists() {
        let contents = std::fs::read_to_string(&path)?;
        serde_json::from_str(&contents).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };
    if !root.is_object() {
        root = serde_json::json!({});
    }

    let obj = root.as_object_mut().expect("root is an object");
    let arr = obj
        .entry("prompt_templates")
        .or_insert_with(|| serde_json::json!([]));
    if !arr.is_array() {
        *arr = serde_json::json!([]);
    }
    let arr = arr.as_array_mut().expect("prompt_templates is an array");

    let entry = serde_json::to_value(template)?;
    if let Some(slot) = arr.iter_mut().find(|value| {
        value.get("name").and_then(|n| n.as_str()) == Some(template.name.as_str())
    }) {
        *slot = entry;
    } else {
        arr.push(entry);
    }

    std::fs::write(&path, serde_json::to_string_pretty(&root)? + "\n")?;
    Ok(path)
}

/// Remove the entry matching `name` from `{repo}/.amf/config.json`. Used
/// when renaming a config-source template so the old name doesn't linger.
fn remove_template_from_config(repo: &Path, name: &str) -> Result<()> {
    let path = repo.join(".amf").join("config.json");
    if !path.exists() {
        return Ok(());
    }
    let contents = std::fs::read_to_string(&path)?;
    let mut root: serde_json::Value =
        serde_json::from_str(&contents).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(arr) = root.get_mut("prompt_templates").and_then(|v| v.as_array_mut()) {
        arr.retain(|v| v.get("name").and_then(|n| n.as_str()) != Some(name));
    }
    std::fs::write(&path, serde_json::to_string_pretty(&root)? + "\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::ExtensionConfig;

    fn text_placeholder(key: &str, default: Option<&str>, required: bool) -> PromptPlaceholder {
        PromptPlaceholder {
            key: key.to_string(),
            label: None,
            kind: PlaceholderKind::Text {
                default: default.map(ToString::to_string),
            },
            required,
        }
    }

    #[test]
    fn resolve_infers_text_slots_in_body_order() {
        let template = PromptTemplate::new(
            "t".to_string(),
            "Fix {{area}} in {{file}}, twice {{area}}".to_string(),
        );
        let slots = resolve_placeholders(&template);
        let keys: Vec<&str> = slots.iter().map(|p| p.key.as_str()).collect();
        // Distinct keys, first-seen order, no duplicate for the repeated slot.
        assert_eq!(keys, vec!["area", "file"]);
        // Inferred slots are plain Text with no default and not required.
        assert!(matches!(slots[0].kind, PlaceholderKind::Text { default: None }));
        assert!(!slots[0].required);
    }

    #[test]
    fn resolve_uses_explicit_definition_when_present() {
        let mut template =
            PromptTemplate::new("t".to_string(), "Hello {{name}}".to_string());
        template.placeholders = vec![PromptPlaceholder {
            key: "name".to_string(),
            label: Some("Your name".to_string()),
            kind: PlaceholderKind::Text {
                default: Some("Ada".to_string()),
            },
            required: true,
        }];
        let slots = resolve_placeholders(&template);
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].label.as_deref(), Some("Your name"));
        assert!(slots[0].required);
        assert_eq!(placeholder_default(&slots[0]), "Ada");
    }

    #[test]
    fn resolve_makes_select_from_inline_options() {
        let template = PromptTemplate::new(
            "t".to_string(),
            "Deploy {{env: dev|staging|prod}} as {{user}}".to_string(),
        );
        let slots = resolve_placeholders(&template);
        assert_eq!(slots.len(), 2);
        // Labelled menu → Select with those options, carrying the heading.
        assert_eq!(slots[0].label.as_deref(), Some("env"));
        match &slots[0].kind {
            PlaceholderKind::Select { options } => {
                assert_eq!(options, &vec!["dev".to_string(), "staging".to_string(), "prod".to_string()]);
            }
            other => panic!("expected Select, got {other:?}"),
        }
        // Bare slot → Text.
        assert!(matches!(slots[1].kind, PlaceholderKind::Text { default: None }));
    }

    #[test]
    fn resolve_makes_bare_menu_select_with_every_option() {
        // No label: every `|` segment is selectable (the reported bug fix).
        let template =
            PromptTemplate::new("t".to_string(), "Use {{dev|staging|prod}}".to_string());
        let slots = resolve_placeholders(&template);
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].label, None);
        match &slots[0].kind {
            PlaceholderKind::Select { options } => assert_eq!(
                options,
                &vec!["dev".to_string(), "staging".to_string(), "prod".to_string()]
            ),
            other => panic!("expected Select, got {other:?}"),
        }
        // The first option is the seeded default, so it is selectable.
        assert_eq!(placeholder_default(&slots[0]), "dev");
    }

    #[test]
    fn resolve_explicit_definition_overrides_inline_options() {
        // A config-authored placeholder wins even when the body has `|options`.
        let mut template = PromptTemplate::new(
            "t".to_string(),
            "Deploy {{env: dev|prod}}".to_string(),
        );
        template.placeholders = vec![PromptPlaceholder {
            key: "env".to_string(),
            label: Some("Environment".to_string()),
            kind: PlaceholderKind::Text {
                default: Some("dev".to_string()),
            },
            required: true,
        }];
        let slots = resolve_placeholders(&template);
        assert_eq!(slots.len(), 1);
        assert!(matches!(slots[0].kind, PlaceholderKind::Text { .. }));
        assert!(slots[0].required);
    }

    #[test]
    fn resolve_ignores_explicit_placeholder_absent_from_body() {
        let mut template = PromptTemplate::new("t".to_string(), "no slots here".to_string());
        template.placeholders = vec![text_placeholder("unused", Some("x"), false)];
        assert!(resolve_placeholders(&template).is_empty());
    }

    #[test]
    fn fill_renders_collected_values_into_body() {
        // End-to-end of the substitution step: resolved slots + values →
        // render_template produces the final prompt.
        let template = PromptTemplate::new(
            "t".to_string(),
            "Fix {{area}} in {{file}}".to_string(),
        );
        let slots = resolve_placeholders(&template);
        let values = ["auth", "login.rs"];
        let pairs: Vec<(String, String)> = slots
            .iter()
            .zip(values.iter())
            .map(|(p, v)| (p.key.clone(), v.to_string()))
            .collect();
        assert_eq!(
            render_template(&template.body, &pairs),
            "Fix auth in login.rs"
        );
    }

    #[test]
    fn project_export_writes_and_roundtrips() {
        let tmp = tempfile::TempDir::new().unwrap();
        let template = PromptTemplate::new("Fix bug".to_string(), "Fix {{area}}".to_string());

        let path = export_template_to_project_config(tmp.path(), &template).unwrap();
        assert_eq!(path, tmp.path().join(".amf").join("config.json"));

        // Re-read through the real config parser to confirm it loads.
        let contents = std::fs::read_to_string(&path).unwrap();
        let config: ExtensionConfig = serde_json::from_str(&contents).unwrap();
        assert_eq!(config.prompt_templates.len(), 1);
        assert_eq!(config.prompt_templates[0].name, "Fix bug");
        assert_eq!(config.prompt_templates[0].body, "Fix {{area}}");
    }

    #[test]
    fn project_export_replaces_same_name_and_preserves_other_keys() {
        let tmp = tempfile::TempDir::new().unwrap();
        let amf = tmp.path().join(".amf");
        std::fs::create_dir_all(&amf).unwrap();
        // Pre-existing config with an unrelated key and a same-name entry.
        std::fs::write(
            amf.join("config.json"),
            r#"{ "allowed_agents": ["claude"], "prompt_templates": [
                { "name": "Review", "body": "old body" }
            ] }"#,
        )
        .unwrap();

        let mut updated = PromptTemplate::new("Review".to_string(), "new body".to_string());
        updated.description = Some("desc".to_string());
        export_template_to_project_config(tmp.path(), &updated).unwrap();

        let contents = std::fs::read_to_string(amf.join("config.json")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&contents).unwrap();
        // Unrelated key preserved.
        assert_eq!(value["allowed_agents"][0], "claude");
        // Same-name entry replaced (not duplicated).
        let templates = value["prompt_templates"].as_array().unwrap();
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0]["body"], "new body");
    }

    #[test]
    fn merge_orders_user_then_project_then_global() {
        let user = vec![PromptTemplate::new("u".to_string(), "1".to_string())];
        let project = vec![PromptTemplate::new("p".to_string(), "2".to_string())];
        let global = vec![PromptTemplate::new("g".to_string(), "3".to_string())];

        let entries = merge_prompt_library_entries(&user, &global, &project, &[]);
        let tagged: Vec<(&str, PromptSource)> = entries
            .iter()
            .map(|e| (e.template.name.as_str(), e.source))
            .collect();
        assert_eq!(
            tagged,
            vec![
                ("u", PromptSource::User),
                ("p", PromptSource::Project),
                ("g", PromptSource::Global),
            ]
        );
    }

    #[test]
    fn merge_shows_all_sources_independently_no_dedup() {
        let user = vec![];
        let project = vec![PromptTemplate::new("dup".to_string(), "proj".to_string())];
        let global = vec![
            PromptTemplate::new("dup".to_string(), "glob".to_string()),
            PromptTemplate::new("only-global".to_string(), "g".to_string()),
        ];

        let entries = merge_prompt_library_entries(&user, &global, &project, &[]);
        // Both Project and Global copies of "dup" are shown — no cross-source dedup.
        let dup: Vec<_> = entries.iter().filter(|e| e.template.name == "dup").collect();
        assert_eq!(dup.len(), 2);
        assert!(dup.iter().any(|e| e.source == PromptSource::Project && e.template.body == "proj"));
        assert!(dup.iter().any(|e| e.source == PromptSource::Global && e.template.body == "glob"));
        // The global-only entry still shows.
        assert!(
            entries
                .iter()
                .any(|e| e.template.name == "only-global" && e.source == PromptSource::Global)
        );
    }

    #[test]
    fn merge_keeps_user_copy_alongside_exported_global() {
        // An exported template exists as both an editable User entry and a
        // read-only Global one; both should be listed.
        let user = vec![PromptTemplate::new("shared".to_string(), "u".to_string())];
        let global = vec![PromptTemplate::new("shared".to_string(), "g".to_string())];
        let project = vec![];

        let entries = merge_prompt_library_entries(&user, &global, &project, &[]);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].source, PromptSource::User);
        assert_eq!(entries[1].source, PromptSource::Global);
    }

    #[test]
    fn load_project_templates_reads_exported_config() {
        let tmp = tempfile::TempDir::new().unwrap();
        let template = PromptTemplate::new("Shipped".to_string(), "body".to_string());
        export_template_to_project_config(tmp.path(), &template).unwrap();

        let loaded = load_project_prompt_templates(tmp.path());
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "Shipped");

        // Missing config dir yields an empty list, not an error.
        let empty = tempfile::TempDir::new().unwrap();
        assert!(load_project_prompt_templates(empty.path()).is_empty());
    }

    #[test]
    fn upsert_replaces_by_name() {
        let mut list = vec![PromptTemplate::new("a".to_string(), "1".to_string())];
        upsert_template(&mut list, PromptTemplate::new("a".to_string(), "2".to_string()));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].body, "2");

        upsert_template(&mut list, PromptTemplate::new("b".to_string(), "3".to_string()));
        assert_eq!(list.len(), 2);
    }

    // ── App-level round-trip: dashboard export → picker shows entry ───
    //
    // Regression for the bug where resolve_export_repo fell back to
    // detect_repo_path() (the CWD git root), which points at the worktree
    // dir when AMF runs inside one, while resolve_library_repo always uses
    // project.repo (the main repo root). Exporting from the dashboard (no
    // from_view) then reopening the picker would find nothing because the
    // export landed in the wrong directory.

    fn project_store_at(repo: &std::path::Path) -> crate::project::ProjectStore {
        use crate::project::{AgentKind, Project, ProjectStore};
        use chrono::Utc;
        let project = Project {
            id: "p1".to_string(),
            name: "my-project".to_string(),
            repo: repo.to_path_buf(),
            collapsed: false,
            features: vec![],
            created_at: Utc::now(),
            preferred_agent: AgentKind::Claude,
            is_git: false,
        };
        ProjectStore {
            version: 5,
            projects: vec![project],
            session_bookmarks: vec![],
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
            extra: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn dashboard_export_then_reopen_shows_project_template() {
        use crate::app::{App, AppMode, PromptExportTarget};
        use crate::traits::{MockTmuxOps, MockWorktreeOps};

        let repo = tempfile::TempDir::new().unwrap();
        let store = project_store_at(repo.path());
        let mut app = App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );

        // Seed a user template and open the library from the dashboard
        // (no view context — this is the case that previously broke).
        app.store
            .prompt_templates
            .push(PromptTemplate::new("My prompt".to_string(), "body".to_string()));
        app.open_prompt_library(None);

        // Export the template to the project config.
        app.export_selected_template(PromptExportTarget::Project)
            .unwrap();

        // Reopen the library (still no view) and assert the Project-source
        // entry is present — it should have been written to `repo/.amf/config.json`,
        // not to some worktree subdirectory.
        app.open_prompt_library(None);
        let AppMode::PromptLibrary(ref state) = app.mode else {
            panic!("expected PromptLibrary mode");
        };
        let project_entries: Vec<_> = state
            .templates
            .iter()
            .filter(|e| e.source == PromptSource::Project)
            .collect();
        assert_eq!(
            project_entries.len(),
            1,
            "exported template must appear as a Project entry"
        );
        assert_eq!(project_entries[0].template.name, "My prompt");

        // The config.json must be at the project's main repo root, not CWD.
        assert!(repo.path().join(".amf").join("config.json").exists());
    }

    #[test]
    fn inject_plain_template_skips_fill_flow() {
        use crate::app::{App, AppMode};
        use crate::traits::{MockTmuxOps, MockWorktreeOps};

        let repo = tempfile::TempDir::new().unwrap();
        let store = project_store_at(repo.path());
        let mut app = App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.store
            .prompt_templates
            .push(PromptTemplate::new("plain".to_string(), "no slots".to_string()));
        app.open_prompt_library(None);

        // No view context → delivery copies to clipboard and returns to Normal,
        // never entering the fill flow.
        app.inject_selected_template().unwrap();
        assert!(!matches!(app.mode, AppMode::PlaceholderFill(_)));
    }

    #[test]
    fn inject_slotted_template_enters_fill_then_renders_on_submit() {
        use crate::app::{App, AppMode};
        use crate::traits::{MockTmuxOps, MockWorktreeOps};

        let repo = tempfile::TempDir::new().unwrap();
        let store = project_store_at(repo.path());
        let mut app = App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.store.prompt_templates.push(PromptTemplate::new(
            "slotted".to_string(),
            "Fix {{area}} in {{file}}".to_string(),
        ));
        app.open_prompt_library(None);

        // Slots present → enter the fill flow on the first slot.
        app.inject_selected_template().unwrap();
        let AppMode::PlaceholderFill(ref state) = app.mode else {
            panic!("expected PlaceholderFill mode");
        };
        assert_eq!(state.placeholders.len(), 2);
        assert_eq!(state.current, 0);

        // Fill the first slot, advance, fill the second, then submit. With no
        // view context, submit delivers to the clipboard and leaves the fill
        // flow.
        if let AppMode::PlaceholderFill(state) = &mut app.mode {
            state.input = crate::editor::TextEditor::new("auth".to_string());
        }
        app.placeholder_fill_next().unwrap();
        let AppMode::PlaceholderFill(ref state) = app.mode else {
            panic!("expected to advance to second slot");
        };
        assert_eq!(state.current, 1);
        if let AppMode::PlaceholderFill(state) = &mut app.mode {
            state.input = crate::editor::TextEditor::new("login.rs".to_string());
        }
        // next() on the last slot submits.
        app.placeholder_fill_next().unwrap();
        assert!(!matches!(app.mode, AppMode::PlaceholderFill(_)));
    }

    #[test]
    fn select_slot_navigates_options_and_records_choice() {
        use crate::app::{App, AppMode};
        use crate::traits::{MockTmuxOps, MockWorktreeOps};

        let repo = tempfile::TempDir::new().unwrap();
        let store = project_store_at(repo.path());
        let mut app = App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        let mut template =
            PromptTemplate::new("sel".to_string(), "Use {{lang}}".to_string());
        template.placeholders = vec![PromptPlaceholder {
            key: "lang".to_string(),
            label: None,
            kind: PlaceholderKind::Select {
                options: vec!["rust".to_string(), "go".to_string(), "python".to_string()],
            },
            required: false,
        }];
        app.store.prompt_templates.push(template);
        app.open_prompt_library(None);

        app.inject_selected_template().unwrap();
        let AppMode::PlaceholderFill(ref state) = app.mode else {
            panic!("expected PlaceholderFill mode");
        };
        // Select slot starts highlighted on its default (first option).
        assert!(state.is_select());
        assert_eq!(state.select_index, 0);
        assert_eq!(state.values[0], "rust");

        // Navigate to "python" and record the choice (mirrors the handler).
        if let AppMode::PlaceholderFill(state) = &mut app.mode {
            state.select_next();
            state.select_next();
            assert_eq!(state.select_index, 2);
            state.commit_current();
            assert_eq!(state.values[0], "python");

            // Wrap-around backwards from index 0 lands on the last option.
            state.select_index = 0;
            state.select_prev();
            assert_eq!(state.select_index, 2);
        }
    }

    #[test]
    fn fill_blocks_submit_on_empty_required_slot() {
        use crate::app::{App, AppMode};
        use crate::traits::{MockTmuxOps, MockWorktreeOps};

        let repo = tempfile::TempDir::new().unwrap();
        let store = project_store_at(repo.path());
        let mut app = App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        let mut template =
            PromptTemplate::new("req".to_string(), "Hello {{name}}".to_string());
        template.placeholders = vec![PromptPlaceholder {
            key: "name".to_string(),
            label: Some("Name".to_string()),
            kind: PlaceholderKind::Text { default: None },
            required: true,
        }];
        app.store.prompt_templates.push(template);
        app.open_prompt_library(None);

        app.inject_selected_template().unwrap();
        // The field is empty; submitting must stay in the fill flow with a
        // required-field message rather than delivering.
        app.submit_placeholder_fill().unwrap();
        assert!(matches!(app.mode, AppMode::PlaceholderFill(_)));
        assert!(app.message.as_deref().unwrap_or("").contains("required"));
    }
}
