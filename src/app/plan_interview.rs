//! App-level lifecycle for plan interviews and deferred feature launches.

use anyhow::Result;

use super::{App, AppMode, PlanInterviewState, PreparedFeatureLaunch, Selection};

impl App {
    pub(crate) fn start_plan_interview(&mut self, prepared: PreparedFeatureLaunch) {
        self.mode = AppMode::PlanInterview(PlanInterviewState::for_feature_creation(prepared));
        self.message = None;
    }

    /// Complete the interview and execute the launch it has been holding.
    pub(crate) fn complete_plan_interview(&mut self) -> Result<()> {
        let pending = match &mut self.mode {
            AppMode::PlanInterview(state) => state.pending_launch.take(),
            _ => return Ok(()),
        };
        self.mode = AppMode::Normal;

        if let Some(prepared) = pending {
            self.finish_feature_launch_without_interview(prepared)
        } else {
            self.message = Some("Plan interview complete".into());
            Ok(())
        }
    }

    /// Abort discovery but keep creating the feature, explicitly without plan
    /// mode so the legacy plan-file behavior is not triggered.
    pub(crate) fn launch_plan_interview_without_plan(&mut self) -> Result<()> {
        let pending = match &mut self.mode {
            AppMode::PlanInterview(state) => state.pending_launch.take(),
            _ => return Ok(()),
        };
        self.mode = AppMode::Normal;

        if let Some(mut prepared) = pending {
            prepared.plan_mode = false;
            self.finish_feature_launch_without_interview(prepared)
        } else {
            self.message = Some("Plan interview cancelled".into());
            Ok(())
        }
    }

    /// Cancel the pending feature launch. A worktree may already have been
    /// created (and may contain hook changes), so keep it rather than removing
    /// user data. Pending placeholder features created for hooks are removed.
    pub(crate) fn cancel_plan_interview_feature(&mut self) -> Result<()> {
        let pending = match &mut self.mode {
            AppMode::PlanInterview(state) => state.pending_launch.take(),
            _ => return Ok(()),
        };
        self.mode = AppMode::Normal;

        let Some(prepared) = pending else {
            self.message = Some("Plan interview cancelled".into());
            return Ok(());
        };

        if let Some(pi) = self
            .store
            .projects
            .iter()
            .position(|project| project.name == prepared.project_name)
        {
            self.store.projects[pi].features.retain(|feature| {
                !(feature.name == prepared.branch && feature.pending_worktree_script)
            });
            self.selection = Selection::Project(pi);
            self.save()?;
        }

        self.message = Some(if prepared.is_worktree {
            format!(
                "Feature creation cancelled; worktree kept at {}",
                prepared.workdir.display()
            )
        } else {
            "Feature creation cancelled".into()
        });
        Ok(())
    }
}
