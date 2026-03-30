use anyhow::Result;

use super::*;

impl App {
    pub fn start_syntax_language_picker(&mut self) {
        self.open_syntax_language_picker(None, None, None);
    }

    pub fn open_syntax_language_picker_for_selected_diff_file(&mut self) {
        let (path, return_to, notice) = match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::DiffViewer(state) => {
                let path = state
                    .files
                    .get(state.selected_file)
                    .map(|file| std::path::PathBuf::from(&file.path));
                let notice = path.as_ref().and_then(|path| syntax_notice_for_path(path));
                (path, Some(Box::new(AppMode::DiffViewer(state))), notice)
            }
            AppMode::DiffReviewPrompt(state) => {
                let path = Some(std::path::PathBuf::from(&state.relative_path));
                let notice = path.as_ref().and_then(|path| syntax_notice_for_path(path));
                (
                    path,
                    Some(Box::new(AppMode::DiffReviewPrompt(state))),
                    notice,
                )
            }
            other => {
                self.mode = other;
                return;
            }
        };

        self.open_syntax_language_picker(path.as_deref(), notice, return_to);
    }

    pub fn close_syntax_language_picker(&mut self) {
        let return_to = match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::SyntaxLanguagePicker(state) => state.return_to,
            other => {
                self.mode = other;
                return;
            }
        };

        self.restore_syntax_picker_return_mode(return_to);
    }

    pub fn refresh_syntax_language_picker(&mut self) {
        if let AppMode::SyntaxLanguagePicker(state) = &mut self.mode {
            let selected_language = state
                .languages
                .get(state.selected)
                .map(|row| row.language)
                .unwrap_or(crate::highlight::HighlightLanguage::Rust);

            state.languages = syntax_rows();
            state.selected = state
                .languages
                .iter()
                .position(|row| row.language == selected_language)
                .unwrap_or(0);
        }
    }

    pub fn poll_syntax_language_picker(&mut self) -> Result<()> {
        let mut completion = None;

        if let AppMode::SyntaxLanguagePicker(state) = &mut self.mode
            && let Some(operation) = &mut state.operation
        {
            while let Ok(event) = operation.output_rx.try_recv() {
                match event {
                    SyntaxOperationEvent::Output(line) => {
                        operation.last_output = Some(line);
                    }
                    SyntaxOperationEvent::Finished(result) => {
                        completion = Some(result);
                    }
                }
            }
        }

        let Some(result) = completion else {
            return Ok(());
        };

        crate::highlight::reload_runtime_state();
        self.refresh_syntax_language_picker();

        let notice = match &result {
            Ok(message) => message.clone(),
            Err(error) => format!("Error: {error}"),
        };

        let mut auto_return = None;
        if let AppMode::SyntaxLanguagePicker(state) = &mut self.mode {
            let completed_operation = state
                .operation
                .as_ref()
                .map(|operation| (operation.action, operation.language));
            state.operation = None;
            state.notice = Some(notice);
            if matches!(result, Ok(_))
                && state.auto_return_on_success
                && matches!(completed_operation, Some((SyntaxOperationAction::Install, language)) if Some(language) == state.return_language)
            {
                auto_return = state.return_to.take();
            }
        }

        match result {
            Ok(message) => {
                self.log_info("syntax", message.clone());
                self.message = Some(message);
            }
            Err(error) => {
                self.report_logged_error("syntax", error);
            }
        }

        if auto_return.is_some() {
            self.restore_syntax_picker_return_mode(auto_return);
        }

        Ok(())
    }

    pub fn syntax_picker_install_selected(&mut self) {
        let language = match &self.mode {
            AppMode::SyntaxLanguagePicker(state) if state.operation.is_none() => {
                state.languages.get(state.selected).map(|row| row.language)
            }
            _ => None,
        };

        if let Some(language) = language {
            self.start_syntax_operation(language, SyntaxOperationAction::Install);
        }
    }

    pub fn syntax_picker_uninstall_selected(&mut self) {
        let language = match &self.mode {
            AppMode::SyntaxLanguagePicker(state) if state.operation.is_none() => {
                state.languages.get(state.selected).map(|row| row.language)
            }
            _ => None,
        };

        if let Some(language) = language {
            self.start_syntax_operation(language, SyntaxOperationAction::Uninstall);
        }
    }

    fn start_syntax_operation(
        &mut self,
        language: crate::highlight::HighlightLanguage,
        action: SyntaxOperationAction,
    ) {
        let (tx, rx) = std::sync::mpsc::channel::<SyntaxOperationEvent>();
        std::thread::spawn(move || {
            let progress_tx = tx.clone();
            let result = match action {
                SyntaxOperationAction::Install => {
                    crate::highlight::install_language(language, move |line| {
                        let _ = progress_tx.send(SyntaxOperationEvent::Output(line));
                    })
                }
                SyntaxOperationAction::Uninstall => {
                    crate::highlight::uninstall_language(language, move |line| {
                        let _ = progress_tx.send(SyntaxOperationEvent::Output(line));
                    })
                }
            };
            let _ = tx.send(SyntaxOperationEvent::Finished(
                result.map_err(|err| err.to_string()),
            ));
        });

        if let AppMode::SyntaxLanguagePicker(state) = &mut self.mode {
            state.notice = None;
            state.operation = Some(SyntaxOperationState {
                language,
                action,
                last_output: None,
                started_at: std::time::Instant::now(),
                output_rx: rx,
            });
        }
    }

    fn open_syntax_language_picker(
        &mut self,
        selected_path: Option<&std::path::Path>,
        notice: Option<String>,
        return_to: Option<Box<AppMode>>,
    ) {
        let languages = syntax_rows();
        let detected_language = selected_path
            .and_then(|path| crate::highlight::language_install_state_for_path(path))
            .map(|(language, _)| language);
        let selected = detected_language
            .and_then(|language| languages.iter().position(|row| row.language == language))
            .unwrap_or(0);

        self.mode = AppMode::SyntaxLanguagePicker(SyntaxLanguagePickerState {
            languages,
            selected,
            notice,
            operation: None,
            return_to,
            auto_return_on_success: detected_language.is_some(),
            return_language: detected_language,
        });
    }

    fn restore_syntax_picker_return_mode(&mut self, return_to: Option<Box<AppMode>>) {
        if let Some(mode) = return_to {
            self.mode = *mode;
            if matches!(self.mode, AppMode::DiffViewer(_)) {
                self.refresh_diff_viewer();
            }
        }
    }
}

fn syntax_rows() -> Vec<SyntaxLanguageRow> {
    crate::highlight::HighlightLanguage::ALL
        .into_iter()
        .map(|language| SyntaxLanguageRow {
            language,
            status: language.install_state(),
        })
        .collect()
}

fn syntax_notice_for_path(path: &std::path::Path) -> Option<String> {
    let (language, status) = crate::highlight::language_install_state_for_path(path)?;
    let action = match status {
        crate::highlight::HighlightInstallState::Installed => "installed",
        crate::highlight::HighlightInstallState::Available => "not installed",
        crate::highlight::HighlightInstallState::Broken => "broken",
    };
    Some(format!(
        "{} uses {} syntax highlighting; parser is {action}.",
        path.display(),
        language.picker_title()
    ))
}
