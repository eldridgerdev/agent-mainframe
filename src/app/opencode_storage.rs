use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct OpencodeSidebarData {
    pub session_id: String,
    pub title: Option<String>,
    pub latest_prompt: Option<String>,
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
    read_sidebar_data_from_root(&storage_root, workdir, preferred_session_id)
}

fn read_sidebar_data_from_root(
    storage_root: &Path,
    workdir: &Path,
    preferred_session_id: Option<&str>,
) -> Option<OpencodeSidebarData> {
    let session = find_session(storage_root, workdir, preferred_session_id)?;
    Some(OpencodeSidebarData {
        latest_prompt: read_latest_user_prompt(storage_root, &session.id),
        reasoning_tokens: read_reasoning_tokens(storage_root, &session.id),
        session_id: session.id,
        title: session.title,
        additions: session.summary.as_ref().map(|summary| summary.additions),
        deletions: session.summary.as_ref().map(|summary| summary.deletions),
        files: session.summary.as_ref().map(|summary| summary.files),
    })
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

        let data = read_sidebar_data_from_root(storage, &workdir, None).unwrap();
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

        let data = read_sidebar_data_from_root(storage, &workdir, Some("ses-picked")).unwrap();
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

        let data = read_sidebar_data_from_root(storage, &workdir, None).unwrap();
        assert_eq!(data.latest_prompt.as_deref(), Some("summary prompt"));
    }
}
