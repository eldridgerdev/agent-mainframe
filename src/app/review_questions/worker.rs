use super::context::{self, AiDiffLoader, PreparedContext, QuestionContext};
use super::state::DraftDestination;
use crate::project::AgentKind;
use anyhow::{Result, ensure};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver},
};
use std::time::{Duration, Instant};

pub(crate) struct RunInput {
    pub harness: AgentKind,
    pub model: Option<String>,
    pub context: QuestionContext,
    pub prompt: String,
    pub cancelled: Arc<AtomicBool>,
    pub discovery: bool,
}

pub(crate) type Runner = fn(&RunInput) -> Result<String>;
pub(crate) fn run(input: &RunInput) -> Result<String> {
    let run = if input.discovery {
        crate::headless::HeadlessRunner::run_review_question
    } else {
        crate::headless::HeadlessRunner::run_review_comment_draft
    };
    run(
        &input.harness,
        &input.context.workdir,
        &input.prompt,
        input.model.as_deref(),
        &input.cancelled,
        Duration::from_secs(180),
    )
}

#[derive(Clone)]
pub(crate) enum Task {
    Answer {
        question: String,
        earlier: String,
    },
    Draft {
        destination: DraftDestination,
        question: String,
        answer: String,
        stamp: String,
    },
    Transfer {
        destination: DraftDestination,
        text: String,
        stamp: String,
    },
}

pub(crate) enum Outcome {
    Answer(PreparedContext, String),
    Draft(DraftDestination, String),
    Transfer(PreparedContext, DraftDestination, String),
}

pub(crate) struct Completion {
    pub owner: String,
    pub request: u64,
    pub version: String,
    pub result: Result<Outcome, String>,
}

pub(crate) struct Job {
    pub owner: String,
    pub request: u64,
    pub turn: usize,
    pub context: QuestionContext,
    pub task: Task,
    pub started: Instant,
    pub cancelled: Arc<AtomicBool>,
    pub receiver: Receiver<Completion>,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

pub(crate) struct Work {
    pub job: Option<Job>,
    pub runner: Runner,
    pub ai_diff: AiDiffLoader,
}
impl Default for Work {
    fn default() -> Self {
        Self {
            job: None,
            runner: run,
            ai_diff: context::load_ai_diff,
        }
    }
}

pub(crate) struct Start {
    pub owner: String,
    pub request: u64,
    pub turn: usize,
    pub context: QuestionContext,
    pub task: Task,
    pub harness: AgentKind,
    pub model: Option<String>,
    pub template: String,
}

impl Work {
    pub fn start(&mut self, start: Start) {
        self.job = None;
        let (sender, receiver) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        self.job = Some(Job {
            owner: start.owner.clone(),
            request: start.request,
            turn: start.turn,
            context: start.context.clone(),
            task: start.task.clone(),
            started: Instant::now(),
            cancelled: cancelled.clone(),
            receiver,
        });
        let runner = self.runner;
        let ai_diff = self.ai_diff;
        std::thread::spawn(move || {
            let result = execute(&start, runner, ai_diff, cancelled).map_err(|e| format!("{e:#}"));
            let _ = sender.send(Completion {
                owner: start.owner,
                request: start.request,
                version: start.context.version,
                result,
            });
        });
    }
}

fn execute(
    start: &Start,
    runner: Runner,
    ai_diff: AiDiffLoader,
    cancelled: Arc<AtomicBool>,
) -> Result<Outcome> {
    ensure!(!cancelled.load(Ordering::Relaxed), "Question cancelled");
    let prepared = context::prepare(&start.context, ai_diff)?;
    let (question, earlier, answer) = match &start.task {
        Task::Answer { question, earlier } => (question.as_str(), earlier.as_str(), ""),
        Task::Draft {
            question,
            answer,
            stamp,
            destination,
        } => {
            ensure!(
                *stamp == prepared.stamp,
                "Answer context changed; ask again before drafting"
            );
            if *destination == DraftDestination::Inline {
                context::validate_anchor(
                    start.context.anchor.as_ref().ok_or_else(|| {
                        anyhow::anyhow!(
                            "Select a diff line before asking to draft an inline comment"
                        )
                    })?,
                    &prepared.files,
                )?;
            }
            (question.as_str(), "", answer.as_str())
        }
        Task::Transfer {
            destination,
            text,
            stamp,
        } => {
            ensure!(
                *stamp == prepared.stamp,
                "Draft context changed; ask again before transferring the comment"
            );
            if *destination == DraftDestination::Inline {
                context::validate_anchor(
                    start
                        .context
                        .anchor
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("No inline destination"))?,
                    &prepared.files,
                )?;
            }
            ensure!(!cancelled.load(Ordering::Relaxed), "Draft cancelled");
            return Ok(Outcome::Transfer(prepared, *destination, text.clone()));
        }
    };
    let tokens = start.context.tokens(&prepared, question, earlier, answer);
    let prompt = crate::prompts::render_template(&start.template, &tokens);
    let input = RunInput {
        harness: start.harness.clone(),
        model: start.model.clone(),
        context: start.context.clone(),
        prompt,
        cancelled: cancelled.clone(),
        discovery: matches!(start.task, Task::Answer { .. }),
    };
    ensure!(!cancelled.load(Ordering::Relaxed), "Question cancelled");
    let text = runner(&input)?;
    ensure!(
        !text.trim().is_empty(),
        "The harness returned an empty answer; retry when ready"
    );
    ensure!(!cancelled.load(Ordering::Relaxed), "Question cancelled");
    ensure!(
        prepared.stamp == context::repository_stamp(&start.context.workdir)?,
        "Repository changed while answering; refresh the review and ask again"
    );
    match start.task {
        Task::Answer { .. } => Ok(Outcome::Answer(prepared, text)),
        Task::Draft { destination, .. } => Ok(Outcome::Draft(destination, text)),
        Task::Transfer { .. } => unreachable!(),
    }
}
