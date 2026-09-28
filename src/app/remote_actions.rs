//! `App`-side handling of the phone's `POST /actions` requests: the same
//! operations the dashboard offers, run on the main loop that owns the
//! state they touch.
//!
//! Two rules keep a remote action from wedging the desk:
//! - Nothing here opens a dialog. A step that would stop to ask — the
//!   resource gate, an `on_start`/`on_stop` hook with a prompt — either
//!   warns and carries on (the gate, as a toast at the desk, the same
//!   policy AMF uses for starts buried in longer flows) or refuses with a
//!   reason the phone shows, when the missing answer is a real choice.
//! - The desk's selection is left where the user put it.

use crate::automation::CreateFeatureRequest;
use crate::project::{AgentKind, ProjectStatus, SessionKind, VibeMode};
use crate::remote_server::{RemoteAction, RemoteCommand};

use super::attention::HarnessCapabilities;
use super::resource_gate::StartIntent;
use super::{App, AppMode, Selection};

const REMOTE_START: &str = "a feature from your phone";

/// `RemoteAction::CreateFeature`, borrowed.
struct RemoteCreateFeature<'a> {
    project_name: &'a str,
    branch: &'a str,
    agent: Option<&'a str>,
    mode: Option<&'a str>,
    use_worktree: Option<bool>,
    review: bool,
}

impl App {
    /// Answer every `/actions` request the server forwarded since last tick.
    pub(super) fn drain_remote_commands(&mut self) -> bool {
        let Some(handle) = &mut self.remote_server else {
            return false;
        };
        let mut commands = Vec::new();
        while let Some(command) = handle.try_recv_command() {
            commands.push(command);
        }
        let changed = !commands.is_empty();
        for RemoteCommand {
            device_id,
            action,
            reply,
        } in commands
        {
            let result = self.apply_remote_action(action.clone());
            match &result {
                Ok(_) => self.log_info("remote_actions", format!("Device {device_id}: {action:?}")),
                Err(reason) => self.log_warn(
                    "remote_actions",
                    format!("Device {device_id}: {action:?} refused: {reason}"),
                ),
            }
            let _ = reply.send(result);
        }
        changed
    }

    pub(crate) fn apply_remote_action(
        &mut self,
        action: RemoteAction,
    ) -> Result<serde_json::Value, String> {
        use serde_json::Value;
        let selection = self.selection.clone();
        let result = match action {
            RemoteAction::StartFeature { feature_id } => {
                self.remote_start_feature(&feature_id).map(Value::String)
            }
            RemoteAction::StopFeature { feature_id } => {
                self.remote_stop_feature(&feature_id).map(Value::String)
            }
            RemoteAction::AddSession { feature_id, kind } => self
                .remote_add_session(&feature_id, &kind)
                .map(Value::String),
            RemoteAction::CreateFeature {
                project_name,
                branch,
                agent,
                mode,
                use_worktree,
                review,
            } => {
                return self
                    .remote_create_feature(RemoteCreateFeature {
                        project_name: &project_name,
                        branch: &branch,
                        agent: agent.as_deref(),
                        mode: mode.as_deref(),
                        use_worktree,
                        review,
                    })
                    .map(Value::String);
            }
            RemoteAction::SessionOpened { session_id } => {
                self.remote_session_opened(&session_id);
                Ok(Value::Null)
            }
            RemoteAction::RemoveSession { session_id } => {
                return self.remote_remove_session(&session_id).map(Value::String);
            }
            RemoteAction::DeleteFeature { feature_id } => {
                return self.remote_delete_feature(&feature_id).map(Value::String);
            }
            RemoteAction::ListTodos { feature_id } => self.remote_list_todos(&feature_id),
            RemoteAction::AddTodo {
                feature_id,
                scope,
                title,
            } => self.remote_add_todo(&feature_id, &scope, &title),
            RemoteAction::SetTodoStatus { todo_id, status } => {
                self.remote_set_todo_status(&todo_id, &status)
            }
            RemoteAction::DeleteTodo { todo_id } => self.remote_delete_todo(&todo_id),
            RemoteAction::StartTodo {
                feature_id,
                todo_id,
            } => self.remote_start_todo(&feature_id, &todo_id),
            RemoteAction::ListPrompts { feature_id } => self.remote_list_prompts(&feature_id),
            RemoteAction::RenderPrompt { body, values } => {
                let values: Vec<(String, String)> = values.into_iter().collect();
                Ok(Value::String(crate::prompt_library::render_template(
                    &body, &values,
                )))
            }
        };
        // Adding a session moves the dashboard cursor onto it; a phone
        // shouldn't move the desk's. (Paths that change indices return above
        // and settle the cursor themselves.)
        self.selection = selection;
        result
    }

    fn remote_feature(&self, feature_id: &str) -> Result<(usize, usize), String> {
        self.feature_indices_by_id(feature_id)
            .ok_or_else(|| "That feature no longer exists.".to_string())
    }

    fn remote_start_feature(&mut self, feature_id: &str) -> Result<String, String> {
        let (pi, fi) = self.remote_feature(feature_id)?;
        let feature = &self.store.projects[pi].features[fi];
        let name = feature.name.clone();
        if feature.status != ProjectStatus::Stopped {
            return Err(format!("'{name}' is already running."));
        }
        if feature.pending_worktree_script {
            return Err(format!(
                "'{name}' is still running its worktree setup script."
            ));
        }
        let workdir = feature.workdir.clone();
        let on_start = self.active_extension.lifecycle_hooks.on_start.clone();
        if on_start.as_ref().is_some_and(|cfg| cfg.prompt().is_some()) {
            return Err(
                "This project's on_start hook asks a question — start it from the desk.".into(),
            );
        }

        self.ensure_feature_running(pi, fi, StartIntent::Warn(REMOTE_START))
            .map_err(|e| format!("Couldn't start '{name}': {e}"))?;
        if let Some(cfg) = on_start {
            self.run_lifecycle_hook(cfg.script(), &workdir, None);
        }
        self.save().map_err(|e| e.to_string())?;
        self.push_toast_info(format!("Started '{name}' from your phone"));
        Ok(format!("Started '{name}'"))
    }

    fn remote_stop_feature(&mut self, feature_id: &str) -> Result<String, String> {
        let (pi, fi) = self.remote_feature(feature_id)?;
        let feature = &self.store.projects[pi].features[fi];
        let name = feature.name.clone();
        if feature.status == ProjectStatus::Stopped {
            return Err(format!("'{name}' is already stopped."));
        }
        if feature.pending_worktree_script {
            return Err(format!(
                "'{name}' is still running its worktree setup script."
            ));
        }
        let workdir = feature.workdir.clone();
        let on_stop = self.active_extension.lifecycle_hooks.on_stop.clone();
        if on_stop.as_ref().is_some_and(|cfg| cfg.prompt().is_some()) {
            return Err(
                "This project's on_stop hook asks a question — stop it from the desk.".into(),
            );
        }
        if let Some(cfg) = on_stop {
            self.run_lifecycle_hook(cfg.script(), &workdir, None);
        }
        self.do_stop_feature(pi, fi)
            .map_err(|e| format!("Couldn't stop '{name}': {e}"))?;
        self.push_toast_info(format!("Stopped '{name}' from your phone"));
        Ok(format!("Stopped '{name}'"))
    }

    fn remote_add_session(&mut self, feature_id: &str, kind: &str) -> Result<String, String> {
        let (pi, fi) = self.remote_feature(feature_id)?;
        let feature = &self.store.projects[pi].features[fi];
        if feature.pending_worktree_script {
            return Err(format!(
                "'{}' is still running its worktree setup script.",
                feature.name
            ));
        }
        let kind = match kind {
            "terminal" => SessionKind::Terminal,
            "agent" => super::session_ops::session_kind_for_agent(&feature.agent),
            other => match AgentKind::from_slug(other) {
                Some(agent) => super::session_ops::session_kind_for_agent(&agent),
                None => return Err(format!("Unknown session type '{other}'.")),
            },
        };
        // Same rule as the dashboard: a harness spends the agent budget,
        // and so does anything that brings a stopped feature up. Warn at the
        // desk rather than park on a dialog nobody at the phone can answer.
        if kind.is_agent_harness() || feature.status == ProjectStatus::Stopped {
            let _ = self.gate_launch(StartIntent::Warn(REMOTE_START));
        }
        let before = self.store.projects[pi].features[fi].sessions.len();
        self.add_builtin_session_unchecked(pi, fi, kind, None)
            .map_err(|e| format!("Couldn't add the session: {e}"))?;
        let feature = &self.store.projects[pi].features[fi];
        match feature.sessions.get(before) {
            Some(session) => Ok(session.id.clone()),
            None => Err("The session wasn't created.".into()),
        }
    }

    fn remote_create_feature(
        &mut self,
        options: RemoteCreateFeature<'_>,
    ) -> Result<String, String> {
        let RemoteCreateFeature {
            project_name,
            branch,
            agent,
            mode,
            use_worktree,
            review,
        } = options;
        let project = self
            .store
            .find_project(project_name)
            .ok_or_else(|| format!("No project named '{project_name}'."))?;
        let agent = match agent {
            None | Some("") => project.preferred_agent.clone(),
            Some(slug) => {
                AgentKind::from_slug(slug).ok_or_else(|| format!("Unknown agent '{slug}'."))?
            }
        };
        let mode = match mode {
            None | Some("") => VibeMode::default(),
            Some(mode) => serde_json::from_value(serde_json::json!(mode))
                .map_err(|_| format!("Unknown mode '{mode}'."))?,
        };
        let request = CreateFeatureRequest {
            project_name: project_name.to_string(),
            branch: branch.trim().to_string(),
            agent,
            mode,
            use_worktree,
            review,
            ..Default::default()
        };
        let response = self
            .create_feature_from_request(&request)
            .map_err(|e| e.to_string())?;
        self.push_toast_info(format!("Created '{}' from your phone", response.branch));
        let feature_id = self
            .store
            .find_project(project_name)
            .and_then(|project| {
                project
                    .features
                    .iter()
                    .find(|feature| feature.branch == response.branch)
            })
            .map(|feature| feature.id.clone())
            .unwrap_or_default();
        Ok(feature_id)
    }

    /// Whether the desk is showing this feature right now — a remote
    /// removal would pull it out from under the person at the keyboard.
    fn desk_is_viewing(&self, project: &str, feature: &str) -> bool {
        matches!(&self.mode, AppMode::Viewing(view)
            if view.project_name == project && view.feature_name == feature)
    }

    fn remote_remove_session(&mut self, session_id: &str) -> Result<String, String> {
        let found = self
            .store
            .projects
            .iter()
            .enumerate()
            .find_map(|(pi, project)| {
                project
                    .features
                    .iter()
                    .enumerate()
                    .find_map(|(fi, feature)| {
                        feature
                            .sessions
                            .iter()
                            .position(|session| session.id == session_id)
                            .map(|si| (pi, fi, si))
                    })
            });
        let Some((pi, fi, si)) = found else {
            return Err("That session no longer exists.".into());
        };
        let project = &self.store.projects[pi];
        let feature = &project.features[fi];
        if self.desk_is_viewing(&project.name, &feature.name) {
            return Err("That feature is open on the desk — close it there first.".into());
        }
        let label = feature.sessions[si].label.clone();

        // `remove_session` acts on the dashboard selection; point it at the
        // session, then put the desk's cursor back, shifted past the gap.
        let saved = std::mem::replace(&mut self.selection, Selection::Session(pi, fi, si));
        let result = self.remove_session();
        self.selection = match saved {
            Selection::Session(p, f, s) if p == pi && f == fi && s == si => {
                Selection::Feature(pi, fi)
            }
            Selection::Session(p, f, s) if p == pi && f == fi && s > si => {
                Selection::Session(p, f, s - 1)
            }
            other => other,
        };
        result.map_err(|e| e.to_string())?;
        self.push_toast_info(format!("Removed '{label}' from your phone"));
        Ok(format!("Removed '{label}'"))
    }

    /// Delete through the dashboard's own flow, then send the deletion to
    /// the background (as `h` does in the progress dialog) so the desk's
    /// screen is left as it was. Anything that would stop to ask — a TODO
    /// list to re-home, a parked plan interview — is refused instead.
    fn remote_delete_feature(&mut self, feature_id: &str) -> Result<String, String> {
        let (pi, fi) = self.remote_feature(feature_id)?;
        let project_name = self.store.projects[pi].name.clone();
        let feature_name = self.store.projects[pi].features[fi].name.clone();
        if self.desk_is_viewing(&project_name, &feature_name) {
            return Err("That feature is open on the desk — close it there first.".into());
        }
        if self.is_feature_being_deleted(&project_name, &feature_name) {
            return Err(format!("'{feature_name}' is already being deleted."));
        }

        let desk_mode = std::mem::replace(
            &mut self.mode,
            AppMode::DeletingFeature(project_name.clone(), feature_name.clone()),
        );
        let message_before = self.message.clone();
        let result = self.delete_feature();
        let outcome = match &self.mode {
            AppMode::DeletingFeatureInProgress(_) => {
                self.hide_deleting_feature();
                Ok(format!("Deleting '{feature_name}'"))
            }
            AppMode::TodoDeleteDisposition(_) => Err(format!(
                "'{feature_name}' has unfinished TODOs — delete it at the desk to choose where they go."
            )),
            _ => Err(match (&result, &self.message) {
                (Err(e), _) => format!("Couldn't delete '{feature_name}': {e}"),
                (Ok(()), Some(message)) if Some(message) != message_before.as_ref() => {
                    message.clone()
                }
                _ => format!("Couldn't delete '{feature_name}'."),
            }),
        };
        self.mode = desk_mode;
        self.message = message_before;
        if outcome.is_ok() {
            self.push_toast_info(format!("Deleting '{feature_name}' from your phone"));
        }
        outcome
    }

    fn remote_list_prompts(&self, feature_id: &str) -> Result<serde_json::Value, String> {
        use crate::prompt_library::{PlaceholderKind, infer_placeholder_slots};
        let (pi, fi) = self.remote_feature(feature_id)?;
        let mut seen = std::collections::HashSet::new();
        let prompts: Vec<serde_json::Value> = self
            .prompt_library_for_feature(pi, fi)
            .into_iter()
            // A worktree is usually a checkout of the project, so its
            // `amf.json` repeats the project's prompts. The desk badges both;
            // a phone list just needs one (the worktree's, which comes first).
            .filter(|entry| {
                seen.insert((entry.template.name.clone(), entry.template.body.clone()))
            })
            .map(|entry| {
                let template = entry.template;
                // Explicit definitions win; otherwise the slots come from the
                // body, the same fallback the desk's fill-in flow uses.
                let slots: Vec<serde_json::Value> = if template.placeholders.is_empty() {
                    infer_placeholder_slots(&template.body)
                        .into_iter()
                        .map(|slot| {
                            serde_json::json!({
                                "key": slot.key,
                                "label": slot.label.clone().unwrap_or_else(|| {
                                    if slot.options.is_empty() { slot.key.clone() } else { "Choose an option".into() }
                                }),
                                "kind": if slot.options.is_empty() { "text" } else { "select" },
                                "options": slot.options,
                                "default": null,
                            })
                        })
                        .collect()
                } else {
                    template
                        .placeholders
                        .iter()
                        .map(|placeholder| {
                            let (kind, options, default) = match &placeholder.kind {
                                PlaceholderKind::Text { default } => ("text", vec![], default.clone()),
                                PlaceholderKind::MultiLine { default } => {
                                    ("multiline", vec![], default.clone())
                                }
                                PlaceholderKind::Select { options } => {
                                    ("select", options.clone(), None)
                                }
                            };
                            serde_json::json!({
                                "key": placeholder.key,
                                "label": placeholder.display_label(),
                                "kind": kind,
                                "options": options,
                                "default": default,
                            })
                        })
                        .collect()
                };
                serde_json::json!({
                    "name": template.name,
                    "description": template.description,
                    "body": template.body,
                    "source": entry.source.label(),
                    "slots": slots,
                })
            })
            .collect();
        Ok(serde_json::json!(prompts))
    }

    /// Mirror of the clear-on-open rule in `view.rs`: harnesses that never
    /// report resuming lose their attention flag when the session is
    /// opened, since nothing else will clear it.
    fn remote_session_opened(&mut self, session_id: &str) {
        let found = self.store.projects.iter().find_map(|project| {
            project.features.iter().find_map(|feature| {
                feature
                    .sessions
                    .iter()
                    .any(|session| session.id == session_id)
                    .then(|| (feature.tmux_session.clone(), feature.agent.clone()))
            })
        });
        if let Some((tmux_session, agent)) = found
            && HarnessCapabilities::for_agent(&agent).clears_on_open()
        {
            self.clear_attention(&tmux_session);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::remote_server::tests::test_app_with_feature_and_db;

    fn feature_id(app: &App) -> String {
        app.store.projects[0].features[0].id.clone()
    }

    #[test]
    fn unknown_feature_is_refused() {
        let (_db, mut app) = test_app_with_feature_and_db();
        assert_eq!(
            app.apply_remote_action(RemoteAction::StopFeature {
                feature_id: "nope".into()
            }),
            Err::<serde_json::Value, _>("That feature no longer exists.".into())
        );
    }

    #[test]
    fn starting_a_running_feature_is_refused() {
        let (_db, mut app) = test_app_with_feature_and_db();
        let id = feature_id(&app);
        assert_eq!(
            app.apply_remote_action(RemoteAction::StartFeature { feature_id: id }),
            Err::<serde_json::Value, _>("'my-feature' is already running.".into())
        );
    }

    #[test]
    fn unknown_session_kind_is_refused() {
        let (_db, mut app) = test_app_with_feature_and_db();
        let id = feature_id(&app);
        assert_eq!(
            app.apply_remote_action(RemoteAction::AddSession {
                feature_id: id,
                kind: "emacs".into()
            }),
            Err::<serde_json::Value, _>("Unknown session type 'emacs'.".into())
        );
    }

    #[test]
    fn create_feature_needs_a_known_project() {
        let (_db, mut app) = test_app_with_feature_and_db();
        assert_eq!(
            app.apply_remote_action(RemoteAction::CreateFeature {
                project_name: "ghost".into(),
                branch: "x".into(),
                agent: None,
                mode: None,
                use_worktree: None,
                review: false,
            }),
            Err::<serde_json::Value, _>("No project named 'ghost'.".into())
        );
    }

    #[test]
    fn stopping_a_feature_kills_its_tmux_session() {
        let (_db, mut app) = test_app_with_feature_and_db();
        let id = feature_id(&app);
        let mut tmux = crate::traits::MockTmuxOps::new();
        tmux.expect_kill_session()
            .withf(|session| session == "amf-my-feature")
            .times(1)
            .returning(|_| Ok(()));
        tmux.expect_session_exists().returning(|_| false);
        app.tmux = Box::new(tmux);

        let result = app.apply_remote_action(RemoteAction::StopFeature { feature_id: id });

        assert_eq!(result, Ok(serde_json::json!("Stopped 'my-feature'")));
        assert_eq!(
            app.store.projects[0].features[0].status,
            ProjectStatus::Stopped
        );
    }
}
