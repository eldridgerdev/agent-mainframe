use super::*;

impl App {
    pub fn open_session_switcher(&mut self) {
        let (
            project_name,
            feature_name,
            tmux_session,
            current_window,
            current_label,
            sessions,
            vibe_mode,
            review,
        ) = match &self.mode {
            AppMode::Viewing(view) => {
                let pi = self
                    .store
                    .projects
                    .iter()
                    .position(|p| p.name == view.project_name);
                let pi = match pi {
                    Some(pi) => pi,
                    None => return,
                };
                let fi = self.store.projects[pi]
                    .features
                    .iter()
                    .position(|f| f.name == view.feature_name);
                let fi = match fi {
                    Some(fi) => fi,
                    None => return,
                };
                let feature = &self.store.projects[pi].features[fi];
                let entries: Vec<SwitcherEntry> = feature
                    .sessions
                    .iter()
                    .map(|s| {
                        let cfg = self
                            .active_extension
                            .custom_sessions
                            .iter()
                            .find(|c| c.name == s.label);
                        SwitcherEntry {
                            tmux_window: s.tmux_window.clone(),
                            kind: s.kind.clone(),
                            label: s.label.clone(),
                            icon: cfg.and_then(|c| c.icon.clone()),
                            icon_nerd: cfg.and_then(|c| c.icon_nerd.clone()),
                        }
                    })
                    .collect();
                (
                    view.project_name.clone(),
                    view.feature_name.clone(),
                    view.session.clone(),
                    view.window.clone(),
                    view.session_label.clone(),
                    entries,
                    view.vibe_mode.clone(),
                    view.review,
                )
            }
            _ => return,
        };

        if sessions.is_empty() {
            return;
        }

        let selected = sessions
            .iter()
            .position(|s| s.tmux_window == current_window)
            .unwrap_or(0);

        self.mode = AppMode::SessionSwitcher(SessionSwitcherState {
            project_name,
            feature_name,
            tmux_session,
            sessions,
            selected,
            return_window: current_window,
            return_label: current_label,
            vibe_mode,
            review,
        });
    }

    pub fn switch_from_switcher(&mut self) {
        let (project_name, feature_name, tmux_session, window, label, kind, vibe_mode, review) =
            match &self.mode {
                AppMode::SessionSwitcher(state) => {
                    let entry = match state.sessions.get(state.selected) {
                        Some(e) => e,
                        None => return,
                    };
                    (
                        state.project_name.clone(),
                        state.feature_name.clone(),
                        state.tmux_session.clone(),
                        entry.tmux_window.clone(),
                        entry.label.clone(),
                        entry.kind.clone(),
                        state.vibe_mode.clone(),
                        state.review,
                    )
                }
                _ => return,
            };

        self.pane_content.clear();
        self.mode = AppMode::Viewing(ViewState::new(
            project_name,
            feature_name,
            tmux_session,
            window,
            label,
            kind,
            vibe_mode,
            review,
        ));
        self.refresh_sidebar_for_current_view();
    }

    pub fn cancel_session_switcher(&mut self) {
        let (project_name, feature_name, tmux_session, window, label, kind, vibe_mode, review) =
            match &self.mode {
                AppMode::SessionSwitcher(state) => {
                    let kind = state
                        .sessions
                        .iter()
                        .find(|entry| entry.tmux_window == state.return_window)
                        .map(|entry| entry.kind.clone())
                        .unwrap_or(SessionKind::Terminal);
                    (
                        state.project_name.clone(),
                        state.feature_name.clone(),
                        state.tmux_session.clone(),
                        state.return_window.clone(),
                        state.return_label.clone(),
                        kind,
                        state.vibe_mode.clone(),
                        state.review,
                    )
                }
                _ => return,
            };

        self.pane_content.clear();
        self.mode = AppMode::Viewing(ViewState::new(
            project_name,
            feature_name,
            tmux_session,
            window,
            label,
            kind,
            vibe_mode,
            review,
        ));
        self.refresh_sidebar_for_current_view();
    }
}
