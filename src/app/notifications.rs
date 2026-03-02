use anyhow::Result;
use serde::Deserialize;
use std::path::{Path, PathBuf};

use super::*;

impl App {
    pub fn scan_notifications(&mut self) {
        #[derive(Deserialize)]
        struct NotificationJson {
            session_id: Option<String>,
            cwd: Option<String>,
            message: Option<String>,
            #[serde(alias = "type")]
            notification_type: Option<String>,
            proceed_signal: Option<String>,
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
                let notify_dir = feature
                    .workdir
                    .join(".claude")
                    .join("notifications");

                let entries =
                    match std::fs::read_dir(&notify_dir) {
                        Ok(e) => e,
                        Err(_) => continue,
                    };

                for entry in entries.flatten() {
                    let path = entry.path();
                    if path
                        .extension()
                        .and_then(|e| e.to_str())
                        != Some("json")
                    {
                        continue;
                    }

                    let data =
                        match std::fs::read_to_string(&path)
                        {
                            Ok(d) => d,
                            Err(_) => continue,
                        };

                    let notif: NotificationJson =
                        match serde_json::from_str(&data) {
                            Ok(n) => n,
                            Err(_) => continue,
                        };

                    inputs.push(PendingInput {
                        session_id: notif
                            .session_id
                            .unwrap_or_default(),
                        cwd: notif.cwd.unwrap_or_default(),
                        message: notif
                            .message
                            .unwrap_or_default(),
                        notification_type: notif
                            .notification_type
                            .unwrap_or_default(),
                        file_path: path,
                        project_name: Some(
                            project.name.clone(),
                        ),
                        feature_name: Some(
                            feature.name.clone(),
                        ),
                        proceed_signal: notif.proceed_signal,
                    });
                }
            }
        }

        let global_notify_dir = crate::project::amf_config_dir()
            .join("notifications");

        if let Ok(entries) = std::fs::read_dir(&global_notify_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str())
                    != Some("json")
                {
                    continue;
                }

                let data = match std::fs::read_to_string(&path) {
                    Ok(d) => d,
                    Err(_) => continue,
                };

                let notif: NotificationJson =
                    match serde_json::from_str(&data) {
                        Ok(n) => n,
                        Err(_) => continue,
                    };

                let session_id =
                    notif.session_id.unwrap_or_default();
                let cwd = notif.cwd.unwrap_or_default();
                let notification_type =
                    notif.notification_type.unwrap_or_default();
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
                        let response_file = notif
                            .response_file
                            .unwrap_or_default();
                        let proceed_signal_path = proceed_signal_val
                            .unwrap_or_default();

                        self.mode = AppMode::ChangeReasonPrompt(
                            ChangeReasonState {
                                session_id,
                                file_path: notif
                                    .file_path
                                    .unwrap_or_default(),
                                relative_path: notif
                                    .relative_path
                                    .unwrap_or_default(),
                                change_id: notif
                                    .change_id
                                    .unwrap_or_default(),
                                tool: notif.tool.unwrap_or_default(),
                                old_snippet: notif
                                    .old_snippet
                                    .unwrap_or_default(),
                                new_snippet: notif
                                    .new_snippet
                                    .unwrap_or_default(),
                                reason: notif.reason.unwrap_or_default(),
                                response_file: PathBuf::from(response_file),
                                proceed_signal: PathBuf::from(proceed_signal_path),
                            },
                        );
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
                        let wlen = feature
                            .workdir
                            .as_os_str()
                            .len();
                        if (cwd_path
                            .starts_with(&feature.workdir)
                            || feature
                                .workdir
                                .starts_with(&cwd_path))
                            && wlen > best_len
                        {
                            project_name =
                                Some(project.name.clone());
                            feature_name =
                                Some(feature.name.clone());
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
                });
            }
        }

        self.pending_inputs = inputs;

        if let AppMode::Viewing(ref view) = self.mode {
            let feat_name = view.feature_name.clone();
            for input in &self.pending_inputs {
                if input.notification_type == "diff-review"
                    && input.feature_name.as_deref()
                        == Some(&feat_name)
                    && let Some(signal_path) =
                        &input.proceed_signal
                {
                    let p = Path::new(signal_path);
                    if let Some(parent) = p.parent() {
                        let _ =
                            std::fs::create_dir_all(parent);
                    }
                    let _ = std::fs::write(p, "");
                }
            }
        }
    }

    pub fn handle_notification_select(
        &mut self,
    ) -> Result<()> {
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

        if input.notification_type != "diff-review"
            && input.notification_type != "input-request"
        {
            let _ = std::fs::remove_file(&input.file_path);
        }

        if let (Some(proj_name), Some(feat_name)) =
            (&input.project_name, &input.feature_name)
        {
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
                        && let Some(signal_path) =
                            &input.proceed_signal
                    {
                        let p = Path::new(signal_path);
                        if let Some(parent) = p.parent() {
                            let _ =
                                std::fs::create_dir_all(
                                    parent,
                                );
                        }
                        let _ = std::fs::write(p, "");
                    }
                    self.selection =
                        Selection::Feature(pi, fi);
                    self.pending_inputs.remove(idx);
                    return self.enter_view();
                }
            }
        }

        self.pending_inputs.remove(idx);
        let _ = std::fs::remove_file(&input.file_path);
        self.mode = AppMode::Normal;
        self.message = Some(
            "Notification cleared (no matching feature)"
                .into(),
        );
        Ok(())
    }
}
