use super::{App, AppMode, ContextSettingsField, ContextSettingsState};

impl App {
    pub fn start_context_settings(&mut self) {
        self.mode = AppMode::ContextSettings(ContextSettingsState {
            field: ContextSettingsField::WindowLimit,
            window_limit_input: self
                .config
                .context_window_override
                .map(|limit| limit.to_string())
                .unwrap_or_default(),
            warning_input: self.config.context_warning_percent.to_string(),
            critical_input: self.config.context_critical_percent.to_string(),
            error: None,
        });
        self.message = None;
    }

    pub fn cancel_context_settings(&mut self) {
        self.mode = AppMode::Normal;
    }

    /// Append a digit to the focused field. Non-digit input is ignored so the
    /// fields never need to display or recover from a parse error mid-edit.
    pub fn context_settings_push_char(&mut self, c: char) {
        if !c.is_ascii_digit() {
            return;
        }
        if let AppMode::ContextSettings(state) = &mut self.mode {
            state.error = None;
            let field = match state.field {
                ContextSettingsField::WindowLimit => &mut state.window_limit_input,
                ContextSettingsField::WarningPercent => &mut state.warning_input,
                ContextSettingsField::CriticalPercent => &mut state.critical_input,
            };
            field.push(c);
        }
    }

    pub fn context_settings_backspace(&mut self) {
        if let AppMode::ContextSettings(state) = &mut self.mode {
            state.error = None;
            let field = match state.field {
                ContextSettingsField::WindowLimit => &mut state.window_limit_input,
                ContextSettingsField::WarningPercent => &mut state.warning_input,
                ContextSettingsField::CriticalPercent => &mut state.critical_input,
            };
            field.pop();
        }
    }

    pub fn context_settings_focus_next(&mut self) {
        if let AppMode::ContextSettings(state) = &mut self.mode {
            state.field = state.field.next();
        }
    }

    pub fn context_settings_focus_prev(&mut self) {
        if let AppMode::ContextSettings(state) = &mut self.mode {
            state.field = state.field.prev();
        }
    }

    /// Validate the entered values and, if they check out, persist them to
    /// `AppConfig` and close the dialog. Returns `false` (leaving the dialog
    /// open with `state.error` set) when validation fails.
    pub fn context_settings_confirm(&mut self) -> bool {
        let AppMode::ContextSettings(state) = &self.mode else {
            return false;
        };

        let settings = match crate::context_tracking::parse_context_settings(
            &state.window_limit_input,
            &state.warning_input,
            &state.critical_input,
        ) {
            Ok(settings) => settings,
            Err(error) => {
                self.context_settings_set_error(error);
                return false;
            }
        };
        self.apply_context_settings(settings);
        self.save_config();

        self.mode = AppMode::Normal;
        self.message = Some("Saved context settings".into());
        true
    }

    pub(crate) fn apply_context_settings(
        &mut self,
        settings: crate::context_tracking::ContextSettings,
    ) {
        use crate::context_tracking::{ContextFreshness, ContextProvenance, ContextThresholds};
        let limit_changed = self.config.context_window_override != settings.window_override;
        self.config.context_window_override = settings.window_override;
        self.config.context_warning_percent = settings.warning_percent;
        self.config.context_critical_percent = settings.critical_percent;
        let thresholds = ContextThresholds {
            warning_percent: settings.warning_percent,
            critical_percent: settings.critical_percent,
        };
        for state in self.context_states.values_mut() {
            if let Some(snapshot) = &mut state.snapshot {
                snapshot.band = thresholds.band_for(snapshot.percentage);
                // A direct sample carries the harness's own limit, which the
                // override never replaces.
                if limit_changed && snapshot.provenance != ContextProvenance::Direct {
                    snapshot.freshness = ContextFreshness::Stale;
                }
            }
        }
        self.context_hint_states.sync_all(&self.context_states);
    }

    fn context_settings_set_error(&mut self, error: String) {
        if let AppMode::ContextSettings(state) = &mut self.mode {
            state.error = Some(error);
        }
    }
}
