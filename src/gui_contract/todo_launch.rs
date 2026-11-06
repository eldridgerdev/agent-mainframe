use super::*;

impl GuiHandle {
    /// Where a TODO's work already lives, mirroring the TUI's `g`/`Enter`
    /// resolution order: a linked feature (a plan run created it) wins and
    /// opens on its first agent session, otherwise a session known to exist in
    /// any feature. A feature link whose feature is gone is cleared so the
    /// dead link stops holding the item back from "implement next". Status is
    /// never touched.
    pub fn todo_work_target(&mut self, todo_id: &str) -> GuiResult<TodoWorkTarget> {
        self.refresh_store()?;
        let todo = self
            .db()?
            .find_todo_by_id(todo_id)
            .map_err(GuiError::from)?
            .ok_or_else(|| GuiError::not_found(format!("TODO '{todo_id}' was not found")))?;
        let mut note = None;
        if let Some(feature_id) = todo.linked_feature_id.as_deref() {
            match self.app.feature_indices_by_id(feature_id) {
                Some((pi, fi)) => {
                    let project = &self.app.store.projects[pi];
                    let feature = &project.features[fi];
                    // Prefer the agent session this TODO was started on, when it
                    // lives in the linked feature; otherwise the first agent.
                    let own = todo.work.agent_session_id.as_deref().and_then(|id| {
                        feature
                            .sessions
                            .iter()
                            .find(|s| s.id == id && s.kind.is_agent_harness())
                    });
                    if let Some(session) =
                        own.or_else(|| feature.sessions.iter().find(|s| s.kind.is_agent_harness()))
                    {
                        return Ok(TodoWorkTarget::Session {
                            target: SessionTarget {
                                project_id: project.id.clone(),
                                feature_id: feature.id.clone(),
                                session_id: session.id.clone(),
                            },
                        });
                    }
                    return Ok(TodoWorkTarget::Feature {
                        target: FeatureTarget {
                            project_id: project.id.clone(),
                            feature_id: feature.id.clone(),
                        },
                    });
                }
                None => {
                    self.db()?
                        .clear_todo_linked_feature_if(todo_id, feature_id)
                        .map_err(GuiError::from)?;
                    note = Some(
                        "The feature planned for this TODO no longer exists; the link was cleared"
                            .to_string(),
                    );
                }
            }
        }
        if let Some(session_id) = todo.work.agent_session_id.as_deref()
            && let Some((pi, fi, si)) = self.app.session_indices_by_id(session_id)
        {
            let project = &self.app.store.projects[pi];
            let feature = &project.features[fi];
            return Ok(TodoWorkTarget::Session {
                target: SessionTarget {
                    project_id: project.id.clone(),
                    feature_id: feature.id.clone(),
                    session_id: feature.sessions[si].id.clone(),
                },
            });
        }
        Ok(TodoWorkTarget::None {
            message: note.unwrap_or_else(|| {
                "No work has started for this TODO, or its session no longer exists".to_string()
            }),
        })
    }

    /// Preview what "implement next" would pick, without changing anything.
    ///
    /// Uses the TUI's pure `next_todo_across` over the scopes in worktree,
    /// project, global order, so priority, scope tie-break and the
    /// held-in-reserve rule are the TUI's, not a second copy. Dead session
    /// links are healed first (status untouched), as the TUI's scan does.
    pub fn preview_next_todo(&mut self, request: &NextTodoRequest) -> GuiResult<NextTodoPreview> {
        use crate::app::NextTodo;
        use crate::db::todos::TodoScope;

        self.refresh_store()?;
        let located = match (&request.project_id, &request.feature_id) {
            (Some(project_id), Some(feature_id)) => {
                let (pi, fi) = self.locate(&FeatureTarget {
                    project_id: project_id.clone(),
                    feature_id: feature_id.clone(),
                })?;
                Some((pi, Some(fi)))
            }
            (Some(project_id), None) => self
                .app
                .store
                .projects
                .iter()
                .position(|p| p.id == *project_id)
                .map(|pi| (pi, None))
                .ok_or_else(|| GuiError::not_found(format!("Project '{project_id}' was not found")))
                .map(Some)?,
            _ => None,
        };

        let mut scopes: Vec<(&'static str, TodoScope, Option<FeatureTarget>)> = Vec::new();
        if let Some((pi, fi)) = located {
            let project = &self.app.store.projects[pi];
            if let Some(fi) = fi
                && let Some(scope) = self.app.worktree_todo_scope(pi, fi)
            {
                scopes.push((
                    "worktree",
                    scope,
                    Some(FeatureTarget {
                        project_id: project.id.clone(),
                        feature_id: project.features[fi].id.clone(),
                    }),
                ));
            }
            if request.include_project {
                scopes.push((
                    "project",
                    TodoScope::Project {
                        project_id: project.id.clone(),
                    },
                    None,
                ));
            }
        }
        if request.include_global {
            scopes.push(("global", TodoScope::Global, None));
        }
        if scopes.is_empty() {
            return Ok(NextTodoPreview::Empty {
                message: "No TODO list is included; turn on a project or global list".to_string(),
            });
        }

        let db = self.db()?;
        db.clear_missing_todo_agent_sessions()
            .map_err(GuiError::from)?;
        let mut lists = Vec::new();
        for (kind, scope, host) in scopes {
            let todos = match db.todo_list(&scope).map_err(GuiError::from)? {
                Some(list) => db.todos(&list.id).map_err(GuiError::from)?,
                None => Vec::new(),
            };
            lists.push((kind, scope, host, todos));
        }
        let slices: Vec<&[crate::db::todos::Todo]> = lists
            .iter()
            .map(|(_, _, _, todos)| todos.as_slice())
            .collect();
        let Some((li, next)) = App::next_todo_across(&slices, &request.skipped_ids) else {
            return Ok(NextTodoPreview::Empty {
                message: App::no_next_todo_message_across(&slices, &request.skipped_ids),
            });
        };
        let (kind, scope, host, todos) = &lists[li];
        let (index, started) = match next {
            NextTodo::Ready(i) => (i, false),
            NextTodo::Started(i) => (i, true),
        };
        let todo = todos[index].clone();
        let reason = if !started {
            None
        } else if todo.work.agent_session_id.is_some() && todo.linked_feature_id.is_some() {
            Some("It already has a session and a planned feature.".to_string())
        } else if todo.work.agent_session_id.is_some() {
            Some("It already has a session.".to_string())
        } else {
            Some("It already has a planned feature.".to_string())
        };
        let candidate = NextTodoCandidate {
            todo,
            scope_kind: kind,
            scope_label: self.app.todo_scope_label(scope),
            host: host.clone(),
        };
        Ok(match reason {
            None => NextTodoPreview::Ready { candidate },
            Some(reason) => NextTodoPreview::Started { candidate, reason },
        })
    }

    /// The list a quick capture from `target` would append to, named for the
    /// user before they type.
    pub fn quick_capture_target(&mut self, target: &FeatureTarget) -> GuiResult<String> {
        self.refresh_store()?;
        let (pi, fi) = self.locate(target)?;
        let scope = self
            .app
            .default_todo_scope(pi, fi)
            .ok_or_else(|| GuiError::not_found("Project not found"))?;
        Ok(self.app.todo_scope_label(&scope))
    }

    /// Append a one-line TODO to the scope the TUI's quick capture resolves
    /// (`default_todo_scope`: the feature's worktree list, or the project's at
    /// the repo root), creating the list lazily with this feature as host and
    /// the feature's TODOs session when it has none, like the TUI.
    pub fn quick_capture_todo(
        &mut self,
        target: &FeatureTarget,
        title: &str,
    ) -> GuiResult<QuickCaptureResponse> {
        self.refresh_store()?;
        let (pi, fi) = self.locate(target)?;
        let title = title.trim();
        if title.is_empty() {
            return Err(GuiError {
                github: None,
                kind: GuiErrorKind::InvalidInput,
                message: "A TODO needs a title".to_string(),
            });
        }
        if title.contains(['\n', '\r']) {
            return Err(GuiError {
                github: None,
                kind: GuiErrorKind::InvalidInput,
                message: "Quick capture is one line; add notes by editing the TODO".to_string(),
            });
        }
        let scope = self
            .app
            .default_todo_scope(pi, fi)
            .ok_or_else(|| GuiError::not_found("Project not found"))?;
        let scope_label = self.app.todo_scope_label(&scope);
        if !self.app.store.projects[pi].features[fi].has_todos_session() {
            self.app
                .add_todos_session_for_picker(pi, fi, None)
                .map_err(GuiError::from)?;
            self.app.message = None;
        }
        let db = self.db()?;
        let list = db
            .load_or_create_todo_list(&scope, Some(&target.feature_id))
            .map_err(GuiError::from)?;
        let todo = db
            .add_todo(&list.id, title, None, crate::db::todos::TodoPriority::Med)
            .map_err(GuiError::from)?;
        Ok(QuickCaptureResponse { todo, scope_label })
    }

    /// Launch a TODO's agent in an explicit feature, using the same session
    /// creation engine as the TUI. The TODO reservation is a conditional SQL
    /// update, so a second GUI/TUI process cannot claim the same item while
    /// this process starts the harness. The prompt is returned unsent.
    pub fn launch_todo_agent(
        &mut self,
        todo_id: &str,
        target: FeatureTarget,
        approved: bool,
    ) -> GuiResult<TodoAgentLaunchResponse> {
        use crate::db::todos::{TodoScope, TodoStatus};

        self.refresh_store()?;
        let (pi, fi) = self.locate(&target)?;
        self.reject_ambiguous_live_session(pi, fi)?;
        let resolved = self
            .db()?
            .resolve_todo_by_id(todo_id)
            .map_err(GuiError::from)?
            .ok_or_else(|| GuiError::not_found(format!("TODO '{todo_id}' was not found")))?;
        let feature = &self.app.store.projects[pi].features[fi];
        let scope_matches = match &resolved.list.scope {
            TodoScope::Worktree {
                project_id,
                workdir,
            } => project_id == &target.project_id && feature.workdir.to_string_lossy() == *workdir,
            TodoScope::Project { project_id } => project_id == &target.project_id,
            TodoScope::Global => true,
        };
        if !scope_matches {
            return Err(GuiError::conflict(
                "Choose a feature belonging to this TODO's current scope",
            ));
        }
        if resolved.todo.work.status != TodoStatus::NotStarted {
            return Err(GuiError::conflict(
                "This TODO is already in progress or completed; no new agent was launched",
            ));
        }
        let prompt = App::todo_spawn_prompt(&resolved.todo);

        // A manually reset TODO may still point at a live session. Reuse it
        // as the TUI does, rather than launching a duplicate harness.
        if let Some(existing_id) = resolved.todo.work.agent_session_id.as_deref()
            && let Some((epi, efi, esi)) = self.app.session_indices_by_id(existing_id)
        {
            let existing_feature = &self.app.store.projects[epi].features[efi];
            let existing_session = &existing_feature.sessions[esi];
            if self.app.tmux.window_exists(
                &existing_feature.tmux_session,
                &existing_session.tmux_window,
            ) {
                if !self
                    .db()?
                    .reserve_todo_agent_launch(todo_id)
                    .map_err(GuiError::from)?
                {
                    return Err(GuiError::conflict(
                        "TODO changed elsewhere; refresh and retry",
                    ));
                }
                if !self
                    .db()?
                    .associate_reserved_todo_agent_session(todo_id, existing_id)
                    .map_err(GuiError::from)?
                {
                    return Err(GuiError::conflict("TODO changed during session reuse"));
                }
                return Ok(TodoAgentLaunchResponse {
                    target: SessionTarget {
                        project_id: self.app.store.projects[epi].id.clone(),
                        feature_id: existing_feature.id.clone(),
                        session_id: existing_id.to_string(),
                    },
                    draft_prompt: prompt,
                    reused_session: true,
                });
            }
        }

        if !approved {
            self.require_start_approval(&format!(
                "Starting an agent for '{}'",
                resolved.todo.title
            ))?;
        }
        if !self
            .db()?
            .reserve_todo_agent_launch(todo_id)
            .map_err(GuiError::from)?
        {
            return Err(GuiError::conflict(
                "TODO changed elsewhere; refresh and retry",
            ));
        }

        let tmux_session = feature.tmux_session.clone();
        let agent = feature.agent.clone();
        let label = App::todo_session_label(&resolved.todo.title);
        let created = self.app.create_agent_session_labeled_identified(
            pi,
            fi,
            &label,
            Some(agent),
            StartIntent::Approved,
        );
        let (_index, session_id, window) = match created {
            Ok(created) => created,
            Err(err) => {
                let _ = self
                    .db()?
                    .rollback_reserved_todo_agent_launch(todo_id, None);
                return Err(GuiError::from(err));
            }
        };

        // `create_agent_session_labeled_identified` retains a live agent when
        // its save conflicts, but its App store is refreshed in that case.
        // Detect the missing record by stable id and clean up only our new
        // window before returning an actionable conflict.
        let Some((current_pi, current_fi)) = self
            .app
            .store
            .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
        else {
            self.abort_created_todo_session(todo_id, &target, &session_id, &tmux_session, &window);
            return Err(GuiError::conflict(
                "Workspace changed during TODO launch; retry",
            ));
        };
        let Some(session) = self.app.store.projects[current_pi].features[current_fi]
            .sessions
            .iter_mut()
            .find(|session| session.id == session_id)
        else {
            self.abort_created_todo_session(todo_id, &target, &session_id, &tmux_session, &window);
            return Err(GuiError::conflict(
                "Workspace changed during TODO launch; retry",
            ));
        };
        session.todo_reference = Some(TodoSessionReference {
            todo_id: todo_id.to_string(),
            launched_from_todo_menu: true,
        });
        match self.app.save_reporting_conflict() {
            Ok(true) => {}
            Ok(false) => {
                self.abort_created_todo_session(
                    todo_id,
                    &target,
                    &session_id,
                    &tmux_session,
                    &window,
                );
                return Err(GuiError::conflict(
                    "Workspace changed during TODO launch; retry",
                ));
            }
            Err(err) => {
                self.abort_created_todo_session(
                    todo_id,
                    &target,
                    &session_id,
                    &tmux_session,
                    &window,
                );
                return Err(GuiError::from(err));
            }
        }

        if !self
            .db()?
            .associate_reserved_todo_agent_session(todo_id, &session_id)
            .map_err(GuiError::from)?
        {
            self.abort_created_todo_session(todo_id, &target, &session_id, &tmux_session, &window);
            return Err(GuiError::conflict(
                "TODO changed while its agent started; the new session was stopped",
            ));
        }

        Ok(TodoAgentLaunchResponse {
            target: SessionTarget {
                project_id: target.project_id,
                feature_id: target.feature_id,
                session_id,
            },
            draft_prompt: prompt,
            reused_session: false,
        })
    }

    /// Create a git worktree feature for a TODO and seed its initial agent.
    /// The existing feature-creation and TODO handoff engines do the work;
    /// this adapter supplies explicit GUI targets and returns the composer
    /// seed without sending it to the agent.
    pub fn launch_todo_in_new_feature(
        &mut self,
        todo_id: &str,
        request: CreateFeatureRequest,
        approved: bool,
    ) -> GuiResult<TodoAgentLaunchResponse> {
        use crate::db::todos::{TodoScope, TodoStatus};

        self.refresh_store()?;
        if !matches!(self.app.mode, AppMode::Normal) {
            return Err(GuiError::conflict("Finish the current workflow first"));
        }
        if request.dry_run || request.plan_mode || request.use_worktree != Some(true) {
            return Err(GuiError::conflict(
                "Starting a TODO in a new feature requires a git worktree without planning",
            ));
        }
        let project = self
            .app
            .store
            .find_project(&request.project_name)
            .ok_or_else(|| GuiError::not_found("Project was deleted; refresh and retry"))?;
        if !project.is_git {
            return Err(GuiError::conflict(
                "New TODO features require a git repository",
            ));
        }
        self.app
            .validate_worktree_hook_choice(&project.repo, request.hook_choice.as_deref())
            .map_err(GuiError::from)?;
        let project_id = project.id.clone();
        let resolved = self
            .db()?
            .resolve_todo_by_id(todo_id)
            .map_err(GuiError::from)?
            .ok_or_else(|| GuiError::not_found("TODO was deleted; refresh and retry"))?;
        let scope_matches = match &resolved.list.scope {
            TodoScope::Worktree { project_id: id, .. } | TodoScope::Project { project_id: id } => {
                id == &project_id
            }
            TodoScope::Global => true,
        };
        if !scope_matches {
            return Err(GuiError::conflict("Choose the TODO's project"));
        }
        if resolved.todo.work.status != TodoStatus::NotStarted {
            return Err(GuiError::conflict(
                "This TODO is already in progress or completed",
            ));
        }
        if !approved {
            self.require_start_approval(&format!(
                "Starting an agent for '{}'",
                resolved.todo.title
            ))?;
        }
        if !self
            .db()?
            .reserve_todo_agent_launch(todo_id)
            .map_err(GuiError::from)?
        {
            return Err(GuiError::conflict(
                "TODO changed elsewhere; refresh and retry",
            ));
        }
        let prompt = App::todo_spawn_prompt(&resolved.todo);
        let origin = TodoPlanOrigin {
            todo_id: todo_id.to_string(),
            list_id: resolved.list.id,
            todo_title: resolved.todo.title,
            host_feature_id: resolved.list.feature_id.unwrap_or_default(),
        };
        let created = self.app.create_feature_from_request(&request);
        let created = match created {
            Ok(created) => created,
            Err(err) => {
                let _ = self
                    .db()?
                    .rollback_reserved_todo_agent_launch(todo_id, None);
                return Err(GuiError::from(err));
            }
        };
        let target = FeatureTarget {
            project_id: created
                .project_id
                .ok_or_else(|| GuiError::conflict("Feature was not created"))?,
            feature_id: created
                .feature_id
                .ok_or_else(|| GuiError::conflict("Feature was not created"))?,
        };
        let (pi, fi) = self.locate(&target)?;
        if !created.started {
            if !approved
                && self
                    .require_start_approval("Starting the new feature's agent")
                    .is_err()
            {
                let _ = self
                    .db()?
                    .rollback_reserved_todo_agent_launch(todo_id, None);
                return Err(GuiError::conflict(
                    "Feature was created, but the resource limit changed before its agent could start. Refresh and start the feature after approving the warning.",
                ));
            }
            if let Err(err) = self
                .app
                .ensure_feature_running(pi, fi, StartIntent::Approved)
            {
                let _ = self
                    .db()?
                    .rollback_reserved_todo_agent_launch(todo_id, None);
                return Err(GuiError::from(err));
            }
            if !self.app.save_reporting_conflict()? {
                let _ = self
                    .db()?
                    .rollback_reserved_todo_agent_launch(todo_id, None);
                return Err(GuiError::conflict(
                    "Feature was created, but the workspace changed before its agent start was saved; refresh",
                ));
            }
        }
        if let Err(err) = self
            .app
            .finish_todo_spawn_in_new_feature(&origin, pi, fi, prompt)
        {
            let _ = self
                .db()?
                .rollback_reserved_todo_agent_launch(todo_id, None);
            return Err(GuiError::from(err));
        }
        let session_id = self
            .db()?
            .find_todo_by_id(todo_id)
            .map_err(GuiError::from)?
            .and_then(|todo| todo.work.agent_session_id);
        let Some(session_id) = session_id else {
            let _ = self
                .db()?
                .rollback_reserved_todo_agent_launch(todo_id, None);
            return Err(GuiError::conflict(
                "Feature was created, but the TODO agent could not be linked; refresh",
            ));
        };
        let draft_prompt = match &self.app.mode {
            AppMode::Compose(state) => state.editor.text().to_string(),
            _ => String::new(),
        };
        self.app.exit_view_without_resuming_plan_interview();
        Ok(TodoAgentLaunchResponse {
            target: SessionTarget {
                project_id: target.project_id,
                feature_id: target.feature_id,
                session_id,
            },
            draft_prompt,
            reused_session: false,
        })
    }

    fn abort_created_todo_session(
        &mut self,
        todo_id: &str,
        target: &FeatureTarget,
        session_id: &str,
        tmux_session: &str,
        window: &str,
    ) {
        if let Err(err) = self.app.tmux.kill_window(tmux_session, window) {
            self.app
                .log_warn("todos", format!("could not stop failed TODO window: {err}"));
        }
        if let Some((pi, fi)) = self
            .app
            .store
            .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
        {
            let sessions = &mut self.app.store.projects[pi].features[fi].sessions;
            let before = sessions.len();
            sessions.retain(|session| session.id != session_id);
            if sessions.len() != before {
                let _ = self.app.save_reporting_conflict();
            }
        }
        if let Some(db) = &self.app.db {
            let _ = db.rollback_reserved_todo_agent_launch(todo_id, None);
        }
    }
}
