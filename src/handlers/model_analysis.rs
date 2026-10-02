use crate::app::{App, AppMode, model_analysis::Status};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};

pub(super) fn handle_model_analysis_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc => app.cancel_model_analysis(),
        KeyCode::Char('r') if key.modifiers.is_empty() => app.retry_model_analysis(),
        KeyCode::Char('s') if key.modifiers.is_empty() => {
            if let AppMode::ModelAnalysis(s) = &mut app.mode {
                s.show_sources = !s.show_sources;
                s.scroll = 0;
            }
        }
        KeyCode::Enter => {
            if let Err(e) = app.apply_model_analysis() {
                app.message = Some(e.to_string());
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if let AppMode::ModelAnalysis(s) = &mut app.mode {
                s.selected = s.selected.saturating_sub(1);
                s.scroll = 0;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let AppMode::ModelAnalysis(s) = &mut app.mode
                && let Status::Ready(choices) = &s.status
            {
                s.selected = (s.selected + 1).min(choices.len().saturating_sub(1));
                s.scroll = 0;
            }
        }
        KeyCode::PageDown => {
            if let AppMode::ModelAnalysis(s) = &mut app.mode {
                s.scroll = s.scroll.saturating_add(8);
            }
        }
        KeyCode::PageUp => {
            if let AppMode::ModelAnalysis(s) = &mut app.mode {
                s.scroll = s.scroll.saturating_sub(8);
            }
        }
        _ => {}
    }
    Ok(())
}
