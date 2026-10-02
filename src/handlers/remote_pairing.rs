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

    let in_setup_view = matches!(
        &app.mode,
        AppMode::RemotePairing(state) if matches!(state.view, PairingDialogView::Setup { .. })
    );

    if in_setup_view {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('s') | KeyCode::Char('?') => {
                app.close_pairing_setup_view()
            }
            KeyCode::Up | KeyCode::Char('k') => app.scroll_pairing_setup(-1),
            KeyCode::Down | KeyCode::Char('j') => app.scroll_pairing_setup(1),
            KeyCode::PageUp => app.scroll_pairing_setup(-10),
            KeyCode::PageDown | KeyCode::Char(' ') => app.scroll_pairing_setup(10),
            KeyCode::Home | KeyCode::Char('g') => app.scroll_pairing_setup(i32::MIN),
            KeyCode::End | KeyCode::Char('G') => app.scroll_pairing_setup(i32::MAX),
            KeyCode::Char('t') => app.start_tailscale_serve(),
            KeyCode::Char('c') => app.copy_tailscale_access_policy(),
            KeyCode::Char('o') => app.open_tailscale_approval_link(),
            KeyCode::Char('r') => app.probe_tailscale(),
            _ => {}
        }
        return Ok(());
    }

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
        KeyCode::Char('s') | KeyCode::Char('?') => {
            app.open_pairing_setup_view();
        }
        KeyCode::Char('t') => {
            app.start_tailscale_serve();
        }
        KeyCode::Char('o') => {
            app.open_tailscale_approval_link();
        }
        _ => {}
    }

    Ok(())
}
