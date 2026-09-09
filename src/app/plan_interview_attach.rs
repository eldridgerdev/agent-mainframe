//! The file-picking overlay that attaches a reference document to a plan
//! interview (`AppMode::PlanInterviewAttachDoc`). Opened with `Ctrl+D` from the
//! interview brief step; see `src/app/plan_interview.rs` for how attachments
//! reach the headless passes.

use ratatui_explorer::FileExplorer;

use super::state::{AttachDocState, PlanInterviewPhase};
use super::{App, AppMode};

impl App {
    /// Open the reference-document picker from the interview brief step. A
    /// no-op off that step or once [`crate::plan_interview::MAX_ATTACHED_DOCS`]
    /// are already attached (with a message in the latter case).
    pub(crate) fn open_plan_interview_attach_doc(&mut self) {
        let start_dir = match &self.mode {
            AppMode::PlanInterview(state) if state.phase == PlanInterviewPhase::Brief => {
                if state.attached_docs.len() >= crate::plan_interview::MAX_ATTACHED_DOCS {
                    self.message = Some(format!(
                        "At most {} reference docs can be attached",
                        crate::plan_interview::MAX_ATTACHED_DOCS
                    ));
                    return;
                }
                state.context_workdir()
            }
            _ => return,
        };

        // The interview is still the live mode here, so a failure just leaves
        // it untouched with a message rather than tearing it down.
        let mut explorer = match FileExplorer::new() {
            Ok(explorer) => explorer,
            Err(e) => {
                self.message = Some(format!("Error: cannot open the file browser: {e}"));
                return;
            }
        };
        let _ = explorer.set_cwd(start_dir);

        let AppMode::PlanInterview(interview) =
            std::mem::replace(&mut self.mode, AppMode::Normal)
        else {
            // The guard above already proved the mode; restore and bail if it
            // somehow changed underneath us.
            return;
        };
        self.mode = AppMode::PlanInterviewAttachDoc(Box::new(AttachDocState {
            explorer,
            interview,
            error: None,
        }));
    }

    /// `Enter` on a highlighted **file**: validate it, add it to the
    /// interview's attachment list, and return to the interview. A rejected
    /// pick keeps the picker open with the reason in its footer. (`Enter` on a
    /// directory is a descend and never reaches here — the key handler routes
    /// it to the explorer.)
    pub(crate) fn confirm_plan_interview_attach_doc(&mut self) {
        let outcome = match &mut self.mode {
            AppMode::PlanInterviewAttachDoc(state) => {
                let current = state.explorer.current();
                if current.is_dir() {
                    return;
                }
                let path = current.path().clone();
                state.interview.attach_doc(&path)
            }
            _ => return,
        };

        match outcome {
            Ok(label) => {
                if let AppMode::PlanInterviewAttachDoc(state) =
                    std::mem::replace(&mut self.mode, AppMode::Normal)
                {
                    let count = state.interview.attached_docs.len();
                    self.mode = AppMode::PlanInterview(state.interview);
                    self.persist_plan_interview_draft();
                    self.message = Some(format!(
                        "Attached {label} ({count}/{} reference docs) — the interview will run read-only",
                        crate::plan_interview::MAX_ATTACHED_DOCS
                    ));
                }
            }
            Err(err) => {
                if let AppMode::PlanInterviewAttachDoc(state) = &mut self.mode {
                    state.error = Some(err.to_string());
                }
            }
        }
    }

    /// Leave the picker without attaching anything, restoring the interview.
    pub(crate) fn cancel_plan_interview_attach_doc(&mut self) {
        if let AppMode::PlanInterviewAttachDoc(state) =
            std::mem::replace(&mut self.mode, AppMode::Normal)
        {
            self.mode = AppMode::PlanInterview(state.interview);
        }
    }
}
