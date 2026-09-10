#![allow(dead_code)]

use crate::app::{App, AppMode};
use crate::db::expert_assist::{ConsultationOrigin, ConsultationRequest, ExpertProfile};
use crate::db::expert_assist::{ConsultationOutcome, ConsultationOwner};
use crate::headless::HeadlessExecutionPolicy;
use crate::headless::job::HeadlessJobLimits;
use crate::headless::job::{HeadlessJobHandle, HeadlessJobStatus};
use crate::project::AgentKind;
use crate::prompts::{PromptContext, PromptId, resolve_prompt};
use anyhow::{Context, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpertAssistField {
    Question,
    Criteria,
    AttemptedFixes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpertAssistPhase {
    Draft,
    Running,
    MissingEvidence,
    Failed,
    Ready,
}

#[derive(Debug, Clone)]
pub struct ExpertAssistState {
    pub consultation_id: Option<String>,
    pub request_revision: i64,
    pub question: String,
    pub acceptance_criteria: String,
    pub attempted_fixes: String,
    pub field: ExpertAssistField,
    pub phase: ExpertAssistPhase,
    pub status: String,
    pub response: Option<String>,
    pub handoff_body: Option<String>,
    pub handoff_revision: i64,
    pub editing_handoff: bool,
    pub handoff_buffer: String,
    pub evidence_digest: Option<String>,
}

pub(crate) struct ExpertJobRuntime {
    pub consultation_id: String,
    pub attempt_id: String,
    pub owner: ConsultationOwner,
    pub handle: HeadlessJobHandle,
}

impl ExpertAssistState {
    pub fn new() -> Self {
        Self {
            consultation_id: None,
            request_revision: 1,
            question: String::new(),
            acceptance_criteria: String::new(),
            attempted_fixes: String::new(),
            field: ExpertAssistField::Question,
            phase: ExpertAssistPhase::Draft,
            status: "Fill in the question and acceptance criteria.".into(),
            response: None,
            handoff_body: None,
            handoff_revision: 0,
            editing_handoff: false,
            handoff_buffer: String::new(),
            evidence_digest: None,
        }
    }

    pub fn active_text_mut(&mut self) -> &mut String {
        match self.field {
            ExpertAssistField::Question => &mut self.question,
            ExpertAssistField::Criteria => &mut self.acceptance_criteria,
            ExpertAssistField::AttemptedFixes => &mut self.attempted_fixes,
        }
    }

    pub fn next_field(&mut self) {
        self.field = match self.field {
            ExpertAssistField::Question => ExpertAssistField::Criteria,
            ExpertAssistField::Criteria => ExpertAssistField::AttemptedFixes,
            ExpertAssistField::AttemptedFixes => ExpertAssistField::Question,
        };
    }
}

impl App {
    pub(crate) fn open_expert_assist_form(&mut self) {
        self.mode = AppMode::ExpertAssist(ExpertAssistState::new());
        self.message = None;
    }

    pub(crate) fn cancel_expert_assist(&mut self) {
        self.mode = AppMode::Normal;
    }

    pub(crate) fn submit_expert_assist_form(&mut self) {
        let AppMode::ExpertAssist(state) = std::mem::replace(&mut self.mode, AppMode::Normal)
        else {
            return;
        };
        if state.question.trim().is_empty() || state.acceptance_criteria.trim().is_empty() {
            self.mode = AppMode::ExpertAssist(state);
            return;
        }
        let context = PromptContext::new()
            .with("question", state.question.clone())
            .with("acceptance_criteria", state.acceptance_criteria.clone())
            .with("attempted_fixes", state.attempted_fixes.clone())
            .with("evidence_packet", "[evidence collection pending]")
            .with("effective_access", "packet_only; no tools");
        let rendered = resolve_prompt(PromptId::ExpertAssistConsult, &AgentKind::Claude, &context);
        let Some((project, feature)) = self.selected_feature() else {
            self.message = Some("Select an agent feature before asking an expert.".into());
            self.mode = AppMode::ExpertAssist(state);
            return;
        };
        let (session_id, tmux_window) = match self.selection {
            crate::app::Selection::Session(_, _, si) => feature
                .sessions
                .get(si)
                .map(|session| (session.id.clone(), session.tmux_window.clone()))
                .unwrap_or_else(|| (feature.id.clone(), "claude".into())),
            _ => (feature.id.clone(), "claude".into()),
        };
        let (tmux_window_id, tmux_pane_id) =
            crate::tmux::TmuxManager::resolve_view_target_ids(&feature.tmux_session, &tmux_window)
                .unwrap_or_else(|_| ("unresolved-window".into(), "unresolved-pane".into()));
        let origin = ConsultationOrigin {
            project_id: project.id.clone(),
            feature_id: feature.id.clone(),
            session_id,
            launch_generation: feature.created_at.to_rfc3339(),
            provider_session_id: None,
            workdir: feature.workdir.clone(),
            repository_identity: project.repo.to_string_lossy().into_owned(),
            tmux_server_identity: "current-tmux-server".into(),
            tmux_session_id: feature.tmux_session.clone(),
            tmux_window_id,
            tmux_pane_id,
        };
        let request = ConsultationRequest {
            question: state.question.clone(),
            acceptance_criteria: state
                .acceptance_criteria
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect(),
            attempted_fixes: state
                .attempted_fixes
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect(),
            evidence: Vec::new(),
            profile: ExpertProfile {
                harness: AgentKind::Claude,
                binary: "claude".into(),
                model: "configured-expert".into(),
                policy: HeadlessExecutionPolicy::PacketOnly,
                limits: HeadlessJobLimits::default(),
            },
            prompt_id: PromptId::ExpertAssistConsult.as_str().into(),
            template_source: "built-in".into(),
            rendered_prompt: rendered.clone(),
        };
        let consultation_id = match self.db.as_ref() {
            Some(db) => match db.create_expert_consultation(&origin, &request) {
                Ok(id) => id,
                Err(error) => {
                    self.message = Some(format!("Error: could not save consultation: {error:#}"));
                    self.mode = AppMode::ExpertAssist(state);
                    return;
                }
            },
            None => {
                self.message = Some("Error: consultation storage is unavailable.".into());
                self.mode = AppMode::ExpertAssist(state);
                return;
            }
        };
        let digest = format!("draft:{}", consultation_id);
        let _ = self.precall_gate_with_metadata(
            crate::app::precall::PrecallAction::ExpertAssistConsult,
            &AgentKind::Claude,
            &rendered,
            Some(consultation_id),
            Some(state.request_revision),
            Some(digest),
        );
    }

    pub(crate) fn poll_expert_assist(&mut self) -> bool {
        let Some(runtime) = self.expert_job.as_ref() else {
            return false;
        };
        let progress = runtime.handle.progress();
        if let AppMode::ExpertAssist(state) = &mut self.mode {
            state.phase = ExpertAssistPhase::Running;
            if let Some(activity) = progress.activity {
                state.status = activity;
            }
        }
        let result = match runtime.handle.try_result() {
            Ok(Some(result)) => result,
            Ok(None) => return true,
            Err(error) => {
                self.message = Some(format!("Error: expert job result unavailable: {error:#}"));
                self.expert_job = None;
                return true;
            }
        };
        let runtime = self.expert_job.take().expect("runtime existed above");
        let response = result.response.clone();
        let error = result.error.clone();
        let outcome = ConsultationOutcome {
            status: result.status,
            response: result.response,
            error: result.error,
            usage: result.usage,
            usage_complete: result.usage_complete,
            elapsed_millis: result.elapsed.as_millis().min(u64::MAX as u128) as u64,
        };
        if let Some(db) = self.db.as_ref()
            && let Err(error) = db.finish_expert_attempt(
                &runtime.consultation_id,
                &runtime.attempt_id,
                &runtime.owner,
                outcome,
            )
        {
            self.message = Some(format!("Error: could not save expert result: {error:#}"));
        }
        if let AppMode::ExpertAssist(state) = &mut self.mode {
            match result.status {
                HeadlessJobStatus::Completed if response.is_some() => {
                    state.phase = ExpertAssistPhase::Ready;
                    state.response = response.clone();
                    state.status = "Advice is ready to review.".into();
                    if let Some(db) = self.db.as_ref()
                        && let Ok(Some(current)) = db.expert_consultation(&runtime.consultation_id)
                        && let Some(body) = response
                        && db
                            .stage_expert_handoff(&runtime.consultation_id, current.revision, &body)
                            .unwrap_or(false)
                        && let Ok(Some(staged)) = db.expert_consultation(&runtime.consultation_id)
                    {
                        state.handoff_body = Some(body);
                        state.handoff_revision = staged.handoff_revision;
                    }
                }
                HeadlessJobStatus::Incomplete => {
                    state.phase = ExpertAssistPhase::MissingEvidence;
                    state.status = error
                        .clone()
                        .unwrap_or_else(|| "Expert needs more evidence.".into());
                }
                _ => {
                    state.phase = ExpertAssistPhase::Failed;
                    state.status = error.unwrap_or_else(|| "Expert consultation failed.".into());
                }
            }
        }
        true
    }

    pub(crate) fn save_expert_handoff_edit(&mut self, id: &str, body: &str) -> bool {
        let Some(db) = self.db.as_ref() else {
            return false;
        };
        let Ok(Some(current)) = db.expert_consultation(id) else {
            return false;
        };
        if !db
            .stage_expert_handoff(id, current.revision, body)
            .unwrap_or(false)
        {
            return false;
        }
        if let AppMode::ExpertAssist(state) = &mut self.mode {
            state.handoff_body = Some(body.to_string());
            state.response = Some(body.to_string());
            state.handoff_revision = db
                .expert_consultation(id)
                .ok()
                .flatten()
                .map_or(state.handoff_revision, |row| row.handoff_revision);
            state.editing_handoff = false;
            state.status = "Edited handoff is staged and ready.".into();
        }
        true
    }

    pub(crate) fn send_expert_handoff(&mut self) -> Result<()> {
        let (id, body) = match &self.mode {
            AppMode::ExpertAssist(state) if state.phase == ExpertAssistPhase::Ready => {
                (state.consultation_id.clone(), state.handoff_body.clone())
            }
            _ => return Ok(()),
        };
        let id = id.context("expert consultation is missing")?;
        let body = body.context("expert handoff is missing")?;
        let Some(db) = self.db.as_ref() else {
            anyhow::bail!("consultation storage is unavailable")
        };
        let consultation = db
            .expert_consultation(&id)?
            .context("consultation is missing")?;
        let origin = &consultation.origin;
        anyhow::ensure!(
            crate::tmux::TmuxManager::session_exists(&origin.tmux_session_id),
            "origin tmux session is gone"
        );
        let window = origin.tmux_window_id.clone();
        let (window_id, pane_id) =
            crate::tmux::TmuxManager::resolve_view_target_ids(&origin.tmux_session_id, &window)?;
        anyhow::ensure!(
            window_id == origin.tmux_window_id && pane_id == origin.tmux_pane_id,
            "origin target changed; refresh consultation before sending"
        );
        let owner = crate::db::expert_assist::ConsultationOwner::current()?;
        let Some(delivery) = db.claim_expert_delivery(&id, consultation.revision, &owner)? else {
            anyhow::bail!("handoff is no longer ready to send")
        };
        let submitted =
            crate::tmux::TmuxManager::send_literal(&origin.tmux_session_id, &window, &body)
                .and_then(|_| {
                    crate::tmux::TmuxManager::send_key_name(
                        &origin.tmux_session_id,
                        &window,
                        "Enter",
                    )
                })
                .is_ok();
        let finished = db.finish_expert_delivery(&id, &delivery, &owner, submitted)?;
        anyhow::ensure!(finished, "delivery state changed while sending");
        if submitted {
            self.message = Some("Expert handoff sent to the validated origin session.".into());
            self.mode = AppMode::Normal;
        } else {
            anyhow::bail!("handoff delivery is unknown; it will not be retried automatically")
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_cycles_fields_and_starts_in_draft() {
        let mut state = ExpertAssistState::new();
        assert_eq!(state.phase, ExpertAssistPhase::Draft);
        assert_eq!(state.field, ExpertAssistField::Question);
        state.next_field();
        assert_eq!(state.field, ExpertAssistField::Criteria);
        state.next_field();
        assert_eq!(state.field, ExpertAssistField::AttemptedFixes);
        state.next_field();
        assert_eq!(state.field, ExpertAssistField::Question);
    }
}
