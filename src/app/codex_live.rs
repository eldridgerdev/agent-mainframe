use serde_json::Value;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodexLiveThreadState {
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub plan_text: Option<String>,
    pub reasoning_text: Option<String>,
    pub command_text: Option<String>,
    pub file_change_text: Option<String>,
    pub input_request_text: Option<String>,
}

impl CodexLiveThreadState {
    pub fn apply_event(&mut self, raw: &Value) -> bool {
        let mut changed = false;

        changed |= assign_if_some(&mut self.thread_id, first_string(raw, &THREAD_ID_PATHS));
        changed |= assign_if_some(&mut self.turn_id, first_string(raw, &TURN_ID_PATHS));

        let Some(event_type) = first_string(raw, &EVENT_TYPE_PATHS) else {
            return changed;
        };

        match event_type.as_str() {
            "plan" | "plan_update" => {
                changed |= assign_if_some(&mut self.plan_text, extract_plan_text(raw));
            }
            "reasoning" | "reasoning_summary" => {
                changed |= assign_if_some(&mut self.reasoning_text, extract_reasoning_text(raw));
            }
            "commandExecution" | "command_execution" => {
                changed |= assign_if_some(&mut self.command_text, extract_command_text(raw));
            }
            "fileChange" | "file_change" => {
                changed |=
                    assign_if_some(&mut self.file_change_text, extract_file_change_text(raw));
            }
            "requestUserInput" | "request_user_input" => {
                changed |= assign_if_some(
                    &mut self.input_request_text,
                    extract_input_request_text(raw),
                );
            }
            "inputResolved" | "input_resolved" => {
                changed |= clear_if_some(&mut self.input_request_text);
            }
            _ => {}
        }

        changed
    }

    pub fn sidebar_work_text(&self) -> Option<String> {
        if let Some(text) = &self.input_request_text {
            return Some(format!("Pending input: {text}"));
        }
        if let Some(text) = &self.file_change_text {
            return Some(text.clone());
        }
        self.command_text.clone()
    }

    pub fn summary_prefix(&self) -> Option<String> {
        self.reasoning_text.clone()
    }
}

const EVENT_TYPE_PATHS: &[&str] = &["/type", "/event", "/payload/type", "/payload/event"];
const THREAD_ID_PATHS: &[&str] = &[
    "/thread_id",
    "/threadId",
    "/payload/thread_id",
    "/payload/threadId",
];
const TURN_ID_PATHS: &[&str] = &["/turn_id", "/turnId", "/payload/turn_id", "/payload/turnId"];

fn assign_if_some(slot: &mut Option<String>, next: Option<String>) -> bool {
    let Some(next) = next else {
        return false;
    };
    if slot.as_deref() == Some(next.as_str()) {
        return false;
    }
    *slot = Some(next);
    true
}

fn clear_if_some(slot: &mut Option<String>) -> bool {
    let changed = slot.is_some();
    *slot = None;
    changed
}

fn first_string(raw: &Value, pointers: &[&str]) -> Option<String> {
    pointers
        .iter()
        .filter_map(|pointer| raw.pointer(pointer).and_then(Value::as_str))
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn extract_plan_text(raw: &Value) -> Option<String> {
    first_string(raw, &["/payload/text", "/payload/plan", "/text", "/plan"])
}

fn extract_reasoning_text(raw: &Value) -> Option<String> {
    first_string(
        raw,
        &["/payload/summary", "/payload/text", "/summary", "/text"],
    )
}

fn extract_command_text(raw: &Value) -> Option<String> {
    let command = first_string(raw, &["/payload/command", "/command"])?;
    let phase = first_string(
        raw,
        &["/payload/phase", "/payload/status", "/phase", "/status"],
    );
    let exit_code = first_string(
        raw,
        &["/payload/exit_code", "/payload/exitCode", "/exit_code"],
    );

    let mut lines = vec![format!("Command: {command}")];
    if let Some(phase) = phase {
        lines.push(format!("State: {phase}"));
    }
    if let Some(exit_code) = exit_code {
        lines.push(format!("Exit: {exit_code}"));
    }
    Some(lines.join("\n"))
}

fn extract_file_change_text(raw: &Value) -> Option<String> {
    let path = first_string(
        raw,
        &[
            "/payload/path",
            "/payload/relative_path",
            "/path",
            "/relative_path",
        ],
    )?;
    let status = first_string(
        raw,
        &["/payload/phase", "/payload/status", "/phase", "/status"],
    );

    let mut lines = vec![format!("File: {path}")];
    if let Some(status) = status {
        lines.push(format!("State: {status}"));
    }
    Some(lines.join("\n"))
}

fn extract_input_request_text(raw: &Value) -> Option<String> {
    first_string(
        raw,
        &["/payload/prompt", "/payload/message", "/prompt", "/message"],
    )
}

#[cfg(test)]
mod tests {
    use super::CodexLiveThreadState;
    use serde_json::json;

    #[test]
    fn reducer_captures_plan_updates() {
        let mut state = CodexLiveThreadState::default();
        let changed = state.apply_event(&json!({
            "type": "plan",
            "thread_id": "thread-1",
            "payload": { "text": "1. Inspect repo\n2. Patch bug" }
        }));

        assert!(changed);
        assert_eq!(state.thread_id.as_deref(), Some("thread-1"));
        assert_eq!(
            state.plan_text.as_deref(),
            Some("1. Inspect repo\n2. Patch bug")
        );
    }

    #[test]
    fn reducer_captures_reasoning_updates() {
        let mut state = CodexLiveThreadState::default();
        state.apply_event(&json!({
            "event": "reasoning",
            "payload": { "summary": "Comparing two parser approaches." }
        }));

        assert_eq!(
            state.reasoning_text.as_deref(),
            Some("Comparing two parser approaches.")
        );
    }

    #[test]
    fn reducer_tracks_command_execution() {
        let mut state = CodexLiveThreadState::default();
        state.apply_event(&json!({
            "type": "commandExecution",
            "payload": {
                "command": "cargo test",
                "phase": "running"
            }
        }));

        assert_eq!(
            state.command_text.as_deref(),
            Some("Command: cargo test\nState: running")
        );
    }

    #[test]
    fn reducer_tracks_file_changes() {
        let mut state = CodexLiveThreadState::default();
        state.apply_event(&json!({
            "type": "fileChange",
            "payload": {
                "relative_path": "src/main.rs",
                "status": "proposed"
            }
        }));

        assert_eq!(
            state.file_change_text.as_deref(),
            Some("File: src/main.rs\nState: proposed")
        );
    }

    #[test]
    fn reducer_tracks_and_clears_input_requests() {
        let mut state = CodexLiveThreadState::default();
        state.apply_event(&json!({
            "type": "requestUserInput",
            "payload": { "prompt": "Need approval for migration." }
        }));
        assert_eq!(
            state.sidebar_work_text().as_deref(),
            Some("Pending input: Need approval for migration.")
        );

        state.apply_event(&json!({
            "type": "inputResolved"
        }));
        assert_eq!(state.input_request_text, None);
    }
}
