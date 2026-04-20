use anyhow::Result;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use super::session_titles::clean_title_from_text;

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

fn is_real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false)
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
            if let Some(content) = entry["message"]["content"].as_str() {
                if let Some(title) = clean_title_from_text(content) {
                    return Some(title);
                }
            }

            if let Some(blocks) = entry["message"]["content"].as_array() {
                for block in blocks {
                    if block["type"] == "text" {
                        if let Some(text) = block["text"].as_str() {
                            if let Some(title) = clean_title_from_text(text) {
                                return Some(title);
                            }
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
            concat!(
                "{\"type\":\"user\",\"message\":{\"content\":\"# AGENTS.md instructions for /tmp/repo\\n<INSTRUCTIONS>\\nkeep\\n</INSTRUCTIONS>\\n<environment_context>\\n  <cwd>/tmp/repo</cwd>\\n</environment_context>\\nactual request\"}}\n"
            ),
        )
        .unwrap();

        assert_eq!(
            extract_session_title(&path).as_deref(),
            Some("actual request")
        );
    }
}
