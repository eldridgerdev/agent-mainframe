use crate::project::SessionKind;
use crate::worktree::WorktreeManager;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct PromptEntry {
    pub text: String,
    pub timestamp: Option<i64>, // unix seconds
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
    latest_prompt_fallback_paths(workdir)
        .into_iter()
        .find_map(|path| std::fs::read_to_string(path).ok())
        .or_else(|| {
            super::codex_sessions::latest_prompt_for_workdir(workdir)
                .ok()
                .flatten()
        })
}

pub fn read_all_prompts(workdir: &Path) -> Vec<PromptEntry> {
    let mut entries = read_prompts_from_claude_sessions(workdir);

    // Fall back to the latest prompt file if no session entries found.
    if entries.is_empty() {
        if let Some(prompt) = read_latest_prompt_entry(workdir) {
            entries.push(prompt);
        }
    }

    sort_prompt_entries(&mut entries);

    entries
}

pub fn read_all_prompts_for_session(
    workdir: &Path,
    session_kind: &SessionKind,
    session_id: Option<&str>,
) -> Vec<PromptEntry> {
    if *session_kind == SessionKind::Codex
        && let Some(session_id) = session_id
    {
        let mut entries = super::codex_sessions::prompt_history_for_session_id(workdir, session_id)
            .unwrap_or_default()
            .into_iter()
            .map(|entry| PromptEntry {
                text: entry.text,
                timestamp: entry.timestamp,
            })
            .collect::<Vec<_>>();

        if !entries.is_empty() {
            sort_prompt_entries(&mut entries);
            return entries;
        }
    }

    read_all_prompts(workdir)
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

fn latest_prompt_fallback_paths(workdir: &Path) -> [PathBuf; 2] {
    [
        latest_prompt_path(workdir),
        workdir.join(".codex").join("latest-prompt.txt"),
    ]
}

fn read_latest_prompt_entry(workdir: &Path) -> Option<PromptEntry> {
    for path in latest_prompt_fallback_paths(workdir) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if text.trim().is_empty() {
            continue;
        }

        let timestamp = std::fs::metadata(&path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64);

        return Some(PromptEntry { text, timestamp });
    }

    None
}

fn sort_prompt_entries(entries: &mut [PromptEntry]) {
    entries.sort_by(|a, b| match (b.timestamp, a.timestamp) {
        (Some(bt), Some(at)) => bt.cmp(&at),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
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
