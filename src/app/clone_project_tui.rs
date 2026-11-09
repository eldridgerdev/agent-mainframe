//! TUI glue for cloning a project from a git URL (`N` form).
//!
//! All clone logic lives in `clone_project.rs`; this module only moves between
//! the form and the pre-flight / progress screens, keeps the derived name and
//! directory in step with the URL field, and reacts to the job's phase after
//! each poll. Nothing here touches git or the filesystem.

use std::path::{Path, PathBuf};

use super::clone_project::{CloneFix, ClonePhase, CloneProjectError};
use super::{
    App, AppMode, CloneProjectViewState, CloneRootPromptState, CreateProjectStep, Selection,
};

/// Every fix offered by the current phase's checks, in display order. The
/// number the user presses is the position here plus one.
pub(crate) fn phase_fixes(phase: &ClonePhase) -> Vec<CloneFix> {
    match phase {
        ClonePhase::PreflightDone { checks, .. } => {
            checks.iter().filter_map(|c| c.fix.clone()).collect()
        }
        _ => Vec::new(),
    }
}

impl App {
    /// Re-derive name and directory after the URL field changed. A value the
    /// user typed (not flagged derived) is never overwritten; a derived value
    /// is replaced, or cleared when the URL stops being usable (so emptying
    /// the field leaves the form exactly as it was before any URL).
    pub(crate) fn refresh_create_project_url(&mut self) {
        let url = match &self.mode {
            AppMode::CreatingProject(state) => state.url.trim().to_string(),
            _ => return,
        };
        // Whatever the form last complained about may be about the old URL.
        self.message = None;
        let derived = if url.is_empty() {
            None
        } else {
            Some(self.derive_clone_target(&url))
        };
        let AppMode::CreatingProject(state) = &mut self.mode else {
            return;
        };
        match derived {
            Some(Ok(target)) => {
                state.url_error = None;
                if state.name.is_empty() || state.name_derived {
                    state.name = target.name;
                    state.name_derived = true;
                }
                if state.path.is_empty() || state.path_derived {
                    state.path = target.dest.to_string_lossy().into_owned();
                    state.path_derived = true;
                }
            }
            other => {
                state.url_error = other.and_then(|r| r.err()).map(|e| e.to_string());
                if state.name_derived {
                    state.name.clear();
                    state.name_derived = false;
                }
                if state.path_derived {
                    state.path.clear();
                    state.path_derived = false;
                }
            }
        }
        self.refresh_create_project_agent_selection();
    }

    /// After any text edit in the New Project form: the URL field re-derives
    /// name and directory; other fields just refresh the harness/git probe.
    pub(crate) fn after_create_project_text_edit(&mut self) {
        if matches!(&self.mode, AppMode::CreatingProject(s) if s.step == CreateProjectStep::Url) {
            self.refresh_create_project_url();
        } else {
            self.refresh_create_project_agent_selection();
        }
    }

    /// Enter on the last field with a URL set: validate and start the
    /// pre-flight. A rejected target stays on the form, focused on the
    /// offending field, with the message.
    pub(crate) fn start_clone_from_form(&mut self) {
        let AppMode::CreatingProject(form) = &self.mode else {
            return;
        };
        let (url, path, name) = (
            form.url.clone(),
            PathBuf::from(&form.path),
            form.name.clone(),
        );
        match self.start_clone_preflight(&url, &path, &name) {
            Ok(()) => {
                let AppMode::CreatingProject(form) =
                    std::mem::replace(&mut self.mode, AppMode::Normal)
                else {
                    return;
                };
                self.message = None;
                self.mode = AppMode::CloneProject(Box::new(CloneProjectViewState {
                    form,
                    notice: None,
                    cancelling: false,
                }));
            }
            Err(CloneProjectError::ProjectsRootMissing { root }) => {
                let AppMode::CreatingProject(form) =
                    std::mem::replace(&mut self.mode, AppMode::Normal)
                else {
                    return;
                };
                self.message = None;
                self.mode = AppMode::CloneRootPrompt(Box::new(CloneRootPromptState {
                    form,
                    root,
                    editing: None,
                    error: None,
                }));
            }
            Err(e) => self.form_error(&e),
        }
    }

    fn root_prompt_mut(&mut self) -> Option<&mut CloneRootPromptState> {
        match &mut self.mode {
            AppMode::CloneRootPrompt(p) => Some(p),
            _ => None,
        }
    }

    fn set_root_prompt_error(&mut self, error: impl Into<String>) {
        if let Some(p) = self.root_prompt_mut() {
            p.error = Some(error.into());
        }
    }

    /// Esc on the choices: back to the form, values untouched, nothing created.
    pub(crate) fn clone_root_prompt_cancel(&mut self) {
        let mode = std::mem::replace(&mut self.mode, AppMode::Normal);
        match mode {
            AppMode::CloneRootPrompt(p) => self.mode = AppMode::CreatingProject(p.form),
            other => self.mode = other,
        }
    }

    /// `c`/Enter: create the projects folder (only it), then carry on into
    /// the pre-flight with the form's own values.
    pub(crate) fn clone_root_prompt_create(&mut self) {
        if self.root_prompt_mut().is_none() {
            return;
        }
        if let Err(e) = self.create_projects_root() {
            self.set_root_prompt_error(e.to_string());
            return;
        }
        let AppMode::CloneRootPrompt(p) = std::mem::replace(&mut self.mode, AppMode::Normal) else {
            return;
        };
        self.mode = AppMode::CreatingProject(p.form);
        self.start_clone_from_form();
    }

    /// `d`: type a different projects folder, prefilled with the current one.
    pub(crate) fn clone_root_prompt_choose(&mut self) {
        let text = self
            .config
            .projects
            .projects_root
            .clone()
            .unwrap_or_else(|| self.config.projects_root().to_string_lossy().into_owned());
        if let Some(p) = self.root_prompt_mut() {
            p.editing = Some(text);
            p.error = None;
        }
    }

    /// Enter while typing: persist the folder, re-derive the destination for
    /// the URL (values the user edited are kept) and return to the form.
    pub(crate) fn clone_root_prompt_confirm_choice(&mut self) {
        let Some(text) = self.root_prompt_mut().and_then(|p| p.editing.clone()) else {
            return;
        };
        if let Err(e) = self.set_projects_root(&text) {
            self.set_root_prompt_error(e.to_string());
            return;
        }
        let AppMode::CloneRootPrompt(p) = std::mem::replace(&mut self.mode, AppMode::Normal) else {
            return;
        };
        self.mode = AppMode::CreatingProject(p.form);
        self.refresh_create_project_url();
        self.message = Some(
            "Projects folder changed. Press Enter again to continue (check the directory).".into(),
        );
    }

    /// Esc while typing: back to the choices.
    pub(crate) fn clone_root_prompt_stop_editing(&mut self) {
        if let Some(p) = self.root_prompt_mut() {
            p.editing = None;
            p.error = None;
        }
    }

    /// Show `error` on the form and focus the field it is about.
    fn form_error(&mut self, error: &CloneProjectError) {
        if let AppMode::CreatingProject(form) = &mut self.mode {
            form.step = match error {
                CloneProjectError::Url(_) => CreateProjectStep::Url,
                CloneProjectError::Dest(_) => CreateProjectStep::Path,
                CloneProjectError::EmptyName | CloneProjectError::NameTaken(_) => {
                    CreateProjectStep::Name
                }
                _ => form.step.clone(),
            };
        }
        self.message = Some(format!("Error: {error}"));
    }

    /// Leave the clone screens for the form with every value kept.
    fn return_to_clone_form(&mut self, message: Option<String>) {
        let mode = std::mem::replace(&mut self.mode, AppMode::Normal);
        if let AppMode::CloneProject(view) = mode {
            self.reset_clone_project();
            self.mode = AppMode::CreatingProject(view.form);
            self.message = message;
        } else {
            self.mode = mode;
        }
    }

    fn clone_view(&self) -> Option<&CloneProjectViewState> {
        match &self.mode {
            AppMode::CloneProject(v) => Some(v),
            _ => None,
        }
    }

    fn set_clone_notice(&mut self, notice: impl Into<String>) {
        if let AppMode::CloneProject(v) = &mut self.mode {
            v.notice = Some(notice.into());
        }
    }

    /// Start (`Enter`/`s` on the pre-flight screen). Refused while blocked.
    pub(crate) fn clone_view_start(&mut self) {
        let Some(view) = self.clone_view() else {
            return;
        };
        let snapshot = self.clone_project_snapshot();
        if !snapshot.can_start {
            let why = match snapshot.phase {
                ClonePhase::PreflightDone { blocked: true, .. } => {
                    "Start is disabled: a check failed. Fix it, then press r to re-check."
                }
                ClonePhase::PreflightRunning => "Pre-flight checks are still running.",
                _ => "Nothing to start.",
            };
            self.set_clone_notice(why);
            return;
        }
        let (url, path, name) = (
            view.form.url.clone(),
            PathBuf::from(&view.form.path),
            view.form.name.clone(),
        );
        if let Err(e) = self.start_clone(&url, Path::new(&path), &name) {
            self.return_to_clone_form(Some(format!("Error: {e}")));
        }
    }

    /// `r`: run the pre-flight again (e.g. after the user ran a fix).
    pub(crate) fn clone_view_recheck(&mut self) {
        let Some(view) = self.clone_view() else {
            return;
        };
        if matches!(
            self.clone_project_snapshot().phase,
            ClonePhase::PreflightRunning
        ) {
            return;
        }
        let (url, path, name) = (
            view.form.url.clone(),
            PathBuf::from(&view.form.path),
            view.form.name.clone(),
        );
        if let Err(e) = self.start_clone_preflight(&url, &path, &name) {
            self.return_to_clone_form(Some(format!("Error: {e}")));
        }
    }

    /// The command for fix number `n` (1-based) on the current screen.
    pub(crate) fn clone_view_fix_command(&self, n: usize) -> Option<CloneFix> {
        phase_fixes(&self.clone_project_snapshot().phase)
            .into_iter()
            .nth(n.checked_sub(1)?)
    }

    /// Copy fix `n`'s command line to the clipboard. Copy-only: AMF never runs
    /// a fix.
    pub(crate) fn clone_view_copy_fix(&mut self, n: usize) {
        let Some(fix) = self.clone_view_fix_command(n) else {
            let msg = if phase_fixes(&self.clone_project_snapshot().phase).is_empty() {
                "No fix command to copy."
            } else {
                "No fix with that number."
            };
            self.set_clone_notice(msg);
            return;
        };
        let notice = match crate::app::util::copy_to_clipboard(&fix.command_line) {
            Ok(()) => format!(
                "Copied: {}  (run it in a terminal, then press r)",
                fix.command_line
            ),
            Err(e) => format!("Clipboard error: {e}. Command: {}", fix.command_line),
        };
        self.set_clone_notice(notice);
    }

    /// Esc on a clone screen.
    pub(crate) fn clone_view_escape(&mut self) {
        let Some(view) = self.clone_view() else {
            return;
        };
        if view.cancelling {
            return;
        }
        match self.clone_project_snapshot().phase {
            ClonePhase::Cloning { .. } => {
                if self.cancel_clone() {
                    if let AppMode::CloneProject(v) = &mut self.mode {
                        v.cancelling = true;
                        v.notice = Some("Cancelling the clone...".into());
                    }
                } else {
                    self.set_clone_notice("The clone can't be cancelled right now.");
                }
            }
            ClonePhase::Registering => {
                self.set_clone_notice(
                    "Registering the project; it can't be cancelled now (a moment).",
                );
            }
            ClonePhase::PreflightRunning => {
                self.cancel_clone();
                self.return_to_clone_form(None);
            }
            _ => self.return_to_clone_form(None),
        }
    }

    /// React to the job's phase after polling: success returns to the
    /// dashboard on the new project; failure or cancellation returns to the
    /// form with its values. Returns whether the UI changed.
    pub fn sync_clone_project_mode(&mut self) -> bool {
        if !matches!(self.mode, AppMode::CloneProject(_)) {
            return false;
        }
        match self.clone_project_snapshot().phase {
            ClonePhase::Succeeded { name, dest, branch } => {
                let agent = match &self.mode {
                    AppMode::CloneProject(v) => Some(v.form.agent.clone()),
                    _ => None,
                };
                self.reset_clone_project();
                self.mode = AppMode::Normal;
                let mut note = String::new();
                if let Some(pi) = self.store.projects.iter().position(|p| p.name == name) {
                    self.selection = Selection::Project(pi);
                    if let Some(agent) = agent {
                        let (agent, _) = self.normalize_agent_for_project_path(&dest, &agent);
                        self.store.projects[pi].preferred_agent = agent;
                        if let Err(e) = self.save() {
                            note = format!(" (could not save the harness choice: {e})");
                        }
                    }
                }
                self.message = Some(format!(
                    "Cloned '{name}' into {} (branch {branch}){note}",
                    dest.display()
                ));
            }
            ClonePhase::Failed {
                message,
                advice,
                fixes,
            } => {
                let mut text = format!("Error: {message}");
                if let Some(a) = advice {
                    text.push(' ');
                    text.push_str(&a);
                }
                for f in fixes {
                    text.push_str(&format!(" [{}: {}]", f.label, f.command_line));
                }
                self.return_to_clone_form(Some(text));
            }
            ClonePhase::Cancelled => self.return_to_clone_form(Some("Clone cancelled.".into())),
            ClonePhase::Idle => self.return_to_clone_form(None),
            _ => return false,
        }
        true
    }
}
