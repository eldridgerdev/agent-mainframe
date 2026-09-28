//! Questions shared by Final Review, manual PR review, and the AI PR pane.
//! Conversations remain owned by the review; workers never own App or sessions.
mod context;
mod state;
#[cfg(test)]
pub(crate) mod test_support {
    pub(crate) use super::context::{git, prepare, validate_anchor};
    pub(crate) use super::worker::{Completion, Job, Outcome, RunInput, Runner, Task};
    pub(crate) fn review_context(app: &crate::app::App) -> Option<super::context::QuestionContext> {
        app.review_question_context()
    }
}
mod worker;
pub(crate) use state::{DraftDestination, Questions};
pub(crate) use worker::Work;

use crate::app::{App, AppMode};
use crate::editor::TextEditor;
use crate::prompts::PromptId;
use context::QuestionContext;
use state::Turn;
use worker::{Outcome, Task};

impl App {
    pub(crate) fn review_questions(&self) -> Option<&Questions> {
        match &self.mode {
            AppMode::DiffViewer(s) if s.review => Some(&s.questions),
            AppMode::AiReview(s) => Some(&s.questions),
            _ => None,
        }
    }
    pub(crate) fn review_questions_mut(&mut self) -> Option<&mut Questions> {
        match &mut self.mode {
            AppMode::DiffViewer(s) if s.review => Some(&mut s.questions),
            AppMode::AiReview(s) => Some(&mut s.questions),
            _ => None,
        }
    }
    fn review_question_context(&self) -> Option<QuestionContext> {
        match &self.mode {
            AppMode::DiffViewer(s) if s.review => Some(QuestionContext::from_diff(s)),
            AppMode::AiReview(s) => Some(QuestionContext::from_ai(s)),
            _ => None,
        }
    }
    pub(crate) fn open_review_questions(&mut self) {
        let Some(context) = self.review_question_context() else {
            return;
        };
        let preferred = match &self.mode {
            AppMode::AiReview(s) => s.harness.clone(),
            _ => None,
        }
        .or_else(|| {
            self.store
                .projects
                .iter()
                .flat_map(|p| &p.features)
                .find(|f| f.workdir == context.workdir)
                .map(|f| f.agent.clone())
        });
        let q = self.review_questions_mut().expect("context has review");
        if q.next_request == 0 {
            q.harness = preferred.unwrap_or_default();
        }
        q.current_version = context.version;
        q.open = true;
    }
    pub(crate) fn close_review_questions(&mut self) {
        if let Some(q) = self.review_questions_mut() {
            q.open = false;
        }
    }
    pub(crate) fn cycle_review_question_harness(&mut self) {
        let Some(context) = self.review_question_context() else {
            return;
        };
        let repo = self.repo_for_project_path(&context.workdir);
        let allowed = self.allowed_agents_for_repo(&repo);
        if let Some(q) = self.review_questions_mut() {
            let current = allowed.iter().position(|a| *a == q.harness).unwrap_or(0);
            if let Some(agent) = allowed.get((current + 1) % allowed.len().max(1)) {
                q.harness = agent.clone();
            }
        }
    }
    pub(crate) fn submit_review_question(&mut self) {
        let Some(context) = self.review_question_context() else {
            return;
        };
        self.cancel_review_question();
        let q = self.review_questions_mut().expect("review");
        let question = q.editor.text().trim().to_string();
        if question.is_empty() {
            q.error = Some("Enter a question first".into());
            return;
        }
        let earlier = q
            .turns
            .iter()
            .filter(|t| t.context.version == context.version && t.answer.is_some())
            .rev()
            .take(8)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|t| {
                format!(
                    "Reviewer: {}\nAnswer: {}",
                    t.question,
                    t.answer.as_deref().unwrap_or("")
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n")
            .chars()
            .take(32_000)
            .collect();
        q.turns.push(Turn {
            question: question.clone(),
            answer: None,
            error: None,
            context: context.clone(),
            prepared: None,
        });
        q.selected = q.turns.len() - 1;
        q.scroll = 0;
        self.start_review_question_task(context, Task::Answer { question, earlier });
    }
    pub(crate) fn retry_review_question(&mut self) {
        let text = self
            .review_questions()
            .and_then(|q| q.turns.get(q.selected))
            .map(|t| t.question.clone());
        if let Some(text) = text {
            self.review_questions_mut().expect("review").editor = TextEditor::new(text);
            self.submit_review_question();
        }
    }
    pub(crate) fn draft_review_question(&mut self, destination: DraftDestination) {
        let Some(turn) = self
            .review_questions()
            .and_then(|q| q.turns.get(q.selected))
            .cloned()
        else {
            return;
        };
        let (Some(answer), Some(prepared)) = (turn.answer, turn.prepared) else {
            return;
        };
        self.start_review_question_task(
            turn.context,
            Task::Draft {
                destination,
                question: turn.question,
                answer,
                stamp: prepared.stamp,
            },
        );
    }
    pub(crate) fn transfer_review_question_draft(&mut self) {
        let Some(q) = self.review_questions() else {
            return;
        };
        let Some((destination, editor)) = &q.draft else {
            return;
        };
        if editor.text().trim().is_empty() {
            return;
        }
        let Some(turn) = q.turns.get(q.selected) else {
            return;
        };
        let Some(prepared) = &turn.prepared else {
            return;
        };
        let context = turn.context.clone();
        let task = Task::Transfer {
            destination: *destination,
            text: editor.text().to_string(),
            stamp: prepared.stamp.clone(),
        };
        self.start_review_question_task(context, task);
    }
    fn start_review_question_task(&mut self, context: QuestionContext, task: Task) {
        self.cancel_review_question();
        let Some(current) = self.review_question_context() else {
            return;
        };
        if current.version != context.version {
            if let Some(q) = self.review_questions_mut() {
                q.error = Some("Review context changed; ask again before drafting".into());
            }
            return;
        }
        let repo = self.repo_for_project_path(&context.workdir);
        let harness = self.review_questions().expect("review").harness.clone();
        if !self.allowed_agents_for_repo(&repo).contains(&harness) {
            self.review_questions_mut().expect("review").error = Some("This harness is unavailable or disabled for this project; choose another with Ctrl+H".into());
            return;
        }
        let id = if matches!(task, Task::Answer { .. }) {
            PromptId::ReviewQuestion
        } else {
            PromptId::ReviewQuestionDraft
        };
        let template = self
            .resolve_headless_template(id, &harness, &repo, &context.workdir)
            .0;
        let model = match &self.mode {
            AppMode::AiReview(s) => s.model.clone(),
            _ => self.config.review_model.clone(),
        };
        let q = self.review_questions_mut().expect("review");
        q.next_request += 1;
        q.request = Some(q.next_request);
        q.started_at = Some(std::time::Instant::now());
        q.editing = false;
        q.current_version = context.version.clone();
        q.error = None;
        let start = worker::Start {
            owner: q.owner.clone(),
            request: q.next_request,
            turn: q.selected,
            context,
            task,
            harness,
            model,
            template,
        };
        self.review_question_work.start(start);
    }
    pub(crate) fn cancel_review_question(&mut self) {
        if let Some(job) = self.review_question_work.job.take()
            && let Some(q) = self.review_questions_mut()
            && q.owner == job.owner
            && q.request == Some(job.request)
        {
            q.request = None;
            q.started_at = None;
            q.error = Some("Question cancelled; your text is retained".into());
            if matches!(job.task, Task::Answer { .. })
                && let Some(turn) = q.turns.get_mut(job.turn)
            {
                turn.error = Some("Cancelled".into());
            }
        }
    }
    pub(crate) fn poll_review_questions(&mut self) -> bool {
        let Some(job) = self.review_question_work.job.as_ref() else {
            return false;
        };
        let current = match &self.mode {
            AppMode::DiffViewer(s) if s.review => Some(QuestionContext::diff_version(s)),
            AppMode::AiReview(s) => Some(QuestionContext::ai_version(s)),
            _ => None,
        };
        let owned = self
            .review_questions()
            .is_some_and(|q| q.owner == job.owner && q.request == Some(job.request));
        if !owned {
            self.review_question_work.job = None;
            return false;
        }
        if current
            .as_ref()
            .is_none_or(|version| *version != job.context.version)
        {
            self.cancel_review_question();
            if let Some(q) = self.review_questions_mut() {
                q.error = Some("Review context changed; obsolete request cancelled".into());
            }
            return true;
        }
        // A stalled preparation (including GitHub fetch) must also time out
        // without blocking the TUI; dropping the job revokes harness access.
        if job.started.elapsed().as_secs() >= 180 {
            self.cancel_review_question();
            if let Some(q) = self.review_questions_mut() {
                q.error = Some("Question timed out; retry when ready".into());
            }
            return true;
        }
        let result = match job.receiver.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.cancel_review_question();
                if let Some(q) = self.review_questions_mut() {
                    q.error = Some("Question worker stopped unexpectedly; retry".into());
                }
                return true;
            }
        };
        if result.owner != job.owner
            || result.request != job.request
            || result.version != job.context.version
        {
            return false;
        }
        let job = self.review_question_work.job.take().expect("pending job");
        let q = self.review_questions_mut().expect("owned review");
        q.request = None;
        q.started_at = None;
        match result.result {
            Ok(Outcome::Answer(prepared, answer)) => {
                if let Some(turn) = q.turns.get_mut(job.turn) {
                    turn.answer = Some(answer);
                    turn.prepared = Some(prepared);
                }
            }
            Ok(Outcome::Draft(destination, text)) => {
                q.selected = job.turn;
                q.draft = Some((destination, TextEditor::new(text)));
            }
            Ok(Outcome::Transfer(prepared, destination, text)) => {
                self.apply_review_question_draft(destination, &job.context, &prepared.files, text);
            }
            Err(error) => {
                q.error = Some(error.clone());
                if matches!(job.task, Task::Answer { .. })
                    && let Some(turn) = q.turns.get_mut(job.turn)
                {
                    turn.error = Some(error);
                }
            }
        }
        true
    }
    fn apply_review_question_draft(
        &mut self,
        destination: DraftDestination,
        context: &QuestionContext,
        files: &[crate::diff::DiffFile],
        text: String,
    ) {
        match &mut self.mode {
            AppMode::DiffViewer(s) => {
                if s.feedback_editing
                    || s.editing_general
                    || s.editing_line_comment
                    || s.editing_file_comment
                    || s.editing_suggestion
                {
                    s.questions.error = Some("Finish or cancel the existing comment editor before transferring this draft; its text is retained".into());
                    return;
                }
                if destination == DraftDestination::Inline {
                    let Some(anchor) = &context.anchor else {
                        return;
                    };
                    let current = QuestionContext::from_diff(s).anchor;
                    if current.as_ref() != Some(anchor) {
                        s.questions.error = Some("Return to the original selected diff line before transferring this inline draft".into());
                        return;
                    }
                }
            }
            AppMode::AiReview(s) => {
                if s.finding_editor.is_some() || s.post_confirm.is_some() {
                    s.questions.error = Some("Finish the existing comment editor or posting dialog before transferring this draft; its text is retained".into());
                    return;
                }
                let anchor = if destination == DraftDestination::Inline {
                    context.anchor.as_ref()
                } else {
                    None
                };
                let diff_hunk = anchor.and_then(|a| {
                    let file = files.iter().find(|f| f.path == a.path)?;
                    let location = file.resolve_source_line(a.side, a.end.line_on(a.side)?)?;
                    crate::app::ai_review::diff_hunk_for_location(files, &a.path, a.side, location)
                });
                if destination == DraftDestination::Inline && diff_hunk.is_none() {
                    s.questions.error = Some("Inline destination is outside the reviewed diff; ask again with a valid selected line".into());
                    return;
                }
                let finding = crate::app::ai_review::AiReviewFinding {
                    path: anchor.map(|a| a.path.clone()),
                    line: anchor
                        .and_then(|a| a.end.line_on(a.side))
                        .and_then(|l| u32::try_from(l).ok()),
                    side: anchor.map(|a| a.side),
                    body: text.clone(),
                    diff_hunk,
                    skipped: false,
                    published: false,
                };
                s.findings.push(finding);
                s.selected = s.findings.len() - 1;
                s.finding_editor = Some(TextEditor::new(text));
                s.questions.draft = None;
                s.questions.open = false;
                return;
            }
            _ => return,
        }
        // Existing comment workflows preserve their anchor/severity and prior
        // text. Opening their editor is local and never submits a comment.
        match destination {
            DraftDestination::General => self.diff_review_start_general_feedback(),
            DraftDestination::Inline => self.diff_review_start_line_comment(),
        }
        if let AppMode::DiffViewer(s) = &mut self.mode {
            let existing = s.feedback_editor.text().to_string();
            let joined = if existing.trim().is_empty() {
                text
            } else {
                format!("{existing}\n\n{text}")
            };
            s.reset_feedback_editor(joined);
            s.questions.draft = None;
            s.questions.open = false;
        }
    }
}
