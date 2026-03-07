use anyhow::Result;
use chrono::Utc;
use serde::Deserialize;
use std::path::{Path, PathBuf};

use super::*;

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

    /// Drain all pending IPC socket messages, converting them into
    /// `pending_inputs` entries or removing them for "clear" messages.
    /// Call this every event loop iteration instead of polling files.
    pub fn drain_ipc_messages(&mut self) {
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
            reason: Option<String>,
            prompt: Option<String>,
        }

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
            let msg: IpcMsg = match serde_json::from_value(raw) {
                Ok(m) => m,
                Err(_) => continue,
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
                continue;
            }

            if msg_type == "thinking-start" {
                if let Some(sid) = msg.session_id {
                    self.ipc_thinking_sessions.insert(sid.clone());
                    self.touch_feature_for_session(&sid);
                    self.log_debug("ipc", format!("thinking-start for {sid}"));
                }
                continue;
            }

            if msg_type == "thinking-stop" {
                if let Some(sid) = msg.session_id {
                    self.ipc_thinking_sessions.remove(&sid);
                    self.log_debug("ipc", format!("thinking-stop for {sid}"));
                }
                continue;
            }

            if msg_type == "tool-start" {
                if let Some(sid) = msg.session_id {
                    self.ipc_tool_sessions.insert(sid.clone());
                    self.touch_feature_for_session(&sid);
                    let label = msg
                        .tool_name
                        .clone()
                        .or(msg.tool.clone())
                        .unwrap_or_default();
                    self.log_debug("ipc", format!("tool-start for {sid} ({label})"));
                }
                continue;
            }

            if msg_type == "tool-stop" {
                if let Some(sid) = msg.session_id {
                    self.ipc_tool_sessions.remove(&sid);
                    self.log_debug("ipc", format!("tool-stop for {sid}"));
                }
                continue;
            }

            if msg_type == "prompt-submit" {
                let cwd = msg.cwd.unwrap_or_default();
                let prompt = msg.prompt.unwrap_or_default();
                if let Some(ref sid) = msg.session_id {
                    self.touch_feature_for_session(sid);
                }
                if !cwd.is_empty() && !prompt.is_empty() {
                    let p = PathBuf::from(&cwd)
                        .join(".claude")
                        .join("latest-prompt.txt");
                    if let Some(parent) = p.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    let _ = std::fs::write(&p, prompt);
                    self.log_debug("ipc", format!("prompt-submit persisted at {}", p.display()));
                }
                continue;
            }

            let session_id = msg.session_id.unwrap_or_default();
            let cwd = msg.cwd.unwrap_or_default();
            let source = msg.source.unwrap_or_default();
            let notification_type = msg.notification_type.unwrap_or(msg_type);

            let cwd_path = PathBuf::from(&cwd);

            // change-reason while viewing → enter that mode.
            if notification_type == "change-reason"
                && let AppMode::Viewing(ref view) = self.mode
            {
                let mut found_feature_name = None;
                for project in &self.store.projects {
                    for feature in &project.features {
                        if cwd_path.starts_with(&feature.workdir)
                            || feature.workdir.starts_with(&cwd_path)
                        {
                            found_feature_name = Some(feature.name.clone());
                        }
                    }
                }

                if found_feature_name.as_deref() == Some(&view.feature_name) {
                    let response_file = msg.response_file.unwrap_or_default();
                    let proceed_signal = msg.proceed_signal.unwrap_or_default();

                    self.mode = AppMode::ChangeReasonPrompt(ChangeReasonState {
                        session_id,
                        file_path: msg.file_path.unwrap_or_default(),
                        relative_path: msg.relative_path.unwrap_or_default(),
                        change_id: msg.change_id.unwrap_or_default(),
                        tool: msg.tool.unwrap_or_default(),
                        old_snippet: msg.old_snippet.unwrap_or_default(),
                        new_snippet: msg.new_snippet.unwrap_or_default(),
                        reason: msg.reason.unwrap_or_default(),
                        response_file: PathBuf::from(response_file),
                        proceed_signal: PathBuf::from(proceed_signal),
                        request_id: msg.request_id.clone(),
                        reply_socket: msg.reply_socket.clone(),
                    });
                    return;
                }
            }

            // Resolve project/feature from cwd.
            let mut project_name = None;
            let mut feature_name = None;
            let mut agent_name = None;
            let mut best_len: usize = 0;
            for project in &self.store.projects {
                for feature in &project.features {
                    let wlen = feature.workdir.as_os_str().len();
                    if (cwd_path.starts_with(&feature.workdir)
                        || feature.workdir.starts_with(&cwd_path))
                        && wlen > best_len
                    {
                        project_name = Some(project.name.clone());
                        feature_name = Some(feature.name.clone());
                        agent_name = Some(feature.agent.display_name().to_string());
                        best_len = wlen;
                    }
                }
            }

            // For diff-review while viewing the matching
            // feature, write the proceed signal immediately.
            let mut auto_responded = false;
            if notification_type == "diff-review"
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
                continue;
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
            self.pending_inputs.push(PendingInput {
                session_id,
                cwd,
                message: msg.message.unwrap_or_default(),
                notification_type,
                file_path: PathBuf::new(),
                project_name,
                feature_name,
                proceed_signal: msg.proceed_signal,
                request_id: msg.request_id,
                reply_socket: msg.reply_socket,
            });
        }
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

                    inputs.push(PendingInput {
                        session_id: notif.session_id.unwrap_or_default(),
                        cwd: notif.cwd.unwrap_or_default(),
                        message: notif.message.unwrap_or_default(),
                        notification_type: notif.notification_type.unwrap_or_default(),
                        file_path: path,
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

                if notification_type == "change-reason"
                    && let AppMode::Viewing(ref view) = self.mode
                {
                    let mut found_feature_name = None;
                    let cwd_path = PathBuf::from(&cwd);
                    for project in &self.store.projects {
                        for feature in &project.features {
                            if cwd_path.starts_with(&feature.workdir)
                                || feature.workdir.starts_with(&cwd_path)
                            {
                                found_feature_name = Some(feature.name.clone());
                            }
                        }
                    }

                    if found_feature_name.as_deref() == Some(&view.feature_name) {
                        let response_file = notif.response_file.unwrap_or_default();
                        let proceed_signal_path = proceed_signal_val.unwrap_or_default();

                        self.mode = AppMode::ChangeReasonPrompt(ChangeReasonState {
                            session_id,
                            file_path: notif.file_path.unwrap_or_default(),
                            relative_path: notif.relative_path.unwrap_or_default(),
                            change_id: notif.change_id.unwrap_or_default(),
                            tool: notif.tool.unwrap_or_default(),
                            old_snippet: notif.old_snippet.unwrap_or_default(),
                            new_snippet: notif.new_snippet.unwrap_or_default(),
                            reason: notif.reason.unwrap_or_default(),
                            response_file: PathBuf::from(response_file),
                            proceed_signal: PathBuf::from(proceed_signal_path),
                            request_id: notif.request_id.clone(),
                            reply_socket: notif.reply_socket.clone(),
                        });
                        let _ = std::fs::remove_file(&path);
                        return;
                    }
                }

                let mut project_name = None;
                let mut feature_name = None;
                let mut best_len: usize = 0;
                let cwd_path = PathBuf::from(&cwd);
                for project in &self.store.projects {
                    for feature in &project.features {
                        let wlen = feature.workdir.as_os_str().len();
                        if (cwd_path.starts_with(&feature.workdir)
                            || feature.workdir.starts_with(&cwd_path))
                            && wlen > best_len
                        {
                            project_name = Some(project.name.clone());
                            feature_name = Some(feature.name.clone());
                            best_len = wlen;
                        }
                    }
                }

                inputs.push(PendingInput {
                    session_id,
                    cwd,
                    message: notif.message.unwrap_or_default(),
                    notification_type,
                    file_path: path,
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

        if input.notification_type != "diff-review" && input.notification_type != "input-request" {
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
                    if input.notification_type == "diff-review" {
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
                    return self.enter_view();
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
