//! Key handling for the Remote Control pairing dialog (`AppMode::
//! RemotePairing`), opened by `App::start_pairing`
//! (`docs/backlog/remote-control-companion-app-plan.md`, Epic 4).

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};

use crate::app::{App, AppMode, PairingDialogStatus};

pub fn handle_remote_pairing_key(app: &mut App, key: KeyEvent) -> Result<()> {
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
        _ => {}
    }

    Ok(())
}
