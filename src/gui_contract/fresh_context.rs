//! Fresh-context continuation without a TUI view or mode transition.
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};

use crate::project::{AgentKind, SessionKind};

use super::{FeatureTarget, GuiError, GuiErrorKind, GuiHandle, GuiResult, SessionTarget};

#[derive(Debug, Clone, Serialize)]
pub struct FreshContextPreview {
    pub revision: String,
    pub prompt: String,
    pub label: String,
}

#[derive(Debug, Deserialize)]
pub struct FreshContextRequest {
    pub revision: String,
    pub prompt: String,
    pub approved: bool,
}

#[derive(Debug, Serialize)]
pub struct FreshContextResponse {
    pub target: SessionTarget,
    pub draft_prompt: String,
}

/// What a start must still agree with: which session it continues from and
/// what the new session will be. The seed is deliberately not part of it.
struct FreshContextSource {
    revision: String,
    label: String,
}

impl GuiHandle {
    pub fn fresh_context_preview(
        &mut self,
        target: &SessionTarget,
    ) -> GuiResult<FreshContextPreview> {
        let source = self.fresh_context_source(target)?;
        let (pi, fi, si) = self.locate_session(target)?;
        let feature = &self.app.store.projects[pi].features[fi];
        let prompt = self
            .app
            .fresh_context_seed(feature, &feature.sessions[si].tmux_window);
        Ok(FreshContextPreview {
            revision: source.revision,
            prompt,
            label: source.label,
        })
    }

    pub fn fresh_context_start(
        &mut self,
        target: &SessionTarget,
        request: FreshContextRequest,
    ) -> GuiResult<FreshContextResponse> {
        if request.prompt.trim().is_empty() {
            return Err(GuiError {
                kind: GuiErrorKind::Internal,
                message: "Enter a continuation prompt before starting".into(),
            });
        }
        let source = self.fresh_context_source(target)?;
        if source.revision != request.revision {
            return Err(GuiError::conflict(
                "The fresh-context source changed. Reload the context and review your draft before starting.",
            ));
        }
        let (pi, fi, _) = self.locate_session(target)?;
        // Match the TUI workflow: use the feature's configured harness.
        let kind = match self.app.store.projects[pi].features[fi].agent {
            AgentKind::Claude => SessionKind::Claude,
            AgentKind::Codex => SessionKind::Codex,
            AgentKind::Opencode => SessionKind::Opencode,
            AgentKind::Pi => SessionKind::Pi,
        };
        let added = self.add_session(
            FeatureTarget {
                project_id: target.project_id.clone(),
                feature_id: target.feature_id.clone(),
            },
            kind,
            Some(source.label),
            request.approved,
        )?;
        Ok(FreshContextResponse {
            target: added.target,
            draft_prompt: request.prompt,
        })
    }

    fn fresh_context_source(&mut self, target: &SessionTarget) -> GuiResult<FreshContextSource> {
        self.refresh_store()?;
        let (pi, fi, si) = self.locate_session(target)?;
        let feature = &self.app.store.projects[pi].features[fi];
        let session = &feature.sessions[si];
        if !session.kind.is_agent_harness() {
            return Err(GuiError::conflict(
                "Fresh context starts from an agent session",
            ));
        }
        let label = self.app.fresh_context_label(feature);
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        // Structural identity only. The seed (changed files, latest prompt)
        // moves whenever the source agent works, which is exactly when fresh
        // context is reached for, and start sends the user's draft rather
        // than the seed anyway. Session membership makes a successful
        // request single-use.
        format!(
            "{:?}",
            (
                target,
                &feature.workdir,
                &feature.agent,
                &session.kind,
                &session.tmux_window,
                &feature.tmux_session
            )
        )
        .hash(&mut hash);
        for item in &feature.sessions {
            item.id.hash(&mut hash);
        }
        label.hash(&mut hash);
        Ok(FreshContextSource {
            revision: format!("{:016x}", hash.finish()),
            label,
        })
    }
}
