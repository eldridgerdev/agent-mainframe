use super::*;

impl GuiHandle {
    /// Offer the same saved-session decision as the TUI when tmux disappears.
    pub fn session_recovery_option(
        &mut self,
        target: &SessionTarget,
    ) -> GuiResult<Option<SessionRecoveryOption>> {
        self.refresh_store()?;
        let (pi, fi, si) = self.locate_session(target)?;
        let feature = &self.app.store.projects[pi].features[fi];
        let session = &feature.sessions[si];
        let harness = match session.kind {
            SessionKind::Claude => "Claude",
            SessionKind::Codex => "Codex",
            SessionKind::Opencode => "OpenCode",
            _ => return Ok(None),
        };
        let Some(saved_id) = crate::app::session_ops::persisted_resume_id(session) else {
            return Ok(None);
        };
        if self.app.tmux.session_exists(&feature.tmux_session) {
            // A running feature: only this session's own window can be
            // brought back, and only when it is gone.
            if self
                .app
                .tmux
                .window_exists(&feature.tmux_session, &session.tmux_window)
            {
                return Ok(None);
            }
        }
        // Unlike the TUI, a feature stopped on purpose still asks: the TUI's
        // silent restart only resumes Claude, so a Codex or OpenCode
        // conversation would otherwise start over without a word.
        Ok(Some(SessionRecoveryOption {
            harness: harness.to_string(),
            saved_id,
        }))
    }

    pub fn saved_agent_sessions(
        &mut self,
        target: &SessionTarget,
    ) -> GuiResult<Vec<SavedAgentSession>> {
        self.refresh_store()?;
        let (pi, fi, si) = self.locate_session(target)?;
        let feature = &self.app.store.projects[pi].features[fi];
        crate::app::session_ops::saved_sessions_for_kind(
            &feature.sessions[si].kind,
            &feature.workdir,
        )
        .map(|sessions| {
            sessions
                .into_iter()
                .map(|(id, title, updated)| SavedAgentSession { id, title, updated })
                .collect()
        })
        .map_err(GuiError::from)
    }

    pub fn recover_session(
        &mut self,
        target: SessionTarget,
        choice: SessionRecoveryChoice,
        picked_id: Option<String>,
        approved: bool,
    ) -> GuiResult<StartFeatureResponse> {
        self.refresh_store()?;
        let (pi, fi, si) = self.locate_session(&target)?;
        self.reject_ambiguous_live_session(pi, fi)?;
        let feature = &self.app.store.projects[pi].features[fi];
        let session = &feature.sessions[si];
        let (agent, provider) = match session.kind {
            SessionKind::Claude => (
                AgentKind::Claude,
                crate::token_tracking::TokenUsageProvider::Claude,
            ),
            SessionKind::Codex => (
                AgentKind::Codex,
                crate::token_tracking::TokenUsageProvider::Codex,
            ),
            SessionKind::Opencode => (
                AgentKind::Opencode,
                crate::token_tracking::TokenUsageProvider::Opencode,
            ),
            _ => return Err(GuiError::conflict("This session cannot be resumed")),
        };
        let feature_running = self.app.tmux.session_exists(&feature.tmux_session);
        if feature_running
            && self
                .app
                .tmux
                .window_exists(&feature.tmux_session, &session.tmux_window)
        {
            return Ok(StartFeatureResponse {
                feature_id: target.feature_id,
                already_running: true,
                message: format!("'{}' is already running", session.label),
            });
        }
        let resume_id = match choice {
            SessionRecoveryChoice::Resume => Some(
                crate::app::session_ops::persisted_resume_id(session).ok_or_else(|| {
                    GuiError::conflict("The saved session ID is no longer available")
                })?,
            ),
            SessionRecoveryChoice::Clear => None,
            SessionRecoveryChoice::Pick => {
                let id = picked_id.ok_or_else(|| GuiError::conflict("Choose a saved session"))?;
                let sessions = crate::app::session_ops::saved_sessions_for_kind(
                    &session.kind,
                    &feature.workdir,
                )
                .map_err(GuiError::from)?;
                if !sessions.iter().any(|(candidate, _, _)| candidate == &id) {
                    return Err(GuiError::conflict(
                        "The chosen session is no longer available",
                    ));
                }
                Some(id)
            }
        };
        if !approved {
            let what = if feature_running {
                &session.label
            } else {
                &feature.name
            };
            self.require_start_approval(&format!("Recovering '{what}'"))?;
        }
        self.app
            .tmux
            .check_harness_available(&agent)
            .map_err(GuiError::from)?;

        if feature_running {
            return self.recover_session_window(target, si, choice, resume_id, &provider);
        }

        let mut created_session = false;
        if let Err(error) = self.app.ensure_feature_running_for_recovery(
            pi,
            fi,
            crate::app::feature_ops::RecoveryLaunch {
                session_id: target.session_id.clone(),
                resume_id: resume_id.clone(),
                // Per tab, like the TUI's session row: the feature's other
                // agent sessions stay stopped, each with its own resume choice.
                only_this_agent: true,
            },
            &mut created_session,
            StartIntent::Approved,
        ) {
            if created_session {
                let tmux_session = &self.app.store.projects[pi].features[fi].tmux_session;
                let _ = self.app.tmux.kill_session(tmux_session);
            }
            if created_session {
                self.app.store.projects[pi].features[fi].status = ProjectStatus::Stopped;
            }
            return Err(GuiError::from(error));
        }

        // Another process may have started the same tmux session between our
        // first existence check and the launch helper's own check. In that
        // case it used its own resume choice; do not record ours over it.
        if !created_session {
            return Ok(StartFeatureResponse {
                feature_id: target.feature_id,
                already_running: true,
                message: "The feature is already running".to_string(),
            });
        }

        let tmux_session = self.app.store.projects[pi].features[fi]
            .tmux_session
            .clone();
        let name = self.app.store.projects[pi].features[fi].name.clone();

        let session = &mut self.app.store.projects[pi].features[fi].sessions[si];
        Self::apply_recovery_choice(session, choice, resume_id.as_deref(), &provider);
        let session_kind = session.kind.clone();
        let outcome =
            self.app.save_reapplying(|store| {
                let Some((pi, fi)) =
                    store.locate_feature_by_id(Some(&target.project_id), &target.feature_id)
                else {
                    return false;
                };
                let feature = &mut store.projects[pi].features[fi];
                if feature.tmux_session != tmux_session {
                    return false;
                }
                let Some(session) = feature.sessions.iter_mut().find(|session| {
                    session.id == target.session_id && session.kind == session_kind
                }) else {
                    return false;
                };
                Self::apply_recovery_choice(session, choice, resume_id.as_deref(), &provider);
                feature.status = ProjectStatus::Idle;
                feature.touch();
                true
            });
        match outcome {
            Ok(ReapplyOutcome::Saved) => {}
            Ok(ReapplyOutcome::TargetGone | ReapplyOutcome::Conflict) => {
                self.stop_failed_recovery(&tmux_session)?;
                return Err(GuiError::conflict(
                    "Workspace changed elsewhere while recovery started; the new session was stopped. Refresh and retry",
                ));
            }
            Err(error) => {
                self.stop_failed_recovery(&tmux_session)?;
                return Err(GuiError::from(error));
            }
        }
        Ok(StartFeatureResponse {
            feature_id: target.feature_id,
            already_running: false,
            message: format!("Started '{name}'"),
        })
    }

    fn apply_recovery_choice(
        session: &mut crate::project::FeatureSession,
        choice: SessionRecoveryChoice,
        resume_id: Option<&str>,
        provider: &crate::token_tracking::TokenUsageProvider,
    ) {
        match choice {
            SessionRecoveryChoice::Clear => {
                session.claude_session_id = None;
                session.clear_token_usage_source();
            }
            SessionRecoveryChoice::Pick => {
                let id = resume_id
                    .expect("picked session ID was validated above")
                    .to_string();
                if session.kind == SessionKind::Claude {
                    session.claude_session_id = Some(id.clone());
                }
                session.set_token_usage_source_exact(crate::token_tracking::TokenUsageSource {
                    provider: provider.clone(),
                    id,
                });
            }
            SessionRecoveryChoice::Resume => {}
        }
    }

    fn stop_failed_recovery(&mut self, tmux_session: &str) -> GuiResult<()> {
        self.app.tmux.kill_session(tmux_session).map_err(|error| GuiError {
            kind: GuiErrorKind::Internal,
            message: format!(
                "Recovery could not be saved, and the newly started background session '{tmux_session}' could not be stopped: {error}"
            ),
        })?;
        // A failed DB write leaves the attempted recovery in memory. Restore
        // the last committed state so a later GUI action cannot save it.
        if let Some(db) = &self.app.db
            && let Ok((store, version)) = db.load_store_versioned()
        {
            self.app.adopt_store_from_disk(store, version);
        }
        Ok(())
    }

    fn recover_session_window(
        &mut self,
        target: SessionTarget,
        si: usize,
        choice: SessionRecoveryChoice,
        resume_id: Option<String>,
        provider: &crate::token_tracking::TokenUsageProvider,
    ) -> GuiResult<StartFeatureResponse> {
        let (pi, fi) = self.locate(&FeatureTarget {
            project_id: target.project_id.clone(),
            feature_id: target.feature_id.clone(),
        })?;
        let session = &mut self.app.store.projects[pi].features[fi].sessions[si];
        let original_session = session.clone();
        Self::apply_recovery_choice(session, choice, resume_id.as_deref(), provider);
        let label = session.label.clone();
        self.app.message = None;
        // `restart_*` saves the record, carrying the choice with it.
        let restart = self
            .app
            .restart_stopped_session_window_resuming(pi, fi, si, Some(resume_id))
            .map_err(GuiError::from)
            .and_then(|started| {
                if started {
                    Ok(())
                } else {
                    Err(GuiError::conflict(format!("'{label}' cannot be restarted")))
                }
            });
        if let Err(error) = restart {
            // A no-op restart never saved the choice. Restore it before a
            // later save can persist it, even without a database attached.
            if let Ok((pi, fi, si)) = self.locate_session(&target) {
                self.app.store.projects[pi].features[fi].sessions[si] = original_session;
            }
            if let Some(db) = &self.app.db
                && let Ok((store, version)) = db.load_store_versioned()
            {
                self.app.adopt_store_from_disk(store, version);
            }
            return Err(error);
        }
        self.app.message = None;
        Ok(StartFeatureResponse {
            feature_id: target.feature_id,
            already_running: false,
            message: format!("Started '{label}'"),
        })
    }

    /// Stop one session and keep it listed: the GUI counterpart of the TUI's
    /// `x` on a session row, sharing its core (`App::stop_session_at`). The
    /// stop is persisted, so the session stays down when its feature next
    /// starts. Stopping the feature's last running session — the last live
    /// tmux window — stops the feature the way `stop_feature` does.
    pub fn stop_session(&mut self, target: SessionTarget) -> GuiResult<StopSessionResponse> {
        self.refresh_store()?;
        let (pi, fi, si) = self.locate_session(&target)?;
        self.reject_ambiguous_live_session(pi, fi)?;
        self.reconcile_feature_status(pi, fi);

        let response = |already_stopped, feature_stopped, message| StopSessionResponse {
            session_id: target.session_id.clone(),
            already_stopped,
            feature_stopped,
            message,
        };

        let feature = &self.app.store.projects[pi].features[fi];
        let session = &feature.sessions[si];
        // A tab recovered on its own leaves the feature's other agents
        // windowless without flagging them stopped, so `stop_session_at` would
        // count them as running and kill this, the last live window -- taking
        // the tmux session down without the feature-stop bookkeeping.
        // With no other runnable session it already stops the feature.
        if feature.status != ProjectStatus::Stopped
            && !session.stopped
            && session.kind.is_tmux_backed()
            && feature
                .sessions
                .iter()
                .enumerate()
                .any(|(index, other)| index != si && other.runs_with_feature())
            && self
                .app
                .tmux
                .window_exists(&feature.tmux_session, &session.tmux_window)
            && !self.another_window_is_live(pi, fi, si)
        {
            let (label, name) = (session.label.clone(), feature.name.clone());
            self.app.do_stop_feature(pi, fi)?;
            return Ok(response(
                false,
                true,
                format!("Stopped '{name}': '{label}' was its last running session"),
            ));
        }
        match self.app.stop_session_at(pi, fi, si)? {
            SessionStop::StopsFeature => {
                let feature = &self.app.store.projects[pi].features[fi];
                let label = feature.sessions[si].label.clone();
                let name = feature.name.clone();
                self.app.do_stop_feature(pi, fi)?;
                Ok(response(
                    false,
                    true,
                    format!("Stopped '{name}': '{label}' was its last running session"),
                ))
            }
            SessionStop::NotStoppable => {
                Err(GuiError::conflict("This session has no terminal to stop"))
            }
            outcome @ SessionStop::OnlyRunnable(_) => {
                Err(GuiError::conflict(outcome.message().unwrap_or_default()))
            }
            outcome @ SessionStop::AlreadyStopped(_) => {
                Ok(response(true, false, outcome.message().unwrap_or_default()))
            }
            outcome @ (SessionStop::Recorded(_) | SessionStop::Stopped(_)) => Ok(response(
                false,
                false,
                outcome.message().unwrap_or_default(),
            )),
        }
    }
}
