//! Key handling for the Remote Control pairing dialog (`AppMode::
//! RemotePairing`), opened by `App::start_pairing`
//! (`docs/backlog/remote-control-companion-app-plan.md`, Epic 4).

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};

use crate::app::{App, AppMode, PairingDialogStatus, PairingDialogView};

pub fn handle_remote_pairing_key(app: &mut App, key: KeyEvent) -> Result<()> {
    let in_devices_view = matches!(
        &app.mode,
        AppMode::RemotePairing(state) if matches!(state.view, PairingDialogView::Devices(_))
    );

    if in_devices_view {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => app.close_paired_devices_view(),
            KeyCode::Up | KeyCode::Char('k') => app.move_paired_device_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => app.move_paired_device_selection(1),
            KeyCode::Char('d') => app.request_revoke_selected_device(),
            // Any other key drops a pending revoke confirmation rather than
            // silently ignoring it — the same "d, d" contract as the
            // prompt overrides manager (`app/prompt_overrides.rs`).
            _ => app.clear_revoke_confirmation(),
        }
        return Ok(());
    }

    let already_paired = matches!(
        &app.mode,
        AppMode::RemotePairing(state) if matches!(state.status, PairingDialogStatus::Paired { .. })
    );

    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.cancel_pairing();
        }
        KeyCode::Enter if already_paired => {
            app.cancel_pairing();
        }
        KeyCode::Char('r') if !already_paired => {
            app.regenerate_pairing_code();
        }
        KeyCode::Char('v') => {
            app.open_paired_devices_view();
        }
        _ => {}
    }

    Ok(())
}
