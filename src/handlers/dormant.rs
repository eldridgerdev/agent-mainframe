use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};

use crate::app::{App, AppMode};

/// Dormant-features overlay.
///
/// Keys follow the dashboard's: `x` stops, `d` deletes, `Enter` opens. The
/// destructive one (`d`) hands off to the ordinary delete-feature confirmation
/// rather than deleting from here.
pub fn handle_dormant_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.mode = AppMode::Normal,
        KeyCode::Char('j') | KeyCode::Down => {
            if let AppMode::Dormant(state) = &mut app.mode
                && !state.features.is_empty()
            {
                state.selected = (state.selected + 1) % state.features.len();
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if let AppMode::Dormant(state) = &mut app.mode
                && !state.features.is_empty()
            {
                state.selected = state
                    .selected
                    .checked_sub(1)
                    .unwrap_or(state.features.len() - 1);
            }
        }
        KeyCode::Char('r') => app.refresh_dormant_view(),
        KeyCode::Char('x') => app.dormant_stop_selected()?,
        KeyCode::Char('e') => app.dormant_kill_editor_selected(),
        KeyCode::Char('d') => app.dormant_delete_selected(),
        KeyCode::Enter | KeyCode::Char('g') => app.dormant_jump_selected()?,
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::dormant::DormantFeature;
    use crate::app::DormantViewState;
    use crate::traits::{MockTmuxOps, MockWorktreeOps};
    use crossterm::event::KeyModifiers;
    use std::path::PathBuf;
    use std::time::Duration;

    fn feature(name: &str, fi: usize) -> DormantFeature {
        DormantFeature {
            pi: 0,
            fi,
            project_name: "proj".into(),
            feature_name: name.into(),
            idle: Duration::from_secs(7200),
            unattended: Duration::from_secs(20 * 3600),
            workdir: PathBuf::from("/tmp").join(name),
            is_worktree: true,
            editor_alive: false,
        }
    }

    fn app_with_overlay(features: Vec<DormantFeature>) -> App {
        let mut app = App::new_for_test(
            crate::project::ProjectStore {
                version: 1,
                projects: Vec::new(),
                session_bookmarks: Vec::new(),
                available_harnesses: Vec::new(),
                prompt_templates: Vec::new(),
                extra: Default::default(),
            },
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.mode = AppMode::Dormant(DormantViewState {
            features,
            selected: 0,
            message: None,
        });
        app
    }

    fn press(app: &mut App, c: char) {
        handle_dormant_key(app, KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)).unwrap();
    }

    #[test]
    fn navigation_wraps_in_both_directions() {
        let mut app = app_with_overlay(vec![feature("a", 0), feature("b", 1)]);

        press(&mut app, 'j');
        assert!(matches!(&app.mode, AppMode::Dormant(s) if s.selected == 1));
        press(&mut app, 'j');
        assert!(matches!(&app.mode, AppMode::Dormant(s) if s.selected == 0));
        press(&mut app, 'k');
        assert!(matches!(&app.mode, AppMode::Dormant(s) if s.selected == 1));
    }

    #[test]
    fn navigation_on_an_empty_list_does_nothing() {
        let mut app = app_with_overlay(Vec::new());
        press(&mut app, 'j');
        press(&mut app, 'k');
        assert!(matches!(&app.mode, AppMode::Dormant(s) if s.selected == 0));
    }

    #[test]
    fn q_and_esc_close_the_overlay() {
        let mut app = app_with_overlay(vec![feature("a", 0)]);
        press(&mut app, 'q');
        assert!(matches!(app.mode, AppMode::Normal));

        let mut app = app_with_overlay(vec![feature("a", 0)]);
        handle_dormant_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).unwrap();
        assert!(matches!(app.mode, AppMode::Normal));
    }

    #[test]
    fn delete_hands_off_to_the_ordinary_confirmation() {
        let mut app = app_with_overlay(vec![feature("alpha", 0)]);
        press(&mut app, 'd');
        match &app.mode {
            AppMode::DeletingFeature(project, feature) => {
                assert_eq!(project, "proj");
                assert_eq!(feature, "alpha");
            }
            other => panic!("expected the delete confirm, got {:?}", std::mem::discriminant(other)),
        }
    }
}
