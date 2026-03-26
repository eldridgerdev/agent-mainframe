use anyhow::Result;
use chrono::Utc;
use serde::Deserialize;
use std::path::{Path, PathBuf};

use super::*;
use crate::app::util::latest_prompt_path;
use crate::automation::{
    CREATE_BATCH_FEATURES_ACTION, CREATE_FEATURE_ACTION, CREATE_PROJECT_ACTION,
    CreateBatchFeaturesRequest, CreateFeatureRequest, CreateProjectRequest,
    automation_error_response,
};

#[derive(Deserialize)]
struct IpcMsg {
    #[serde(rename = "type")]
    msg_type: Option<String>,
    source: Option<String>,
    session_id: Option<String>,
    cwd: Option<String>,
    message: Option<String>,
    notification_type: Option<String>,
    proceed_signal: Option<String>,
    request_id: Option<String>,
    reply_socket: Option<String>,
    file_path: Option<String>,
    relative_path: Option<String>,
    tool: Option<String>,
    tool_name: Option<String>,
    change_id: Option<String>,
    old_snippet: Option<String>,
    new_snippet: Option<String>,
    content_preview: Option<String>,
    response_file: Option<String>,
    original_file: Option<String>,
    proposed_file: Option<String>,
    is_new_file: Option<bool>,
    reason: Option<String>,
    prompt: Option<String>,
}

impl App {
    fn touch_feature_for_session(&mut self, session_id: &str) {
        for project in &mut self.store.projects {
            for feature in &mut project.features {
                if feature.tmux_session == session_id {
                    feature.last_accessed = Utc::now();
                    if feature.status == ProjectStatus::Stopped {
                        feature.status = ProjectStatus::Idle;
                    }
                    return;
                }
            }
        }
    }

    fn respond_to_notification(
        &mut self,
        request_id: Option<&str>,
        reply_socket: Option<&str>,
        proceed_signal: Option<&str>,
        payload: serde_json::Value,
    ) {
        if let (Some(req), Some(sock)) = (request_id, reply_socket) {
            if !req.is_empty() && !sock.is_empty() {
                let mut body = payload;
                if let Some(obj) = body.as_object_mut() {
                    obj.insert("request_id".to_string(), serde_json::json!(req));
                }
                let serialized = serde_json::to_string(&body).unwrap_or_else(|_| "{}".to_string());
                match crate::ipc::send(Path::new(sock), &serialized) {
                    Ok(_) => {
                        self.log_debug("ipc", format!("Replied over IPC to request {req}"));
                        return;
                    }
                    Err(e) => {
                        self.log_warn(
                            "ipc",
                            format!(
                                "IPC reply failed for request {req}: {e}; falling back to signal file"
                            ),
                        );
                    }
                }
            }
        }

        if let Some(signal_path) = proceed_signal {
            let p = Path::new(signal_path);
            if let Some(parent) = p.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(p, "");
        }
    }

    fn project_feature_for_cwd(
        &self,
        cwd_path: &Path,
    ) -> (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<(usize, usize)>,
    ) {
        let mut project_name = None;
        let mut feature_name = None;
        let mut agent_name = None;
        let mut indices = None;
        let mut best_len: usize = 0;

        for (pi, project) in self.store.projects.iter().enumerate() {
            for (fi, feature) in project.features.iter().enumerate() {
                let wlen = feature.workdir.as_os_str().len();
                if (cwd_path.starts_with(&feature.workdir) || feature.workdir.starts_with(cwd_path))
                    && wlen > best_len
                {
                    project_name = Some(project.name.clone());
                    feature_name = Some(feature.name.clone());
                    agent_name = Some(feature.agent.display_name().to_string());
                    indices = Some((pi, fi));
                    best_len = wlen;
                }
            }
        }

        (project_name, feature_name, agent_name, indices)
    }

    fn codex_feature_for_message(
        &self,
        session_id: Option<&str>,
        cwd_path: &Path,
    ) -> Option<(String, String)> {
        if let Some(session_id) = session_id {
            for project in &self.store.projects {
                for feature in &project.features {
                    if feature.tmux_session != session_id || feature.agent != AgentKind::Codex {
                        continue;
                    }
                    if let Some(session) = feature
                        .sessions
                        .iter()
                        .find(|s| s.kind == SessionKind::Codex)
                    {
                        return Some((feature.tmux_session.clone(), session.tmux_window.clone()));
                    }
                }
            }
        }

        let (_, _, _, indices) = self.project_feature_for_cwd(cwd_path);
        let (pi, fi) = indices?;
        let feature = self.store.projects.get(pi)?.features.get(fi)?;
        if feature.agent != AgentKind::Codex {
            return None;
        }

        feature
            .sessions
            .iter()
            .find(|session| session.kind == SessionKind::Codex)
            .map(|session| (feature.tmux_session.clone(), session.tmux_window.clone()))
    }

    pub(crate) fn open_diff_review_prompt(&mut self, input: &PendingInput) {
        let response_file = input.response_file.clone().unwrap_or_default();
        let proceed_signal = input.proceed_signal.clone().unwrap_or_default();
        let return_to_view = match &self.mode {
            AppMode::Viewing(view) => Some(view.clone()),
            _ => None,
        };
        let diff_path = input
            .relative_path
            .clone()
            .filter(|path| !path.is_empty())
            .or_else(|| input.target_file_path.clone())
            .unwrap_or_default();
        let (mut diff_file, diff_error) = match (
            input.original_file.as_deref(),
            input.proposed_file.as_deref(),
        ) {
            (Some(original), Some(proposed)) => match crate::diff::load_review_file(
                Path::new(original),
                Path::new(proposed),
                &diff_path,
            ) {
                Ok(file) => (Some(file), None),
                Err(err) => (None, Some(err.to_string())),
            },
            _ => (None, None),
        };
        if input.is_new_file == Some(true)
            && let Some(file) = &mut diff_file
        {
            file.status = crate::diff::DiffFileStatus::Added;
            file.old_path = None;
            file.deletions = 0;
        }
        self.mode = AppMode::DiffReviewPrompt(DiffReviewState {
            session_id: input.session_id.clone(),
            workdir: PathBuf::from(&input.cwd),
            file_path: input.target_file_path.clone().unwrap_or_default(),
            relative_path: input.relative_path.clone().unwrap_or_default(),
            change_id: input.change_id.clone().unwrap_or_default(),
            tool: input.tool.clone().unwrap_or_default(),
            old_snippet: input.old_snippet.clone().unwrap_or_default(),
            new_snippet: input.new_snippet.clone().unwrap_or_default(),
            diff_file,
            diff_error,
            patch_scroll: 0,
            reason: input.reason.clone().unwrap_or_default(),
            editing_feedback: false,
            layout: self.preferred_diff_viewer_layout(),
            explanation: None,
            explanation_child: None,
            response_file: PathBuf::from(response_file),
            proceed_signal: PathBuf::from(proceed_signal),
            request_id: input.request_id.clone(),
            reply_socket: input.reply_socket.clone(),
            return_to_view,
        });
    }

    /// Drain all pending IPC socket messages, converting them into
    /// `pending_inputs` entries or removing them for "clear" messages.
    /// Call this every event loop iteration instead of polling files.
    pub fn drain_ipc_messages(&mut self) {
        // Collect first to avoid holding a borrow on self.ipc
        // while mutating other self fields below.
        let mut messages = Vec::new();
        if let Some(ref guard) = self.ipc {
            while let Ok(v) = guard.rx.try_recv() {
                messages.push(v);
            }
        }
        if messages.is_empty() {
            return;
        }
        self.log_debug("ipc", format!("Draining {} message(s)", messages.len()));

        for raw in messages {
            self.handle_ipc_message_value(raw);
        }
    }

    pub(crate) fn handle_ipc_message_value(&mut self, raw: serde_json::Value) {
        let msg_type = raw
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("stop")
            .to_string();

        if matches!(msg_type.as_str(), "codex-live-event" | "codex_live_event") {
            let session_id = raw
                .get("session_id")
                .and_then(|v| v.as_str())
                .filter(|sid| !sid.is_empty());
            let event = raw.get("event").unwrap_or(&raw);

            if let Some(session_id) = session_id {
                if self.apply_codex_live_event(session_id, event) {
                    self.log_debug(
                        "ipc",
                        format!("Applied codex live event for session {session_id}"),
                    );
                }
            } else {
                self.log_warn(
                    "ipc",
                    "Ignored codex live event without session_id".to_string(),
                );
            }
            return;
        }

        if msg_type == crate::automation::AUTOMATION_REQUEST_TYPE {
            let request_id = raw
                .get("request_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let reply_socket = raw
                .get("reply_socket")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let action = raw
                .get("action")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let payload = match action.as_str() {
                CREATE_PROJECT_ACTION => {
                    match serde_json::from_value::<CreateProjectRequest>(raw.clone()) {
                        Ok(request) => match self.create_project_from_request(&request) {
                            Ok(response) => {
                                self.log_info(
                                    "automation",
                                    format!(
                                        "Automation created project '{}' at {}",
                                        response.project_name,
                                        response.project_path.display()
                                    ),
                                );
                                serde_json::to_value(response).unwrap_or_else(|err| {
                                    automation_error_response(
                                        CREATE_PROJECT_ACTION,
                                        format!("Failed to serialize response: {err}"),
                                    )
                                })
                            }
                            Err(err) => {
                                self.log_error(
                                    "automation",
                                    format!("Automation '{}' failed: {err}", CREATE_PROJECT_ACTION),
                                );
                                automation_error_response(CREATE_PROJECT_ACTION, err.to_string())
                            }
                        },
                        Err(err) => automation_error_response(
                            CREATE_PROJECT_ACTION,
                            format!("Invalid automation payload: {err}"),
                        ),
                    }
                }
                CREATE_FEATURE_ACTION => {
                    match serde_json::from_value::<CreateFeatureRequest>(raw.clone()) {
                        Ok(request) => match self.create_feature_from_request(&request) {
                            Ok(response) => {
                                self.log_info(
                                    "automation",
                                    format!(
                                        "Automation created feature '{}' in project '{}'",
                                        response.branch, response.project_name
                                    ),
                                );
                                serde_json::to_value(response).unwrap_or_else(|err| {
                                    automation_error_response(
                                        CREATE_FEATURE_ACTION,
                                        format!("Failed to serialize response: {err}"),
                                    )
                                })
                            }
                            Err(err) => {
                                self.log_error(
                                    "automation",
                                    format!("Automation '{}' failed: {err}", CREATE_FEATURE_ACTION),
                                );
                                automation_error_response(CREATE_FEATURE_ACTION, err.to_string())
                            }
                        },
                        Err(err) => automation_error_response(
                            CREATE_FEATURE_ACTION,
                            format!("Invalid automation payload: {err}"),
                        ),
                    }
                }
                CREATE_BATCH_FEATURES_ACTION => {
                    match serde_json::from_value::<CreateBatchFeaturesRequest>(raw.clone()) {
                        Ok(request) => match self.create_batch_features_from_request(&request) {
                            Ok(response) => {
                                self.log_info(
                                    "automation",
                                    format!(
                                        "Automation created batch project '{}' with {} features",
                                        response.project_name,
                                        response.features.len()
                                    ),
                                );
                                serde_json::to_value(response).unwrap_or_else(|err| {
                                    automation_error_response(
                                        CREATE_BATCH_FEATURES_ACTION,
                                        format!("Failed to serialize response: {err}"),
                                    )
                                })
                            }
                            Err(err) => {
                                self.log_error(
                                    "automation",
                                    format!(
                                        "Automation '{}' failed: {err}",
                                        CREATE_BATCH_FEATURES_ACTION
                                    ),
                                );
                                automation_error_response(
                                    CREATE_BATCH_FEATURES_ACTION,
                                    err.to_string(),
                                )
                            }
                        },
                        Err(err) => automation_error_response(
                            CREATE_BATCH_FEATURES_ACTION,
                            format!("Invalid automation payload: {err}"),
                        ),
                    }
                }
                _ => automation_error_response(
                    if action.is_empty() {
                        "unknown"
                    } else {
                        &action
                    },
                    format!(
                        "Unknown automation action '{}'",
                        if action.is_empty() {
                            "<missing>"
                        } else {
                            &action
                        }
                    ),
                ),
            };

            self.respond_to_notification(
                request_id.as_deref(),
                reply_socket.as_deref(),
                None,
                payload,
            );
            return;
        }

        let msg: IpcMsg = match serde_json::from_value(raw) {
            Ok(m) => m,
            Err(_) => return,
        };

        let msg_type = msg.msg_type.as_deref().unwrap_or("stop").to_string();

        // "clear" removes any pending notification for this
        // session, sent by clear-notify.sh on PreToolUse.
        if msg_type == "clear" {
            if let Some(ref sid) = msg.session_id {
                let before = self.pending_inputs.len();
                self.pending_inputs.retain(|i| &i.session_id != sid);
                let removed = before - self.pending_inputs.len();
                if removed > 0 {
                    self.log_debug(
                        "ipc",
                        format!(
                            "Cleared {removed} notification(s) \
                                 for session {sid}"
                        ),
                    );
                }
            }
            return;
        }

        if msg_type == "thinking-start" {
            if let Some(sid) = msg.session_id {
                self.ipc_thinking_sessions.insert(sid.clone());
                self.touch_feature_for_session(&sid);
                self.log_debug("ipc", format!("thinking-start for {sid}"));
            }
            return;
        }

        if msg_type == "thinking-stop" {
            if let Some(sid) = msg.session_id {
                self.ipc_thinking_sessions.remove(&sid);
                self.log_debug("ipc", format!("thinking-stop for {sid}"));
            }
            return;
        }

        if msg_type == "tool-start" {
            let cwd_path = PathBuf::from(msg.cwd.as_deref().unwrap_or_default());
            if let Some(sid) = msg.session_id {
                self.ipc_tool_sessions.insert(sid.clone());
                self.touch_feature_for_session(&sid);
                let label = msg
                    .tool_name
                    .clone()
                    .or(msg.tool.clone())
                    .unwrap_or_default();
                if let Some((codex_session, _)) =
                    self.codex_feature_for_message(Some(&sid), &cwd_path)
                {
                    let command = if label.is_empty() {
                        "tool".to_string()
                    } else {
                        label.clone()
                    };
                    self.apply_codex_live_event(
                        &codex_session,
                        &serde_json::json!({
                            "type": "commandExecution",
                            "payload": {
                                "command": command,
                                "phase": "running"
                            }
                        }),
                    );
                }
                self.log_debug("ipc", format!("tool-start for {sid} ({label})"));
            }
            return;
        }

        if msg_type == "tool-stop" {
            let cwd_path = PathBuf::from(msg.cwd.as_deref().unwrap_or_default());
            let label = msg
                .tool_name
                .clone()
                .or(msg.tool.clone())
                .unwrap_or_default();
            if let Some(sid) = msg.session_id {
                self.ipc_tool_sessions.remove(&sid);
                if let Some((codex_session, _)) =
                    self.codex_feature_for_message(Some(&sid), &cwd_path)
                {
                    let command = if label.is_empty() {
                        "tool".to_string()
                    } else {
                        label.clone()
                    };
                    self.apply_codex_live_event(
                        &codex_session,
                        &serde_json::json!({
                            "type": "commandExecution",
                            "payload": {
                                "command": command,
                                "phase": "completed"
                            }
                        }),
                    );
                }
                self.log_debug("ipc", format!("tool-stop for {sid}"));
            }
            return;
        }

        if msg_type == "prompt-submit" {
            let cwd = msg.cwd.unwrap_or_default();
            let prompt = msg.prompt.unwrap_or_default();
            let session_id = msg.session_id.clone();
            let cwd_path = PathBuf::from(&cwd);
            if let Some(ref sid) = session_id {
                self.touch_feature_for_session(sid);
            }
            if !cwd.is_empty() && !prompt.is_empty() {
                let p = latest_prompt_path(&PathBuf::from(&cwd));
                if let Some(parent) = p.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(&p, &prompt);
                self.log_debug("ipc", format!("prompt-submit persisted at {}", p.display()));
            }
            let normalized_prompt = prompt.trim();
            if !normalized_prompt.is_empty() {
                if let Some(ref sid) = session_id {
                    self.latest_prompt_cache
                        .insert(sid.clone(), normalized_prompt.to_string());
                } else if !cwd.is_empty() {
                    let cwd_path = PathBuf::from(&cwd);
                    if let Some((pi, fi)) = self.project_feature_for_cwd(&cwd_path).3 {
                        let tmux_session =
                            self.store.projects[pi].features[fi].tmux_session.clone();
                        self.latest_prompt_cache
                            .insert(tmux_session, normalized_prompt.to_string());
                    }
                }
            }
            if let Some((pi, fi)) = self.project_feature_for_cwd(&cwd_path).3 {
                self.refresh_sidebar_plan_for_feature(pi, fi);
            }
            if let Some((codex_session, codex_window)) =
                self.codex_feature_for_message(session_id.as_deref(), &cwd_path)
            {
                self.note_codex_prompt_submit(&codex_session, &codex_window);
                self.apply_codex_live_event(
                    &codex_session,
                    &serde_json::json!({ "type": "inputResolved" }),
                );
            }
            return;
        }

        let session_id = msg.session_id.unwrap_or_default();
        let cwd = msg.cwd.unwrap_or_default();
        let source = msg.source.unwrap_or_default();
        let notification_type = msg.notification_type.unwrap_or(msg_type);

        let cwd_path = PathBuf::from(&cwd);

        let is_structured_diff_review = notification_type == "change-reason"
            || (notification_type == "diff-review" && self.use_custom_diff_review_viewer());

        // change-reason/diff-review while viewing -> enter diff review mode.
        let (_, found_feature_name_for_open, _, _) = self.project_feature_for_cwd(&cwd_path);
        if is_structured_diff_review
            && let AppMode::Viewing(view) = &self.mode
            && found_feature_name_for_open.as_deref() == Some(&view.feature_name)
        {
            let input = PendingInput {
                session_id,
                cwd,
                message: msg.message.unwrap_or_default(),
                notification_type,
                file_path: PathBuf::new(),
                target_file_path: msg.file_path,
                relative_path: msg.relative_path,
                change_id: msg.change_id,
                tool: msg.tool.or(msg.tool_name),
                old_snippet: msg.old_snippet,
                new_snippet: msg.new_snippet,
                original_file: msg.original_file,
                proposed_file: msg.proposed_file,
                is_new_file: msg.is_new_file,
                reason: msg.reason,
                response_file: msg.response_file,
                project_name: None,
                feature_name: found_feature_name_for_open,
                proceed_signal: msg.proceed_signal,
                request_id: msg.request_id.clone(),
                reply_socket: msg.reply_socket.clone(),
            };
            self.open_diff_review_prompt(&input);
            return;
        }

        // Resolve project/feature from cwd.
        let (project_name, feature_name, agent_name, indices) =
            self.project_feature_for_cwd(&cwd_path);
        if let Some((pi, fi)) = indices {
            if self.store.projects[pi].features[fi].agent == AgentKind::Codex {
                self.refresh_sidebar_plan_for_feature(pi, fi);
            }
        }

        // For diff-review while viewing the matching
        // feature, write the proceed signal immediately when
        // using the legacy popup flow.
        let mut auto_responded = false;
        if notification_type == "diff-review"
            && !self.use_custom_diff_review_viewer()
            && let AppMode::Viewing(ref view) = self.mode
        {
            if feature_name.as_deref() == Some(&view.feature_name) {
                self.respond_to_notification(
                    msg.request_id.as_deref(),
                    msg.reply_socket.as_deref(),
                    msg.proceed_signal.as_deref(),
                    serde_json::json!({
                        "type": "review-response",
                        "decision": "proceed"
                    }),
                );
                auto_responded = true;
            }
        }
        if auto_responded {
            return;
        }

        // IPC messages have no on-disk file_path; use a
        // sentinel so existing code that removes the file
        // gracefully no-ops.
        self.log_debug(
                "ipc",
                format!(
                    "Received '{notification_type}' (source={}) for session {session_id} (agent={}, feature={})",
                    if source.is_empty() { "unknown" } else { &source },
                    agent_name.unwrap_or_else(|| "unknown".to_string()),
                    feature_name.clone().unwrap_or_else(|| "unknown".to_string())
                ),
            );
        if source == "codex-notify" {
            self.log_info(
                "ipc",
                format!(
                    "Codex notify hook delivered input-request over IPC (session={session_id})"
                ),
            );
        }
        if let Some((codex_session, _)) =
            self.codex_feature_for_message(Some(&session_id), &cwd_path)
        {
            if notification_type == "input-request" {
                let tool = msg.tool_name.clone().or(msg.tool.clone());
                let path = msg.relative_path.clone().or(msg.file_path.clone());
                self.apply_codex_live_event(
                    &codex_session,
                    &serde_json::json!({
                        "type": "requestUserInput",
                        "payload": {
                            "prompt": msg.message.clone().unwrap_or_default(),
                            "tool": tool,
                            "relative_path": path
                        }
                    }),
                );
            } else if matches!(notification_type.as_str(), "change-reason" | "diff-review") {
                let review_status = if notification_type == "change-reason" {
                    "needs-reason"
                } else {
                    "needs-review"
                };
                let tool = msg.tool_name.clone().or(msg.tool.clone());
                self.apply_codex_live_event(
                    &codex_session,
                    &serde_json::json!({
                        "type": "fileChange",
                        "payload": {
                            "relative_path": msg
                                .relative_path
                                .clone()
                                .or(msg.file_path.clone())
                                .unwrap_or_default(),
                            "status": review_status,
                            "tool": tool,
                            "message": msg.message.clone(),
                            "reason": msg.reason.clone()
                        }
                    }),
                );
            }
        }
        self.pending_inputs.push(PendingInput {
            session_id,
            cwd,
            message: msg.message.unwrap_or_default(),
            notification_type,
            file_path: PathBuf::new(),
            target_file_path: msg.file_path,
            relative_path: msg.relative_path,
            change_id: msg.change_id,
            tool: msg.tool.or(msg.tool_name),
            old_snippet: msg.old_snippet,
            new_snippet: msg.new_snippet,
            original_file: msg.original_file,
            proposed_file: msg.proposed_file,
            is_new_file: msg.is_new_file,
            reason: msg.reason,
            response_file: msg.response_file,
            project_name,
            feature_name,
            proceed_signal: msg.proceed_signal,
            request_id: msg.request_id,
            reply_socket: msg.reply_socket,
        });
    }

    pub fn scan_notifications(&mut self) {
        #[derive(Deserialize)]
        struct NotificationJson {
            session_id: Option<String>,
            cwd: Option<String>,
            message: Option<String>,
            #[serde(alias = "type")]
            notification_type: Option<String>,
            proceed_signal: Option<String>,
            request_id: Option<String>,
            reply_socket: Option<String>,
            file_path: Option<String>,
            relative_path: Option<String>,
            tool: Option<String>,
            change_id: Option<String>,
            old_snippet: Option<String>,
            new_snippet: Option<String>,
            content_preview: Option<String>,
            response_file: Option<String>,
            original_file: Option<String>,
            proposed_file: Option<String>,
            is_new_file: Option<bool>,
            reason: Option<String>,
        }

        let mut inputs = Vec::new();

        for project in &self.store.projects {
            for feature in &project.features {
                let notify_dir = feature.workdir.join(".claude").join("notifications");

                let entries = match std::fs::read_dir(&notify_dir) {
                    Ok(e) => e,
                    Err(_) => continue,
                };

                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) != Some("json") {
                        continue;
                    }

                    let data = match std::fs::read_to_string(&path) {
                        Ok(d) => d,
                        Err(_) => continue,
                    };

                    let notif: NotificationJson = match serde_json::from_str(&data) {
                        Ok(n) => n,
                        Err(_) => continue,
                    };

                    let notification_type = notif.notification_type.clone().unwrap_or_default();
                    let is_structured_diff_review = notification_type == "change-reason"
                        || (notification_type == "diff-review"
                            && self.use_custom_diff_review_viewer());
                    if is_structured_diff_review
                        && let AppMode::Viewing(view) = &self.mode
                        && feature.name == view.feature_name
                    {
                        let input = PendingInput {
                            session_id: notif.session_id.unwrap_or_default(),
                            cwd: notif.cwd.unwrap_or_default(),
                            message: notif.message.unwrap_or_default(),
                            notification_type,
                            file_path: path.clone(),
                            target_file_path: notif.file_path,
                            relative_path: notif.relative_path,
                            change_id: notif.change_id,
                            tool: notif.tool,
                            old_snippet: notif.old_snippet,
                            new_snippet: notif.new_snippet,
                            original_file: notif.original_file,
                            proposed_file: notif.proposed_file,
                            is_new_file: notif.is_new_file,
                            reason: notif.reason,
                            response_file: notif.response_file,
                            project_name: Some(project.name.clone()),
                            feature_name: Some(feature.name.clone()),
                            proceed_signal: notif.proceed_signal,
                            request_id: notif.request_id.clone(),
                            reply_socket: notif.reply_socket.clone(),
                        };
                        self.open_diff_review_prompt(&input);
                        let _ = std::fs::remove_file(&path);
                        return;
                    }

                    inputs.push(PendingInput {
                        session_id: notif.session_id.unwrap_or_default(),
                        cwd: notif.cwd.unwrap_or_default(),
                        message: notif.message.unwrap_or_default(),
                        notification_type,
                        file_path: path,
                        target_file_path: notif.file_path,
                        relative_path: notif.relative_path,
                        change_id: notif.change_id,
                        tool: notif.tool,
                        old_snippet: notif.old_snippet,
                        new_snippet: notif.new_snippet,
                        original_file: notif.original_file,
                        proposed_file: notif.proposed_file,
                        is_new_file: notif.is_new_file,
                        reason: notif.reason,
                        response_file: notif.response_file,
                        project_name: Some(project.name.clone()),
                        feature_name: Some(feature.name.clone()),
                        proceed_signal: notif.proceed_signal,
                        request_id: notif.request_id,
                        reply_socket: notif.reply_socket,
                    });
                }
            }
        }

        let global_notify_dir = crate::project::amf_config_dir().join("notifications");

        if let Ok(entries) = std::fs::read_dir(&global_notify_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }

                let data = match std::fs::read_to_string(&path) {
                    Ok(d) => d,
                    Err(_) => continue,
                };

                let notif: NotificationJson = match serde_json::from_str(&data) {
                    Ok(n) => n,
                    Err(_) => continue,
                };

                let session_id = notif.session_id.unwrap_or_default();
                let cwd = notif.cwd.unwrap_or_default();
                let notification_type = notif.notification_type.unwrap_or_default();
                let proceed_signal_val = notif.proceed_signal.clone();
                let is_structured_diff_review = notification_type == "change-reason"
                    || (notification_type == "diff-review" && self.use_custom_diff_review_viewer());

                let cwd_path = PathBuf::from(&cwd);
                let (_, found_feature_name_for_open, _, _) =
                    self.project_feature_for_cwd(&cwd_path);
                if is_structured_diff_review
                    && let AppMode::Viewing(view) = &self.mode
                    && found_feature_name_for_open.as_deref() == Some(&view.feature_name)
                {
                    let input = PendingInput {
                        session_id,
                        cwd,
                        message: notif.message.unwrap_or_default(),
                        notification_type,
                        file_path: path.clone(),
                        target_file_path: notif.file_path,
                        relative_path: notif.relative_path,
                        change_id: notif.change_id,
                        tool: notif.tool,
                        old_snippet: notif.old_snippet,
                        new_snippet: notif.new_snippet,
                        original_file: notif.original_file,
                        proposed_file: notif.proposed_file,
                        is_new_file: notif.is_new_file,
                        reason: notif.reason,
                        response_file: notif.response_file,
                        project_name: None,
                        feature_name: found_feature_name_for_open,
                        proceed_signal: proceed_signal_val,
                        request_id: notif.request_id.clone(),
                        reply_socket: notif.reply_socket.clone(),
                    };
                    self.open_diff_review_prompt(&input);
                    let _ = std::fs::remove_file(&path);
                    return;
                }

                let (project_name, feature_name, _, _) = self.project_feature_for_cwd(&cwd_path);

                inputs.push(PendingInput {
                    session_id,
                    cwd,
                    message: notif.message.unwrap_or_default(),
                    notification_type,
                    file_path: path,
                    target_file_path: notif.file_path,
                    relative_path: notif.relative_path,
                    change_id: notif.change_id,
                    tool: notif.tool,
                    old_snippet: notif.old_snippet,
                    new_snippet: notif.new_snippet,
                    original_file: notif.original_file,
                    proposed_file: notif.proposed_file,
                    is_new_file: notif.is_new_file,
                    reason: notif.reason,
                    response_file: notif.response_file,
                    project_name,
                    feature_name,
                    proceed_signal: notif.proceed_signal,
                    request_id: notif.request_id,
                    reply_socket: notif.reply_socket,
                });
            }
        }

        // Preserve IPC-origin pending inputs (which use an empty
        // file_path sentinel) when refreshing from file-based sources.
        for existing in self.pending_inputs.clone() {
            if existing.file_path.as_os_str().is_empty()
                && !inputs.iter().any(|i| {
                    i.session_id == existing.session_id
                        && i.notification_type == existing.notification_type
                        && i.request_id == existing.request_id
                })
            {
                inputs.push(existing);
            }
        }

        self.pending_inputs = inputs;
        let file_count = self
            .pending_inputs
            .iter()
            .filter(|i| !i.file_path.as_os_str().is_empty())
            .count();
        if file_count != self.last_file_notification_count {
            self.log_info(
                "ipc",
                format!("File-notification fallback pending count: {}", file_count),
            );
            self.last_file_notification_count = file_count;
        }

        if let AppMode::Viewing(ref view) = self.mode {
            let feat_name = view.feature_name.clone();
            let responses: Vec<(Option<String>, Option<String>, Option<String>)> = self
                .pending_inputs
                .iter()
                .filter(|input| {
                    input.notification_type == "diff-review"
                        && input.feature_name.as_deref() == Some(&feat_name)
                })
                .map(|input| {
                    (
                        input.request_id.clone(),
                        input.reply_socket.clone(),
                        input.proceed_signal.clone(),
                    )
                })
                .collect();
            if !self.use_custom_diff_review_viewer() {
                for (request_id, reply_socket, proceed_signal) in responses {
                    self.respond_to_notification(
                        request_id.as_deref(),
                        reply_socket.as_deref(),
                        proceed_signal.as_deref(),
                        serde_json::json!({
                            "type": "review-response",
                            "decision": "proceed"
                        }),
                    );
                }
                self.pending_inputs.retain(|input| {
                    !(input.notification_type == "diff-review"
                        && input.feature_name.as_deref() == Some(&feat_name))
                });
            }
        }
    }

    pub fn handle_notification_select(&mut self) -> Result<()> {
        let idx = match &self.mode {
            AppMode::NotificationPicker(i, _) => *i,
            _ => return Ok(()),
        };

        let input = match self.pending_inputs.get(idx) {
            Some(i) => i.clone(),
            None => {
                self.mode = AppMode::Normal;
                return Ok(());
            }
        };

        let is_structured_diff_review = input.notification_type == "change-reason"
            || (input.notification_type == "diff-review" && self.use_custom_diff_review_viewer());

        if input.notification_type != "diff-review"
            && input.notification_type != "input-request"
            && input.notification_type != "change-reason"
        {
            let _ = std::fs::remove_file(&input.file_path);
        }

        if let (Some(proj_name), Some(feat_name)) = (&input.project_name, &input.feature_name) {
            let pi = self
                .store
                .projects
                .iter()
                .position(|p| &p.name == proj_name);
            if let Some(pi) = pi {
                let fi = self.store.projects[pi]
                    .features
                    .iter()
                    .position(|f| &f.name == feat_name);
                if let Some(fi) = fi {
                    if input.notification_type == "diff-review"
                        && !self.use_custom_diff_review_viewer()
                    {
                        self.respond_to_notification(
                            input.request_id.as_deref(),
                            input.reply_socket.as_deref(),
                            input.proceed_signal.as_deref(),
                            serde_json::json!({
                                "type": "review-response",
                                "decision": "proceed"
                            }),
                        );
                    }
                    self.selection = Selection::Feature(pi, fi);
                    self.pending_inputs.remove(idx);
                    self.enter_view()?;
                    if is_structured_diff_review {
                        self.open_diff_review_prompt(&input);
                        let _ = std::fs::remove_file(&input.file_path);
                        return Ok(());
                    }
                    return Ok(());
                }
            }
        }

        self.pending_inputs.remove(idx);
        let _ = std::fs::remove_file(&input.file_path);
        self.mode = AppMode::Normal;
        self.message = Some("Notification cleared (no matching feature)".into());
        Ok(())
    }
}
