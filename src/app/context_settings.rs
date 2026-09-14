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

        let window_override = if state.window_limit_input.trim().is_empty() {
            None
        } else {
            match state.window_limit_input.trim().parse::<u64>() {
                Ok(0) | Err(_) => {
                    self.context_settings_set_error(
                        "Context window must be a positive number of tokens (or blank to clear)"
                            .to_string(),
                    );
                    return false;
                }
                Ok(limit) => Some(limit),
            }
        };

        let warning_percent = match state.warning_input.trim().parse::<u8>() {
            Ok(value) if (1..=100).contains(&value) => value,
            _ => {
                self.context_settings_set_error(
                    "Warning % must be a whole number between 1 and 100".to_string(),
                );
                return false;
            }
        };

        let critical_percent = match state.critical_input.trim().parse::<u8>() {
            Ok(value) if (1..=100).contains(&value) => value,
            _ => {
                self.context_settings_set_error(
                    "Critical % must be a whole number between 1 and 100".to_string(),
                );
                return false;
            }
        };

        if critical_percent <= warning_percent {
            self.context_settings_set_error(
                "Critical % must be greater than Warning %".to_string(),
            );
            return false;
        }

        self.config.context_window_override = window_override;
        self.config.context_warning_percent = warning_percent;
        self.config.context_critical_percent = critical_percent;
        self.save_config();

        self.mode = AppMode::Normal;
        self.message = Some("Saved context settings".into());
        true
    }

    fn context_settings_set_error(&mut self, error: String) {
        if let AppMode::ContextSettings(state) = &mut self.mode {
            state.error = Some(error);
        }
    }
}
