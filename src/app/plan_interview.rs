//! App-level lifecycle for plan interviews and deferred feature launches.

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::Path;

use anyhow::{Context, Result};

use super::{App, AppMode, PlanInterviewState, PreparedFeatureLaunch, Selection};
use crate::plan_interview::PlanQuestion;

const PLAN_FILE_NAME: &str = "plan.md";

impl App {
    pub(crate) fn start_plan_interview(&mut self, prepared: PreparedFeatureLaunch) {
        self.mode = AppMode::PlanInterview(PlanInterviewState::for_feature_creation(prepared));
        self.message = None;
    }

    /// Complete the interview and execute the launch it has been holding.
    pub(crate) fn complete_plan_interview(&mut self) -> Result<()> {
        let (workdir, plan) = match &self.mode {
            AppMode::PlanInterview(state) => {
                let Some(prepared) = state.pending_launch.as_ref() else {
                    return Ok(());
                };
                (
                    prepared.workdir.clone(),
                    render_static_plan(
                        &state.feature_name,
                        &state.brief,
                        &state.questions,
                        &state.answers,
                    ),
                )
            }
            _ => return Ok(()),
        };

        // Keep the interview open if either write fails so the user can retry
        // or abort without losing the answers they just entered.
        write_plan_file(&workdir, &plan)?;

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

fn render_static_plan(
    feature_name: &str,
    brief: &str,
    questions: &[PlanQuestion],
    answers: &[Option<String>],
) -> String {
    let mut plan = format!("# Plan: {feature_name}\n\n## Feature brief\n\n{brief}\n\n## Q&A\n");

    for (index, question) in questions.iter().enumerate() {
        plan.push_str("\n### ");
        plan.push_str(&question.text);
        plan.push_str("\n\n");
        match answers.get(index).and_then(|answer| answer.as_deref()) {
            Some(answer) => plan.push_str(answer),
            None => plan.push_str("_Skipped._"),
        }
        plan.push('\n');
    }

    plan
}

fn write_plan_file(workdir: &Path, contents: &str) -> Result<()> {
    let claude_dir = workdir.join(".claude");
    fs::create_dir_all(&claude_dir)
        .with_context(|| format!("failed to create plan directory {}", claude_dir.display()))?;

    ensure_gitignore_entry(&claude_dir.join(".gitignore"), PLAN_FILE_NAME)?;

    let plan_path = claude_dir.join(PLAN_FILE_NAME);
    fs::write(&plan_path, contents)
        .with_context(|| format!("failed to write plan file {}", plan_path.display()))
}

fn ensure_gitignore_entry(path: &Path, entry: &str) -> Result<()> {
    let current = fs::read_to_string(path).unwrap_or_default();
    if current.lines().any(|line| line == entry) {
        return Ok(());
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open gitignore {}", path.display()))?;
    if !current.is_empty() && !current.ends_with('\n') {
        writeln!(file).with_context(|| format!("failed to update gitignore {}", path.display()))?;
    }
    writeln!(file, "{entry}")
        .with_context(|| format!("failed to update gitignore {}", path.display()))
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::plan_interview::{PlanQuestionKind, QuestionSource};

    #[test]
    fn static_plan_preserves_brief_answers_and_skipped_questions() {
        let questions = vec![
            PlanQuestion {
                id: "scope".into(),
                text: "What is in scope?".into(),
                kind: PlanQuestionKind::FreeText,
                source: QuestionSource::Builtin,
                optional: true,
            },
            PlanQuestion {
                id: "risks".into(),
                text: "What are the risks?".into(),
                kind: PlanQuestionKind::FreeText,
                source: QuestionSource::Builtin,
                optional: true,
            },
        ];

        let plan = render_static_plan(
            "guided-plans",
            "Collect a brief\nbefore launch.",
            &questions,
            &[Some("Native TUI only.\nNo AI yet.".into()), None],
        );

        assert!(plan.starts_with("# Plan: guided-plans\n\n## Feature brief"));
        assert!(plan.contains("Collect a brief\nbefore launch."));
        assert!(plan.contains("### What is in scope?\n\nNative TUI only.\nNo AI yet."));
        assert!(plan.contains("### What are the risks?\n\n_Skipped._"));
    }

    #[test]
    fn writing_plan_creates_claude_dir_and_idempotent_ignore_entry() {
        let workdir = TempDir::new().unwrap();
        fs::create_dir(workdir.path().join(".claude")).unwrap();
        fs::write(workdir.path().join(".claude/.gitignore"), "notifications/").unwrap();

        write_plan_file(workdir.path(), "# First plan\n").unwrap();
        write_plan_file(workdir.path(), "# Updated plan\n").unwrap();

        assert_eq!(
            fs::read_to_string(workdir.path().join(".claude/plan.md")).unwrap(),
            "# Updated plan\n"
        );
        assert_eq!(
            fs::read_to_string(workdir.path().join(".claude/.gitignore")).unwrap(),
            "notifications/\nplan.md\n"
        );
    }
}
