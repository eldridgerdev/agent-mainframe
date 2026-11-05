use std::collections::HashMap;
use std::sync::Mutex;

use super::super::{AppState, GuiError, SessionTarget};
use agent_mainframe::gui_contract::GuiErrorKind;
use agent_mainframe::gui_dormancy;
use agent_mainframe::gui_terminal::{
    TerminalFrame, TerminalHandle, TerminalHistory, WheelDirection,
};
use serde::{Deserialize, Serialize};
use tauri::{Emitter, State};

/// Live terminal attachments, keyed deterministically by `feature_id:session_id`
/// (not a generated id) so attaching to the same session twice -- the
/// reconnect case `AMF_PLAN.md` Task 6 asks for -- replaces the old entry
/// rather than accumulating a second one; dropping the replaced
/// `TerminalHandle` detaches it exactly the same way an explicit
/// `detach_terminal` would.
pub(crate) struct TerminalState(Mutex<Attachments<TerminalHandle>>);

impl TerminalState {
    pub(crate) fn new() -> Self {
        Self(Mutex::new(Attachments::new()))
    }
}

/// The key alone can't say *which* attachment a detach means: a pane that
/// unmounts while its `attach_terminal` is still in flight detaches after a
/// newer pane for the same session has already attached, and removing by key
/// would take the newer pane's handle. Each attachment therefore also gets a
/// generation, and a detach only removes the entry it created. Generic so the
/// bookkeeping is testable without a real tmux pane.
struct Attachments<T> {
    next_generation: u64,
    entries: HashMap<String, (u64, T)>,
}

impl<T> Attachments<T> {
    fn new() -> Self {
        Self {
            next_generation: 0,
            entries: HashMap::new(),
        }
    }

    /// Returns the new entry's generation and whatever it replaced, which the
    /// caller drops outside the lock.
    fn insert(&mut self, key: String, value: T) -> (u64, Option<T>) {
        self.next_generation += 1;
        let generation = self.next_generation;
        let previous = self.entries.insert(key, (generation, value));
        (generation, previous.map(|(_, value)| value))
    }

    fn get(&self, key: &str) -> Option<&T> {
        self.entries.get(key).map(|(_, value)| value)
    }

    /// Removes `key` only while it still holds `generation`; a stale detach
    /// is a no-op.
    fn remove(&mut self, key: &str, generation: u64) -> Option<T> {
        match self.entries.get(key) {
            Some((current, _)) if *current == generation => {
                self.entries.remove(key).map(|(_, value)| value)
            }
            _ => None,
        }
    }
}

fn terminal_key(target: &SessionTarget) -> String {
    format!("{}:{}", target.feature_id, target.session_id)
}

fn not_found(message: impl Into<String>) -> GuiError {
    GuiError {
        kind: GuiErrorKind::NotFound,
        message: message.into(),
    }
}

#[derive(Serialize)]
pub(crate) struct AttachTerminalResponse {
    key: String,
    /// Passed back to `detach_terminal` so it removes this attachment and
    /// never a newer one for the same session.
    generation: u64,
    initial: TerminalFrame,
}

#[derive(Deserialize)]
pub(crate) struct TerminalSize {
    cols: u16,
    rows: u16,
}

/// Attach to a session's tmux pane and start streaming updates as
/// `terminal-output:{key}` events (`AMF_PLAN.md` Task 6). Re-attaching the
/// same `target` (a GUI reconnect after closing and reopening) replaces the
/// prior attachment rather than erroring or leaking it.
#[tauri::command]
pub(crate) fn attach_terminal(
    app: tauri::AppHandle,
    state: State<AppState>,
    terminals: State<TerminalState>,
    target: SessionTarget,
    size: TerminalSize,
) -> Result<AttachTerminalResponse, GuiError> {
    let terminal_target = {
        let mut gui = state.0.lock().expect("gui handle mutex poisoned");
        let resolved = gui.resolve_session_target(&target)?;
        // Opening a session is attention, as the TUI's view entry is.
        if let Err(err) = gui_dormancy::note_opened(&mut gui, &target) {
            eprintln!("amf-gui: could not record feature access: {}", err.message);
        }
        resolved
    };
    let key = terminal_key(&target);

    let event_name = format!("terminal-output:{key}");
    let emit_app = app.clone();
    let (handle, initial) = TerminalHandle::attach(
        &terminal_target.tmux_session,
        &terminal_target.tmux_window,
        size.cols,
        size.rows,
        move |frame| {
            if let Err(err) = emit_app.emit(&event_name, frame) {
                eprintln!("amf-gui: failed to emit {event_name}: {err}");
            }
        },
    )
    .map_err(GuiError::from)?;

    // Dropped outside the lock (after replacing the map entry) so a wedged
    // old attachment's bounded-but-real teardown wait (see
    // `TerminalHandle`'s `Drop`) never happens while holding this mutex.
    let (generation, previous) = terminals
        .0
        .lock()
        .expect("terminal registry mutex poisoned")
        .insert(key.clone(), handle);
    drop(previous);

    Ok(AttachTerminalResponse {
        key,
        generation,
        initial,
    })
}

#[tauri::command]
pub(crate) fn terminal_input(
    terminals: State<TerminalState>,
    key: String,
    text: String,
) -> Result<(), GuiError> {
    let terminals = terminals
        .0
        .lock()
        .expect("terminal registry mutex poisoned");
    let handle = terminals
        .get(&key)
        .ok_or_else(|| not_found(format!("No attached terminal for '{key}'")))?;
    handle.send_input(&text).map_err(GuiError::from)
}

#[tauri::command]
pub(crate) fn terminal_submit_prompt(
    terminals: State<TerminalState>,
    key: String,
    text: String,
) -> Result<(), GuiError> {
    let terminals = terminals
        .0
        .lock()
        .expect("terminal registry mutex poisoned");
    let handle = terminals
        .get(&key)
        .ok_or_else(|| not_found(format!("No attached terminal for '{key}'")))?;
    handle.submit_prompt(&text).map_err(GuiError::from)
}

#[tauri::command]
pub(crate) fn resize_terminal(
    terminals: State<TerminalState>,
    key: String,
    size: TerminalSize,
) -> Result<(), GuiError> {
    let terminals = terminals
        .0
        .lock()
        .expect("terminal registry mutex poisoned");
    let handle = terminals
        .get(&key)
        .ok_or_else(|| not_found(format!("No attached terminal for '{key}'")))?;
    handle.resize(size.cols, size.rows).map_err(GuiError::from)
}

/// A read-only snapshot of the pane's tmux history for the frontend's
/// scroll-back view (see `gui_terminal`'s module docs). Sends nothing to the
/// pane.
#[tauri::command]
pub(crate) fn terminal_history(
    terminals: State<TerminalState>,
    key: String,
) -> Result<TerminalHistory, GuiError> {
    let terminals = terminals
        .0
        .lock()
        .expect("terminal registry mutex poisoned");
    let handle = terminals
        .get(&key)
        .ok_or_else(|| not_found(format!("No attached terminal for '{key}'")))?;
    handle.history().map_err(GuiError::from)
}

/// Forward wheel steps to a full-screen program that asked for mouse
/// reporting; `false` (nothing sent) once it no longer does.
#[tauri::command]
pub(crate) fn terminal_wheel(
    terminals: State<TerminalState>,
    key: String,
    direction: WheelDirection,
    steps: u8,
    col: u16,
    row: u16,
) -> Result<bool, GuiError> {
    let terminals = terminals
        .0
        .lock()
        .expect("terminal registry mutex poisoned");
    let handle = terminals
        .get(&key)
        .ok_or_else(|| not_found(format!("No attached terminal for '{key}'")))?;
    handle
        .scroll_program(direction, steps, col, row)
        .map_err(GuiError::from)
}

/// Ask for a fresh frame, for a pane returning from its scroll-back view.
/// A pane that has since detached has nothing to refresh, which is fine.
#[tauri::command]
pub(crate) fn terminal_refresh(terminals: State<TerminalState>, key: String) {
    if let Some(handle) = terminals
        .0
        .lock()
        .expect("terminal registry mutex poisoned")
        .get(&key)
    {
        handle.refresh();
    }
}

/// Explicit detach, for a pane the frontend is done with before the window
/// itself closes (switching sessions, say). Removing it from the map drops
/// it, which is exactly what closing the whole GUI relies on to detach every
/// live attachment without killing their underlying tmux sessions -- see
/// `TerminalHandle`'s own doc comment. `generation` is the one
/// `attach_terminal` returned; a detach for a superseded attachment leaves
/// the current one in place (see [`Attachments`]).
#[tauri::command]
pub(crate) fn detach_terminal(terminals: State<TerminalState>, key: String, generation: u64) {
    let removed = terminals
        .0
        .lock()
        .expect("terminal registry mutex poisoned")
        .remove(&key, generation);
    drop(removed);
}

// Task 8 ("Share TODO and planning workflows incrementally," TODO half):
// thin wrappers over `gui_todos`, matching the rest of this file. None of
// these need `emit_workspace_changed` -- TODO lists live outside
// `ProjectStore` (see `gui_todos`'s own module doc), so a TODO mutation
// never changes what `get_snapshot` returns.

#[cfg(test)]
mod tests {
    use super::Attachments;

    #[test]
    fn a_stale_detach_leaves_the_newer_attachment_in_place() {
        let mut attachments = Attachments::new();
        let (old, _) = attachments.insert("f:s".to_string(), "old");
        let (new, replaced) = attachments.insert("f:s".to_string(), "new");
        assert_eq!(replaced, Some("old"));
        assert_ne!(old, new);

        assert_eq!(attachments.remove("f:s", old), None);
        assert_eq!(attachments.get("f:s"), Some(&"new"));

        assert_eq!(attachments.remove("f:s", new), Some("new"));
        assert_eq!(attachments.get("f:s"), None);
    }
}
