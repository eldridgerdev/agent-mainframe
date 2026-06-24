use crate::project::SessionKind;
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
    pub fn completed_count(&self) -> usize {
        self.tasks
            .iter()
            .filter(|task| task.status == "completed")
            .count()
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

pub fn fuzzy_match_score(candidate: &str, query: &str) -> Option<usize> {
    let query = query.trim().to_ascii_lowercase();
    if query.is_empty() {
        return Some(0);
    }

    let candidate = candidate.to_ascii_lowercase();
    let candidate_chars: Vec<char> = candidate.chars().collect();
    let query_chars: Vec<char> = query.chars().collect();

    if query_chars.len() > candidate_chars.len() {
        // Still allow subsequence matches, so keep scanning rather than bailing here.
    }

    let mut score = candidate_chars.len().saturating_sub(query_chars.len());
    let mut search_start = 0usize;
    let mut prev_match: Option<usize> = None;

    for qc in query_chars {
        let relative_pos = candidate_chars[search_start..]
            .iter()
            .position(|&c| c == qc)?;
        let match_pos = search_start + relative_pos;

        if let Some(prev) = prev_match {
            score += match_pos.saturating_sub(prev + 1);
        } else {
            score += match_pos;
        }

        prev_match = Some(match_pos);
        search_start = match_pos + 1;
    }

    Some(score)
}

pub fn markdown_file_picker_score(
    path: &Path,
    workdir: &Path,
    repo_root: Option<&Path>,
    query: &str,
) -> Option<usize> {
    let label = crate::markdown::markdown_view_relative_label(path, workdir, repo_root);
    let basename = path.file_name().and_then(|name| name.to_str());

    let mut best = fuzzy_match_score(&label, query);
    if let Some(name) = basename
        && let Some(score) = fuzzy_match_score(name, query)
    {
        best = Some(best.map_or(score, |existing| existing.min(score)));
    }
    best
}

pub fn worktree_picker_score(
    worktree: &crate::worktree::WorktreeInfo,
    query: &str,
) -> Option<usize> {
    let path = worktree.path.display().to_string();
    let basename = worktree.path.file_name().and_then(|name| name.to_str());

    let mut best = fuzzy_match_score(&path, query);
    if let Some(branch) = worktree.branch.as_deref()
        && let Some(score) = fuzzy_match_score(branch, query)
    {
        best = Some(best.map_or(score, |existing| existing.min(score)));
    }
    if let Some(name) = basename
        && let Some(score) = fuzzy_match_score(name, query)
    {
        best = Some(best.map_or(score, |existing| existing.min(score)));
    }
    best
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
    let tasks_root = claude_tasks_root();
    let session_id = session_id
        .map(str::trim)
        .filter(|session_id| !session_id.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| latest_claude_session_id(workdir));

    if let Some(session_id) = session_id.as_deref()
        && let Some(state) = read_claude_task_state_from_task_store(&tasks_root, session_id)
    {
        return Some(state);
    }

    if let Some(state) = read_latest_claude_task_state_from_task_store(&tasks_root) {
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
    // Inner function returning Option so we can use `?` for early exit.
    fn inner(workdir: &Path) -> Option<Vec<PromptEntry>> {
        use std::io::{Read, Seek, SeekFrom};

        let home = std::env::var("HOME").ok()?;
        let encoded = encode_claude_path(workdir);
        let projects_dir = PathBuf::from(&home)
            .join(".claude")
            .join("projects")
            .join(&encoded);

        if !is_real_dir(&projects_dir) {
            return None;
        }

        // Only read the most-recently-modified session file — the latest
        // prompt is overwhelmingly likely to be there, and we avoid reading
        // all session bytes across potentially many files.
        let (file_ts, newest_path) = std::fs::read_dir(&projects_dir)
            .ok()?
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                if path.extension().is_none_or(|ext| ext != "jsonl") {
                    return None;
                }
                let meta = entry.metadata().ok()?;
                let mtime = meta.modified().ok()?;
                let ts = mtime
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|d| d.as_secs() as i64);
                Some((mtime, ts, path))
            })
            .max_by_key(|(mtime, _, _)| *mtime)
            .map(|(_, ts, path)| (ts, path))?;

        // Read only the last 64 KB of the file — avoids loading multi-MB
        // session histories just to find the most recent user prompt.
        const TAIL: u64 = 65_536;
        let mut file = std::fs::File::open(&newest_path).ok()?;
        let file_len = file.metadata().ok()?.len();
        let start = file_len.saturating_sub(TAIL);
        if start > 0 {
            file.seek(SeekFrom::Start(start)).ok()?;
        }
        let mut buf = Vec::new();
        file.read_to_end(&mut buf).ok()?;

        // If we seeked into the middle, skip the first (partial) line.
        let data = if start > 0 {
            let skip = buf
                .iter()
                .position(|&b| b == b'\n')
                .map_or(buf.len(), |p| p + 1);
            &buf[skip..]
        } else {
            &buf[..]
        };

        let content = std::str::from_utf8(data).unwrap_or("");
        let mut entries = Vec::new();
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
        Some(entries)
    }

    inner(workdir).unwrap_or_default()
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
    if !is_real_dir(&message_root) {
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
    if !is_real_dir(&session_root) {
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
    if !is_real_dir(&part_root) {
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
    if !is_real_dir(&projects_dir) {
        return None;
    }
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

fn claude_tasks_root() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".claude").join("tasks"))
}

fn read_latest_claude_task_state_from_task_store(
    tasks_root: &Option<PathBuf>,
) -> Option<ClaudeTaskState> {
    let tasks_root = tasks_root.as_ref()?;
    if !is_real_dir(tasks_root) {
        return None;
    }

    let mut task_dirs: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(tasks_root)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !is_real_dir(&path) {
                return None;
            }
            latest_modified_in_dir(&path).map(|modified| (modified, path))
        })
        .collect();
    task_dirs.sort_by_key(|(modified, _)| *modified);
    task_dirs
        .into_iter()
        .rev()
        .find_map(|(_, path)| read_claude_task_state_from_task_dir(&path))
}

fn latest_modified_in_dir(dir: &Path) -> Option<std::time::SystemTime> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|entry| entry.metadata().ok()?.modified().ok())
        .max()
}

fn read_claude_task_state_from_task_store(
    tasks_root: &Option<PathBuf>,
    session_id: &str,
) -> Option<ClaudeTaskState> {
    let tasks_dir = tasks_root.as_ref()?.join(session_id);
    read_claude_task_state_from_task_dir(&tasks_dir)
}

fn read_claude_task_state_from_task_dir(tasks_dir: &Path) -> Option<ClaudeTaskState> {
    if !is_real_dir(&tasks_dir) {
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
    if !is_real_dir(root) {
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

fn is_real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false)
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

    /// Round-trips an image and text through the real Windows clipboard.
    /// Only meaningful under WSL; a no-op elsewhere so CI stays green.
    #[test]
    fn wsl_clipboard_round_trips_image_and_text() {
        if !is_wsl() {
            return;
        }

        // 1x1 red PNG.
        const PNG: &[u8] = &[
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
            0x00, 0x90, 0x77, 0x53, 0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x08,
            0xd7, 0x63, 0xf8, 0xcf, 0xc0, 0x00, 0x00, 0x00, 0x03, 0x00, 0x01, 0x36, 0x37, 0x82,
            0x9e, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ];

        copy_image_to_clipboard(PNG, "image/png").expect("copy image to clipboard");
        match read_clipboard().expect("read clipboard") {
            ClipboardContent::Image { data, mime } => {
                assert!(!data.is_empty());
                assert_eq!(mime, "image/png");
                assert_eq!(&data[..8], &PNG[..8], "should read back PNG bytes");
            }
            ClipboardContent::Text(t) => panic!("expected image, got text: {t:?}"),
        }

        copy_to_clipboard("amf-wsl-roundtrip").expect("copy text to clipboard");
        match read_clipboard().expect("read clipboard") {
            ClipboardContent::Text(t) => assert_eq!(t, "amf-wsl-roundtrip"),
            ClipboardContent::Image { .. } => panic!("expected text, got image"),
        }
    }

    #[test]
    fn latest_claude_task_store_fallback_reads_newest_task_directory() {
        let temp = TempDir::new().unwrap();
        let tasks_root = Some(temp.path().to_path_buf());
        let stale_dir = temp.path().join("stale-session");
        let fresh_dir = temp.path().join("detached-task-store");
        let empty_dir = temp.path().join("empty-newest-store");
        std::fs::create_dir_all(&stale_dir).unwrap();
        std::fs::create_dir_all(&fresh_dir).unwrap();
        std::fs::create_dir_all(&empty_dir).unwrap();
        std::fs::write(
            stale_dir.join("1.json"),
            r#"{"id":"1","subject":"Old task","status":"pending"}"#,
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(
            fresh_dir.join("1.json"),
            r#"{"id":"1","subject":"Render cursor highlight","activeForm":"Rendering cursor + markers","status":"in_progress"}"#,
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(empty_dir.join(".highwatermark"), "1").unwrap();

        let state = read_latest_claude_task_state_from_task_store(&tasks_root).unwrap();

        assert_eq!(state.tasks.len(), 1);
        assert_eq!(state.tasks[0].subject, "Render cursor highlight");
        assert_eq!(
            state.tasks[0].active_form.as_deref(),
            Some("Rendering cursor + markers")
        );
        assert_eq!(state.tasks[0].status, "in_progress");
    }

    #[test]
    fn claude_task_store_prefers_exact_session_directory() {
        let temp = TempDir::new().unwrap();
        let tasks_root = Some(temp.path().to_path_buf());
        let exact_dir = temp.path().join("claude-session");
        let other_dir = temp.path().join("detached-task-store");
        std::fs::create_dir_all(&exact_dir).unwrap();
        std::fs::create_dir_all(&other_dir).unwrap();
        std::fs::write(
            exact_dir.join("1.json"),
            r#"{"id":"1","subject":"Exact session task","status":"pending"}"#,
        )
        .unwrap();
        std::fs::write(
            other_dir.join("1.json"),
            r#"{"id":"1","subject":"Newest detached task","status":"pending"}"#,
        )
        .unwrap();

        let state = read_claude_task_state_from_task_store(&tasks_root, "claude-session").unwrap();

        assert_eq!(state.tasks.len(), 1);
        assert_eq!(state.tasks[0].subject, "Exact session task");
    }

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

    #[test]
    fn fuzzy_match_score_matches_subsequence() {
        assert!(fuzzy_match_score("plan-notes.md", "pn").is_some());
        assert!(fuzzy_match_score("plan-notes.md", "pln").is_some());
        assert!(fuzzy_match_score("plan-notes.md", "zn").is_none());
    }

    #[test]
    fn markdown_file_picker_score_prefers_basename_hits() {
        let workdir = PathBuf::from("/tmp/demo");
        let path = workdir.join("docs").join("plan-notes.md");
        assert!(markdown_file_picker_score(&path, &workdir, None, "pn").is_some());
    }
}

/// Returns true when running under WSL, where the Windows clipboard is
/// reachable via `powershell.exe`/`clip.exe` rather than `wl-paste`/`xclip`
/// (which are usually absent and cannot see the Windows clipboard anyway).
pub fn is_wsl() -> bool {
    use std::sync::OnceLock;
    static IS_WSL: OnceLock<bool> = OnceLock::new();
    *IS_WSL.get_or_init(|| {
        if std::env::var_os("WSL_DISTRO_NAME").is_some() {
            return true;
        }
        std::fs::read_to_string("/proc/version")
            .map(|v| {
                let v = v.to_ascii_lowercase();
                v.contains("microsoft") || v.contains("wsl")
            })
            .unwrap_or(false)
    })
}

/// Translate a Linux path to its Windows form (`\\wsl.localhost\...`) so a
/// Windows process launched from WSL can reach it. Returns None when
/// `wslpath` is unavailable or fails.
fn wsl_windows_path(path: &std::path::Path) -> Option<String> {
    let out = std::process::Command::new("wslpath")
        .arg("-w")
        .arg(path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

pub fn copy_to_clipboard(text: &str) -> anyhow::Result<()> {
    use std::io::Write;
    // On WSL, push text to the Windows clipboard via clip.exe.
    if is_wsl() && copy_to_clipboard_wsl(text).is_ok() {
        return Ok(());
    }
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

/// Open a URL in the user's default browser. Uses `xdg-open` on Linux and
/// `open` on macOS; the child is detached so AMF does not block on it.
pub fn open_in_browser(url: &str) -> anyhow::Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("Failed to launch {opener}: {e}"))
}

pub enum ClipboardContent {
    Text(String),
    Image { data: Vec<u8>, mime: String },
}

/// Read the system clipboard, preferring image content over text so a
/// copied screenshot pastes as an image rather than a file path.
pub fn read_clipboard() -> anyhow::Result<ClipboardContent> {
    if is_wsl()
        && let Some(content) = read_clipboard_wsl()
    {
        return Ok(content);
    }
    if let Some(content) = read_clipboard_wayland() {
        return Ok(content);
    }
    if let Some(content) = read_clipboard_x11() {
        return Ok(content);
    }
    Err(anyhow::anyhow!(
        "No clipboard utility found (wl-paste or xclip)"
    ))
}

fn clipboard_image_mime(types: &str) -> Option<String> {
    // Prefer png; otherwise take the first image type offered.
    let mut first_image = None;
    for line in types.lines() {
        let mime = line.trim();
        if mime == "image/png" {
            return Some(mime.to_string());
        }
        if mime.starts_with("image/") && first_image.is_none() {
            first_image = Some(mime.to_string());
        }
    }
    first_image
}

fn read_clipboard_wayland() -> Option<ClipboardContent> {
    let types = std::process::Command::new("wl-paste")
        .arg("--list-types")
        .output()
        .ok()?;
    if !types.status.success() {
        return None;
    }

    let types = String::from_utf8_lossy(&types.stdout);
    if let Some(mime) = clipboard_image_mime(&types) {
        let output = std::process::Command::new("wl-paste")
            .args(["--type", &mime])
            .output()
            .ok()?;
        if output.status.success() && !output.stdout.is_empty() {
            return Some(ClipboardContent::Image {
                data: output.stdout,
                mime,
            });
        }
    }

    let output = std::process::Command::new("wl-paste")
        .arg("--no-newline")
        .output()
        .ok()?;
    if output.status.success() {
        return Some(ClipboardContent::Text(
            String::from_utf8_lossy(&output.stdout).into_owned(),
        ));
    }
    None
}

fn read_clipboard_x11() -> Option<ClipboardContent> {
    let types = std::process::Command::new("xclip")
        .args(["-selection", "clipboard", "-t", "TARGETS", "-o"])
        .output()
        .ok()?;
    if !types.status.success() {
        return None;
    }

    let types = String::from_utf8_lossy(&types.stdout);
    if let Some(mime) = clipboard_image_mime(&types) {
        let output = std::process::Command::new("xclip")
            .args(["-selection", "clipboard", "-t", &mime, "-o"])
            .output()
            .ok()?;
        if output.status.success() && !output.stdout.is_empty() {
            return Some(ClipboardContent::Image {
                data: output.stdout,
                mime,
            });
        }
    }

    let output = std::process::Command::new("xclip")
        .args(["-selection", "clipboard", "-o"])
        .output()
        .ok()?;
    if output.status.success() {
        return Some(ClipboardContent::Text(
            String::from_utf8_lossy(&output.stdout).into_owned(),
        ));
    }
    None
}

/// Put image bytes on the system clipboard so Claude Code can ingest
/// them via its own Ctrl+V image paste.
pub fn copy_image_to_clipboard(data: &[u8], mime: &str) -> anyhow::Result<()> {
    use std::io::Write;
    // On WSL, hand the image to the Windows clipboard so the harness's
    // own Ctrl+V image paste can ingest it.
    if is_wsl() && copy_image_to_clipboard_wsl(data).is_ok() {
        return Ok(());
    }
    if let Ok(mut child) = std::process::Command::new("wl-copy")
        .args(["--type", mime])
        .stdin(std::process::Stdio::piped())
        .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(data);
        }
        let status = child.wait()?;
        if status.success() {
            return Ok(());
        }
    }
    if let Ok(mut child) = std::process::Command::new("xclip")
        .args(["-selection", "clipboard", "-t", mime])
        .stdin(std::process::Stdio::piped())
        .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(data);
        }
        let status = child.wait()?;
        if status.success() {
            return Ok(());
        }
    }
    Err(anyhow::anyhow!(
        "No clipboard utility found (wl-copy or xclip)"
    ))
}

/// Read the Windows clipboard from WSL via `powershell.exe`. A single
/// invocation prefers an image (e.g. a screenshot), which it saves to a
/// temp PNG we then read; otherwise it returns the clipboard text.
fn read_clipboard_wsl() -> Option<ClipboardContent> {
    let tmp = std::env::temp_dir().join(format!("amf-clip-{}.png", uuid::Uuid::new_v4()));
    let win_path = wsl_windows_path(&tmp)?;
    let script = format!(
        "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8; \
         Add-Type -AssemblyName System.Windows.Forms; \
         Add-Type -AssemblyName System.Drawing; \
         $img=[System.Windows.Forms.Clipboard]::GetImage(); \
         if($img -ne $null){{ \
           $img.Save('{path}',[System.Drawing.Imaging.ImageFormat]::Png); \
           Write-Output 'AMFIMAGE' \
         }} else {{ Write-Output 'AMFTEXT'; Get-Clipboard -Raw }}",
        path = win_path.replace('\'', "''")
    );
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-STA", "-Command", &script])
        .output()
        .ok()?;

    let result = if output.status.success() {
        let raw = String::from_utf8_lossy(&output.stdout);
        if raw.starts_with("AMFIMAGE") {
            std::fs::read(&tmp).ok().map(|data| ClipboardContent::Image {
                data,
                mime: "image/png".to_string(),
            })
        } else if let Some(rest) = raw.strip_prefix("AMFTEXT") {
            // Drop the marker line and any single trailing newline that
            // PowerShell appends, then normalise CRLF to LF.
            let text = rest
                .strip_prefix("\r\n")
                .or_else(|| rest.strip_prefix('\n'))
                .unwrap_or(rest);
            let text = text
                .strip_suffix("\r\n")
                .or_else(|| text.strip_suffix('\n'))
                .unwrap_or(text);
            Some(ClipboardContent::Text(text.replace("\r\n", "\n")))
        } else {
            None
        }
    } else {
        None
    };

    let _ = std::fs::remove_file(&tmp);
    result
}

/// Push text onto the Windows clipboard from WSL via `clip.exe`.
fn copy_to_clipboard_wsl(text: &str) -> anyhow::Result<()> {
    use std::io::Write;
    let mut child = std::process::Command::new("clip.exe")
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes())?;
    }
    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow::anyhow!("clip.exe exited with {status}"))
    }
}

/// Place image bytes on the Windows clipboard from WSL. The bytes are
/// written to a temp file that `powershell.exe` loads and sets via
/// `Clipboard::SetImage`.
fn copy_image_to_clipboard_wsl(data: &[u8]) -> anyhow::Result<()> {
    let tmp = std::env::temp_dir().join(format!("amf-clip-{}.png", uuid::Uuid::new_v4()));
    std::fs::write(&tmp, data)?;
    let win_path = wsl_windows_path(&tmp)
        .ok_or_else(|| anyhow::anyhow!("wslpath could not translate temp path"))?;
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; \
         Add-Type -AssemblyName System.Drawing; \
         $img=[System.Drawing.Image]::FromFile('{path}'); \
         [System.Windows.Forms.Clipboard]::SetImage($img); $img.Dispose()",
        path = win_path.replace('\'', "''")
    );
    let status = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-STA", "-Command", &script])
        .status();
    let _ = std::fs::remove_file(&tmp);
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(anyhow::anyhow!("powershell SetImage exited with {s}")),
        Err(e) => Err(anyhow::anyhow!("failed to launch powershell: {e}")),
    }
}
