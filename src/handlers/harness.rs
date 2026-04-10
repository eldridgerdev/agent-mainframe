use anyhow::Result;
use crossterm::event::KeyCode;

use crate::app::{App, AppMode, HarnessCheckStatus, HarnessSetupState};
use crate::app::HarnessCheckResult;

pub fn handle_harness_setup_key(app: &mut App, key: KeyCode) -> Result<()> {
    let state = match &app.mode {
        AppMode::HarnessSetup(s) => s.clone(),
        _ => return Ok(()),
    };

    match key {
        KeyCode::Char('j') | KeyCode::Down => {
            if let AppMode::HarnessSetup(s) = &mut app.mode {
                if s.selected + 1 < s.harnesses.len() {
                    s.selected += 1;
                }
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if let AppMode::HarnessSetup(s) = &mut app.mode {
                if s.selected > 0 {
                    s.selected -= 1;
                }
            }
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            check_and_toggle(app)?;
        }
        KeyCode::Char('c') => {
            confirm_harness_setup(app)?;
        }
        KeyCode::Esc => {
            if state.is_startup {
                let enabled = state.enabled_harnesses();
                if enabled.is_empty() {
                    app.message =
                        Some("Select at least one harness before continuing".into());
                } else {
                    apply_harnesses(app, enabled)?;
                }
            } else {
                app.mode = AppMode::Normal;
            }
        }
        _ => {}
    }

    Ok(())
}

fn check_and_toggle(app: &mut App) -> Result<()> {
    let (idx, kind) = match &app.mode {
        AppMode::HarnessSetup(s) => (s.selected, s.harnesses[s.selected].kind.clone()),
        _ => return Ok(()),
    };

    let state = match &mut app.mode {
        AppMode::HarnessSetup(s) => s,
        _ => return Ok(()),
    };

    let harness = &mut state.harnesses[idx];

    if harness.enabled {
        // Toggle off
        harness.enabled = false;
        harness.status = HarnessCheckStatus::Unchecked;
        return Ok(());
    }

    // Already checking — ignore repeated Enter
    if harness.status == HarnessCheckStatus::Checking {
        return Ok(());
    }

    // Show progress immediately, check in background
    harness.status = HarnessCheckStatus::Checking;

    let tx = app.harness_check_tx.clone();
    let kind_clone = kind.clone();
    std::thread::spawn(move || {
        let result = App::check_harness_available(&kind_clone);
        let _ = tx.send(HarnessCheckResult { kind: kind_clone, result });
    });

    Ok(())
}

fn confirm_harness_setup(app: &mut App) -> Result<()> {
    let state = match &app.mode {
        AppMode::HarnessSetup(s) => s.clone(),
        _ => return Ok(()),
    };

    let enabled = state.enabled_harnesses();
    if enabled.is_empty() {
        if state.is_startup {
            app.message = Some("Select at least one harness before continuing".into());
        } else {
            app.mode = AppMode::Normal;
        }
        return Ok(());
    }

    apply_harnesses(app, enabled)
}

fn apply_harnesses(app: &mut App, harnesses: Vec<crate::project::AgentKind>) -> Result<()> {
    app.store.available_harnesses = harnesses;
    app.save()?;
    let count = app.store.available_harnesses.len();
    app.mode = AppMode::Normal;
    app.message = Some(format!(
        "Configured {} harness{}",
        count,
        if count == 1 { "" } else { "es" }
    ));
    Ok(())
}

pub fn build_harness_setup_state(app: &App, is_startup: bool) -> HarnessSetupState {
    HarnessSetupState::new(is_startup, &app.store.available_harnesses)
}
