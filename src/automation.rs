use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::project::{AgentKind, VibeMode};

pub const AUTOMATION_REQUEST_TYPE: &str = "automation";
pub const AUTOMATION_RESULT_TYPE: &str = "automation-result";
pub const CREATE_PROJECT_ACTION: &str = "create_project";
pub const CREATE_FEATURE_ACTION: &str = "create_feature";
pub const CREATE_BATCH_FEATURES_ACTION: &str = "create_batch_features";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CreateProjectRequest {
    pub path: PathBuf,
    pub project_name: String,
    pub dry_run: bool,
}

impl Default for CreateProjectRequest {
    fn default() -> Self {
        Self {
            path: PathBuf::new(),
            project_name: String::new(),
            dry_run: false,
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
            "dry_run": self.dry_run,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CreateFeatureRequest {
    pub project_name: String,
    pub branch: String,
    pub agent: AgentKind,
    pub mode: VibeMode,
    pub review: bool,
    pub use_worktree: Option<bool>,
    pub enable_chrome: bool,
    pub enable_notes: bool,
    pub hook_choice: Option<String>,
    pub dry_run: bool,
}

impl CreateFeatureRequest {
    pub fn ipc_payload(&self) -> serde_json::Value {
        serde_json::json!({
            "type": AUTOMATION_REQUEST_TYPE,
            "action": CREATE_FEATURE_ACTION,
            "project_name": self.project_name,
            "branch": self.branch,
            "agent": self.agent,
            "mode": self.mode,
            "review": self.review,
            "use_worktree": self.use_worktree,
            "enable_chrome": self.enable_chrome,
            "enable_notes": self.enable_notes,
            "hook_choice": self.hook_choice,
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
    pub enable_notes: bool,
    pub dry_run: bool,
}

impl Default for CreateBatchFeaturesRequest {
    fn default() -> Self {
        Self {
            workspace_path: PathBuf::new(),
            project_name: String::new(),
            feature_count: 3,
            feature_prefix: "feature".to_string(),
            agent: AgentKind::default(),
            mode: VibeMode::default(),
            review: false,
            enable_chrome: false,
            enable_notes: false,
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
            "enable_notes": self.enable_notes,
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
}

impl CreateProjectResponse {
    pub fn success(
        request: &CreateProjectRequest,
        project_path: PathBuf,
        is_git: bool,
        message: String,
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
    ) -> Self {
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
