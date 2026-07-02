use anyhow::Result;
use std::fs::File;
use std::hash::Hash;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use super::session_titles::clean_title_from_text;

#[derive(Debug, Clone)]
pub struct ClaudeSessionInfo {
    pub id: String,
    pub title: String,
    pub updated: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeSidebarMetadata {
    pub model: Option<String>,
}

pub fn fetch_claude_sessions(workdir: &Path) -> Result<Vec<ClaudeSessionInfo>> {
    let home = std::env::var("HOME")?;
    let encoded = encode_path(workdir);
    let projects_dir = PathBuf::from(&home)
        .join(".claude")
        .join("projects")
        .join(&encoded);

    if !is_real_dir(&projects_dir) {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();

    for entry in std::fs::read_dir(&projects_dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.extension().is_none_or(|ext| ext != "jsonl") {
            continue;
        }

        let session_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();

        if session_id.is_empty() {
            continue;
        }

        let metadata = entry.metadata()?;
        let modified = metadata.modified()?;
        let timestamp = modified
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let title = extract_session_title(&path).unwrap_or_else(|| "Untitled".to_string());

        sessions.push(ClaudeSessionInfo {
            id: session_id,
            title,
            updated: timestamp,
        });
    }

    sessions.sort_by(|a, b| b.updated.cmp(&a.updated));

    Ok(sessions)
}

pub fn sidebar_metadata_for_session_id(
    workdir: &Path,
    session_id: &str,
) -> Result<Option<ClaudeSidebarMetadata>> {
    let home = std::env::var("HOME")?;
    let path = PathBuf::from(&home)
        .join(".claude")
        .join("projects")
        .join(encode_path(workdir))
        .join(format!("{session_id}.jsonl"));

    if !path.is_file() {
        return Ok(None);
    }

    Ok(Some(ClaudeSidebarMetadata {
        model: extract_latest_model(&path),
    }))
}

pub(crate) fn sidebar_input_signature(workdir: &Path, session_id: Option<&str>) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    workdir.hash(&mut hasher);
    session_id.hash(&mut hasher);

    if let (Ok(home), Some(session_id)) = (std::env::var("HOME"), session_id) {
        hash_metadata(
            &mut hasher,
            PathBuf::from(home)
                .join(".claude")
                .join("projects")
                .join(encode_path(workdir))
                .join(format!("{session_id}.jsonl")),
        );
    }

    hasher.finish()
}

fn is_real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false)
}

fn extract_latest_model(jsonl_path: &Path) -> Option<String> {
    let file = File::open(jsonl_path).ok()?;
    let reader = BufReader::new(file);
    let mut latest = None;

    for line in reader.lines() {
        let line = line.ok()?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };

        if entry["type"] == "assistant"
            && let Some(model) = entry["message"]["model"]
                .as_str()
                .map(str::trim)
                .filter(|model| !model.is_empty())
        {
            latest = Some(model.to_string());
        }
    }

    latest
}

fn extract_session_title(jsonl_path: &Path) -> Option<String> {
    let file = File::open(jsonl_path).ok()?;
    let reader = BufReader::new(file);

    for line in reader.lines() {
        let line = line.ok()?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let entry: serde_json::Value = serde_json::from_str(line).ok()?;

        if entry["type"] == "user" {
            if let Some(content) = entry["message"]["content"].as_str()
                && let Some(title) = clean_title_from_text(content)
            {
                return Some(title);
            }

            if let Some(blocks) = entry["message"]["content"].as_array() {
                for block in blocks {
                    if block["type"] == "text"
                        && let Some(text) = block["text"].as_str()
                        && let Some(title) = clean_title_from_text(text)
                    {
                        return Some(title);
                    }
                }
            }
        }
    }

    None
}

fn encode_path(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn hash_metadata(hasher: &mut impl std::hash::Hasher, path: impl AsRef<Path>) {
    let path = path.as_ref();
    path.hash(hasher);
    match std::fs::metadata(path) {
        Ok(metadata) => {
            true.hash(hasher);
            metadata.len().hash(hasher);
            metadata
                .modified()
                .ok()
                .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos())
                .hash(hasher);
        }
        Err(_) => false.hash(hasher),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn extract_session_title_skips_agents_boilerplate() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("session.jsonl");
        std::fs::write(
            &path,
            "{\"type\":\"user\",\"message\":{\"content\":\"# AGENTS.md instructions for /tmp/repo\\n<INSTRUCTIONS>\\nkeep\\n</INSTRUCTIONS>\\n<environment_context>\\n  <cwd>/tmp/repo</cwd>\\n</environment_context>\\nactual request\"}}\n",
        )
        .unwrap();

        assert_eq!(
            extract_session_title(&path).as_deref(),
            Some("actual request")
        );
    }

    #[test]
    fn extracts_latest_assistant_model() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("session.jsonl");
        std::fs::write(
            &path,
            concat!(
                "{\"type\":\"assistant\",\"message\":{\"model\":\"claude-sonnet-4-6\"}}\n",
                "{\"type\":\"user\",\"message\":{\"content\":\"hi\"}}\n",
                "{\"type\":\"assistant\",\"message\":{\"model\":\"claude-opus-4-6\"}}\n"
            ),
        )
        .unwrap();

        assert_eq!(
            extract_latest_model(&path).as_deref(),
            Some("claude-opus-4-6")
        );
    }
}
