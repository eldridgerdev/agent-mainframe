use crate::worktree::WorktreeManager;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct PromptEntry {
    pub text: String,
    pub timestamp: Option<i64>, // unix seconds
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaudeTask {
    pub id: String,
    pub subject: String,
    pub description: Option<String>,
    pub active_form: Option<String>,
    pub status: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClaudeTaskState {
    pub tasks: Vec<ClaudeTask>,
}

impl ClaudeTaskState {
    pub fn current_task(&self) -> Option<&ClaudeTask> {
        self.tasks.iter().find(|task| task.status == "in_progress")
    }

    pub fn completed_count(&self) -> usize {
        self.tasks
            .iter()
            .filter(|task| task.status == "completed")
            .count()
    }

    pub fn pending_count(&self) -> usize {
        self.tasks
            .iter()
            .filter(|task| task.status == "pending")
            .count()
    }

    pub fn last_completed_task(&self) -> Option<&ClaudeTask> {
        self.tasks
            .iter()
            .rev()
            .find(|task| task.status == "completed")
    }
}

pub fn shorten_path(path: &std::path::Path) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return format!("~/{}", rest.display());
    }
    path.display().to_string()
}

pub fn slugify(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

pub fn detect_repo_path() -> String {
    let cwd = std::env::current_dir().unwrap_or_default();
    WorktreeManager::repo_root(&cwd)
        .unwrap_or(cwd)
        .to_string_lossy()
        .into_owned()
}

pub fn detect_branch() -> String {
    let cwd = std::env::current_dir().unwrap_or_default();
    WorktreeManager::current_branch(&cwd)
        .ok()
        .flatten()
        .unwrap_or_default()
}

pub fn latest_prompt_path(workdir: &Path) -> PathBuf {
    workdir.join(".claude").join("latest-prompt.txt")
}

pub fn read_latest_prompt(workdir: &Path) -> Option<String> {
    let paths = [
        latest_prompt_path(workdir),
        workdir.join(".codex").join("latest-prompt.txt"),
    ];

    paths
        .into_iter()
        .find_map(|path| std::fs::read_to_string(path).ok())
        .or_else(|| {
            super::codex_sessions::latest_prompt_for_workdir(workdir)
                .ok()
                .flatten()
        })
}

pub fn read_latest_prompt_entry(workdir: &Path) -> Option<PromptEntry> {
    read_all_prompts(workdir).into_iter().next()
}

pub fn read_all_prompts(workdir: &Path) -> Vec<PromptEntry> {
    let mut entries = read_prompts_from_claude_sessions(workdir);

    // Fall back to latest-prompt.txt if no session entries found
    if entries.is_empty() {
        let path = latest_prompt_path(workdir);
        if let Ok(text) = std::fs::read_to_string(&path) {
            if !text.trim().is_empty() {
                let ts = std::fs::metadata(&path)
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64);
                entries.push(PromptEntry {
                    text,
                    timestamp: ts,
                });
            }
        }
    }

    // Sort by timestamp descending (latest first), None timestamps at end
    entries.sort_by(|a, b| match (b.timestamp, a.timestamp) {
        (Some(bt), Some(at)) => bt.cmp(&at),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });

    entries
}

pub fn read_claude_task_state(workdir: &Path, session_id: Option<&str>) -> Option<ClaudeTaskState> {
    let session_id = session_id
        .map(str::trim)
        .filter(|session_id| !session_id.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| latest_claude_session_id(workdir));

    if let Some(session_id) = session_id.as_deref()
        && let Some(state) = read_claude_task_state_from_task_store(session_id)
    {
        return Some(state);
    }

    let path = session_id
        .as_deref()
        .and_then(|session_id| claude_session_jsonl_path(workdir, session_id))
        .or_else(|| latest_claude_session_jsonl_path(workdir))?;
    let content = std::fs::read_to_string(path).ok()?;
    parse_claude_task_state_from_jsonl(&content)
}

fn read_prompts_from_claude_sessions(workdir: &Path) -> Vec<PromptEntry> {
    let home = match std::env::var("HOME") {
        Ok(h) => h,
        Err(_) => return Vec::new(),
    };
    let encoded = encode_claude_path(workdir);
    let projects_dir = PathBuf::from(&home)
        .join(".claude")
        .join("projects")
        .join(&encoded);

    if !projects_dir.is_dir() {
        return Vec::new();
    }

    let read_dir = match std::fs::read_dir(&projects_dir) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    let mut entries = Vec::new();

    for dir_entry in read_dir.flatten() {
        let path = dir_entry.path();
        if path.extension().is_none_or(|ext| ext != "jsonl") {
            continue;
        }
        let file_ts = dir_entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64);

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let value: serde_json::Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };

            if value["type"] != "user" {
                continue;
            }

            let text = match extract_user_prompt_text(&value) {
                Some(t) if !t.trim().is_empty() => t,
                _ => continue,
            };

            let ts = value
                .get("timestamp")
                .and_then(|v| v.as_str())
                .and_then(parse_prompt_timestamp)
                .or(file_ts);

            entries.push(PromptEntry {
                text,
                timestamp: ts,
            });
        }
    }

    entries
}

fn claude_projects_dir(workdir: &Path) -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(
        PathBuf::from(home)
            .join(".claude")
            .join("projects")
            .join(encode_claude_path(workdir)),
    )
}

fn claude_session_jsonl_path(workdir: &Path, session_id: &str) -> Option<PathBuf> {
    let projects_dir = claude_projects_dir(workdir)?;
    let path = projects_dir.join(format!("{session_id}.jsonl"));
    path.is_file().then_some(path)
}

fn latest_claude_session_jsonl_path(workdir: &Path) -> Option<PathBuf> {
    let projects_dir = claude_projects_dir(workdir)?;
    let read_dir = std::fs::read_dir(projects_dir).ok()?;

    read_dir
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "jsonl") {
                return None;
            }
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, path))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
}

fn latest_claude_session_id(workdir: &Path) -> Option<String> {
    latest_claude_session_jsonl_path(workdir)?
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(ToOwned::to_owned)
}

fn read_claude_task_state_from_task_store(session_id: &str) -> Option<ClaudeTaskState> {
    let home = std::env::var("HOME").ok()?;
    let tasks_dir = PathBuf::from(home)
        .join(".claude")
        .join("tasks")
        .join(session_id);
    if !tasks_dir.is_dir() {
        return None;
    }

    let mut task_entries: Vec<(u64, PathBuf)> = std::fs::read_dir(&tasks_dir)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let id = path.file_stem()?.to_str()?.parse::<u64>().ok()?;
            (path.extension().and_then(|ext| ext.to_str()) == Some("json")).then_some((id, path))
        })
        .collect();
    task_entries.sort_by_key(|(id, _)| *id);

    let mut tasks = Vec::new();
    for (_, path) in task_entries {
        let content = std::fs::read_to_string(path).ok()?;
        let value: serde_json::Value = serde_json::from_str(&content).ok()?;
        let id = value.get("id")?.as_str()?.trim();
        let subject = value.get("subject")?.as_str()?.trim();
        if id.is_empty() || subject.is_empty() {
            continue;
        }

        tasks.push(ClaudeTask {
            id: id.to_string(),
            subject: subject.to_string(),
            description: value
                .get("description")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(ToOwned::to_owned),
            active_form: value
                .get("activeForm")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(ToOwned::to_owned),
            status: value
                .get("status")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .unwrap_or("pending")
                .to_string(),
        });
    }

    (!tasks.is_empty()).then_some(ClaudeTaskState { tasks })
}

fn parse_claude_task_state_from_jsonl(content: &str) -> Option<ClaudeTaskState> {
    let mut state = ClaudeTaskState::default();

    for line in content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => continue,
        };

        let Some(contents) = value
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(|content| content.as_array())
        else {
            continue;
        };

        for item in contents {
            if item.get("type").and_then(|value| value.as_str()) != Some("tool_use") {
                continue;
            }

            match item.get("name").and_then(|value| value.as_str()) {
                Some("TaskCreate") => apply_task_create(&mut state, item.get("input")),
                Some("TaskUpdate") => apply_task_update(&mut state, item.get("input")),
                _ => {}
            }
        }
    }

    (!state.tasks.is_empty()).then_some(state)
}

fn apply_task_create(state: &mut ClaudeTaskState, input: Option<&serde_json::Value>) {
    let Some(input) = input else {
        return;
    };

    let subject = input
        .get("subject")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim();
    if subject.is_empty() {
        return;
    }

    let id = (state.tasks.len() + 1).to_string();
    let description = input
        .get("description")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let active_form = input
        .get("activeForm")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    state.tasks.push(ClaudeTask {
        id,
        subject: subject.to_string(),
        description,
        active_form,
        status: "pending".to_string(),
    });
}

fn apply_task_update(state: &mut ClaudeTaskState, input: Option<&serde_json::Value>) {
    let Some(input) = input else {
        return;
    };

    let task_id = input
        .get("taskId")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim();
    if task_id.is_empty() {
        return;
    }

    let status = input
        .get("status")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let active_form = input
        .get("activeForm")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let subject = input
        .get("subject")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let description = input
        .get("description")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    let task = if let Some(task) = state.tasks.iter_mut().find(|task| task.id == task_id) {
        task
    } else {
        state.tasks.push(ClaudeTask {
            id: task_id.to_string(),
            subject: subject.clone().unwrap_or_else(|| format!("Task {task_id}")),
            description: description.clone(),
            active_form: None,
            status: "pending".to_string(),
        });
        state.tasks.last_mut().expect("inserted task should exist")
    };

    if let Some(subject) = subject {
        task.subject = subject;
    }
    if let Some(description) = description {
        task.description = Some(description);
    }
    if let Some(status) = status {
        task.status = status;
    }
    if active_form.is_some() {
        task.active_form = active_form;
    }
}

fn extract_user_prompt_text(value: &serde_json::Value) -> Option<String> {
    if let Some(content) = value["message"]["content"].as_str() {
        return Some(content.to_string());
    }
    if let Some(blocks) = value["message"]["content"].as_array() {
        let texts: Vec<&str> = blocks
            .iter()
            .filter(|b| b["type"] == "text")
            .filter_map(|b| b["text"].as_str())
            .collect();
        if !texts.is_empty() {
            return Some(texts.join("\n"));
        }
    }
    None
}

fn parse_prompt_timestamp(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp())
}

fn encode_claude_path(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

pub fn copy_to_clipboard(text: &str) -> anyhow::Result<()> {
    use std::io::Write;
    // Try wl-copy (Wayland)
    if let Ok(mut child) = std::process::Command::new("wl-copy")
        .stdin(std::process::Stdio::piped())
        .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = child.wait();
        return Ok(());
    }
    // Fallback to xclip
    if let Ok(mut child) = std::process::Command::new("xclip")
        .args(["-selection", "clipboard"])
        .stdin(std::process::Stdio::piped())
        .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = child.wait();
        return Ok(());
    }
    Err(anyhow::anyhow!(
        "No clipboard utility found (wl-copy or xclip)"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn parse_claude_task_state_tracks_task_create_and_update() {
        let content = r#"
{"message":{"content":[{"type":"tool_use","name":"TaskCreate","input":{"subject":"Explore UI","description":"Read the sidebar code","activeForm":"Exploring UI"}}]}}
{"message":{"content":[{"type":"tool_use","name":"TaskCreate","input":{"subject":"Add tests","description":"Cover task parsing"}}]}}
{"message":{"content":[{"type":"tool_use","name":"TaskUpdate","input":{"taskId":"1","status":"in_progress","activeForm":"Inspecting sidebar rendering"}}]}}
{"message":{"content":[{"type":"tool_use","name":"TaskUpdate","input":{"taskId":"1","status":"completed"}}]}}
{"message":{"content":[{"type":"tool_use","name":"TaskUpdate","input":{"taskId":"2","status":"in_progress","activeForm":"Writing parser tests"}}]}}
"#;

        let state = parse_claude_task_state_from_jsonl(content).expect("task state");
        assert_eq!(state.tasks.len(), 2);
        assert_eq!(state.completed_count(), 1);
        assert_eq!(state.pending_count(), 0);
        assert_eq!(state.current_task().map(|task| task.id.as_str()), Some("2"));
        assert_eq!(
            state
                .current_task()
                .and_then(|task| task.active_form.as_deref()),
            Some("Writing parser tests")
        );
        assert_eq!(
            state.tasks[0],
            ClaudeTask {
                id: "1".to_string(),
                subject: "Explore UI".to_string(),
                description: Some("Read the sidebar code".to_string()),
                active_form: Some("Inspecting sidebar rendering".to_string()),
                status: "completed".to_string(),
            }
        );
    }

    #[test]
    fn read_claude_task_state_falls_back_to_latest_session_file() {
        let workdir = TempDir::new().unwrap();
        let home = dirs::home_dir().expect("home dir");
        let projects_dir = home
            .join(".claude")
            .join("projects")
            .join(encode_claude_path(workdir.path()));
        std::fs::create_dir_all(&projects_dir).unwrap();
        std::fs::write(
            projects_dir.join("older.jsonl"),
            r#"{"message":{"content":[{"type":"tool_use","name":"TaskCreate","input":{"subject":"Old task"}}]}}"#,
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(
            projects_dir.join("newer.jsonl"),
            concat!(
                r#"{"message":{"content":[{"type":"tool_use","name":"TaskCreate","input":{"subject":"Fresh task"}}]}}"#,
                "\n",
                r#"{"message":{"content":[{"type":"tool_use","name":"TaskUpdate","input":{"taskId":"1","status":"in_progress","activeForm":"Working newest session"}}]}}"#
            ),
        )
        .unwrap();

        let state = read_claude_task_state(workdir.path(), None).expect("fallback task state");
        assert_eq!(
            state.current_task().map(|task| task.subject.as_str()),
            Some("Fresh task")
        );
        assert_eq!(
            state
                .current_task()
                .and_then(|task| task.active_form.as_deref()),
            Some("Working newest session")
        );
        let _ = std::fs::remove_dir_all(&projects_dir);
    }

    #[test]
    fn read_claude_task_state_prefers_task_store_when_available() {
        let workdir = TempDir::new().unwrap();
        let home = dirs::home_dir().expect("home dir");
        let session_id = "831cde32-0aa9-4791-9eda-8c7b6699d1ae-test";

        let projects_dir = home
            .join(".claude")
            .join("projects")
            .join(encode_claude_path(workdir.path()));
        std::fs::create_dir_all(&projects_dir).unwrap();
        std::fs::write(
            projects_dir.join(format!("{session_id}.jsonl")),
            r#"{"message":{"content":[{"type":"tool_use","name":"TaskCreate","input":{"subject":"Transcript task"}}]}}"#,
        )
        .unwrap();

        let tasks_dir = home.join(".claude").join("tasks").join(session_id);
        std::fs::create_dir_all(&tasks_dir).unwrap();
        std::fs::write(
            tasks_dir.join("1.json"),
            r#"{
  "id": "1",
  "subject": "Stored task",
  "description": "From tasks dir",
  "activeForm": "Working from tasks dir",
  "status": "in_progress"
}"#,
        )
        .unwrap();

        let state = read_claude_task_state(workdir.path(), Some(session_id)).expect("task state");
        assert_eq!(state.tasks.len(), 1);
        assert_eq!(state.tasks[0].subject, "Stored task");
        assert_eq!(
            state
                .current_task()
                .and_then(|task| task.active_form.as_deref()),
            Some("Working from tasks dir")
        );

        let _ = std::fs::remove_dir_all(&projects_dir);
        let _ = std::fs::remove_dir_all(&tasks_dir);
    }
}
