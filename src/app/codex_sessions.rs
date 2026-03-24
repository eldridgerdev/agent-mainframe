use anyhow::Result;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct CodexSessionInfo {
    pub id: String,
    pub title: String,
    pub updated: i64,
}

#[derive(Debug, Clone)]
struct ParsedCodexSession {
    info: CodexSessionInfo,
    latest_prompt: Option<String>,
}

pub fn fetch_codex_sessions(workdir: &Path) -> Result<Vec<CodexSessionInfo>> {
    let Some(sessions_root) = codex_sessions_root() else {
        return Ok(Vec::new());
    };
    fetch_codex_sessions_from_root(workdir, &sessions_root)
}

pub fn session_info_for_workdir(
    workdir: &Path,
    session_id: &str,
) -> Result<Option<CodexSessionInfo>> {
    let Some(sessions_root) = codex_sessions_root() else {
        return Ok(None);
    };
    session_info_for_workdir_from_root(workdir, session_id, &sessions_root)
}

fn fetch_codex_sessions_from_root(
    workdir: &Path,
    sessions_root: &Path,
) -> Result<Vec<CodexSessionInfo>> {
    if !sessions_root.is_dir() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();
    let mut stack = vec![sessions_root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                continue;
            }
            if let Some(session) = parse_codex_session_file(&path, workdir) {
                sessions.push(session);
            }
        }
    }

    sessions.sort_by(|a, b| b.updated.cmp(&a.updated));
    Ok(sessions)
}

fn session_info_for_workdir_from_root(
    workdir: &Path,
    session_id: &str,
    sessions_root: &Path,
) -> Result<Option<CodexSessionInfo>> {
    if !sessions_root.is_dir() {
        return Ok(None);
    }

    let mut stack = vec![sessions_root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(info) = parse_codex_session_file(&path, workdir) else {
                continue;
            };
            if info.id == session_id {
                return Ok(Some(info));
            }
        }
    }

    Ok(None)
}

pub fn latest_prompt_for_workdir(workdir: &Path) -> Result<Option<String>> {
    let Some(sessions_root) = codex_sessions_root() else {
        return Ok(None);
    };
    latest_prompt_for_workdir_from_root(workdir, &sessions_root)
}

pub fn latest_prompt_for_session_id(workdir: &Path, session_id: &str) -> Result<Option<String>> {
    let Some(sessions_root) = codex_sessions_root() else {
        return Ok(None);
    };
    latest_prompt_for_session_id_from_root(workdir, session_id, &sessions_root)
}

fn latest_prompt_for_workdir_from_root(
    workdir: &Path,
    sessions_root: &Path,
) -> Result<Option<String>> {
    if !sessions_root.is_dir() {
        return Ok(None);
    }

    let mut newest_prompt: Option<(i64, String)> = None;
    let mut stack = vec![sessions_root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(parsed) = parse_codex_session_file_details(&path, workdir) else {
                continue;
            };
            let Some(prompt) = parsed.latest_prompt else {
                continue;
            };
            match &newest_prompt {
                Some((updated, _)) if *updated >= parsed.info.updated => {}
                _ => newest_prompt = Some((parsed.info.updated, prompt)),
            }
        }
    }

    Ok(newest_prompt.map(|(_, prompt)| prompt))
}

fn latest_prompt_for_session_id_from_root(
    workdir: &Path,
    session_id: &str,
    sessions_root: &Path,
) -> Result<Option<String>> {
    if !sessions_root.is_dir() {
        return Ok(None);
    }

    let mut newest_prompt: Option<(i64, String)> = None;
    let mut stack = vec![sessions_root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(parsed) = parse_codex_session_file_details(&path, workdir) else {
                continue;
            };
            if parsed.info.id != session_id {
                continue;
            }
            let Some(prompt) = parsed.latest_prompt else {
                continue;
            };
            match &newest_prompt {
                Some((updated, _)) if *updated >= parsed.info.updated => {}
                _ => newest_prompt = Some((parsed.info.updated, prompt)),
            }
        }
    }

    Ok(newest_prompt.map(|(_, prompt)| prompt))
}

fn parse_codex_session_file(path: &Path, workdir: &Path) -> Option<CodexSessionInfo> {
    parse_codex_session_file_details(path, workdir).map(|parsed| parsed.info)
}

fn parse_codex_session_file_details(path: &Path, workdir: &Path) -> Option<ParsedCodexSession> {
    let contents = std::fs::read_to_string(path).ok()?;

    let mut session_id: Option<String> = None;
    let mut session_cwd: Option<PathBuf> = None;
    let mut title: Option<String> = None;
    let mut updated = 0_i64;
    let mut latest_prompt: Option<String> = None;
    let mut latest_prompt_updated = 0_i64;

    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let value: Value = serde_json::from_str(line).ok()?;
        let line_updated = value
            .get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(parse_timestamp)
            .unwrap_or(updated);

        if let Some(ts) = value.get("timestamp").and_then(|v| v.as_str())
            && let Some(parsed) = parse_timestamp(ts)
        {
            updated = updated.max(parsed);
        }

        match value.get("type").and_then(|v| v.as_str()) {
            Some("session_meta") => {
                let payload = value.get("payload")?;
                session_id = payload
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(ToOwned::to_owned)
                    .or(session_id);
                session_cwd = payload
                    .get("cwd")
                    .and_then(|v| v.as_str())
                    .map(PathBuf::from)
                    .or(session_cwd);
                if let Some(ts) = payload.get("timestamp").and_then(|v| v.as_str())
                    && let Some(parsed) = parse_timestamp(ts)
                {
                    updated = updated.max(parsed);
                }
            }
            Some("event_msg") => {
                if title.is_none() {
                    title = extract_title_from_event(&value);
                }
                if let Some(prompt) = extract_prompt_from_event(&value) {
                    latest_prompt = Some(prompt);
                    latest_prompt_updated = line_updated;
                }
            }
            Some("response_item") => {
                if title.is_none() {
                    title = extract_title_from_response_item(&value);
                }
                if let Some(prompt) = extract_prompt_from_response_item(&value)
                    && line_updated >= latest_prompt_updated
                {
                    latest_prompt = Some(prompt);
                    latest_prompt_updated = line_updated;
                }
            }
            _ => {}
        }
    }

    if session_cwd.as_deref()? != workdir {
        return None;
    }

    let id = session_id?;
    Some(ParsedCodexSession {
        info: CodexSessionInfo {
            id,
            title: title.unwrap_or_else(|| "Untitled".to_string()),
            updated,
        },
        latest_prompt,
    })
}

fn extract_title_from_event(value: &Value) -> Option<String> {
    title_from_text(&extract_prompt_from_event(value)?)
}

fn extract_title_from_response_item(value: &Value) -> Option<String> {
    title_from_text(&extract_prompt_from_response_item(value)?)
}

fn extract_prompt_from_event(value: &Value) -> Option<String> {
    let payload = value.get("payload")?;
    if payload.get("type")?.as_str()? != "user_message" {
        return None;
    }
    payload
        .get("message")
        .and_then(|v| v.as_str())
        .map(ToOwned::to_owned)
}

fn extract_prompt_from_response_item(value: &Value) -> Option<String> {
    let payload = value.get("payload")?;
    if payload.get("type")?.as_str()? != "message" {
        return None;
    }
    if payload.get("role")?.as_str()? != "user" {
        return None;
    }

    let content = payload.get("content")?.as_array()?;
    let texts: Vec<&str> = content
        .iter()
        .filter(|item| item.get("type").and_then(|v| v.as_str()) == Some("input_text"))
        .filter_map(|item| item.get("text").and_then(|v| v.as_str()))
        .collect();

    if texts.is_empty() {
        None
    } else {
        Some(texts.join("\n"))
    }
}

fn title_from_text(text: &str) -> Option<String> {
    Some(
        text.lines()
            .find(|line| !line.trim().is_empty())?
            .trim()
            .to_string(),
    )
}

fn parse_timestamp(timestamp: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|dt| dt.with_timezone(&Utc).timestamp())
}

fn codex_sessions_root() -> Option<PathBuf> {
    let from_dirs = dirs::home_dir().map(|h| h.join(".codex").join("sessions"));
    if from_dirs.as_ref().is_some_and(|p| p.exists()) {
        return from_dirs;
    }

    let from_env = std::env::var("HOME")
        .ok()
        .map(PathBuf::from)
        .map(|h| h.join(".codex").join("sessions"));
    if from_env.as_ref().is_some_and(|p| p.exists()) {
        return from_env;
    }

    let from_user_home = std::env::var("USER")
        .ok()
        .map(|u| PathBuf::from("/home").join(u))
        .map(|h| h.join(".codex").join("sessions"));
    if from_user_home.as_ref().is_some_and(|p| p.exists()) {
        return from_user_home;
    }

    from_dirs.or(from_env).or(from_user_home)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_session(root: &Path, relative: &str, contents: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn fetch_codex_sessions_filters_by_workdir_and_sorts_newest_first() {
        let tmp = TempDir::new().unwrap();
        let workdir = tmp.path().join("repo");
        let other = tmp.path().join("other");

        write_session(
            tmp.path(),
            "2026/03/07/a.jsonl",
            &format!(
                concat!(
                    "{{\"timestamp\":\"2026-03-07T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"sess-a\",\"timestamp\":\"2026-03-07T09:59:00Z\",\"cwd\":\"{}\"}}}}\n",
                    "{{\"timestamp\":\"2026-03-07T10:01:00Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"older title\"}}}}\n"
                ),
                workdir.display()
            ),
        );
        write_session(
            tmp.path(),
            "2026/03/07/b.jsonl",
            &format!(
                concat!(
                    "{{\"timestamp\":\"2026-03-07T11:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"sess-b\",\"timestamp\":\"2026-03-07T10:59:00Z\",\"cwd\":\"{}\"}}}}\n",
                    "{{\"timestamp\":\"2026-03-07T11:01:00Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"newer title\"}}}}\n"
                ),
                workdir.display()
            ),
        );
        write_session(
            tmp.path(),
            "2026/03/07/c.jsonl",
            &format!(
                concat!(
                    "{{\"timestamp\":\"2026-03-07T12:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"sess-c\",\"timestamp\":\"2026-03-07T11:59:00Z\",\"cwd\":\"{}\"}}}}\n",
                    "{{\"timestamp\":\"2026-03-07T12:01:00Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"ignored\"}}}}\n"
                ),
                other.display()
            ),
        );

        let sessions = fetch_codex_sessions_from_root(&workdir, tmp.path()).unwrap();
        let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();

        assert_eq!(ids, vec!["sess-b", "sess-a"]);
        assert_eq!(sessions[0].title, "newer title");
    }

    #[test]
    fn parse_codex_session_file_uses_response_item_as_title_fallback() {
        let tmp = TempDir::new().unwrap();
        let workdir = tmp.path().join("repo");
        let session_path = tmp.path().join("session.jsonl");

        std::fs::write(
            &session_path,
            format!(
                concat!(
                    "{{\"timestamp\":\"2026-03-07T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"sess-1\",\"timestamp\":\"2026-03-07T09:59:00Z\",\"cwd\":\"{}\"}}}}\n",
                    "{{\"timestamp\":\"2026-03-07T10:01:00Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"restore codex\\nsecond line\"}}]}}}}\n"
                ),
                workdir.display()
            ),
        )
        .unwrap();

        let session = parse_codex_session_file(&session_path, &workdir).unwrap();
        assert_eq!(session.id, "sess-1");
        assert_eq!(session.title, "restore codex");
        assert_eq!(session.updated, 1_772_877_660);
    }

    #[test]
    fn latest_prompt_for_workdir_uses_latest_matching_session_prompt() {
        let tmp = TempDir::new().unwrap();
        let workdir = tmp.path().join("repo");

        write_session(
            tmp.path(),
            "2026/03/08/older.jsonl",
            &format!(
                concat!(
                    "{{\"timestamp\":\"2026-03-08T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"sess-old\",\"timestamp\":\"2026-03-08T09:59:00Z\",\"cwd\":\"{}\"}}}}\n",
                    "{{\"timestamp\":\"2026-03-08T10:01:00Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"older prompt\"}}]}}}}\n"
                ),
                workdir.display()
            ),
        );

        write_session(
            tmp.path(),
            "2026/03/08/newer.jsonl",
            &format!(
                concat!(
                    "{{\"timestamp\":\"2026-03-08T11:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"sess-new\",\"timestamp\":\"2026-03-08T10:59:00Z\",\"cwd\":\"{}\"}}}}\n",
                    "{{\"timestamp\":\"2026-03-08T11:01:00Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"latest prompt\"}}]}}}}\n"
                ),
                workdir.display()
            ),
        );

        let prompt = latest_prompt_for_workdir_from_root(&workdir, tmp.path()).unwrap();
        assert_eq!(prompt.as_deref(), Some("latest prompt"));
    }

    #[test]
    fn latest_prompt_for_workdir_ignores_other_workdirs() {
        let tmp = TempDir::new().unwrap();
        let workdir = tmp.path().join("repo");
        let other = tmp.path().join("other");

        write_session(
            tmp.path(),
            "2026/03/08/other.jsonl",
            &format!(
                concat!(
                    "{{\"timestamp\":\"2026-03-08T11:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"sess-other\",\"timestamp\":\"2026-03-08T10:59:00Z\",\"cwd\":\"{}\"}}}}\n",
                    "{{\"timestamp\":\"2026-03-08T11:01:00Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"wrong prompt\"}}]}}}}\n"
                ),
                other.display()
            ),
        );

        let prompt = latest_prompt_for_workdir_from_root(&workdir, tmp.path()).unwrap();
        assert_eq!(prompt, None);
    }

    #[test]
    fn session_info_for_workdir_returns_matching_session() {
        let tmp = TempDir::new().unwrap();
        let workdir = tmp.path().join("repo");

        write_session(
            tmp.path(),
            "2026/03/08/current.jsonl",
            &format!(
                concat!(
                    "{{\"timestamp\":\"2026-03-08T11:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"sess-current\",\"timestamp\":\"2026-03-08T10:59:00Z\",\"cwd\":\"{}\"}}}}\n",
                    "{{\"timestamp\":\"2026-03-08T11:01:00Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"current title\"}}]}}}}\n"
                ),
                workdir.display()
            ),
        );

        let info =
            session_info_for_workdir_from_root(&workdir, "sess-current", tmp.path()).unwrap();
        assert_eq!(
            info.as_ref().map(|info| info.title.as_str()),
            Some("current title")
        );
    }

    #[test]
    fn latest_prompt_for_session_id_returns_matching_prompt() {
        let tmp = TempDir::new().unwrap();
        let workdir = tmp.path().join("repo");

        write_session(
            tmp.path(),
            "2026/03/08/other.jsonl",
            &format!(
                concat!(
                    "{{\"timestamp\":\"2026-03-08T11:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"sess-other\",\"timestamp\":\"2026-03-08T10:59:00Z\",\"cwd\":\"{}\"}}}}\n",
                    "{{\"timestamp\":\"2026-03-08T11:01:00Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"other prompt\"}}]}}}}\n"
                ),
                workdir.display()
            ),
        );

        write_session(
            tmp.path(),
            "2026/03/08/current.jsonl",
            &format!(
                concat!(
                    "{{\"timestamp\":\"2026-03-08T12:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"sess-current\",\"timestamp\":\"2026-03-08T11:59:00Z\",\"cwd\":\"{}\"}}}}\n",
                    "{{\"timestamp\":\"2026-03-08T12:01:00Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"current prompt\"}}]}}}}\n"
                ),
                workdir.display()
            ),
        );

        let prompt =
            latest_prompt_for_session_id_from_root(&workdir, "sess-current", tmp.path()).unwrap();
        assert_eq!(prompt.as_deref(), Some("current prompt"));
    }

    #[test]
    fn latest_prompt_for_session_id_prefers_newest_matching_file() {
        let tmp = TempDir::new().unwrap();
        let workdir = tmp.path().join("repo");

        write_session(
            tmp.path(),
            "2026/03/08/older.jsonl",
            &format!(
                concat!(
                    "{{\"timestamp\":\"2026-03-08T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"sess-current\",\"timestamp\":\"2026-03-08T09:59:00Z\",\"cwd\":\"{}\"}}}}\n",
                    "{{\"timestamp\":\"2026-03-08T10:01:00Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"older prompt\"}}]}}}}\n"
                ),
                workdir.display()
            ),
        );

        write_session(
            tmp.path(),
            "2026/03/08/newer.jsonl",
            &format!(
                concat!(
                    "{{\"timestamp\":\"2026-03-08T13:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"sess-current\",\"timestamp\":\"2026-03-08T12:59:00Z\",\"cwd\":\"{}\"}}}}\n",
                    "{{\"timestamp\":\"2026-03-08T13:01:00Z\",\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"newer prompt\"}}]}}}}\n"
                ),
                workdir.display()
            ),
        );

        let prompt =
            latest_prompt_for_session_id_from_root(&workdir, "sess-current", tmp.path()).unwrap();
        assert_eq!(prompt.as_deref(), Some("newer prompt"));
    }
}
