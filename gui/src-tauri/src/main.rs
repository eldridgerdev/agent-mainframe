// Tauri commands wiring `gui_contract::GuiHandle` and `gui_terminal`
// (Tasks 4-6). The commands here are thin on purpose -- lock the shared
// handle, delegate, emit a fresh snapshot on mutation -- because the actual
// behavior (id resolution, idempotency, structured errors, terminal
// lifecycle) lives in the shared library and is covered by its own
// Rust-side tests; what those tests cannot exercise is the IPC round trip
// itself (JSON serialization of the snapshot and of `GuiError`), which is
// this file's job. Task 7 ("Deliver the vertical slice") adds the real
// navigation/forms consuming these commands in `gui/src`.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod login_path;

use std::collections::HashMap;
use std::sync::Mutex;

use agent_mainframe::automation::{
    CreateFeatureRequest, CreateFeatureResponse, CreateProjectRequest, CreateProjectResponse,
};
use agent_mainframe::gui_contract::{
    FeatureTarget, GuiError, GuiErrorKind, GuiHandle, SessionTarget, StartFeatureResponse,
    StopFeatureResponse, TodoAgentLaunchResponse, WorkspaceSnapshot,
};
use agent_mainframe::gui_plans::{self, PlanAction, PlanInput, PlanStatus};
use agent_mainframe::gui_terminal::TerminalHandle;
use agent_mainframe::gui_todos::{self, TodoListView, TodoPriority, TodoScopeRequest, TodoStatus};
use agent_mainframe::project::{AgentKind, VibeMode};
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, State};

struct AppState(Mutex<GuiHandle>);

/// Live terminal attachments, keyed deterministically by `feature_id:session_id`
/// (not a generated id) so attaching to the same session twice -- the
/// reconnect case `AMF_PLAN.md` Task 6 asks for -- replaces the old entry
/// rather than accumulating a second one; dropping the replaced
/// `TerminalHandle` detaches it exactly the same way an explicit
/// `detach_terminal` would.
struct TerminalState(Mutex<Attachments<TerminalHandle>>);

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
struct HarnessInfo {
    slug: String,
    display_name: String,
}

/// Returns the harnesses AMF supports, straight from the same [`AgentKind`]
/// the TUI and the automation JSON API already use.
#[tauri::command]
fn supported_harnesses() -> Vec<HarnessInfo> {
    AgentKind::ALL
        .iter()
        .map(|kind| HarnessInfo {
            slug: kind.slug().to_string(),
            display_name: kind.display_name().to_string(),
        })
        .collect()
}

#[derive(Serialize)]
struct ModeInfo {
    slug: String,
    display_name: String,
    description: String,
}

/// Returns the vibe modes AMF supports. `slug` is derived from `VibeMode`'s
/// own `Serialize` impl (its wire format for `CreateFeatureRequest.mode`)
/// rather than a hand-written match, so it cannot drift from what
/// `create_feature` actually expects.
#[tauri::command]
fn supported_modes() -> Vec<ModeInfo> {
    VibeMode::ALL
        .iter()
        .map(|mode| ModeInfo {
            slug: serde_json::to_value(mode)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_default(),
            display_name: mode.display_name().to_string(),
            description: mode.description().to_string(),
        })
        .collect()
}

#[tauri::command]
fn get_snapshot(state: State<AppState>) -> Result<WorkspaceSnapshot, GuiError> {
    state
        .0
        .lock()
        .expect("gui handle mutex poisoned")
        .refresh_snapshot()
}

/// Emits the post-mutation snapshot on `workspace-changed` so any open
/// window resynchronizes without a second round trip. A snapshot-on-change
/// broadcast, not a diff -- acceptable for the first slice (see
/// `gui_contract::WorkspaceSnapshot`'s doc comment); revisioned/fine-grained
/// events are deferred.
fn emit_workspace_changed(app: &tauri::AppHandle, snapshot: &WorkspaceSnapshot) {
    if let Err(err) = app.emit("workspace-changed", snapshot) {
        eprintln!("amf-gui: failed to emit workspace-changed: {err}");
    }
}

#[tauri::command]
fn create_project(
    app: tauri::AppHandle,
    state: State<AppState>,
    request: CreateProjectRequest,
) -> Result<CreateProjectResponse, GuiError> {
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    let response = gui.create_project(request)?;
    emit_workspace_changed(&app, &gui.snapshot());
    Ok(response)
}

#[tauri::command]
fn create_feature(
    app: tauri::AppHandle,
    state: State<AppState>,
    request: CreateFeatureRequest,
) -> Result<CreateFeatureResponse, GuiError> {
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    let response = gui.create_feature(request)?;
    emit_workspace_changed(&app, &gui.snapshot());
    Ok(response)
}

#[tauri::command]
fn start_feature(
    app: tauri::AppHandle,
    state: State<AppState>,
    target: FeatureTarget,
    approved: bool,
) -> Result<StartFeatureResponse, GuiError> {
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    let response = gui.start_feature_with_approval(target, approved)?;
    emit_workspace_changed(&app, &gui.snapshot());
    Ok(response)
}

#[tauri::command]
fn stop_feature(
    app: tauri::AppHandle,
    state: State<AppState>,
    target: FeatureTarget,
) -> Result<StopFeatureResponse, GuiError> {
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    let response = gui.stop_feature(target)?;
    emit_workspace_changed(&app, &gui.snapshot());
    Ok(response)
}

#[derive(Serialize)]
struct AttachTerminalResponse {
    key: String,
    /// Passed back to `detach_terminal` so it removes this attachment and
    /// never a newer one for the same session.
    generation: u64,
    initial: String,
}

#[derive(Deserialize)]
struct TerminalSize {
    cols: u16,
    rows: u16,
}

/// Attach to a session's tmux pane and start streaming updates as
/// `terminal-output:{key}` events (`AMF_PLAN.md` Task 6). Re-attaching the
/// same `target` (a GUI reconnect after closing and reopening) replaces the
/// prior attachment rather than erroring or leaking it.
#[tauri::command]
fn attach_terminal(
    app: tauri::AppHandle,
    state: State<AppState>,
    terminals: State<TerminalState>,
    target: SessionTarget,
    size: TerminalSize,
) -> Result<AttachTerminalResponse, GuiError> {
    let terminal_target = state
        .0
        .lock()
        .expect("gui handle mutex poisoned")
        .resolve_session_target(&target)?;
    let key = terminal_key(&target);

    let event_name = format!("terminal-output:{key}");
    let emit_app = app.clone();
    let (handle, initial) = TerminalHandle::attach(
        &terminal_target.tmux_session,
        &terminal_target.tmux_window,
        size.cols,
        size.rows,
        move |replay| {
            if let Err(err) = emit_app.emit(&event_name, replay) {
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
fn terminal_input(
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
fn terminal_submit_prompt(
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
fn resize_terminal(
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

/// Explicit detach, for a pane the frontend is done with before the window
/// itself closes (switching sessions, say). Removing it from the map drops
/// it, which is exactly what closing the whole GUI relies on to detach every
/// live attachment without killing their underlying tmux sessions -- see
/// `TerminalHandle`'s own doc comment. `generation` is the one
/// `attach_terminal` returned; a detach for a superseded attachment leaves
/// the current one in place (see [`Attachments`]).
#[tauri::command]
fn detach_terminal(terminals: State<TerminalState>, key: String, generation: u64) {
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
#[tauri::command]
fn todo_list(
    state: State<AppState>,
    request: TodoScopeRequest,
) -> Result<Option<TodoListView>, GuiError> {
    gui_todos::load(
        &state.0.lock().expect("gui handle mutex poisoned"),
        &request,
    )
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
fn todo_add(
    state: State<AppState>,
    request: TodoScopeRequest,
    host_feature_id: Option<String>,
    title: String,
    body: Option<String>,
    priority: TodoPriority,
) -> Result<TodoListView, GuiError> {
    gui_todos::add(
        &state.0.lock().expect("gui handle mutex poisoned"),
        &request,
        host_feature_id.as_deref(),
        &title,
        body.as_deref(),
        priority,
    )
}

#[tauri::command]
fn todo_set_status(
    state: State<AppState>,
    todo_id: String,
    status: TodoStatus,
) -> Result<(), GuiError> {
    gui_todos::set_status(
        &state.0.lock().expect("gui handle mutex poisoned"),
        &todo_id,
        status,
    )
}

#[tauri::command]
fn todo_delete(state: State<AppState>, todo_id: String) -> Result<(), GuiError> {
    gui_todos::delete(
        &state.0.lock().expect("gui handle mutex poisoned"),
        &todo_id,
    )
}

#[tauri::command]
fn todo_move(
    state: State<AppState>,
    todo_id: String,
    target: TodoScopeRequest,
) -> Result<TodoListView, GuiError> {
    gui_todos::move_to(
        &state.0.lock().expect("gui handle mutex poisoned"),
        &todo_id,
        &target,
    )
}

#[tauri::command]
fn todo_copy(
    state: State<AppState>,
    todo_id: String,
    target: TodoScopeRequest,
) -> Result<TodoListView, GuiError> {
    gui_todos::copy_to(
        &state.0.lock().expect("gui handle mutex poisoned"),
        &todo_id,
        &target,
    )
}

#[tauri::command]
fn todo_reorder(state: State<AppState>, ordered_ids: Vec<String>) -> Result<(), GuiError> {
    gui_todos::reorder(
        &state.0.lock().expect("gui handle mutex poisoned"),
        &ordered_ids,
    )
}

#[tauri::command]
fn todo_launch_agent(
    app: tauri::AppHandle,
    state: State<AppState>,
    todo_id: String,
    target: FeatureTarget,
    approved: bool,
) -> Result<TodoAgentLaunchResponse, GuiError> {
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    let response = gui.launch_todo_agent(&todo_id, target, approved)?;
    emit_workspace_changed(&app, &gui.snapshot());
    Ok(response)
}

#[tauri::command]
fn todo_launch_new_feature(
    app: tauri::AppHandle,
    state: State<AppState>,
    todo_id: String,
    request: CreateFeatureRequest,
    approved: bool,
) -> Result<TodoAgentLaunchResponse, GuiError> {
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    let response = gui.launch_todo_in_new_feature(&todo_id, request, approved)?;
    emit_workspace_changed(&app, &gui.snapshot());
    Ok(response)
}

#[tauri::command]
fn plan_begin(
    state: State<AppState>,
    target: FeatureTarget,
    quick: bool,
) -> Result<PlanStatus, GuiError> {
    gui_plans::begin(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &target,
        quick,
    )
}

#[tauri::command]
fn plan_begin_todo_host(
    state: State<AppState>,
    todo_id: String,
    target: FeatureTarget,
) -> Result<PlanStatus, GuiError> {
    gui_plans::begin_todo_in_host(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &todo_id,
        &target,
    )
}

#[tauri::command]
fn plan_begin_creation(
    state: State<AppState>,
    request: CreateFeatureRequest,
    quick: bool,
) -> Result<PlanStatus, GuiError> {
    gui_plans::begin_feature_creation(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &request,
        quick,
    )
}

#[tauri::command]
fn plan_begin_todo_new(
    state: State<AppState>,
    todo_id: String,
    request: CreateFeatureRequest,
) -> Result<PlanStatus, GuiError> {
    gui_plans::begin_todo_in_new_feature(
        &mut state.0.lock().expect("gui handle mutex poisoned"),
        &todo_id,
        &request,
    )
}

#[tauri::command]
fn plan_snapshot(state: State<AppState>) -> PlanStatus {
    gui_plans::poll(&mut state.0.lock().expect("gui handle mutex poisoned"))
}

#[tauri::command]
fn plan_act(
    app: tauri::AppHandle,
    state: State<AppState>,
    expected_step: String,
    action: PlanAction,
    input: Option<PlanInput>,
) -> Result<PlanStatus, GuiError> {
    let mut gui = state.0.lock().expect("gui handle mutex poisoned");
    let result = gui_plans::act(&mut gui, &expected_step, action, input)?;
    emit_workspace_changed(&app, &gui.snapshot());
    Ok(result)
}

fn main() {
    if cfg!(target_os = "macos") {
        // SAFETY: first statement of `main`, before Tauri or anything else
        // starts a thread.
        unsafe { login_path::adopt_login_shell_path() };
    }
    tauri::Builder::default()
        .setup(|app| {
            let db_path = agent_mainframe::project::db_path();
            let gui = GuiHandle::new(db_path)?;
            app.manage(AppState(Mutex::new(gui)));
            app.manage(TerminalState(Mutex::new(Attachments::new())));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            supported_harnesses,
            supported_modes,
            get_snapshot,
            create_project,
            create_feature,
            start_feature,
            stop_feature,
            attach_terminal,
            terminal_input,
            terminal_submit_prompt,
            resize_terminal,
            detach_terminal,
            todo_list,
            todo_add,
            todo_set_status,
            todo_delete,
            todo_move,
            todo_copy,
            todo_reorder,
            todo_launch_agent,
            todo_launch_new_feature,
            plan_begin,
            plan_begin_todo_host,
            plan_begin_creation,
            plan_begin_todo_new,
            plan_snapshot,
            plan_act,
        ])
        .run(tauri::generate_context!())
        .expect("error while running amf-gui");
}

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
