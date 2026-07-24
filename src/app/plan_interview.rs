//! App-level lifecycle for plan interviews and deferred feature launches.

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::Path;
use std::sync::mpsc;

use anyhow::{Context, Result};

use super::pr_review::estimate_tokens;
use super::{
    App, AppMode, PlanInterviewPhase, PlanInterviewState, PreparedFeatureLaunch, Selection,
};
use crate::headless::HeadlessRunner;
use crate::plan_interview::{self, PlanQuestion};

const PLAN_FILE_NAME: &str = "plan.md";

impl App {
    pub(crate) fn start_plan_interview(&mut self, prepared: PreparedFeatureLaunch) {
        let questions = self
            .store
            .find_project(&prepared.project_name)
            .map(|project| {
                self.extension_for_repo(&project.repo)
                    .plan_interview_questions()
            })
            .unwrap_or_else(crate::plan_interview::builtin_questions);
        self.mode = AppMode::PlanInterview(PlanInterviewState::for_feature_creation(
            prepared, questions,
        ));
        self.message = None;
    }

    /// Called once the interview's question flow or AI consent step reaches
    /// `Done`. Starts the next AI-adaptive round only after explicit user
    /// opt-in; otherwise completes the interview without spending tokens.
    /// No-op unless the mode is actually `Done`.
    pub(crate) fn continue_plan_interview_after_done(&mut self) -> Result<()> {
        let (is_done, should_start_next_round) = match &self.mode {
            AppMode::PlanInterview(state) => (
                state.phase == PlanInterviewPhase::Done,
                state.ai_followups_opted_in
                    && !state.skip_ai_rounds
                    && state.ai_rounds_completed < plan_interview::MAX_AI_ROUNDS,
            ),
            _ => (false, false),
        };
        if !is_done {
            return Ok(());
        }
        if should_start_next_round {
            self.start_next_plan_interview_ai_round()
        } else {
            self.complete_plan_interview()
        }
    }

    /// Resolve the interview engine (lazily, once) and spawn one AI-adaptive
    /// round off the UI thread. Falls straight through to completion when no
    /// headless-capable harness is available — AI rounds are best-effort.
    fn start_next_plan_interview_ai_round(&mut self) -> Result<()> {
        let (
            preferred_harness,
            resolved_harness,
            round,
            feature_name,
            brief,
            questions,
            answers,
            workdir,
        ) = match &self.mode {
            AppMode::PlanInterview(state) => (
                state.preferred_harness.clone(),
                state.ai_harness.clone(),
                state.ai_rounds_completed + 1,
                state.feature_name.clone(),
                state.brief.clone(),
                state.questions.clone(),
                state.answers.clone(),
                state
                    .pending_launch
                    .as_ref()
                    .map(|prepared| prepared.workdir.clone())
                    .unwrap_or_else(|| std::env::current_dir().unwrap_or_default()),
            ),
            _ => return Ok(()),
        };

        let harness = match resolved_harness {
            Some(resolved) => resolved,
            None => HeadlessRunner::select_for_interview(&preferred_harness),
        };
        // Cache the resolution (even `None`) so later rounds don't re-probe.
        if let AppMode::PlanInterview(state) = &mut self.mode {
            state.ai_harness = Some(harness.clone());
        }

        let Some(harness) = harness else {
            self.log_info(
                "plan_interview",
                "no headless-capable harness available; skipping AI rounds".to_string(),
            );
            return self.complete_plan_interview();
        };

        let context = plan_interview::gather_repository_context(&workdir);
        let prompt = plan_interview::build_interviewer_prompt(
            &feature_name,
            &brief,
            &questions,
            &answers,
            &context,
            round,
        );
        let token_estimate = estimate_tokens(&prompt);

        self.log_info(
            "plan_interview",
            format!(
                "starting AI round {round} with {} (~{token_estimate} tokens)",
                harness.display_name()
            ),
        );

        let (tx, rx) = mpsc::channel();
        self.plan_interview_ai_bg = Some(rx);
        let thread_harness = harness;
        let thread_workdir = workdir;
        std::thread::spawn(move || {
            let result = HeadlessRunner::run(&thread_harness, &thread_workdir, &prompt, None, true);
            let _ = tx.send((round, result));
        });

        if let AppMode::PlanInterview(state) = &mut self.mode {
            state.begin_ai_round(token_estimate);
        }
        Ok(())
    }

    /// Poll the in-flight AI-adaptive round. Returns `true` when a redraw is
    /// warranted.
    pub fn poll_plan_interview_ai_bg(&mut self) -> bool {
        let Some(rx) = self.plan_interview_ai_bg.as_ref() else {
            return false;
        };
        let confirming_abort = matches!(
            &self.mode,
            AppMode::PlanInterview(state) if state.abort_confirmation
        );
        if confirming_abort {
            // Leave the round result unread until the user resolves the
            // pending abort choice — draining it here could start another
            // paid round or launch the feature out from under them. Esc
            // (resume) clears `abort_confirmation` and the next poll tick
            // picks the result back up normally; y/n move `self.mode` out of
            // `PlanInterview`, so the next tick falls through to the
            // "navigated away, nothing to apply" path below instead.
            return false;
        }
        let (round, result) = match rx.try_recv() {
            Ok(message) => message,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.plan_interview_ai_bg = None;
                self.log_warn(
                    "plan_interview",
                    "AI round worker thread ended unexpectedly".to_string(),
                );
                let was_loading = matches!(
                    &self.mode,
                    AppMode::PlanInterview(state) if state.phase == PlanInterviewPhase::AiLoading
                );
                if was_loading {
                    if let AppMode::PlanInterview(state) = &mut self.mode {
                        state.ai_round_started_at = None;
                        state.ai_rounds_completed = plan_interview::MAX_AI_ROUNDS;
                        state.phase = PlanInterviewPhase::Done;
                    }
                    if let Err(e) = self.complete_plan_interview() {
                        self.report_logged_error(
                            "plan_interview",
                            format!("Failed to complete plan interview: {e}"),
                        );
                    }
                }
                return true;
            }
        };
        self.plan_interview_ai_bg = None;

        let is_loading = matches!(
            &self.mode,
            AppMode::PlanInterview(state) if state.phase == PlanInterviewPhase::AiLoading
        );
        if !is_loading {
            // The user navigated or aborted away from the loading screen;
            // the background call already finished, nothing to apply.
            return false;
        }

        let existing_ids: Vec<String> = match &self.mode {
            AppMode::PlanInterview(state) => state.questions.iter().map(|q| q.id.clone()).collect(),
            _ => return false,
        };

        let new_questions = match result {
            Ok(response) => {
                let parsed = plan_interview::parse_ai_questions(&response, &existing_ids, round);
                if parsed.is_empty() {
                    self.log_debug(
                        "plan_interview",
                        format!("AI round {round} returned no usable follow-up questions"),
                    );
                }
                parsed
            }
            Err(e) => {
                self.log_warn("plan_interview", format!("AI round {round} failed: {e}"));
                Vec::new()
            }
        };

        if let AppMode::PlanInterview(state) = &mut self.mode {
            state.apply_ai_round(round, new_questions);
        }

        let reached_done = matches!(
            &self.mode,
            AppMode::PlanInterview(state) if state.phase == PlanInterviewPhase::Done
        );
        if reached_done
            && let Err(e) = self.continue_plan_interview_after_done()
        {
            self.report_logged_error(
                "plan_interview",
                format!("Failed to continue plan interview: {e}"),
            );
        }
        true
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
    use crate::markdown::{preferred_plan_markdown_path, read_plan_preview};
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

    #[test]
    fn non_worktree_plan_write_is_the_plan_sidebar_reads() {
        let repo = TempDir::new().unwrap();
        let expected_plan = repo.path().join(".claude/plan.md");

        write_plan_file(
            repo.path(),
            "# Plan: first feature\n\n## Feature brief\n\nShip it.\n",
        )
        .unwrap();

        assert_eq!(
            preferred_plan_markdown_path(repo.path(), Some(repo.path())),
            Some(expected_plan.clone())
        );
        assert_eq!(
            read_plan_preview(repo.path(), Some(repo.path())).as_deref(),
            Some("Plan: first feature\nFeature brief\nShip it.")
        );
        assert!(expected_plan.is_file());
        assert!(!repo.path().join("PLAN.md").exists());
        assert!(!repo.path().join("plan.md").exists());
    }
}
