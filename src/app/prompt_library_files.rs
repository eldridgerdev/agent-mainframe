//! TUI adapters for the shared raw-preserving prompt-file engine.
use super::*;
use crate::prompt_library::{
    PromptSource,
    declarative::{
        self as files, Document, Edit, Export, ExportSource, Location, WorkspaceContext,
    },
};
use anyhow::{Context, Result, ensure};

#[derive(Clone)]
pub(crate) struct PromptFileEdit {
    pub edit: Edit,
    pub context: WorkspaceContext,
}
#[derive(Clone)]
pub(crate) struct PromptFileExport {
    source: ExportSource,
    context: WorkspaceContext,
    user: Option<(String, String)>,
}
#[derive(Clone)]
pub(crate) struct PromptFileReplacement {
    plan: Export,
    source: PromptFileExport,
    target: PromptSource,
}
impl App {
    pub(crate) fn guard_prompt_file_discard(&mut self, reload: bool) -> bool {
        if let AppMode::PromptEditor(state) = &mut self.mode
            && let Some(snapshot) = &state.file_edit
        {
            let original =
                files::validate_template(&snapshot.edit.raw).expect("validated snapshot");
            if state.name != original.name
                || state.editor.text() != original.body
                || state.tags != crate::prompt_library::format_tags(&original.tags)
            {
                state.file_discard = Some(reload);
                self.message =
                    Some("Discard unsaved template edits? y confirm · Esc keep editing".into());
                return true;
            }
        }
        false
    }
    pub fn reload_prompt_file_editor(&mut self) {
        if !self.guard_prompt_file_discard(true) {
            self.finish_reload_prompt_file_editor();
        }
    }
    pub(crate) fn finish_reload_prompt_file_editor(&mut self) {
        let result = (|| {
            let AppMode::PromptEditor(state) = &self.mode else {
                return Ok(None);
            };
            let Some(snapshot) = &state.file_edit else {
                return Ok(None);
            };
            self.verify_prompt_file_context(&snapshot.context)?;
            Ok::<_, anyhow::Error>(Some(snapshot.edit.reload()?))
        })();
        match result {
            Ok(Some(edit)) => {
                let template = files::validate_template(&edit.raw).expect("validated snapshot");
                if let AppMode::PromptEditor(state) = &mut self.mode {
                    state.name = template.name;
                    state.tags = crate::prompt_library::format_tags(&template.tags);
                    state.editor = crate::editor::TextEditor::with_vim(template.body);
                    state.file_edit.as_mut().unwrap().edit = edit;
                    state.file_discard = None;
                }
                self.message = Some("Reloaded current template".into());
            }
            Ok(None) => {}
            Err(error) => {
                if let AppMode::PromptEditor(state) = &mut self.mode {
                    state.file_discard = None;
                }
                self.message = Some(format!("Reload refused; your draft is kept: {error}"));
            }
        }
    }
    fn prompt_file_context(&self, from_view: Option<&ViewState>) -> Result<WorkspaceContext> {
        let repo = self.resolve_library_repo(from_view);
        let project = repo
            .as_ref()
            .and_then(|repo| self.store.projects.iter().find(|p| &p.repo == repo));
        let workdir = self.resolve_worktree_dir(from_view);
        let feature = project.and_then(|p| {
            workdir
                .as_ref()
                .and_then(|dir| p.features.iter().find(|f| &f.workdir == dir))
        });
        WorkspaceContext::capture(
            &self.store,
            project.map(|p| p.id.as_str()),
            feature.map(|f| f.id.as_str()),
        )
    }
    fn verify_prompt_file_context(&self, context: &WorkspaceContext) -> Result<()> {
        if let Some(db) = &self.db {
            context.verify(&db.load_store()?)
        } else {
            context.verify(&self.store)
        }
    }
    fn prompt_file_location(context: &WorkspaceContext, source: PromptSource) -> Result<Location> {
        match source {
            PromptSource::Global => Ok(Location::Global(
                crate::project::amf_config_dir().join("config.json"),
            )),
            PromptSource::Project => context.local_project(),
            PromptSource::Worktree => context.local_worktree(),
            PromptSource::User => {
                anyhow::bail!("User templates are stored in the library database")
            }
        }
    }
    pub(crate) fn selected_prompt_file_edit(&self) -> Result<PromptFileEdit> {
        let AppMode::PromptLibrary(picker) = &self.mode else {
            anyhow::bail!("Open the prompt library first")
        };
        let entry = picker.selected_entry().context("Select a template first")?;
        let index = *picker
            .filtered
            .get(picker.selected)
            .context("Selection was removed")?;
        let ordinal = picker.templates[..index]
            .iter()
            .filter(|candidate| candidate.source == entry.source)
            .count();
        let context = self.prompt_file_context(picker.from_view.as_ref())?;
        let edit = Edit::read(
            Document::read(Self::prompt_file_location(&context, entry.source)?)?,
            ordinal,
        )?;
        let actual = files::validate_template(&edit.raw)?;
        ensure!(
            actual.name == entry.template.name
                && actual.body == entry.template.body
                && actual.tags == entry.template.tags
                && actual.description == entry.template.description
                && actual.placeholders == entry.template.placeholders,
            "Template changed or moved; reopen the library before editing"
        );
        Ok(PromptFileEdit { edit, context })
    }
    pub(crate) fn commit_prompt_file_edit(
        &mut self,
        snapshot: &PromptFileEdit,
        name: &str,
        body: &str,
        tags: &str,
    ) -> Result<()> {
        let committed = snapshot.edit.commit(name, body, tags, || {
            self.verify_prompt_file_context(&snapshot.context)
        })?;
        self.accept_prompt_file_document(&committed.document);
        Ok(())
    }
    fn accept_prompt_file_document(&mut self, document: &Document) {
        if matches!(document.location, Location::Global(_)) {
            // Only update the field we authored; unrelated in-memory settings
            // remain under their ordinary settings writer's control.
            if let Ok(rows) = document.rows() {
                let templates = rows
                    .iter()
                    .filter_map(|row| files::validate_template(row).ok())
                    .collect::<Vec<_>>();
                self.config.extension.prompt_templates = templates.clone();
                self.saved_config.extension.prompt_templates = templates;
            }
        }
    }
    fn capture_prompt_file_export(&self) -> Result<PromptFileExport> {
        let AppMode::PromptLibrary(picker) = &self.mode else {
            anyhow::bail!("Open the prompt library first")
        };
        let entry = picker.selected_entry().context("Select a template first")?;
        let context = self.prompt_file_context(picker.from_view.as_ref())?;
        if entry.source == PromptSource::User {
            if let Some(db) = &self.db {
                let snapshot = db
                    .prompt_template_snapshot(&entry.template.id)?
                    .context("This User template was deleted")?;
                ensure!(
                    snapshot.template == entry.template,
                    "This User template changed; reopen the library before exporting"
                );
                let source = ExportSource {
                    raw: snapshot.raw_template()?,
                    revision: snapshot.revision.clone(),
                    file: None,
                };
                Ok(PromptFileExport {
                    source,
                    context,
                    user: Some((entry.template.id.clone(), snapshot.revision)),
                })
            } else {
                let raw = serde_json::to_value(&entry.template)?;
                Ok(PromptFileExport {
                    source: ExportSource {
                        revision: files::revision(&raw)?,
                        raw,
                        file: None,
                    },
                    context,
                    user: None,
                })
            }
        } else {
            let snapshot = self.selected_prompt_file_edit()?;
            Ok(PromptFileExport {
                source: ExportSource::from_file(snapshot.edit)?,
                context,
                user: None,
            })
        }
    }
    pub fn begin_prompt_file_export(&mut self) {
        match self.capture_prompt_file_export() {
            Ok(source) => {
                if let AppMode::PromptLibrary(state) = &mut self.mode {
                    state.file_export = Some(source);
                    state.pending_export = true;
                    state.confirm_delete = false;
                }
                let view = match &self.mode {
                    AppMode::PromptLibrary(state) => state.from_view.as_ref(),
                    _ => None,
                };
                self.message = Some(self.build_export_menu_message(view));
            }
            Err(error) => self.message = Some(format!("Export refused: {error}")),
        }
    }
    pub(crate) fn export_prompt_file(&mut self, target: PromptExportTarget) -> Result<()> {
        let source = match &self.mode {
            AppMode::PromptLibrary(state) => state.file_export.clone(),
            _ => None,
        }
        .map(Ok)
        .unwrap_or_else(|| self.capture_prompt_file_export());
        let result = (|| {
            let source = source?;
            self.verify_prompt_file_context(&source.context)?;
            let target = match target {
                PromptExportTarget::Global => PromptSource::Global,
                PromptExportTarget::Project => PromptSource::Project,
                PromptExportTarget::Worktree => PromptSource::Worktree,
            };
            let plan = Export::prepare(
                source.source.clone(),
                Document::read(Self::prompt_file_location(&source.context, target)?)?,
            )?;
            let pending = PromptFileReplacement {
                plan,
                source,
                target,
            };
            if pending.plan.replacing && !pending.plan.no_op {
                let path = pending.plan.destination.location.canonical();
                if let AppMode::PromptLibrary(state) = &mut self.mode {
                    state.replacement = Some(pending);
                    state.pending_export = false;
                }
                self.message = Some(format!(
                    "Replace same-name template in {}? y confirm · Esc cancel",
                    path.display()
                ));
                Ok(())
            } else {
                self.finish_prompt_file_export(pending)
            }
        })();
        if let Err(error) = result {
            self.message = Some(format!("Export refused; nothing was written: {error}"));
        }
        Ok(())
    }
    pub fn confirm_prompt_file_export(&mut self) -> Result<()> {
        let pending = match &self.mode {
            AppMode::PromptLibrary(state) => state.replacement.clone(),
            _ => None,
        };
        if let Some(pending) = pending
            && let Err(error) = self.finish_prompt_file_export(pending)
        {
            self.message = Some(format!("Export refused; nothing was written: {error}"));
        }
        Ok(())
    }
    fn finish_prompt_file_export(&mut self, pending: PromptFileReplacement) -> Result<()> {
        let document = pending.plan.commit(true, || {
            self.verify_prompt_file_context(&pending.source.context)?;
            if let Some((id, revision)) = &pending.source.user {
                let snapshot = self
                    .db
                    .as_ref()
                    .context("Library database unavailable")?
                    .prompt_template_snapshot(id)?
                    .context("The User source was deleted")?;
                ensure!(
                    &snapshot.revision == revision,
                    "The User source changed. Cancel and prepare the export again."
                );
            }
            Ok(())
        })?;
        self.accept_prompt_file_document(&document);
        let name = files::validate_template(&pending.source.source.raw)?.name;
        let view = match &self.mode {
            AppMode::PromptLibrary(state) => state.from_view.clone(),
            _ => None,
        };
        self.rebuild_prompt_library(view, Some((&name, pending.target)));
        self.message = Some(if pending.plan.no_op {
            "Destination already contains this template; nothing was written".into()
        } else {
            format!("Exported to {}", document.location.canonical().display())
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{AgentKind, Project, ProjectStore};
    use crate::prompt_library::PromptTemplate;
    use crate::traits::{MockTmuxOps, MockWorktreeOps};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use serde_json::{Value, json};
    fn fixture() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let mut store = ProjectStore::empty();
        store.projects.push(Project::new(
            "Project".into(),
            dir.path().into(),
            false,
            AgentKind::Claude,
        ));
        let app = App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        (dir, app)
    }
    fn seed(dir: &std::path::Path) {
        std::fs::write(dir.join("amf.json"),serde_json::to_vec(&json!({"opaque":42,"prompt_templates":[{"name":"File","body":"hello","description":"kept","opaque":"row"}]})).unwrap()).unwrap();
    }
    fn body(app: &App) -> &str {
        let AppMode::PromptEditor(state) = &app.mode else {
            panic!("editor")
        };
        state.editor.text()
    }
    #[test]
    fn declarative_tui_redo_is_preserved_for_both_user_and_file_editor_bodies() {
        for file in [false, true] {
            let (dir, mut app) = fixture();
            if file {
                seed(dir.path());
            } else {
                app.store
                    .prompt_templates
                    .push(PromptTemplate::new("User".into(), "hello".into()));
            }
            app.open_prompt_library(None);
            app.start_edit_selected_template();
            let AppMode::PromptEditor(state) = &mut app.mode else {
                panic!("editor")
            };
            state
                .editor
                .handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
            state
                .editor
                .handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
            for code in [KeyCode::Char('x'), KeyCode::Char('u')] {
                crate::handlers::handle_prompt_editor_key(
                    &mut app,
                    KeyEvent::new(code, KeyModifiers::NONE),
                )
                .unwrap();
            }
            assert_eq!(body(&app), "hello");
            crate::handlers::handle_prompt_editor_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
            )
            .unwrap();
            assert_eq!(body(&app), "ello");
        }
    }
    #[test]
    fn declarative_tui_stale_save_retains_draft_and_reload_and_cancel_require_discard() {
        let (dir, mut app) = fixture();
        seed(dir.path());
        app.open_prompt_library(None);
        app.start_edit_selected_template();
        if let AppMode::PromptEditor(state) = &mut app.mode {
            state.name = "Renamed".into();
            state.editor = crate::editor::TextEditor::with_vim("Local draft".into());
        }
        let mut external: Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("amf.json")).unwrap()).unwrap();
        external["external"] = true.into();
        std::fs::write(
            dir.path().join("amf.json"),
            serde_json::to_vec(&external).unwrap(),
        )
        .unwrap();
        app.submit_prompt_editor().unwrap();
        assert_eq!(body(&app), "Local draft");
        assert!(app.message.as_ref().unwrap().contains("draft is kept"));
        app.cancel_prompt_editor();
        assert!(
            matches!(&app.mode,AppMode::PromptEditor(state) if state.file_discard==Some(false))
        );
        crate::handlers::handle_prompt_editor_key(
            &mut app,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        )
        .unwrap();
        assert_eq!(body(&app), "Local draft");
        crate::handlers::handle_prompt_editor_key(
            &mut app,
            KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE),
        )
        .unwrap();
        assert!(matches!(&app.mode,AppMode::PromptEditor(state) if state.file_discard==Some(true)));
        crate::handlers::handle_prompt_editor_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
        )
        .unwrap();
        assert_eq!(body(&app), "hello");
        if let AppMode::PromptEditor(state) = &mut app.mode {
            state.name = "Renamed".into();
        }
        app.submit_prompt_editor().unwrap();
        let value: Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("amf.json")).unwrap()).unwrap();
        assert_eq!(value["opaque"], 42);
        assert_eq!(value["external"], true);
        assert_eq!(value["prompt_templates"][0]["opaque"], "row");
        assert_eq!(value["prompt_templates"][0]["name"], "Renamed");
    }
    #[test]
    fn declarative_tui_same_name_export_requires_confirmation_and_rechecks_destination() {
        let (dir, mut app) = fixture();
        seed(dir.path());
        app.store
            .prompt_templates
            .push(PromptTemplate::new("File".into(), "User body".into()));
        app.open_prompt_library(None);
        app.begin_prompt_file_export();
        app.export_selected_template(PromptExportTarget::Project)
            .unwrap();
        assert!(matches!(&app.mode,AppMode::PromptLibrary(state) if state.replacement.is_some()));
        let before = std::fs::read(dir.path().join("amf.json")).unwrap();
        app.confirm_prompt_file_export().unwrap();
        assert_ne!(std::fs::read(dir.path().join("amf.json")).unwrap(), before);
        if let AppMode::PromptLibrary(state) = &mut app.mode {
            state.selected = 0;
        }
        app.begin_prompt_file_export();
        app.export_selected_template(PromptExportTarget::Project)
            .unwrap(); // identical source now no-op
        assert!(
            app.message
                .as_ref()
                .unwrap()
                .contains("nothing was written")
        );
    }
}
