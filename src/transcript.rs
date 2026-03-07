use anyhow::Result;
use std::path::{Path, PathBuf};

/// Finds the most recently modified `.jsonl` transcript file
/// for the given workdir in Claude Code's project directory.
///
/// Claude Code stores transcripts at
/// `~/.claude/projects/{encoded-path}/{session-id}.jsonl`
/// where the path encoding replaces all non-alphanumeric
/// chars with `-`.
pub fn find_latest_transcript(workdir: &Path) -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let encoded = encode_path(workdir);
    let projects_dir = PathBuf::from(&home)
        .join(".claude")
        .join("projects")
        .join(&encoded);

    if !projects_dir.is_dir() {
        return None;
    }

    std::fs::read_dir(&projects_dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((entry.path(), modified))
        })
        .max_by_key(|(_, modified)| *modified)
        .map(|(path, _)| path)
}

/// Reads a Claude Code JSONL transcript and exports it as
/// readable markdown.
///
/// Filters for user and assistant messages, extracting only
/// text content blocks (skipping tool_use, tool_result, and
/// thinking blocks).
pub fn export_transcript_markdown(jsonl_path: &Path) -> Result<String> {
    let content = std::fs::read_to_string(jsonl_path)?;
    let mut output = String::from(
        "# Session Transcript\n\n\
         Context from the parent session. This transcript\n\
         was exported when forking to provide continuity.\n",
    );

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let entry: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let msg_type = entry["type"].as_str().unwrap_or("");
        if msg_type != "user" && msg_type != "assistant" {
            continue;
        }

        let role = entry["message"]["role"].as_str().unwrap_or(msg_type);
        let heading = match role {
            "user" => "User",
            "assistant" => "Assistant",
            _ => continue,
        };

        let text = extract_text_content(&entry["message"]["content"]);
        if text.is_empty() {
            continue;
        }

        output.push_str(&format!("\n## {}\n\n{}\n", heading, text));
    }

    Ok(output)
}

/// Extracts text from a message content field, which can be
/// either a plain string or an array of content blocks.
fn extract_text_content(content: &serde_json::Value) -> String {
    if let Some(s) = content.as_str() {
        return s.to_string();
    }

    if let Some(blocks) = content.as_array() {
        let texts: Vec<&str> = blocks
            .iter()
            .filter(|block| block["type"].as_str() == Some("text"))
            .filter_map(|block| block["text"].as_str())
            .collect();
        return texts.join("\n\n");
    }

    String::new()
}

/// Encodes a path the same way Claude Code does:
/// replace all non-alphanumeric characters with `-`.
fn encode_path(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_path_replaces_non_alnum() {
        let p = Path::new("/home/user/my-project");
        assert_eq!(encode_path(p), "-home-user-my-project");
    }

    #[test]
    fn encode_path_preserves_alnum() {
        let p = Path::new("abc123");
        assert_eq!(encode_path(p), "abc123");
    }

    #[test]
    fn extract_text_from_string() {
        let v = serde_json::json!("hello world");
        assert_eq!(extract_text_content(&v), "hello world");
    }

    #[test]
    fn extract_text_from_blocks() {
        let v = serde_json::json!([
            {"type": "text", "text": "first"},
            {"type": "tool_use", "name": "Read"},
            {"type": "text", "text": "second"},
        ]);
        assert_eq!(extract_text_content(&v), "first\n\nsecond");
    }

    #[test]
    fn extract_text_skips_non_text_blocks() {
        let v = serde_json::json!([
            {"type": "tool_use", "name": "Read"},
            {"type": "tool_result", "content": "ok"},
        ]);
        assert_eq!(extract_text_content(&v), "");
    }

    #[test]
    fn export_markdown_parses_jsonl() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.jsonl");
        let jsonl = r#"{"type":"user","message":{"role":"user","content":"Fix the bug"}}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"I'll fix it."}]}}
{"type":"progress","data":"stuff"}
"#;
        std::fs::write(&path, jsonl).unwrap();

        let md = export_transcript_markdown(&path).unwrap();
        assert!(md.contains("## User"));
        assert!(md.contains("Fix the bug"));
        assert!(md.contains("## Assistant"));
        assert!(md.contains("I'll fix it."));
        assert!(!md.contains("progress"));
    }
}
