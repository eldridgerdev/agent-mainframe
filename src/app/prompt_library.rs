use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::Utc;

use super::*;
use crate::editor::TextEditor;
use crate::prompt_library::{PromptSource, PromptTemplate, render_template};

/// Where `export_selected_template` writes a user template.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptExportTarget {
    /// `~/.config/amf/config.json` (the `extension` block).
    Global,
    /// `{repo}/.amf/config.json`.
    Project,
}

impl App {
    // ── Picker: open / navigate / filter ─────────────────────────

    /// Build the merged, source-tagged template list and enter the
    /// prompt-library picker. Surfaces editable `User` templates from the
    /// SQLite store alongside read-only `Project` (`{repo}/.amf/config.json`)
    /// and `Global` (`~/.config/amf/config.json`) declarative templates.
    pub fn open_prompt_library(&mut self, from_view: Option<ViewState>) {
        let project = self
            .resolve_library_repo(from_view.as_ref())
            .map(|repo| load_project_prompt_templates(&repo))
            .unwrap_or_default();
        let templates = merge_prompt_library_entries(
            &self.store.prompt_templates,
            &self.config.extension.prompt_templates,
            &project,
        );

        let filtered = (0..templates.len()).collect();
        self.mode = AppMode::PromptLibrary(PromptLibraryState {
            templates,
            filtered,
            query: String::new(),
            search_active: false,
            selected: 0,
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

    /// Render the selected template and deliver it to the originating
    /// session. Phase 1 has no fill-in flow, so any `{{slots}}` collapse
    /// to empty; plain templates inject verbatim.
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

        let rendered = render_template(&template.body, &[]);
        self.deliver_prompt(rendered, from_view)
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
        if !entry.source.is_editable() {
            self.push_toast_warning("Read-only template — press y to duplicate first");
            return;
        }

        let return_to = std::mem::replace(&mut self.mode, AppMode::Normal);
        self.mode = AppMode::PromptEditor(PromptEditorState {
            editing_id: Some(entry.template.id.clone()),
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

        match &state.editing_id {
            Some(id) => {
                if let Some(template) = self
                    .store
                    .prompt_templates
                    .iter_mut()
                    .find(|template| &template.id == id)
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
        self.push_toast_success("Saved prompt");
        self.return_from_prompt_editor(*state.return_to);
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
        match return_to {
            AppMode::PromptLibrary(picker) => self.open_prompt_library(picker.from_view),
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
        if !entry.source.is_editable() {
            self.push_toast_warning("Read-only template can't be deleted");
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
        self.store.prompt_templates.push(template);
        self.save()?;
        self.open_prompt_library(from_view);
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

        match target {
            PromptExportTarget::Global => {
                upsert_template(&mut self.config.extension.prompt_templates, template.clone());
                self.save_config();
                self.push_toast_success(format!(
                    "Exported \"{}\" to global config",
                    template.name
                ));
            }
            PromptExportTarget::Project => {
                let Some(repo) = self.resolve_export_repo(from_view.as_ref()) else {
                    self.push_toast_warning(
                        "No project to export to — open a session or run AMF in a repo",
                    );
                    return Ok(());
                };
                match export_template_to_project_config(&repo, &template) {
                    Ok(path) => self.push_toast_success(format!(
                        "Exported \"{}\" to {}",
                        template.name,
                        crate::app::util::shorten_path(&path)
                    )),
                    Err(e) => self.push_toast_warning(format!("Export failed: {e}")),
                }
            }
        }
        Ok(())
    }

    /// Resolve the repo whose `.amf/config.json` project templates should
    /// appear in the picker: the viewed feature's repo when opened from a
    /// session, else the currently selected project's repo. Unlike
    /// `resolve_export_repo`, this never falls back to the working
    /// directory, so the picker only shows project templates when there is
    /// real project context (and unit tests stay deterministic).
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

    /// Resolve the repo to export a project template into: the viewed
    /// feature's repo when opened from a session, else the repo
    /// containing AMF's working directory.
    fn resolve_export_repo(&self, from_view: Option<&ViewState>) -> Option<PathBuf> {
        if let Some(view) = from_view
            && let Some(project) = self
                .store
                .projects
                .iter()
                .find(|project| project.name == view.project_name)
        {
            return Some(project.repo.clone());
        }
        let detected = crate::app::util::detect_repo_path();
        (!detected.is_empty()).then(|| PathBuf::from(detected))
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

/// Merge the three template sources into one source-tagged list for the
/// picker: editable `User` entries first, then read-only `Project`, then
/// `Global`. A `Global` entry whose name also exists in `Project` is
/// dropped, matching the config merge rule that project wins. `User`
/// entries are never deduped against config — an exported template
/// legitimately exists as both an editable copy and a declarative one.
fn merge_prompt_library_entries(
    user: &[PromptTemplate],
    global: &[PromptTemplate],
    project: &[PromptTemplate],
) -> Vec<PromptLibraryEntry> {
    let mut entries: Vec<PromptLibraryEntry> = Vec::new();
    for template in user {
        entries.push(PromptLibraryEntry {
            template: template.clone(),
            source: PromptSource::User,
        });
    }
    for template in project {
        entries.push(PromptLibraryEntry {
            template: template.clone(),
            source: PromptSource::Project,
        });
    }
    for template in global {
        if project.iter().any(|p| p.name == template.name) {
            continue;
        }
        entries.push(PromptLibraryEntry {
            template: template.clone(),
            source: PromptSource::Global,
        });
    }
    entries
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::ExtensionConfig;

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

        let entries = merge_prompt_library_entries(&user, &global, &project);
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
    fn merge_project_wins_over_global_by_name() {
        let user = vec![];
        let project = vec![PromptTemplate::new("dup".to_string(), "proj".to_string())];
        let global = vec![
            PromptTemplate::new("dup".to_string(), "glob".to_string()),
            PromptTemplate::new("only-global".to_string(), "g".to_string()),
        ];

        let entries = merge_prompt_library_entries(&user, &global, &project);
        // "dup" appears once, from the project source.
        let dup: Vec<_> = entries.iter().filter(|e| e.template.name == "dup").collect();
        assert_eq!(dup.len(), 1);
        assert_eq!(dup[0].source, PromptSource::Project);
        assert_eq!(dup[0].template.body, "proj");
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

        let entries = merge_prompt_library_entries(&user, &global, &project);
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
}
