use crate::project::SessionKind;
use crate::worktree::WorktreeManager;
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
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

pub(crate) fn read_latest_prompt_for_session(
    workdir: &Path,
    session_kind: Option<&crate::project::SessionKind>,
    preferred_session_id: Option<&str>,
) -> Option<String> {
    let entries = read_all_prompts_for_session(workdir, session_kind, preferred_session_id);
    entries
        .into_iter()
        .max_by(|a, b| match (a.timestamp, b.timestamp) {
            (Some(at), Some(bt)) => at.cmp(&bt),
            (Some(_), None) => std::cmp::Ordering::Greater,
            (None, Some(_)) => std::cmp::Ordering::Less,
            (None, None) => std::cmp::Ordering::Equal,
        })
        .map(|entry| entry.text)
}

pub fn read_all_prompts(workdir: &Path) -> Vec<PromptEntry> {
    read_all_prompts_for_session(workdir, None, None)
}

pub(crate) fn read_all_prompts_for_session(
    workdir: &Path,
    session_kind: Option<&SessionKind>,
    preferred_session_id: Option<&str>,
) -> Vec<PromptEntry> {
    let mut entries = match session_kind {
        Some(SessionKind::Opencode) => {
            read_prompts_from_opencode_storage(workdir, preferred_session_id)
        }
        Some(SessionKind::Codex) => read_prompts_from_codex_history(workdir, preferred_session_id),
        _ => read_prompts_from_claude_sessions(workdir),
    };

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

fn read_prompts_from_codex_history(workdir: &Path, session_id: Option<&str>) -> Vec<PromptEntry> {
    let Some(session_id) = session_id else {
        return Vec::new();
    };

    let mut entries = super::codex_sessions::prompt_history_for_session_id(workdir, session_id)
        .unwrap_or_default()
        .into_iter()
        .map(|entry| PromptEntry {
            text: entry.text,
            timestamp: entry.timestamp,
        })
        .collect::<Vec<_>>();

    entries.sort_by(|a, b| match (b.timestamp, a.timestamp) {
        (Some(bt), Some(at)) => bt.cmp(&at),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });

    entries
}

fn read_prompts_from_opencode_storage(
    workdir: &Path,
    preferred_session_id: Option<&str>,
) -> Vec<PromptEntry> {
    let Some(storage_root) = dirs::data_dir().map(|dir| dir.join("opencode").join("storage"))
    else {
        return Vec::new();
    };
    read_prompts_from_opencode_storage_root(&storage_root, workdir, preferred_session_id)
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

fn read_prompts_from_opencode_storage_root(
    storage_root: &Path,
    workdir: &Path,
    preferred_session_id: Option<&str>,
) -> Vec<PromptEntry> {
    let Some(session_id) = find_opencode_session_id(storage_root, workdir, preferred_session_id)
    else {
        return Vec::new();
    };
    let message_root = storage_root.join("message").join(&session_id);
    if !message_root.is_dir() {
        return Vec::new();
    }

    let mut entries = Vec::new();
    for message_path in walk_json_files(&message_root) {
        let Ok(contents) = std::fs::read_to_string(&message_path) else {
            continue;
        };
        let Ok(message) = serde_json::from_str::<OpencodeMessage>(&contents) else {
            continue;
        };
        if message.role != "user" {
            continue;
        }

        let text = read_opencode_prompt_text(storage_root, &message.id).or_else(|| {
            message
                .summary
                .and_then(|summary| summary.title)
                .map(|title| title.trim().to_string())
                .filter(|title| !title.is_empty())
        });
        let Some(text) = text else {
            continue;
        };

        entries.push(PromptEntry {
            text,
            timestamp: Some(message.time.created / 1000),
        });
    }

    entries
}

fn find_opencode_session_id(
    storage_root: &Path,
    workdir: &Path,
    preferred_session_id: Option<&str>,
) -> Option<String> {
    let session_root = storage_root.join("session");
    if !session_root.is_dir() {
        return None;
    }

    let sessions = walk_json_files(&session_root)
        .into_iter()
        .filter_map(|path| parse_opencode_session(&path))
        .collect::<Vec<_>>();

    if let Some(session_id) = preferred_session_id
        && sessions.iter().any(|session| session.id == session_id)
    {
        return Some(session_id.to_string());
    }

    sessions
        .into_iter()
        .filter(|session| session.directory == workdir)
        .max_by_key(|session| session.updated)
        .map(|session| session.id)
}

fn read_opencode_prompt_text(storage_root: &Path, message_id: &str) -> Option<String> {
    let part_root = storage_root.join("part").join(message_id);
    if !part_root.is_dir() {
        return None;
    }

    let mut texts = Vec::new();
    for part_path in walk_json_files(&part_root) {
        let Ok(contents) = std::fs::read_to_string(part_path) else {
            continue;
        };
        let Ok(part) = serde_json::from_str::<OpencodePart>(&contents) else {
            continue;
        };
        if part.part_type != "text" {
            continue;
        }
        let Some(text) = part.text.map(|text| text.trim().to_string()) else {
            continue;
        };
        if !text.is_empty() {
            texts.push(text);
        }
    }

    if texts.is_empty() {
        None
    } else {
        Some(texts.join("\n"))
    }
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

fn walk_json_files(root: &Path) -> Vec<PathBuf> {
    if !root.is_dir() {
        return Vec::new();
    }

    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

#[derive(Debug, Deserialize)]
struct OpencodeSessionFile {
    id: String,
    directory: String,
    time: OpencodeTime,
}

#[derive(Debug)]
struct OpencodeSessionRecord {
    id: String,
    directory: PathBuf,
    updated: i64,
}

fn parse_opencode_session(path: &Path) -> Option<OpencodeSessionRecord> {
    let contents = std::fs::read_to_string(path).ok()?;
    let session = serde_json::from_str::<OpencodeSessionFile>(&contents).ok()?;
    Some(OpencodeSessionRecord {
        id: session.id,
        directory: PathBuf::from(session.directory),
        updated: session.time.updated,
    })
}

#[derive(Debug, Deserialize)]
struct OpencodeTime {
    updated: i64,
}

#[derive(Debug, Deserialize)]
struct OpencodeMessage {
    id: String,
    role: String,
    time: OpencodeMessageTime,
    #[serde(default)]
    summary: Option<OpencodeMessageSummary>,
}

#[derive(Debug, Deserialize)]
struct OpencodeMessageTime {
    created: i64,
}

#[derive(Debug, Deserialize)]
struct OpencodeMessageSummary {
    #[serde(default)]
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpencodePart {
    #[serde(rename = "type")]
    part_type: String,
    #[serde(default)]
    text: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn reads_opencode_prompts_from_selected_session_storage() {
        let temp = TempDir::new().unwrap();
        let workdir = PathBuf::from("/tmp/opencode-prompts");
        let storage = temp.path();

        std::fs::create_dir_all(storage.join("session").join("project-a")).unwrap();
        std::fs::create_dir_all(storage.join("message").join("ses-picked")).unwrap();
        std::fs::create_dir_all(storage.join("part").join("msg-1")).unwrap();
        std::fs::create_dir_all(storage.join("part").join("msg-2")).unwrap();

        std::fs::write(
            storage
                .join("session")
                .join("project-a")
                .join("ses-picked.json"),
            "{\"id\":\"ses-picked\",\"directory\":\"/other\",\"time\":{\"updated\":2}}",
        )
        .unwrap();
        std::fs::write(
            storage
                .join("message")
                .join("ses-picked")
                .join("msg-1.json"),
            "{\"id\":\"msg-1\",\"role\":\"user\",\"time\":{\"created\":1000}}",
        )
        .unwrap();
        std::fs::write(
            storage
                .join("message")
                .join("ses-picked")
                .join("msg-2.json"),
            "{\"id\":\"msg-2\",\"role\":\"user\",\"time\":{\"created\":3000}}",
        )
        .unwrap();
        std::fs::write(
            storage.join("part").join("msg-1").join("prt-1.json"),
            "{\"type\":\"text\",\"text\":\"older prompt\"}",
        )
        .unwrap();
        std::fs::write(
            storage.join("part").join("msg-2").join("prt-1.json"),
            "{\"type\":\"text\",\"text\":\"latest prompt\"}",
        )
        .unwrap();

        let entries =
            read_prompts_from_opencode_storage_root(storage, &workdir, Some("ses-picked"));

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].text, "older prompt");
        assert_eq!(entries[1].text, "latest prompt");
    }

    #[test]
    fn reads_opencode_prompts_by_workdir_and_falls_back_to_summary_title() {
        let temp = TempDir::new().unwrap();
        let workdir = PathBuf::from("/tmp/opencode-prompts");
        let storage = temp.path();

        std::fs::create_dir_all(storage.join("session").join("project-a")).unwrap();
        std::fs::create_dir_all(storage.join("message").join("ses-1")).unwrap();

        std::fs::write(
            storage.join("session").join("project-a").join("ses-1.json"),
            format!(
                "{{\"id\":\"ses-1\",\"directory\":\"{}\",\"time\":{{\"updated\":5}}}}",
                workdir.display()
            ),
        )
        .unwrap();
        std::fs::write(
            storage.join("message").join("ses-1").join("msg-1.json"),
            "{\"id\":\"msg-1\",\"role\":\"assistant\",\"time\":{\"created\":1000}}",
        )
        .unwrap();
        std::fs::write(
            storage.join("message").join("ses-1").join("msg-2.json"),
            "{\"id\":\"msg-2\",\"role\":\"user\",\"time\":{\"created\":2000},\"summary\":{\"title\":\"summary prompt\"}}",
        )
        .unwrap();

        let entries = read_prompts_from_opencode_storage_root(storage, &workdir, None);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].text, "summary prompt");
        assert_eq!(entries[0].timestamp, Some(2));
    }

    #[test]
    fn latest_prompt_for_session_prefers_newest_timestamp() {
        let workdir = PathBuf::from("/tmp/unused");
        let latest = read_latest_prompt_for_session(&workdir, None, None);
        assert_eq!(latest, None);
    }
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
