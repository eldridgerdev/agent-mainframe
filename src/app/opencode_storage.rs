use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct OpencodeSidebarData {
    pub session_id: String,
    pub title: Option<String>,
    pub latest_prompt: Option<String>,
    pub status: Option<String>,
    pub last_tool: Option<String>,
    pub todo_count: Option<u64>,
    pub todo_preview: Vec<String>,
    pub pending_permission: Option<String>,
    pub last_error: Option<String>,
    pub reasoning_tokens: Option<u64>,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
    pub files: Option<u64>,
}

impl OpencodeSidebarData {
    pub fn change_summary_line(&self) -> Option<String> {
        match (self.files, self.additions, self.deletions) {
            (Some(files), Some(additions), Some(deletions)) => {
                let file_label = if files == 1 { "file" } else { "files" };
                Some(format!(
                    "Changes: {files} {file_label} · +{additions} / -{deletions}"
                ))
            }
            _ => None,
        }
    }
}

pub fn read_sidebar_data(
    workdir: &Path,
    preferred_session_id: Option<&str>,
) -> Option<OpencodeSidebarData> {
    let storage_root = dirs::data_dir()?.join("opencode").join("storage");
    let sidecar_root = workdir.join(".amf").join("opencode-sidebar");
    read_sidebar_data_from_roots(&storage_root, &sidecar_root, workdir, preferred_session_id)
}

fn read_sidebar_data_from_roots(
    storage_root: &Path,
    sidecar_root: &Path,
    workdir: &Path,
    preferred_session_id: Option<&str>,
) -> Option<OpencodeSidebarData> {
    let session = find_session(storage_root, workdir, preferred_session_id);
    let sidecar = read_sidecar(sidecar_root, preferred_session_id);

    let session_id = sidecar
        .as_ref()
        .map(|data| data.session_id.clone())
        .or_else(|| session.as_ref().map(|session| session.id.clone()))?;

    let storage_data = session.map(|session| OpencodeSidebarData {
        latest_prompt: read_latest_user_prompt(storage_root, &session.id),
        reasoning_tokens: read_reasoning_tokens(storage_root, &session.id),
        session_id: session.id,
        title: session.title,
        status: None,
        last_tool: None,
        todo_count: None,
        todo_preview: Vec::new(),
        pending_permission: None,
        last_error: None,
        additions: session.summary.as_ref().map(|summary| summary.additions),
        deletions: session.summary.as_ref().map(|summary| summary.deletions),
        files: session.summary.as_ref().map(|summary| summary.files),
    });

    let mut merged = storage_data.unwrap_or_else(|| OpencodeSidebarData {
        session_id,
        ..OpencodeSidebarData::default()
    });

    if let Some(sidecar) = sidecar {
        if sidecar.title.is_some() {
            merged.title = sidecar.title;
        }
        if sidecar.latest_prompt.is_some() {
            merged.latest_prompt = sidecar.latest_prompt;
        }
        if sidecar.status.is_some() {
            merged.status = sidecar.status;
        }
        if sidecar.last_tool.is_some() {
            merged.last_tool = sidecar.last_tool;
        }
        if sidecar.todo_count.is_some() {
            merged.todo_count = sidecar.todo_count;
        }
        if !sidecar.todo_preview.is_empty() {
            merged.todo_preview = sidecar.todo_preview;
        }
        if sidecar.pending_permission.is_some() {
            merged.pending_permission = sidecar.pending_permission;
        }
        if sidecar.last_error.is_some() {
            merged.last_error = sidecar.last_error;
        }
        if sidecar.additions.is_some() {
            merged.additions = sidecar.additions;
        }
        if sidecar.deletions.is_some() {
            merged.deletions = sidecar.deletions;
        }
        if sidecar.files.is_some() {
            merged.files = sidecar.files;
        }
    }

    Some(merged)
}

fn find_session(
    storage_root: &Path,
    workdir: &Path,
    preferred_session_id: Option<&str>,
) -> Option<OpencodeSessionRecord> {
    let session_root = storage_root.join("session");
    if !session_root.is_dir() {
        return None;
    }

    if let Some(session_id) = preferred_session_id
        && let Some(session) = walk_json_files(&session_root)
            .into_iter()
            .filter_map(|path| parse_session_file(&path))
            .find(|session| session.id == session_id)
    {
        return Some(session);
    }

    walk_json_files(&session_root)
        .into_iter()
        .filter_map(|path| parse_session_file(&path))
        .filter(|session| session.directory == workdir)
        .max_by_key(|session| session.updated)
}

fn read_latest_user_prompt(storage_root: &Path, session_id: &str) -> Option<String> {
    let message_root = storage_root.join("message").join(session_id);
    if !message_root.is_dir() {
        return None;
    }

    walk_json_files(&message_root)
        .into_iter()
        .filter_map(|path| parse_message_file(&path))
        .filter(|message| message.role == "user")
        .max_by_key(|message| message.time.created)
        .and_then(|message| {
            read_message_parts(storage_root, &message.id).or(message.summary.and_then(|summary| {
                summary
                    .title
                    .map(|title| title.trim().to_string())
                    .filter(|title| !title.is_empty())
            }))
        })
}

fn read_message_parts(storage_root: &Path, message_id: &str) -> Option<String> {
    let part_root = storage_root.join("part").join(message_id);
    if !part_root.is_dir() {
        return None;
    }

    let mut texts = Vec::new();
    for path in walk_json_files(&part_root) {
        let Ok(contents) = std::fs::read_to_string(path) else {
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

fn read_reasoning_tokens(storage_root: &Path, session_id: &str) -> Option<u64> {
    let message_root = storage_root.join("message").join(session_id);
    if !message_root.is_dir() {
        return None;
    }

    let mut total = 0u64;
    let mut found = false;

    for message_path in walk_json_files(&message_root) {
        let Some(message_id) = message_path.file_stem().and_then(|name| name.to_str()) else {
            continue;
        };
        let part_root = storage_root.join("part").join(message_id);
        if !part_root.is_dir() {
            continue;
        }

        for part_path in walk_json_files(&part_root) {
            let Ok(contents) = std::fs::read_to_string(part_path) else {
                continue;
            };
            let Ok(part) = serde_json::from_str::<OpencodePart>(&contents) else {
                continue;
            };
            if part.part_type != "step-finish" {
                continue;
            }
            let Some(tokens) = part.tokens else {
                continue;
            };
            total = total.saturating_add(tokens.reasoning);
            found = true;
        }
    }

    if found { Some(total) } else { None }
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

fn parse_session_file(path: &Path) -> Option<OpencodeSessionRecord> {
    let contents = std::fs::read_to_string(path).ok()?;
    let session = serde_json::from_str::<OpencodeSessionFile>(&contents).ok()?;
    Some(OpencodeSessionRecord {
        id: session.id,
        directory: PathBuf::from(session.directory),
        updated: session.time.updated,
        title: session.title.map(|title| title.trim().to_string()),
        summary: session.summary,
    })
}

fn parse_message_file(path: &Path) -> Option<OpencodeMessage> {
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<OpencodeMessage>(&contents).ok()
}

fn read_sidecar(
    sidecar_root: &Path,
    preferred_session_id: Option<&str>,
) -> Option<OpencodeSidebarData> {
    if !sidecar_root.is_dir() {
        return None;
    }

    if let Some(session_id) = preferred_session_id {
        let path = sidecar_root.join(format!("{session_id}.json"));
        if path.exists() {
            return parse_sidecar_file(&path).map(|(_, data)| data);
        }
    }

    walk_json_files(sidecar_root)
        .into_iter()
        .filter_map(|path| parse_sidecar_file(&path))
        .max_by_key(|(updated_at, _)| *updated_at)
        .map(|(_, data)| data)
}

fn parse_sidecar_file(path: &Path) -> Option<(i64, OpencodeSidebarData)> {
    let contents = std::fs::read_to_string(path).ok()?;
    let sidecar = serde_json::from_str::<OpencodeSidebarSidecar>(&contents).ok()?;
    let updated_at = sidecar.updated_at.unwrap_or_default();
    Some((
        updated_at,
        OpencodeSidebarData {
            session_id: sidecar.session_id,
            title: None,
            latest_prompt: sidecar.latest_prompt,
            status: sidecar.status,
            last_tool: sidecar.last_tool,
            todo_count: sidecar.todo_count,
            todo_preview: sidecar.todo_preview,
            pending_permission: sidecar.pending_permission,
            last_error: sidecar.last_error,
            reasoning_tokens: None,
            additions: sidecar.additions,
            deletions: sidecar.deletions,
            files: sidecar.files,
        },
    ))
}

#[derive(Debug, Clone)]
struct OpencodeSessionRecord {
    id: String,
    directory: PathBuf,
    updated: i64,
    title: Option<String>,
    summary: Option<OpencodeSessionSummary>,
}

#[derive(Debug, Deserialize)]
struct OpencodeSessionFile {
    id: String,
    directory: String,
    time: OpencodeTime,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    summary: Option<OpencodeSessionSummary>,
}

#[derive(Debug, Deserialize)]
struct OpencodeTime {
    updated: i64,
}

#[derive(Debug, Clone, Deserialize)]
struct OpencodeSessionSummary {
    additions: u64,
    deletions: u64,
    files: u64,
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
    #[serde(default)]
    tokens: Option<OpencodePartTokens>,
}

#[derive(Debug, Deserialize)]
struct OpencodePartTokens {
    #[serde(default)]
    reasoning: u64,
}

#[derive(Debug, Deserialize)]
struct OpencodeSidebarSidecar {
    session_id: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    last_tool: Option<String>,
    #[serde(default)]
    latest_prompt: Option<String>,
    #[serde(default)]
    todo_count: Option<u64>,
    #[serde(default)]
    todo_preview: Vec<String>,
    #[serde(default)]
    pending_permission: Option<String>,
    #[serde(default)]
    last_error: Option<String>,
    #[serde(default)]
    additions: Option<u64>,
    #[serde(default)]
    deletions: Option<u64>,
    #[serde(default)]
    files: Option<u64>,
    #[serde(default, deserialize_with = "deserialize_optional_timestamp")]
    updated_at: Option<i64>,
}

fn deserialize_optional_timestamp<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    let parsed = match value {
        Some(serde_json::Value::String(text)) => chrono::DateTime::parse_from_rfc3339(&text)
            .ok()
            .map(|dt| dt.timestamp()),
        Some(serde_json::Value::Number(num)) => num.as_i64(),
        _ => None,
    };
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn reads_sidebar_data_for_latest_matching_workdir_session() {
        let temp = TempDir::new().unwrap();
        let workdir = PathBuf::from("/tmp/opencode-sidebar");
        let storage = temp.path();

        std::fs::create_dir_all(storage.join("session").join("project-a")).unwrap();
        std::fs::create_dir_all(storage.join("session").join("project-b")).unwrap();
        std::fs::create_dir_all(storage.join("message").join("ses-old")).unwrap();
        std::fs::create_dir_all(storage.join("message").join("ses-new")).unwrap();
        std::fs::create_dir_all(storage.join("part").join("msg-old")).unwrap();
        std::fs::create_dir_all(storage.join("part").join("msg-new")).unwrap();

        std::fs::write(
            storage.join("session").join("project-a").join("ses-old.json"),
            format!(
                "{{\"id\":\"ses-old\",\"directory\":\"{}\",\"title\":\"Older\",\"time\":{{\"updated\":10}},\"summary\":{{\"additions\":1,\"deletions\":2,\"files\":3}}}}",
                workdir.display()
            ),
        )
        .unwrap();
        std::fs::write(
            storage.join("session").join("project-b").join("ses-new.json"),
            format!(
                "{{\"id\":\"ses-new\",\"directory\":\"{}\",\"title\":\"Newest\",\"time\":{{\"updated\":20}},\"summary\":{{\"additions\":4,\"deletions\":5,\"files\":6}}}}",
                workdir.display()
            ),
        )
        .unwrap();
        std::fs::write(
            storage.join("message").join("ses-old").join("msg-old.json"),
            "{\"id\":\"msg-old\",\"role\":\"user\",\"time\":{\"created\":1}}",
        )
        .unwrap();
        std::fs::write(
            storage.join("message").join("ses-new").join("msg-new.json"),
            "{\"id\":\"msg-new\",\"role\":\"user\",\"time\":{\"created\":2}}",
        )
        .unwrap();
        std::fs::write(
            storage.join("part").join("msg-old").join("prt-1.json"),
            "{\"type\":\"text\",\"text\":\"older prompt\"}",
        )
        .unwrap();
        std::fs::write(
            storage.join("part").join("msg-new").join("prt-1.json"),
            "{\"type\":\"text\",\"text\":\"latest prompt\"}",
        )
        .unwrap();
        std::fs::write(
            storage.join("part").join("msg-new").join("prt-2.json"),
            "{\"type\":\"step-finish\",\"tokens\":{\"reasoning\":9}}",
        )
        .unwrap();

        let sidecar_root = temp.path().join("sidecar");
        let data = read_sidebar_data_from_roots(storage, &sidecar_root, &workdir, None).unwrap();
        assert_eq!(data.session_id, "ses-new");
        assert_eq!(data.title.as_deref(), Some("Newest"));
        assert_eq!(data.latest_prompt.as_deref(), Some("latest prompt"));
        assert_eq!(data.reasoning_tokens, Some(9));
        assert_eq!(
            data.change_summary_line().as_deref(),
            Some("Changes: 6 files · +4 / -5")
        );
    }

    #[test]
    fn prefers_explicit_session_id_for_sidebar_data() {
        let temp = TempDir::new().unwrap();
        let workdir = PathBuf::from("/tmp/opencode-sidebar");
        let storage = temp.path();

        std::fs::create_dir_all(storage.join("session").join("project-a")).unwrap();
        std::fs::create_dir_all(storage.join("message").join("ses-picked")).unwrap();
        std::fs::create_dir_all(storage.join("part").join("msg-picked")).unwrap();

        std::fs::write(
            storage.join("session").join("project-a").join("ses-picked.json"),
            "{\"id\":\"ses-picked\",\"directory\":\"/some/other/workdir\",\"title\":\"Picked\",\"time\":{\"updated\":1},\"summary\":{\"additions\":7,\"deletions\":1,\"files\":2}}",
        )
        .unwrap();
        std::fs::write(
            storage
                .join("message")
                .join("ses-picked")
                .join("msg-picked.json"),
            "{\"id\":\"msg-picked\",\"role\":\"user\",\"time\":{\"created\":3}}",
        )
        .unwrap();
        std::fs::write(
            storage.join("part").join("msg-picked").join("prt-1.json"),
            "{\"type\":\"text\",\"text\":\"picked prompt\"}",
        )
        .unwrap();

        let sidecar_root = temp.path().join("sidecar");
        let data =
            read_sidebar_data_from_roots(storage, &sidecar_root, &workdir, Some("ses-picked"))
                .unwrap();
        assert_eq!(data.session_id, "ses-picked");
        assert_eq!(data.title.as_deref(), Some("Picked"));
        assert_eq!(data.latest_prompt.as_deref(), Some("picked prompt"));
    }

    #[test]
    fn falls_back_to_message_summary_when_text_parts_are_missing() {
        let temp = TempDir::new().unwrap();
        let workdir = PathBuf::from("/tmp/opencode-sidebar");
        let storage = temp.path();

        std::fs::create_dir_all(storage.join("session").join("project-a")).unwrap();
        std::fs::create_dir_all(storage.join("message").join("ses-1")).unwrap();

        std::fs::write(
            storage.join("session").join("project-a").join("ses-1.json"),
            format!(
                "{{\"id\":\"ses-1\",\"directory\":\"{}\",\"time\":{{\"updated\":1}}}}",
                workdir.display()
            ),
        )
        .unwrap();
        std::fs::write(
            storage.join("message").join("ses-1").join("msg-1.json"),
            "{\"id\":\"msg-1\",\"role\":\"user\",\"time\":{\"created\":1},\"summary\":{\"title\":\"summary prompt\"}}",
        )
        .unwrap();

        let sidecar_root = temp.path().join("sidecar");
        let data = read_sidebar_data_from_roots(storage, &sidecar_root, &workdir, None).unwrap();
        assert_eq!(data.latest_prompt.as_deref(), Some("summary prompt"));
    }

    #[test]
    fn sidecar_data_overrides_storage_when_present() {
        let temp = TempDir::new().unwrap();
        let workdir = PathBuf::from("/tmp/opencode-sidebar");
        let storage = temp.path().join("storage");
        let sidecar = temp.path().join("sidecar");

        std::fs::create_dir_all(storage.join("session").join("project-a")).unwrap();
        std::fs::create_dir_all(storage.join("message").join("ses-1")).unwrap();
        std::fs::create_dir_all(storage.join("part").join("msg-1")).unwrap();
        std::fs::create_dir_all(&sidecar).unwrap();

        std::fs::write(
            storage.join("session").join("project-a").join("ses-1.json"),
            format!(
                "{{\"id\":\"ses-1\",\"directory\":\"{}\",\"title\":\"Stored\",\"time\":{{\"updated\":1}},\"summary\":{{\"additions\":4,\"deletions\":1,\"files\":2}}}}",
                workdir.display()
            ),
        )
        .unwrap();
        std::fs::write(
            storage.join("message").join("ses-1").join("msg-1.json"),
            "{\"id\":\"msg-1\",\"role\":\"user\",\"time\":{\"created\":1}}",
        )
        .unwrap();
        std::fs::write(
            storage.join("part").join("msg-1").join("prt-1.json"),
            "{\"type\":\"text\",\"text\":\"stored prompt\"}",
        )
        .unwrap();
        std::fs::write(
            sidecar.join("ses-1.json"),
            "{\"session_id\":\"ses-1\",\"status\":\"busy\",\"last_tool\":\"edit\",\"latest_prompt\":\"live prompt\",\"todo_count\":3,\"pending_permission\":\"edit\",\"last_error\":\"patch failed\",\"additions\":8,\"deletions\":2,\"files\":5,\"updated_at\":\"2026-03-25T12:00:00Z\"}",
        )
        .unwrap();

        let data =
            read_sidebar_data_from_roots(&storage, &sidecar, &workdir, Some("ses-1")).unwrap();
        assert_eq!(data.title.as_deref(), Some("Stored"));
        assert_eq!(data.latest_prompt.as_deref(), Some("live prompt"));
        assert_eq!(data.status.as_deref(), Some("busy"));
        assert_eq!(data.last_tool.as_deref(), Some("edit"));
        assert_eq!(data.todo_count, Some(3));
        assert!(data.todo_preview.is_empty());
        assert_eq!(data.pending_permission.as_deref(), Some("edit"));
        assert_eq!(data.last_error.as_deref(), Some("patch failed"));
        assert_eq!(data.additions, Some(8));
        assert_eq!(data.files, Some(5));
    }

    #[test]
    fn parses_todo_preview_from_sidecar() {
        let temp = TempDir::new().unwrap();
        let sidecar = temp.path().join("sidecar");
        std::fs::create_dir_all(&sidecar).unwrap();
        std::fs::write(
            sidecar.join("ses-1.json"),
            "{\"session_id\":\"ses-1\",\"todo_count\":3,\"todo_preview\":[\"finish parser\",\"wire UI\",\"add tests\"],\"updated_at\":\"2026-03-25T12:00:00Z\"}",
        )
        .unwrap();

        let data = read_sidecar(&sidecar, Some("ses-1")).unwrap();
        assert_eq!(data.todo_count, Some(3));
        assert_eq!(
            data.todo_preview,
            vec![
                "finish parser".to_string(),
                "wire UI".to_string(),
                "add tests".to_string()
            ]
        );
    }
}
