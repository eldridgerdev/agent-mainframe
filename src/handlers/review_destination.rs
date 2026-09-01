//! Key handling for the final review's destination-choice overlays.
//!
//! Two of them are sub-states of the diff viewer (`AppMode::DiffViewer`) and
//! are routed from `handlers::diff` before the viewer's own keys — the
//! destination picker (`t`) and the companion-feature setup overlay. The third,
//! the integration overlay, is a top-level `AppMode::ReviewIntegrate` opened
//! from the dashboard.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::App;

/// The destination picker (`t` in the review viewer): `j/k` move, `⏎` chooses
/// the highlighted destination (or opens the companion-feature setup for the
/// `New feature…` row), `esc`/`q` closes without changing the destination.
pub fn handle_review_destination_pick_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.review_destination_pick_cancel(),
        KeyCode::Down | KeyCode::Char('j') => app.review_destination_pick_move(1),
        KeyCode::Up | KeyCode::Char('k') => app.review_destination_pick_move(-1),
        KeyCode::Enter => app.review_destination_pick_confirm()?,
        _ => {}
    }
    Ok(())
}

/// The companion-feature setup overlay: `j/k` move between rows, `h/l` (or
/// `space`) change the focused row, `⏎` creates the feature and points the
/// destination at it, `esc`/`Ctrl+Q` abandons it. On the branch row typing
/// edits the name, so the vim verbs are suppressed there.
pub fn handle_review_feature_setup_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('q') {
        app.review_feature_setup_cancel();
        return Ok(());
    }
    match key.code {
        KeyCode::Esc => app.review_feature_setup_cancel(),
        KeyCode::Enter => app.review_feature_setup_confirm()?,
        KeyCode::Down | KeyCode::Tab => app.review_feature_setup_move(1),
        KeyCode::Up | KeyCode::BackTab => app.review_feature_setup_move(-1),
        KeyCode::Right => app.review_feature_setup_adjust(1),
        KeyCode::Left => app.review_feature_setup_adjust(-1),
        KeyCode::Backspace => app.review_feature_setup_branch_backspace(),
        KeyCode::Char(c) => {
            if app.review_feature_setup_on_branch_row() {
                app.review_feature_setup_branch_push(c);
            } else {
                match c {
                    'j' => app.review_feature_setup_move(1),
                    'k' => app.review_feature_setup_move(-1),
                    'l' | ' ' => app.review_feature_setup_adjust(1),
                    'h' => app.review_feature_setup_adjust(-1),
                    _ => {}
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// The integration overlay (dashboard `t` on a companion review feature):
/// `j/k` choose push vs cherry-pick, `⏎` runs it, `esc`/`q` closes.
pub fn handle_review_integrate_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('q') => app.review_integrate_cancel(),
        KeyCode::Down | KeyCode::Char('j') => app.review_integrate_move(1),
        KeyCode::Up | KeyCode::Char('k') => app.review_integrate_move(-1),
        KeyCode::Enter => app.review_integrate_confirm()?,
        _ => {}
    }
    Ok(())
}
