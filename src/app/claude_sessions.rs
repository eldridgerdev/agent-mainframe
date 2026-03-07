use anyhow::Result;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ClaudeSessionInfo {
    pub id: String,
    pub title: String,
    pub updated: i64,
}

pub fn fetch_claude_sessions(workdir: &Path) -> Result<Vec<ClaudeSessionInfo>> {
    let home = std::env::var("HOME")?;
    let encoded = encode_path(workdir);
    let projects_dir = PathBuf::from(&home)
        .join(".claude")
        .join("projects")
        .join(&encoded);

    if !projects_dir.is_dir() {
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

fn extract_session_title(jsonl_path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(jsonl_path).ok()?;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let entry: serde_json::Value = serde_json::from_str(line).ok()?;

        if entry["type"] == "user" {
            if let Some(content) = entry["message"]["content"].as_str() {
                let title = content.lines().next().unwrap_or("");
                let title = if title.len() > 60 {
                    format!("{}...", &title[..57])
                } else {
                    title.to_string()
                };
                return Some(title);
            }

            if let Some(blocks) = entry["message"]["content"].as_array() {
                for block in blocks {
                    if block["type"] == "text" {
                        if let Some(text) = block["text"].as_str() {
                            let title = text.lines().next().unwrap_or("");
                            let title = if title.len() > 60 {
                                format!("{}...", &title[..57])
                            } else {
                                title.to_string()
                            };
                            return Some(title);
                        }
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
