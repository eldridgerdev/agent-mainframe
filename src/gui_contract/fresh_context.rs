//! Fresh-context continuation without a TUI view or mode transition.
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};

use crate::project::{AgentKind, SessionKind};

use super::{FeatureTarget, GuiError, GuiHandle, GuiResult, SessionTarget};

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

impl GuiHandle {
    pub fn fresh_context_preview(
        &mut self,
        target: &SessionTarget,
    ) -> GuiResult<FreshContextPreview> {
        self.refresh_store()?;
        let (pi, fi, si) = self.locate_session(target)?;
        let feature = &self.app.store.projects[pi].features[fi];
        let session = &feature.sessions[si];
        if !session.kind.is_agent_harness() {
            return Err(GuiError::conflict(
                "Fresh context starts from an agent session",
            ));
        }
        let prompt = self.app.fresh_context_seed(feature, &session.tmux_window);
        let label = self.app.fresh_context_label(feature);
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        // Session membership also makes a successful request single-use. Do
        // not hash volatile token counts or status readings from collectors.
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
        prompt.hash(&mut hash);
        label.hash(&mut hash);
        Ok(FreshContextPreview {
            revision: format!("{:016x}", hash.finish()),
            prompt,
            label,
        })
    }

    pub fn fresh_context_start(
        &mut self,
        target: &SessionTarget,
        request: FreshContextRequest,
    ) -> GuiResult<FreshContextResponse> {
        let preview = self.fresh_context_preview(target)?;
        if preview.revision != request.revision {
            return Err(GuiError::conflict(
                "The fresh-context source changed. Reload the context and review your draft before starting.",
            ));
        }
        if request.prompt.trim().is_empty() {
            return Err(GuiError::conflict(
                "Enter a continuation prompt before starting",
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
            Some(preview.label),
            request.approved,
        )?;
        Ok(FreshContextResponse {
            target: added.target,
            draft_prompt: request.prompt,
        })
    }
}
