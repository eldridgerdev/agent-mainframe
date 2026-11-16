use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::project::{AgentKind, VibeMode};

pub const AUTOMATION_REQUEST_TYPE: &str = "automation";
pub const AUTOMATION_RESULT_TYPE: &str = "automation-result";
pub const CREATE_PROJECT_ACTION: &str = "create_project";
pub const CREATE_SESSION_ACTION: &str = "create_session";
pub const CREATE_FEATURE_ACTION: &str = "create_feature";
pub const CREATE_BATCH_FEATURES_ACTION: &str = "create_batch_features";
pub const SEED_AI_REVIEW_ACTION: &str = "seed_ai_review";
pub const LIST_SESSIONS_ACTION: &str = "list_sessions";
pub const CAPTURE_SESSION_ACTION: &str = "capture_session";
pub const SEND_SESSION_INPUT_ACTION: &str = "send_session_input";
pub const REMOVE_SESSION_ACTION: &str = "remove_session";

/// Environment variable AMF sets in every agent session. It holds the agent's
/// own session id, which identifies the feature it is working in.
pub const FEATURE_SESSION_ID_ENV: &str = "AMF_FEATURE_SESSION_ID";

/// Which feature an automation request acts on. Resolution order:
/// `feature_session_id` (any session id in the feature, e.g. the agent's own),
/// then `tmux_session`, then `project_name` + `feature`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(default)]
pub struct FeatureTarget {
    pub project_name: String,
    pub feature: String,
    pub feature_session_id: Option<String>,
    pub tmux_session: Option<String>,
}

impl FeatureTarget {
    pub fn is_empty(&self) -> bool {
        self.project_name.trim().is_empty()
            && self.feature.trim().is_empty()
            && self.feature_session_id.as_deref().is_none_or(str::is_empty)
            && self.tmux_session.as_deref().is_none_or(str::is_empty)
    }

    /// Fill an empty target from the calling agent's environment, so a
    /// harness running inside AMF can act on its own feature without knowing
    /// its project or feature names. An explicit target is never touched.
    pub fn fill_from_env(&mut self, env_session_id: Option<String>) {
        if self.is_empty()
            && let Some(id) = env_session_id.filter(|id| !id.trim().is_empty())
        {
            self.feature_session_id = Some(id);
        }
    }
}

/// Picks one session inside the targeted feature: by id, else by label.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SessionSelector {
    pub session_id: Option<String>,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ListSessionsRequest {
    #[serde(flatten)]
    pub target: FeatureTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CaptureSessionRequest {
    #[serde(flatten)]
    pub target: FeatureTarget,
    #[serde(flatten)]
    pub session: SessionSelector,
    /// Most recent lines to return, including scrollback. Default 200.
    pub lines: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SendSessionInputRequest {
    #[serde(flatten)]
    pub target: FeatureTarget,
    #[serde(flatten)]
    pub session: SessionSelector,
    /// Text pasted into the session.
    pub text: Option<String>,
    /// Press Enter after `text`.
    pub run: bool,
    /// A tmux key name sent after the text, e.g. `C-c`, `Enter`, `q`.
    pub key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RemoveSessionRequest {
    #[serde(flatten)]
    pub target: FeatureTarget,
    #[serde(flatten)]
    pub session: SessionSelector,
}

macro_rules! session_ipc_payload {
    ($ty:ty, $action:expr) => {
        impl $ty {
            pub fn ipc_payload(&self) -> serde_json::Value {
                let mut payload = serde_json::to_value(self).expect("serializable request");
                payload["type"] = AUTOMATION_REQUEST_TYPE.into();
                payload["action"] = $action.into();
                payload
            }
        }
    };
}
session_ipc_payload!(ListSessionsRequest, LIST_SESSIONS_ACTION);
session_ipc_payload!(CaptureSessionRequest, CAPTURE_SESSION_ACTION);
session_ipc_payload!(SendSessionInputRequest, SEND_SESSION_INPUT_ACTION);
session_ipc_payload!(RemoveSessionRequest, REMOVE_SESSION_ACTION);

#[derive(Debug, Clone, Serialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub kind: String,
    pub label: String,
    pub tmux_window: Option<String>,
    /// Whether the session's tmux window is currently up.
    pub running: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ListSessionsResponse {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub action: &'static str,
    pub ok: bool,
    pub project_name: String,
    pub feature: String,
    pub tmux_session: String,
    pub sessions: Vec<SessionSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CaptureSessionResponse {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub action: &'static str,
    pub ok: bool,
    pub session_id: String,
    pub label: String,
    pub text: String,
}

/// Reply for `send_session_input` and `remove_session`.
#[derive(Debug, Clone, Serialize)]
pub struct SessionActionResponse {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub action: &'static str,
    pub ok: bool,
    pub session_id: String,
    pub label: String,
    pub message: String,
}

/// Deterministic fixture data for the AI Review pane. This is intentionally an
/// automation-only test seam: it never launches a model or contacts GitHub.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SeedAiReviewRequest {
    pub pr_number: u32,
    pub head_sha: String,
    pub summary: String,
    pub findings: Vec<SeedAiReviewFinding>,
    /// Open the completed fixture directly after persisting it. This is for
    /// deterministic visual tests; it deliberately does not resolve a PR or
    /// start a review.
    pub open: bool,
    pub workdir: Option<PathBuf>,
    pub repository: Option<String>,
    pub head_ref: Option<String>,
    /// Optional run attribution (harness/model/usage and the review memory the
    /// run read and sent), so a visual test can show the attribution and
    /// memory lines without running a review. Omitted, the row has none.
    pub attribution: Option<crate::app::ai_review::AiReviewAttribution>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SeedAiReviewFinding {
    pub path: Option<String>,
    pub line: Option<u32>,
    pub side: Option<crate::diff::DiffSide>,
    pub body: String,
    pub diff_hunk: Option<String>,
    pub skipped: bool,
    pub published: bool,
}

impl SeedAiReviewRequest {
    pub fn ipc_payload(&self) -> serde_json::Value {
        serde_json::json!({
            "type": AUTOMATION_REQUEST_TYPE,
            "action": SEED_AI_REVIEW_ACTION,
            "pr_number": self.pr_number,
            "head_sha": self.head_sha,
            "summary": self.summary,
            "findings": self.findings,
            "open": self.open,
            "workdir": self.workdir,
            "repository": self.repository,
            "head_ref": self.head_ref,
            "attribution": self.attribution,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SeedAiReviewResponse {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub action: &'static str,
    pub ok: bool,
    pub pr_number: u32,
    pub head_sha: String,
    pub finding_count: usize,
    pub opened: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CreateProjectRequest {
    pub path: PathBuf,
    pub project_name: String,
    pub preferred_agent: Option<AgentKind>,
    pub dry_run: bool,
    /// Register the project as containerized. Set only by the clone-from-URL
    /// flow, never accepted from an automation request or shown in its schema.
    #[serde(skip)]
    pub containerized: bool,
}

impl Default for CreateProjectRequest {
    fn default() -> Self {
        Self {
            path: PathBuf::new(),
            project_name: String::new(),
            preferred_agent: None,
            dry_run: false,
            containerized: false,
        }
    }
}

impl CreateProjectRequest {
    pub fn ipc_payload(&self) -> serde_json::Value {
        serde_json::json!({
            "type": AUTOMATION_REQUEST_TYPE,
            "action": CREATE_PROJECT_ACTION,
            "path": self.path,
            "project_name": self.project_name,
            "preferred_agent": self.preferred_agent,
            "dry_run": self.dry_run,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CreateSessionRequest {
    pub project_name: String,
    pub feature: String,
    /// Alternative to `project_name` + `feature`: any session id in the
    /// target feature. The CLI fills it from `$AMF_FEATURE_SESSION_ID` when no
    /// target is given.
    pub feature_session_id: Option<String>,
    pub tmux_session: Option<String>,
    pub kind: String,
    pub label: Option<String>,
    pub initial_prompt: Option<String>,
    pub submit_prompt: bool,
    /// Terminal sessions only: shell text pasted into the new terminal.
    pub command: Option<String>,
    /// Press Enter after `command`. Requires `command`.
    pub run: bool,
    /// Terminal sessions only: markdown reference notes for the user (commands
    /// to run, what to look for). Shown in the sidebar's Terminal Notes section
    /// and opened with `leader u`. On `reuse` they replace the earlier notes.
    pub notes: Option<String>,
    /// Terminal sessions only: reuse a live terminal with the same label
    /// instead of creating another.
    pub reuse: bool,
    pub dry_run: bool,
}

impl CreateSessionRequest {
    pub fn target(&self) -> FeatureTarget {
        FeatureTarget {
            project_name: self.project_name.clone(),
            feature: self.feature.clone(),
            feature_session_id: self.feature_session_id.clone(),
            tmux_session: self.tmux_session.clone(),
        }
    }

    pub fn ipc_payload(&self) -> serde_json::Value {
        let mut payload = serde_json::to_value(self).expect("serializable request");
        payload["type"] = AUTOMATION_REQUEST_TYPE.into();
        payload["action"] = CREATE_SESSION_ACTION.into();
        payload
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateSessionResponse {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub action: &'static str,
    pub ok: bool,
    pub dry_run: bool,
    pub tmux_session: String,
    pub tmux_window: Option<String>,
    pub session_id: Option<String>,
    /// An existing terminal with the requested label was reused (`reuse`).
    pub reused: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CreateFeatureRequest {
    /// Create the feature stopped, without launching sessions.
    pub no_start: bool,
    pub project_name: String,
    pub branch: String,
    pub agent: AgentKind,
    pub mode: VibeMode,
    pub review: bool,
    /// Design Review (Decision Log). Claude only for now, and exclusive with
    /// `review`; an unsupported request is refused, not silently ignored.
    pub design_review: bool,
    pub plan_mode: bool,
    pub create_terminal: bool,
    pub use_worktree: Option<bool>,
    pub enable_chrome: bool,
    pub hook_choice: Option<String>,
    /// Attach this already-existing git worktree as the feature instead of
    /// creating one. Must be one of the repo's registered worktrees and not
    /// already used by another feature; the worktree hook is not run.
    pub existing_worktree: Option<PathBuf>,
    pub dry_run: bool,
    pub initial_prompt: Option<String>,
    pub submit_prompt: bool,
}

impl CreateFeatureRequest {
    pub fn ipc_payload(&self) -> serde_json::Value {
        serde_json::json!({
            "type": AUTOMATION_REQUEST_TYPE,
            "action": CREATE_FEATURE_ACTION,
            "no_start": self.no_start,
            "project_name": self.project_name,
            "branch": self.branch,
            "agent": self.agent,
            "mode": self.mode,
            "review": self.review,
            "design_review": self.design_review,
            "plan_mode": self.plan_mode,
            "create_terminal": self.create_terminal,
            "use_worktree": self.use_worktree,
            "enable_chrome": self.enable_chrome,
            "hook_choice": self.hook_choice,
            "existing_worktree": self.existing_worktree,
            "initial_prompt": self.initial_prompt,
            "submit_prompt": self.submit_prompt,
            "dry_run": self.dry_run,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CreateBatchFeaturesRequest {
    pub workspace_path: PathBuf,
    pub project_name: String,
    pub feature_count: usize,
    pub feature_prefix: String,
    pub agent: AgentKind,
    pub mode: VibeMode,
    pub review: bool,
    pub enable_chrome: bool,
    pub dry_run: bool,
    pub initial_prompt: Option<String>,
    pub submit_prompt: bool,
}

impl Default for CreateBatchFeaturesRequest {
    fn default() -> Self {
        Self {
            workspace_path: PathBuf::new(),
            project_name: String::new(),
            feature_count: 3,
            feature_prefix: "feature".to_string(),
            initial_prompt: None,
            submit_prompt: false,
            agent: AgentKind::default(),
            mode: VibeMode::default(),
            review: false,
            enable_chrome: false,
            dry_run: false,
        }
    }
}

impl CreateBatchFeaturesRequest {
    pub fn ipc_payload(&self) -> serde_json::Value {
        serde_json::json!({
            "type": AUTOMATION_REQUEST_TYPE,
            "action": CREATE_BATCH_FEATURES_ACTION,
            "workspace_path": self.workspace_path,
            "project_name": self.project_name,
            "feature_count": self.feature_count,
            "feature_prefix": self.feature_prefix,
            "agent": self.agent,
            "mode": self.mode,
            "review": self.review,
            "enable_chrome": self.enable_chrome,
            "initial_prompt": self.initial_prompt,
            "submit_prompt": self.submit_prompt,
            "dry_run": self.dry_run,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateProjectResponse {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub action: &'static str,
    pub ok: bool,
    pub dry_run: bool,
    pub input_path: PathBuf,
    pub project_name: String,
    pub project_path: PathBuf,
    pub is_git: bool,
    pub message: String,
    /// The created project's stable id, so a caller can address it by id
    /// from here on instead of re-deriving a name lookup. `None` for a
    /// `dry_run` response, since no project (and so no id) was created.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

impl CreateProjectResponse {
    pub fn success(
        request: &CreateProjectRequest,
        project_path: PathBuf,
        is_git: bool,
        message: String,
        project_id: Option<String>,
    ) -> Self {
        Self {
            msg_type: AUTOMATION_RESULT_TYPE,
            action: CREATE_PROJECT_ACTION,
            ok: true,
            dry_run: request.dry_run,
            input_path: request.path.clone(),
            project_name: request.project_name.clone(),
            project_path,
            is_git,
            message,
            project_id,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AutomationHookPrompt {
    pub title: String,
    pub options: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateFeatureResponse {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub action: &'static str,
    pub ok: bool,
    pub dry_run: bool,
    pub project_name: String,
    pub branch: String,
    pub workdir: PathBuf,
    pub is_worktree: bool,
    pub tmux_session: String,
    pub started: bool,
    pub worktree_hook_ran: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_hook_succeeded: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_hook_prompt: Option<AutomationHookPrompt>,
    pub message: String,
    /// The created feature's (and its project's) stable id. `None` for a
    /// `dry_run` response, since no feature was created.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feature_id: Option<String>,
}

impl CreateFeatureResponse {
    #[allow(clippy::too_many_arguments)]
    pub fn success(
        request: &CreateFeatureRequest,
        workdir: PathBuf,
        is_worktree: bool,
        tmux_session: String,
        started: bool,
        worktree_hook_ran: bool,
        worktree_hook_succeeded: Option<bool>,
        worktree_hook_prompt: Option<AutomationHookPrompt>,
        message: String,
        ids: Option<(String, String)>,
    ) -> Self {
        let (project_id, feature_id) = match ids {
            Some((p, f)) => (Some(p), Some(f)),
            None => (None, None),
        };
        Self {
            msg_type: AUTOMATION_RESULT_TYPE,
            action: CREATE_FEATURE_ACTION,
            ok: true,
            dry_run: request.dry_run,
            project_name: request.project_name.clone(),
            branch: request.branch.clone(),
            workdir,
            is_worktree,
            tmux_session,
            started,
            worktree_hook_ran,
            worktree_hook_succeeded,
            worktree_hook_prompt,
            message,
            project_id,
            feature_id,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BatchFeatureAutomationResult {
    pub name: String,
    pub branch: String,
    pub workdir: PathBuf,
    pub tmux_session: String,
    pub started: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateBatchFeaturesResponse {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub action: &'static str,
    pub ok: bool,
    pub dry_run: bool,
    pub workspace_path: PathBuf,
    pub project_name: String,
    pub project_repo: PathBuf,
    pub features: Vec<BatchFeatureAutomationResult>,
    pub message: String,
}

impl CreateBatchFeaturesResponse {
    pub fn success(
        request: &CreateBatchFeaturesRequest,
        project_repo: PathBuf,
        features: Vec<BatchFeatureAutomationResult>,
        message: String,
    ) -> Self {
        Self {
            msg_type: AUTOMATION_RESULT_TYPE,
            action: CREATE_BATCH_FEATURES_ACTION,
            ok: true,
            dry_run: request.dry_run,
            workspace_path: request.workspace_path.clone(),
            project_name: request.project_name.clone(),
            project_repo,
            features,
            message,
        }
    }
}

pub fn automation_error_response(action: &str, error: impl Into<String>) -> serde_json::Value {
    serde_json::json!({
        "type": AUTOMATION_RESULT_TYPE,
        "action": action,
        "ok": false,
        "error": error.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_no_start_defaults_off_and_survives_ipc() {
        let default: CreateFeatureRequest = serde_json::from_str("{}").unwrap();
        assert!(!default.no_start);
        let request: CreateFeatureRequest = serde_json::from_str(r#"{"no_start": true}"#).unwrap();
        assert_eq!(request.ipc_payload()["no_start"], true);
    }

    #[test]
    fn session_requests_round_trip_flattened_target_and_selector() {
        let request = SendSessionInputRequest {
            target: FeatureTarget {
                feature_session_id: Some("agent-1".into()),
                ..Default::default()
            },
            session: SessionSelector {
                label: Some("dev server".into()),
                ..Default::default()
            },
            text: Some("rs".into()),
            run: true,
            key: Some("C-c".into()),
        };
        let payload = request.ipc_payload();
        assert_eq!(payload["action"], SEND_SESSION_INPUT_ACTION);
        assert_eq!(payload["feature_session_id"], "agent-1");
        assert_eq!(payload["label"], "dev server");
        let back: SendSessionInputRequest = serde_json::from_value(payload).unwrap();
        assert_eq!(back.target, request.target);
        assert_eq!(back.session.label.as_deref(), Some("dev server"));
        assert!(back.run);
        let empty: CaptureSessionRequest = serde_json::from_str("{}").unwrap();
        assert!(empty.target.is_empty() && empty.lines.is_none());
    }

    #[test]
    fn initial_prompt_and_submission_survive_cli_ipc_requests() {
        let input = serde_json::json!({
            "initial_prompt": "First line\nSecond line",
            "submit_prompt": true,
        });
        let feature: CreateFeatureRequest = serde_json::from_value(input.clone()).unwrap();
        let batch: CreateBatchFeaturesRequest = serde_json::from_value(input.clone()).unwrap();
        let session: CreateSessionRequest = serde_json::from_value(input.clone()).unwrap();
        for payload in [
            feature.ipc_payload(),
            batch.ipc_payload(),
            session.ipc_payload(),
        ] {
            assert_eq!(payload["initial_prompt"], input["initial_prompt"]);
            assert_eq!(payload["submit_prompt"], true);
            assert_eq!(payload["type"], AUTOMATION_REQUEST_TYPE);
        }
        assert_eq!(session.ipc_payload()["action"], CREATE_SESSION_ACTION);
        let default: CreateSessionRequest = serde_json::from_str("{}").unwrap();
        assert!(!default.submit_prompt);
        assert!(default.initial_prompt.is_none());
    }
}

#[cfg(test)]
mod seed_ai_review_tests {
    use super::*;

    #[test]
    fn seed_ai_review_payload_carries_the_fixture_attribution() {
        // The CLI forwards `ipc_payload()` to the running dashboard, so a
        // field missing here never reaches the seeded cache row.
        let request = SeedAiReviewRequest {
            pr_number: 1,
            attribution: Some(crate::app::ai_review::AiReviewAttribution {
                harness: Some("Claude".to_string()),
                memory: Some(crate::app::ai_review::AiReviewMemoryUsage {
                    context_chars: 12,
                    prompts: 1,
                    prompts_with_memory: 1,
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };

        let received: SeedAiReviewRequest = serde_json::from_value(request.ipc_payload()).unwrap();

        assert_eq!(received.attribution, request.attribution);
    }
}
