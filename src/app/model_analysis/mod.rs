//! Scoped model advice and the final pre-launch validation.
mod discovery;
mod session_control;
#[cfg(test)]
mod tests;

use super::{
    App, AppMode, PendingPlanLaunch, PlanInterviewPhase, PreparedFeatureLaunch,
    Selection as DashboardSelection,
};
use crate::{
    headless::HeadlessRunner,
    model_evidence::{
        Recommendation, ResearchNote, prompt_context, prompt_context_for_task,
        render_analysis_prompt, research_notes, validate_response,
    },
    model_options::{EligibleOptions, HarnessCapability, LaunchPath, ModelChoice},
    project::{AgentKind, SessionKind},
    prompts::{PromptContext, PromptId},
};
use anyhow::{Context, Result, ensure};
use chrono::Utc;
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Selection {
    pub choice: ModelChoice,
    pub project_id: String,
    pub repo: PathBuf,
    pub plan_hash: String,
    pub feature_id: String,
}
pub(crate) fn fingerprint(plan: &str) -> String {
    format!("{:x}", Sha256::digest(plan.as_bytes()))
}

#[derive(Debug)]
pub(crate) enum Status {
    Loading,
    Ready(Vec<Recommendation>),
    Insufficient,
    Error(String),
}
pub struct State {
    pub(crate) origin: Box<AppMode>,
    target: Target,
    pub(crate) status: Status,
    pub(crate) selected: usize,
    pub(crate) scroll: u16,
    pub(crate) show_sources: bool,
    launch: Option<PendingPlanLaunch>,
    apply: Option<Selection>,
    session_apply: Option<SessionApplication>,
    task_context: Option<String>,
    committing: bool,
    saved_feature_id: Option<String>,
    generation: u64,
}
#[derive(Debug, Clone, PartialEq)]
struct Target {
    project_id: String,
    repo: PathBuf,
    workdir: PathBuf,
    preferred: AgentKind,
    kind: TargetKind,
}
#[derive(Debug, Clone, PartialEq)]
enum TargetKind {
    Plan {
        prepared: Box<PreparedFeatureLaunch>,
        interview_key: String,
        plan: String,
    },
    Session {
        feature_id: String,
        session_id: String,
        tmux_session: String,
        window: String,
        session_kind: SessionKind,
        feature_name: String,
        summary: Option<String>,
        selected_plan_path: Option<PathBuf>,
        exact_conversation: Option<String>,
    },
}
impl Target {
    fn plan(&self) -> Option<(&PreparedFeatureLaunch, &str)> {
        match &self.kind {
            TargetKind::Plan { prepared, plan, .. } => Some((prepared.as_ref(), plan)),
            TargetKind::Session { .. } => None,
        }
    }

    fn is_session(&self) -> bool {
        matches!(self.kind, TargetKind::Session { .. })
    }

    fn task_context(&self) -> Result<(String, &'static str)> {
        match &self.kind {
            TargetKind::Plan { plan, .. } => Ok((plan.clone(), "implementation")),
            TargetKind::Session {
                feature_name,
                summary,
                selected_plan_path,
                ..
            } => {
                let mut context = format!("Current feature: {feature_name}");
                if let Some(summary) = summary.as_deref().filter(|s| !s.trim().is_empty()) {
                    context.push_str("\nFeature summary: ");
                    context.push_str(summary);
                }
                if let Some(plan) = super::plan::resolve_effective_plan_for(
                    &self.workdir,
                    selected_plan_path.as_deref(),
                ) {
                    let body = std::fs::read_to_string(plan.path())?;
                    context.push_str("\nCurrent plan:\n");
                    context.extend(body.chars().take(24_000));
                    context.push_str(&format!("\nPlan fingerprint: {}", fingerprint(&body)));
                }
                Ok((context, "existing agent session"))
            }
        }
    }
}

fn session_agent(kind: &SessionKind) -> Option<AgentKind> {
    match kind {
        SessionKind::Claude => Some(AgentKind::Claude),
        SessionKind::Codex => Some(AgentKind::Codex),
        SessionKind::Opencode => Some(AgentKind::Opencode),
        SessionKind::Pi => Some(AgentKind::Pi),
        _ => None,
    }
}

impl State {
    pub(crate) fn is_checking_setting(&self) -> bool {
        self.launch.is_some() || self.apply.is_some() || self.session_apply.is_some()
    }

    pub(crate) fn is_existing_session(&self) -> bool {
        self.target.is_session()
    }

    pub(crate) fn can_apply_session(&self) -> bool {
        matches!(
            &self.target.kind,
            TargetKind::Session {
                exact_conversation: Some(_),
                ..
            }
        ) && self.target.preferred == AgentKind::Codex
    }

    pub(crate) fn is_committing(&self) -> bool {
        self.committing
    }
}

#[derive(Clone)]
struct SessionApplication {
    request: session_control::Request,
    task_context: String,
    evidence: Vec<ResearchNote>,
}

pub(crate) struct RunInput {
    pub allowed: Vec<AgentKind>,
    pub preferred: AgentKind,
    pub workdir: PathBuf,
    pub templates: Vec<(AgentKind, String)>,
    pub context: PromptContext,
    pub cancelled: Arc<AtomicBool>,
}
fn run(input: &RunInput) -> Result<String> {
    let mut allowed = input.allowed.clone();
    let mut errors = vec![];
    while let Some(harness) = HeadlessRunner::select_configured(&input.preferred, &allowed) {
        ensure!(
            !input.cancelled.load(Ordering::Relaxed),
            "analysis cancelled"
        );
        let template = input
            .templates
            .iter()
            .find(|(h, _)| *h == harness)
            .map(|(_, t)| t)
            .ok_or_else(|| anyhow::anyhow!("missing configured analyzer template"))?;
        let prompt = render_analysis_prompt(template, &input.context)?;
        match HeadlessRunner::run_review_comment_draft(
            &harness,
            &input.workdir,
            &prompt,
            None,
            &input.cancelled,
            Duration::from_secs(120),
        ) {
            Ok(raw) => return Ok(raw),
            Err(e) => errors.push(format!("{}: {e}", harness.display_name())),
        }
        allowed.retain(|h| *h != harness);
    }
    anyhow::bail!(
        "No configured analyzer runner succeeded. {}",
        errors.join("; ")
    )
}

type Discover = fn(&Path, &[AgentKind], &AtomicBool) -> Result<Vec<HarnessCapability>>;
pub(crate) struct Work {
    job: Option<Job>,
    pub discover: Discover,
    pub runner: fn(&RunInput) -> Result<String>,
    next: u64,
    pub launch_args: Option<LaunchOverride>,
    pub now: fn() -> chrono::DateTime<Utc>,
    prepare_session:
        fn(&session_control::Request, &AtomicBool) -> Result<Box<dyn session_control::Prepared>>,
}
pub(crate) struct LaunchOverride {
    pub feature_id: String,
    pub args: Vec<String>,
    pub retry: bool,
}
impl Default for Work {
    fn default() -> Self {
        Self {
            job: None,
            discover: discovery::discover,
            runner: run,
            next: 0,
            launch_args: None,
            now: Utc::now,
            prepare_session: session_control::prepare,
        }
    }
}
struct Job {
    cancelled: Arc<AtomicBool>,
    rx: mpsc::Receiver<Completion>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}
struct Completion {
    generation: u64,
    target: Target,
    allowed: Vec<AgentKind>,
    result: Result<Outcome, String>,
}
enum Outcome {
    Advice(Vec<Recommendation>, Option<String>),
    Validated(EligibleOptions),
    SessionPrepared(Box<dyn session_control::Prepared>),
    SessionApplied,
}
impl Work {
    pub fn pending(&self) -> bool {
        self.job.is_some()
    }
    fn cancel(&mut self) {
        self.job = None;
    }
}

impl App {
    fn session_window_exists(&self, target: &Target) -> bool {
        match &target.kind {
            TargetKind::Session {
                tmux_session,
                window,
                ..
            } => {
                self.tmux.session_exists(tmux_session)
                    && self.tmux.window_exists(tmux_session, window)
            }
            _ => false,
        }
    }
    pub(crate) fn model_analysis_available(&self) -> bool {
        matches!(&self.mode,AppMode::PlanInterview(s) if s.phase==PlanInterviewPhase::Review && s.todo_origin.is_none() && s.pending_launch.as_ref().is_some_and(|p|p.todo_origin.is_none()))
    }
    fn session_model_target(&self, pi: usize, fi: usize, si: usize) -> Result<Target> {
        let project = self
            .store
            .projects
            .get(pi)
            .ok_or_else(|| anyhow::anyhow!("target project was deleted"))?;
        let feature = project
            .features
            .get(fi)
            .ok_or_else(|| anyhow::anyhow!("target feature was deleted"))?;
        let session = feature
            .sessions
            .get(si)
            .ok_or_else(|| anyhow::anyhow!("target session was deleted"))?;
        let preferred = session_agent(&session.kind)
            .ok_or_else(|| anyhow::anyhow!("model advice requires an agent harness session"))?;
        ensure!(feature.workdir.is_dir(), "target workdir was removed");
        Ok(Target {
            project_id: project.id.clone(),
            repo: project.repo.clone(),
            workdir: feature.workdir.clone(),
            preferred,
            kind: TargetKind::Session {
                feature_id: feature.id.clone(),
                session_id: session.id.clone(),
                tmux_session: feature.tmux_session.clone(),
                window: session.tmux_window.clone(),
                session_kind: session.kind.clone(),
                feature_name: feature.name.clone(),
                summary: feature.summary.clone(),
                selected_plan_path: feature.selected_plan_path.clone(),
                exact_conversation: session
                    .token_usage_source
                    .as_ref()
                    .filter(|source| {
                        source.provider == crate::token_tracking::TokenUsageProvider::Codex
                            && session.token_usage_source_match
                                == Some(crate::project::TokenUsageSourceMatch::Exact)
                            && !source.id.trim().is_empty()
                    })
                    .map(|source| source.id.clone()),
            },
        })
    }
    fn model_target(&self, mode: &AppMode) -> Result<Target> {
        let AppMode::PlanInterview(s) = mode else {
            return match mode {
                AppMode::Normal => {
                    let DashboardSelection::Session(pi, fi, si) = self.selection else {
                        anyhow::bail!("select an agent session to request model advice")
                    };
                    self.session_model_target(pi, fi, si)
                }
                AppMode::Viewing(view) => {
                    let pi = self
                        .store
                        .projects
                        .iter()
                        .position(|p| p.name == view.project_name)
                        .ok_or_else(|| anyhow::anyhow!("target project was deleted"))?;
                    let project = &self.store.projects[pi];
                    let fi = project
                        .features
                        .iter()
                        .position(|f| f.tmux_session == view.session)
                        .ok_or_else(|| anyhow::anyhow!("target feature was deleted"))?;
                    let si = project.features[fi]
                        .sessions
                        .iter()
                        .position(|s| s.tmux_window == view.window && s.kind == view.session_kind)
                        .ok_or_else(|| anyhow::anyhow!("target session changed or was deleted"))?;
                    self.session_model_target(pi, fi, si)
                }
                _ => anyhow::bail!("model advice requires plan review or an agent session"),
            };
        };
        ensure!(
            s.phase == PlanInterviewPhase::Review && s.todo_origin.is_none(),
            "unsupported model advice entry point"
        );
        let prepared = s
            .pending_launch
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no new feature launch"))?;
        ensure!(
            prepared.todo_origin.is_none(),
            "TODO model advice is not supported"
        );
        let project = self
            .store
            .projects
            .iter()
            .find(|p| p.name == prepared.project_name)
            .ok_or_else(|| anyhow::anyhow!("target project was deleted"))?;
        let plan = s
            .synthesized_plan
            .clone()
            .filter(|p| !p.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("reviewed implementation plan is missing"))?;
        ensure!(
            s.workdir == prepared.workdir && prepared.workdir.is_dir(),
            "target workdir changed or was removed"
        );
        Ok(Target {
            project_id: project.id.clone(),
            repo: project.repo.clone(),
            workdir: prepared.workdir.clone(),
            preferred: prepared.agent.clone(),
            kind: TargetKind::Plan {
                prepared: Box::new(prepared),
                interview_key: s.interview_key.clone(),
                plan,
            },
        })
    }
    pub(crate) fn open_model_analysis(&mut self) -> Result<()> {
        let target = self.model_target(&self.mode)?;
        let origin = Box::new(std::mem::replace(&mut self.mode, AppMode::Normal));
        self.mode = AppMode::ModelAnalysis(Box::new(State {
            origin,
            target,
            status: Status::Loading,
            selected: 0,
            scroll: 0,
            show_sources: false,
            launch: None,
            apply: None,
            session_apply: None,
            task_context: None,
            committing: false,
            saved_feature_id: None,
            generation: 0,
        }));
        self.retry_model_analysis();
        Ok(())
    }
    pub(crate) fn cancel_model_analysis(&mut self) {
        if matches!(&self.mode, AppMode::ModelAnalysis(s) if s.committing) {
            return;
        }
        self.model_analysis_work.cancel();
        if let AppMode::ModelAnalysis(state) = std::mem::replace(&mut self.mode, AppMode::Normal) {
            self.mode = *state.origin;
        }
        self.message = None;
    }
    pub(crate) fn retry_model_analysis(&mut self) {
        if matches!(&self.mode, AppMode::ModelAnalysis(s) if s.committing) {
            return;
        }
        self.model_analysis_work.cancel();
        let AppMode::ModelAnalysis(state) = &self.mode else {
            return;
        };
        let target = state.target.clone();
        let launch = state.launch.is_some() || state.apply.is_some();
        let session_apply = state.session_apply.clone();
        let setup = (|| -> Result<_> {
            ensure!(
                self.model_target(&state.origin)? == target,
                "implementation task changed; return to plan review"
            );
            if let Some(id) = &state.saved_feature_id {
                let (prepared, _) = target
                    .plan()
                    .ok_or_else(|| anyhow::anyhow!("invalid launch target"))?;
                ensure!(
                    self.store
                        .projects
                        .iter()
                        .filter(|p| p.id == target.project_id)
                        .flat_map(|p| &p.features)
                        .any(|f| f.id == *id
                            && f.workdir == prepared.workdir
                            && f.agent == prepared.agent),
                    "saved launch target was deleted or changed; return to plan review"
                );
            }
            let allowed = self.allowed_agents_for_repo(&target.repo);
            let choice_allowed = if let Some((prepared, _)) = target.plan() {
                allowed
                    .iter()
                    .filter(|h| self.ensure_agent_mode_supported(h, &prepared.mode).is_ok())
                    .cloned()
                    .collect::<Vec<_>>()
            } else {
                allowed
                    .iter()
                    .filter(|h| **h == target.preferred)
                    .cloned()
                    .collect::<Vec<_>>()
            };
            let notes = if launch || session_apply.is_some() {
                vec![]
            } else {
                let db = self
                    .db
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("project evidence database unavailable"))?;
                db.save_model_research(&target.project_id, &target.repo, &research_notes())?;
                db.load_model_research(&target.project_id, &target.repo)?
            };
            let templates = allowed
                .iter()
                .map(|h| {
                    (
                        h.clone(),
                        self.resolve_headless_template(
                            PromptId::ModelAnalysis,
                            h,
                            &target.repo,
                            &target.workdir,
                        )
                        .0,
                    )
                })
                .collect::<Vec<_>>();
            Ok((allowed, choice_allowed, notes, templates))
        })();
        let (allowed, choice_allowed, notes, templates) = match setup {
            Ok(v) => v,
            Err(e) => {
                if let AppMode::ModelAnalysis(s) = &mut self.mode {
                    s.status = Status::Error(e.to_string())
                };
                return;
            }
        };
        self.model_analysis_work.next += 1;
        let generation = self.model_analysis_work.next;
        if let AppMode::ModelAnalysis(s) = &mut self.mode {
            s.generation = generation;
            s.status = Status::Loading;
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancelled.clone();
        let discover = self.model_analysis_work.discover;
        let runner = self.model_analysis_work.runner;
        let now_fn = self.model_analysis_work.now;
        let prepare_session = self.model_analysis_work.prepare_session;
        let (tx, rx) = mpsc::channel();
        self.model_analysis_work.job = Some(Job { cancelled, rx });
        std::thread::spawn(move || {
            let result = (|| -> Result<Outcome> {
                if let Some(application) = session_apply {
                    ensure!(
                        target.task_context()?.0 == application.task_context,
                        "Session task changed; retry analysis"
                    );
                    ensure!(
                        application
                            .evidence
                            .iter()
                            .all(|n| n.applies(&application.request.choice, now_fn())),
                        "Research expired; retry analysis"
                    );
                    return Ok(Outcome::SessionPrepared(prepare_session(
                        &application.request,
                        &worker_cancel,
                    )?));
                }
                let caps = discover(&target.workdir, &choice_allowed, &worker_cancel)?;
                ensure!(!worker_cancel.load(Ordering::Relaxed), "analysis cancelled");
                let options = EligibleOptions::new(&choice_allowed, &caps, LaunchPath::Interactive);
                if launch {
                    return Ok(Outcome::Validated(options));
                }
                let now = now_fn();
                if !options
                    .choices()
                    .iter()
                    .any(|c| notes.iter().any(|n| n.applies(c, now)))
                {
                    return Ok(Outcome::Advice(vec![], None));
                }
                let (task_context, task_phase) = target.task_context()?;
                let context = if target.is_session() {
                    prompt_context_for_task(&task_context, task_phase, &options, &notes, now)?
                } else {
                    prompt_context(&task_context, &options, &notes, now)?
                };
                let raw = runner(&RunInput {
                    allowed: allowed.clone(),
                    preferred: target.preferred.clone(),
                    workdir: target.workdir.clone(),
                    templates,
                    context,
                    cancelled: worker_cancel.clone(),
                })?;
                ensure!(!worker_cancel.load(Ordering::Relaxed), "analysis cancelled");
                if target.is_session() {
                    ensure!(
                        target.task_context()?.0 == task_context,
                        "session task context changed during analysis; retry"
                    );
                }
                Ok(Outcome::Advice(
                    validate_response(&raw, &options, &notes, now_fn())?,
                    target.is_session().then_some(task_context),
                ))
            })()
            .map_err(|e| e.to_string());
            let _ = tx.send(Completion {
                generation,
                target,
                allowed,
                result,
            });
        });
    }
    pub(crate) fn poll_model_analysis(&mut self) -> bool {
        let result = self
            .model_analysis_work
            .job
            .as_ref()
            .map(|j| j.rx.try_recv());
        let completion = match result {
            Some(Ok(c)) => c,
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.model_analysis_work.cancel();
                if let AppMode::ModelAnalysis(s) = &mut self.mode {
                    s.status = Status::Error(if s.committing {
                        "Settings verification worker stopped; inspect the harness settings before retrying"
                    } else { "Analyzer worker stopped; retry when ready" }.into());
                    s.committing = false;
                    s.session_apply = None;
                }
                return true;
            }
            _ => return false,
        };
        let AppMode::ModelAnalysis(state) = &self.mode else {
            self.model_analysis_work.cancel();
            return false;
        };
        if state.generation != completion.generation || state.target != completion.target {
            return false;
        }
        let valid = self
            .model_target(&state.origin)
            .is_ok_and(|t| t == completion.target)
            && self.allowed_agents_for_repo(&completion.target.repo) == completion.allowed
            && state.saved_feature_id.as_ref().is_none_or(|id| {
                let Some((prepared, _)) = completion.target.plan() else {
                    return false;
                };
                self.store
                    .projects
                    .iter()
                    .filter(|p| p.id == completion.target.project_id)
                    .flat_map(|p| &p.features)
                    .any(|f| {
                        f.id == *id && f.workdir == prepared.workdir && f.agent == prepared.agent
                    })
            });
        let was_committing = state.committing;
        self.model_analysis_work.cancel();
        if let AppMode::ModelAnalysis(s) = &mut self.mode {
            s.committing = false;
        }
        if !valid {
            if was_committing {
                self.log_warn("model", "Session target changed while verifying its settings update; inspect that conversation's settings".into());
            }
            if let AppMode::ModelAnalysis(s) = &mut self.mode {
                s.status = Status::Error(
                    if was_committing { "Target changed while verifying the update; inspect the conversation's settings" }
                    else { "Target or configured harnesses changed; return to the originating workflow" }.into(),
                );
                s.session_apply = None;
            }
            return true;
        }
        match completion.result {
            Err(e) => {
                if let AppMode::ModelAnalysis(s) = &mut self.mode {
                    s.status = Status::Error(e);
                    s.session_apply = None;
                }
            }
            Ok(Outcome::Advice(choices, task_context)) => {
                if let AppMode::ModelAnalysis(s) = &mut self.mode {
                    s.task_context = task_context;
                    s.selected = 0;
                    s.status = if choices.is_empty() {
                        Status::Insufficient
                    } else {
                        Status::Ready(choices)
                    }
                }
            }
            Ok(Outcome::SessionPrepared(prepared)) => {
                let checked = (|| -> Result<_> {
                    let AppMode::ModelAnalysis(s) = &self.mode else {
                        unreachable!()
                    };
                    let application = s
                        .session_apply
                        .as_ref()
                        .context("Missing session selection")?;
                    ensure!(
                        s.target.task_context()?.0 == application.task_context,
                        "Session task changed; retry analysis"
                    );
                    ensure!(
                        self.session_window_exists(&s.target),
                        "Session stopped or was removed; use its own model picker"
                    );
                    ensure!(
                        application.evidence.iter().all(|n| n.applies(
                            &application.request.choice,
                            (self.model_analysis_work.now)()
                        )),
                        "Research expired; retry analysis"
                    );
                    Ok(())
                })();
                if let Err(e) = checked {
                    if let AppMode::ModelAnalysis(s) = &mut self.mode {
                        s.status = Status::Error(e.to_string());
                        s.session_apply = None;
                    }
                } else {
                    let AppMode::ModelAnalysis(s) = &mut self.mode else {
                        unreachable!()
                    };
                    s.committing = true;
                    let generation = s.generation;
                    let target = completion.target;
                    let allowed = completion.allowed;
                    let cancelled = Arc::new(AtomicBool::new(false));
                    let worker_cancel = cancelled.clone();
                    let (tx, rx) = mpsc::channel();
                    self.model_analysis_work.job = Some(Job { cancelled, rx });
                    std::thread::spawn(move || {
                        let result = prepared
                            .commit(&worker_cancel)
                            .map(|_| Outcome::SessionApplied)
                            .map_err(|e| format!("{e:#}"));
                        let _ = tx.send(Completion {
                            generation,
                            target,
                            allowed,
                            result,
                        });
                    });
                }
            }
            Ok(Outcome::SessionApplied) => {
                let AppMode::ModelAnalysis(s) = std::mem::replace(&mut self.mode, AppMode::Normal)
                else {
                    unreachable!()
                };
                self.mode = *s.origin;
                self.message = Some(
                    "Model and effort verified for subsequent turns in this conversation.".into(),
                );
            }
            Ok(Outcome::Validated(options)) => {
                let selection = match &self.mode {
                    AppMode::ModelAnalysis(s) => s.apply.as_ref().or(completion
                        .target
                        .plan()
                        .and_then(|(p, _)| p.model_selection.as_ref())),
                    _ => None,
                };
                let validated = selection
                    .filter(|selection| {
                        research_notes()
                            .iter()
                            .any(|n| n.applies(&selection.choice, (self.model_analysis_work.now)()))
                    })
                    .ok_or_else(|| anyhow::anyhow!("missing selected setting"))
                    .and_then(|s| options.revalidate(&s.choice))
                    .and_then(|c| options.interactive_args(c));
                match validated {
                    Err(e) => {
                        if let AppMode::ModelAnalysis(s) = &mut self.mode {
                            s.status = Status::Error(format!(
                                "Selected setting is no longer eligible: {e}"
                            ))
                        }
                    }
                    Ok(args) => {
                        let AppMode::ModelAnalysis(mut s) =
                            std::mem::replace(&mut self.mode, AppMode::Normal)
                        else {
                            return false;
                        };
                        if let Some(selection) = s.apply.take() {
                            self.mode = *s.origin;
                            if let AppMode::PlanInterview(p) = &mut self.mode {
                                let prepared = p.pending_launch.as_mut().expect("validated origin");
                                if prepared.session_name
                                    == format!("{} 1", prepared.agent.display_name())
                                {
                                    prepared.session_name =
                                        format!("{} 1", selection.choice.harness().display_name());
                                }
                                prepared.agent = selection.choice.harness().clone();
                                prepared.model_selection = Some(selection);
                            }
                            self.message=Some("Setting applied to the initial implementation launch. Accept the plan to start.".into());
                            return true;
                        }
                        let pending = s.launch.take().expect("validation launch");
                        let feature_id = pending
                            .prepared
                            .model_selection
                            .as_ref()
                            .expect("validated setting")
                            .feature_id
                            .clone();
                        let previous =
                            self.model_analysis_work
                                .launch_args
                                .replace(LaunchOverride {
                                    feature_id,
                                    args,
                                    retry: s.saved_feature_id.is_some(),
                                });
                        debug_assert!(previous.is_none());
                        let retry = pending.clone();
                        let launch_result = self.resume_validated_plan_launch(pending);
                        self.model_analysis_work.launch_args = None;
                        if let Err(e) = launch_result {
                            // Creation retains the row and tmux cleanup is scoped
                            // to this launch. A retry resolves that exact row.
                            s.status = Status::Error(e.to_string());
                            s.saved_feature_id = retry
                                .prepared
                                .model_selection
                                .as_ref()
                                .map(|selection| selection.feature_id.clone())
                                .filter(|id| {
                                    self.store
                                        .projects
                                        .iter()
                                        .flat_map(|p| &p.features)
                                        .any(|f| f.id == *id)
                                });
                            s.launch = Some(retry);
                            self.mode = AppMode::ModelAnalysis(s);
                        }
                    }
                }
            }
        }
        true
    }
    pub(crate) fn apply_model_analysis(&mut self) -> Result<()> {
        let AppMode::ModelAnalysis(s) = &self.mode else {
            return Ok(());
        };
        if s.launch.is_some() || s.apply.is_some() || s.session_apply.is_some() {
            return Ok(());
        }
        ensure!(
            self.model_target(&s.origin)? == s.target,
            "implementation task changed"
        );
        let Status::Ready(choices) = &s.status else {
            return Ok(());
        };
        let r = choices
            .get(s.selected)
            .ok_or_else(|| anyhow::anyhow!("no selected recommendation"))?;
        ensure!(
            r.evidence
                .iter()
                .all(|n| n.applies(&r.choice, (self.model_analysis_work.now)())),
            "research expired; retry analysis"
        );
        if s.target.is_session() {
            ensure!(
                s.can_apply_session(),
                "Live application requires an exactly identified Codex conversation; use the harness's own model picker"
            );
            ensure!(
                self.session_window_exists(&s.target),
                "Session stopped or was removed"
            );
            let TargetKind::Session {
                exact_conversation: Some(thread_id),
                ..
            } = &s.target.kind
            else {
                unreachable!()
            };
            let application = SessionApplication {
                request: session_control::Request {
                    thread_id: thread_id.clone(),
                    workdir: s.target.workdir.clone(),
                    choice: r.choice.clone(),
                },
                task_context: s
                    .task_context
                    .clone()
                    .context("Missing analyzed session context; retry analysis")?,
                evidence: r.evidence.clone(),
            };
            if let AppMode::ModelAnalysis(s) = &mut self.mode {
                s.session_apply = Some(application);
            }
            self.retry_model_analysis();
            return Ok(());
        }
        let (prepared, plan) = s.target.plan().expect("plan advice target");
        let selection = Selection {
            choice: r.choice.clone(),
            project_id: s.target.project_id.clone(),
            repo: s.target.repo.clone(),
            plan_hash: fingerprint(plan),
            feature_id: prepared
                .model_selection
                .as_ref()
                .map(|v| v.feature_id.clone())
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        };
        if let AppMode::ModelAnalysis(s) = &mut self.mode {
            s.apply = Some(selection);
        }
        self.retry_model_analysis();
        Ok(())
    }
    pub(crate) fn validate_model_plan_launch(&mut self, pending: PendingPlanLaunch) -> Result<()> {
        let target = self.model_target(&self.mode)?;
        let (prepared, plan) = target
            .plan()
            .ok_or_else(|| anyhow::anyhow!("model launch requires plan review"))?;
        let selection = pending
            .prepared
            .model_selection
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("missing selected setting"))?;
        ensure!(
            selection.project_id == target.project_id
                && selection.repo == target.repo
                && selection.plan_hash == fingerprint(&pending.plan)
                && pending.plan == plan
                && prepared.model_selection.as_ref() == Some(selection),
            "selected advice no longer matches the project or plan; run model advice again"
        );
        let origin = Box::new(std::mem::replace(&mut self.mode, AppMode::Normal));
        self.mode = AppMode::ModelAnalysis(Box::new(State {
            origin,
            target,
            status: Status::Loading,
            selected: 0,
            scroll: 0,
            show_sources: false,
            launch: Some(pending),
            apply: None,
            session_apply: None,
            task_context: None,
            committing: false,
            saved_feature_id: None,
            generation: 0,
        }));
        self.retry_model_analysis();
        Ok(())
    }
}
