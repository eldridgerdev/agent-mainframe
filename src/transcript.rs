use anyhow::Result;
use std::path::{Path, PathBuf};

/// Resolve a specific Claude session; do not substitute a newer transcript.
pub(crate) fn session_transcript_path(workdir: &Path, session_id: &str) -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Home directory is unavailable"))?;
    session_transcript_path_in(&home.canonicalize()?, workdir, session_id)
}

pub(crate) struct CapturedContext {
    path: PathBuf,
    bytes: Vec<u8>,
    pub markdown: String,
}

impl CapturedContext {
    pub(crate) fn capture(workdir: &Path, session_id: &str) -> Result<Self> {
        Self::from_path(session_transcript_path(workdir, session_id)?)
    }

    pub(crate) fn from_path(path: PathBuf) -> Result<Self> {
        let bytes =
            crate::screenshot_evidence::read_bounded(&path, 32 * 1024 * 1024).map_err(|err| {
                anyhow::anyhow!(
                    "Cannot read selected transcript {}: {err:#}",
                    path.display()
                )
            })?;
        let text = std::str::from_utf8(&bytes)
            .map_err(|err| anyhow::anyhow!("Selected transcript is not UTF-8: {err}"))?;
        let markdown = export_transcript_text(text);
        Ok(Self {
            path,
            bytes,
            markdown,
        })
    }

    pub(crate) fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            crate::screenshot_evidence::read_bounded(&self.path, 32 * 1024 * 1024)? == self.bytes,
            "Selected transcript changed; prepare again"
        );
        Ok(())
    }
}

fn session_transcript_path_in(home: &Path, workdir: &Path, session_id: &str) -> Result<PathBuf> {
    anyhow::ensure!(
        !session_id.is_empty()
            && session_id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'),
        "Invalid Claude session ID"
    );
    Ok(home
        .join(".claude/projects")
        .join(encode_path(workdir))
        .join(format!("{session_id}.jsonl")))
}

/// Reads a Claude Code JSONL transcript and exports it as
/// readable markdown.
///
/// Filters for user and assistant messages, extracting only
/// text content blocks (skipping tool_use, tool_result, and
/// thinking blocks).
#[cfg(test)]
pub fn export_transcript_markdown(jsonl_path: &Path) -> Result<String> {
    let content = std::fs::read_to_string(jsonl_path)?;
    Ok(export_transcript_text(&content))
}

/// Render already captured transcript bytes, so a caller can bind a preview to
/// one exact session and export without re-reading a different transcript.
pub(crate) fn export_transcript_text(content: &str) -> String {
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

    output
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
    fn session_path_never_selects_another_session_or_traverses() {
        let home = Path::new("/fixture-home");
        let path =
            session_transcript_path_in(home, Path::new("/work/project"), "session-123").unwrap();
        assert_eq!(
            path,
            home.join(".claude/projects/-work-project/session-123.jsonl")
        );
        assert!(session_transcript_path_in(home, Path::new("/work/project"), "../other").is_err());
        assert!(session_transcript_path_in(home, Path::new("/work/project"), "").is_err());
    }

    #[test]
    fn captured_context_binds_selected_bytes_and_refuses_later_changes() {
        let home = tempfile::tempdir().unwrap();
        let home_path = home.path().canonicalize().unwrap();
        let workdir = Path::new("/fixture/project");
        let selected = session_transcript_path_in(&home_path, workdir, "selected").unwrap();
        std::fs::create_dir_all(selected.parent().unwrap()).unwrap();
        std::fs::write(&selected, "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"selected request\"}}\n").unwrap();
        let other = session_transcript_path_in(&home_path, workdir, "other").unwrap();
        std::fs::write(other, "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"newer unrelated request\"}}\n").unwrap();
        let captured = CapturedContext::from_path(selected.clone()).unwrap();
        assert!(captured.markdown.contains("selected request"));
        assert!(!captured.markdown.contains("newer unrelated request"));
        captured.validate().unwrap();
        std::fs::write(selected, b"changed").unwrap();
        assert!(captured.validate().is_err());
    }

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
