use anyhow::Result;

use super::*;

impl App {
    pub fn start_rename_session(&mut self) {
        let (pi, fi, si) = match &self.selection {
            Selection::Session(pi, fi, si) => {
                (*pi, *fi, *si)
            }
            _ => return,
        };

        let label = match self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
            .and_then(|f| f.sessions.get(si))
        {
            Some(s) => s.label.clone(),
            None => return,
        };

        self.mode =
            AppMode::RenamingSession(RenameSessionState {
                project_idx: pi,
                feature_idx: fi,
                session_idx: si,
                input: label,
                return_to: RenameReturnTo::Dashboard,
            });
    }

    pub fn start_rename_from_switcher(&mut self) {
        let (pi, fi, si, switcher_state) = match &self.mode
        {
            AppMode::SessionSwitcher(state) => {
                let pi = self
                    .store
                    .projects
                    .iter()
                    .position(|p| {
                        p.name == state.project_name
                    });
                let pi = match pi {
                    Some(pi) => pi,
                    None => return,
                };
                let fi = self.store.projects[pi]
                    .features
                    .iter()
                    .position(|f| {
                        f.name == state.feature_name
                    });
                let fi = match fi {
                    Some(fi) => fi,
                    None => return,
                };
                let si = state.selected;
                (pi, fi, si, state)
            }
            _ => return,
        };

        let label = match self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
            .and_then(|f| f.sessions.get(si))
        {
            Some(s) => s.label.clone(),
            None => return,
        };

        let saved_switcher = SessionSwitcherState {
            project_name: switcher_state
                .project_name
                .clone(),
            feature_name: switcher_state
                .feature_name
                .clone(),
            tmux_session: switcher_state
                .tmux_session
                .clone(),
            sessions: switcher_state
                .sessions
                .iter()
                .map(|s| SwitcherEntry {
                    tmux_window: s.tmux_window.clone(),
                    kind: s.kind.clone(),
                    label: s.label.clone(),
                    icon: s.icon.clone(),
                    icon_nerd: s.icon_nerd.clone(),
                })
                .collect(),
            selected: switcher_state.selected,
            return_window: switcher_state
                .return_window
                .clone(),
            return_label: switcher_state
                .return_label
                .clone(),
            vibe_mode: switcher_state.vibe_mode.clone(),
            review: switcher_state.review,
        };

        self.mode =
            AppMode::RenamingSession(RenameSessionState {
                project_idx: pi,
                feature_idx: fi,
                session_idx: si,
                input: label,
                return_to: RenameReturnTo::SessionSwitcher(
                    saved_switcher,
                ),
            });
    }

    pub fn apply_rename_session(&mut self) -> Result<()> {
        let (pi, fi, si, input) = match &self.mode {
            AppMode::RenamingSession(state) => (
                state.project_idx,
                state.feature_idx,
                state.session_idx,
                state.input.clone(),
            ),
            _ => return Ok(()),
        };

        if input.is_empty() {
            self.message =
                Some("Name cannot be empty".into());
            return Ok(());
        }

        if let Some(session) = self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
            .and_then(|f| f.sessions.get_mut(si))
        {
            session.label = input.clone();
        }
        self.save()?;

        let old_mode = std::mem::replace(
            &mut self.mode,
            AppMode::Normal,
        );
        if let AppMode::RenamingSession(rename_state) =
            old_mode
        {
            match rename_state.return_to {
                RenameReturnTo::Dashboard => {
                    self.mode = AppMode::Normal;
                }
                RenameReturnTo::SessionSwitcher(
                    mut switcher,
                ) => {
                    let feature = &self.store.projects[pi]
                        .features[fi];
                    switcher.sessions = feature
                        .sessions
                        .iter()
                        .map(|s| {
                            let cfg = self
                                .active_extension
                                .custom_sessions
                                .iter()
                                .find(|c| c.name == s.label);
                            SwitcherEntry {
                                tmux_window: s
                                    .tmux_window
                                    .clone(),
                                kind: s.kind.clone(),
                                label: s.label.clone(),
                                icon: cfg.and_then(|c| {
                                    c.icon.clone()
                                }),
                                icon_nerd: cfg.and_then(|c| {
                                    c.icon_nerd.clone()
                                }),
                            }
                        })
                        .collect();
                    self.mode =
                        AppMode::SessionSwitcher(switcher);
                }
            }
        }

        self.message =
            Some(format!("Renamed to '{}'", input));
        Ok(())
    }

    pub fn cancel_rename_session(&mut self) {
        let old_mode = std::mem::replace(
            &mut self.mode,
            AppMode::Normal,
        );
        if let AppMode::RenamingSession(state) = old_mode {
            match state.return_to {
                RenameReturnTo::Dashboard => {
                    self.mode = AppMode::Normal;
                }
                RenameReturnTo::SessionSwitcher(
                    switcher,
                ) => {
                    self.mode =
                        AppMode::SessionSwitcher(switcher);
                }
            }
        }
    }
}
