use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::project::SessionKind;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum TokenUsageProvider {
    Claude,
    Opencode,
    Codex,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct TokenUsageSource {
    pub provider: TokenUsageProvider,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTokenUsage {
    pub source: TokenUsageSource,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub reasoning_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone)]
pub struct SessionTokenTracker {
    home_dir: Option<PathBuf>,
    data_dir: Option<PathBuf>,
    codex_sessions: Option<(Instant, Vec<CodexSessionRecord>)>,
    opencode_sessions: Option<(Instant, Vec<OpencodeSessionMeta>)>,
    usage_cache: HashMap<TokenUsageSource, (Instant, Option<SessionTokenUsage>)>,
}

impl Default for SessionTokenTracker {
    fn default() -> Self {
        Self {
            home_dir: dirs::home_dir(),
            data_dir: dirs::data_dir(),
            codex_sessions: None,
            opencode_sessions: None,
            usage_cache: HashMap::new(),
        }
    }
}

impl SessionTokenTracker {
    pub fn new(home_dir: Option<PathBuf>, data_dir: Option<PathBuf>) -> Self {
        Self {
            home_dir,
            data_dir,
            codex_sessions: None,
            opencode_sessions: None,
            usage_cache: HashMap::new(),
        }
    }

    pub fn discover_source(
        &mut self,
        session_kind: &SessionKind,
        workdir: &Path,
        created_at: DateTime<Utc>,
    ) -> Option<TokenUsageSource> {
        match provider_for_session_kind(session_kind)? {
            TokenUsageProvider::Claude => self.discover_claude_source(workdir, created_at),
            TokenUsageProvider::Opencode => self.discover_opencode_source(workdir, created_at),
            TokenUsageProvider::Codex => self.discover_codex_source(workdir, created_at),
        }
    }

    pub fn read_usage(
        &mut self,
        source: &TokenUsageSource,
        workdir: &Path,
    ) -> Option<SessionTokenUsage> {
        if let Some((refreshed_at, cached)) = self.usage_cache.get(source)
            && refreshed_at.elapsed().as_secs() < 30
        {
            return cached.clone();
        }

        let usage = match source.provider {
            TokenUsageProvider::Claude => self.read_claude_usage(workdir, &source.id),
            TokenUsageProvider::Opencode => self.read_opencode_usage(&source.id),
            TokenUsageProvider::Codex => self.read_codex_usage(&source.id),
        };
        self.usage_cache
            .insert(source.clone(), (Instant::now(), usage.clone()));
        usage
    }

    fn discover_claude_source(
        &self,
        workdir: &Path,
        created_at: DateTime<Utc>,
    ) -> Option<TokenUsageSource> {
        let projects_dir = self.claude_projects_dir(workdir)?;
        let threshold = created_at.timestamp_millis() - 120_000;
        let mut newest_match: Option<(String, i64)> = None;
        let mut newest_any: Option<(String, i64)> = None;

        for entry in std::fs::read_dir(projects_dir).ok()? {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                continue;
            }

            let id = path.file_stem()?.to_str()?.to_string();
            let modified = file_modified_millis(&path)?;
            if newest_any.as_ref().is_none_or(|(_, ts)| modified > *ts) {
                newest_any = Some((id.clone(), modified));
            }
            if modified >= threshold
                && newest_match
                    .as_ref()
                    .is_none_or(|(_, ts)| modified > *ts)
            {
                newest_match = Some((id, modified));
            }
        }

        newest_match.or(newest_any).map(|(id, _)| TokenUsageSource {
            provider: TokenUsageProvider::Claude,
            id,
        })
    }

    fn read_claude_usage(&self, workdir: &Path, session_id: &str) -> Option<SessionTokenUsage> {
        let projects_dir = self.claude_projects_dir(workdir)?;
        let root = projects_dir.join(format!("{session_id}.jsonl"));
        if !root.exists() {
            return None;
        }

        let mut input_tokens = 0u64;
        let mut output_tokens = 0u64;
        let mut cache_read_tokens = 0u64;
        let mut cache_write_tokens = 0u64;
        let mut seen_requests = HashSet::new();
        let mut paths = vec![root];
        let subagents_dir = projects_dir.join(session_id).join("subagents");
        collect_jsonl_files(&subagents_dir, &mut paths);

        for path in paths {
            let Ok(contents) = std::fs::read_to_string(path) else {
                continue;
            };
            for line in contents.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
                    continue;
                };
                let usage = value.get("message").and_then(|message| message.get("usage"));
                let Some(usage) = usage else {
                    continue;
                };

                let dedupe_key = value
                    .get("requestId")
                    .and_then(|v| v.as_str())
                    .map(|request_id| format!("request:{request_id}"))
                    .or_else(|| {
                        value
                            .get("message")
                            .and_then(|message| message.get("id"))
                            .and_then(|v| v.as_str())
                            .map(|message_id| format!("message:{message_id}"))
                    })
                    .or_else(|| {
                        value
                            .get("uuid")
                            .and_then(|v| v.as_str())
                            .map(|uuid| format!("uuid:{uuid}"))
                    });

                let Some(dedupe_key) = dedupe_key else {
                    continue;
                };
                if !seen_requests.insert(dedupe_key) {
                    continue;
                }

                input_tokens =
                    input_tokens.saturating_add(json_u64(usage, "input_tokens"));
                output_tokens = output_tokens.saturating_add(json_u64(usage, "output_tokens"));
                cache_read_tokens =
                    cache_read_tokens.saturating_add(json_u64(usage, "cache_read_input_tokens"));
                cache_write_tokens = cache_write_tokens
                    .saturating_add(json_u64(usage, "cache_creation_input_tokens"));
            }
        }

        let total_tokens = input_tokens
            .saturating_add(output_tokens)
            .saturating_add(cache_read_tokens)
            .saturating_add(cache_write_tokens);

        Some(SessionTokenUsage {
            source: TokenUsageSource {
                provider: TokenUsageProvider::Claude,
                id: session_id.to_string(),
            },
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            reasoning_tokens: 0,
            total_tokens,
        })
    }

    fn discover_codex_source(
        &mut self,
        workdir: &Path,
        created_at: DateTime<Utc>,
    ) -> Option<TokenUsageSource> {
        let threshold = created_at.timestamp();
        let mut newest_match: Option<(String, i64)> = None;
        let mut newest_any: Option<(String, i64)> = None;

        for meta in self.codex_sessions() {
            if meta.cwd != workdir {
                continue;
            }

            if newest_any
                .as_ref()
                .is_none_or(|(_, updated)| meta.updated > *updated)
            {
                newest_any = Some((meta.id.clone(), meta.updated));
            }
            if meta.updated >= threshold
                && newest_match
                    .as_ref()
                    .is_none_or(|(_, updated)| meta.updated > *updated)
            {
                newest_match = Some((meta.id.clone(), meta.updated));
            }
        }

        newest_match.or(newest_any).map(|(id, _)| TokenUsageSource {
            provider: TokenUsageProvider::Codex,
            id,
        })
    }

    fn read_codex_usage(&mut self, session_id: &str) -> Option<SessionTokenUsage> {
        let session_path = self
            .codex_sessions()
            .iter()
            .find(|record| record.id == session_id)
            .map(|record| record.path.clone())
            .or_else(|| {
                self.refresh_codex_sessions();
                self.codex_sessions()
                    .iter()
                    .find(|record| record.id == session_id)
                    .map(|record| record.path.clone())
            })?;
        let contents = std::fs::read_to_string(session_path).ok()?;

        let mut latest_total: Option<CodexUsageSnapshot> = None;
        let mut accumulated_last = CodexUsageSnapshot::default();

        for line in contents.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(event) = serde_json::from_str::<CodexSessionEvent>(trimmed) else {
                continue;
            };
            if event.event_type != "event_msg" {
                continue;
            }
            let Some(payload) = event.payload else {
                continue;
            };
            if payload.payload_type.as_deref() != Some("token_count") {
                continue;
            }

            let Some(info) = payload.info else {
                continue;
            };

            if let Some(total) = info.total_token_usage {
                latest_total = Some(total.into());
            } else if let Some(last) = info.last_token_usage {
                accumulated_last.saturating_add_assign(&last.into());
            }
        }

        let snapshot = latest_total.unwrap_or(accumulated_last);
        Some(SessionTokenUsage {
            source: TokenUsageSource {
                provider: TokenUsageProvider::Codex,
                id: session_id.to_string(),
            },
            input_tokens: snapshot.input_tokens,
            output_tokens: snapshot.output_tokens,
            cache_read_tokens: snapshot.cache_read_tokens,
            cache_write_tokens: 0,
            reasoning_tokens: snapshot.reasoning_tokens,
            total_tokens: snapshot.total_tokens,
        })
    }

    fn discover_opencode_source(
        &mut self,
        workdir: &Path,
        created_at: DateTime<Utc>,
    ) -> Option<TokenUsageSource> {
        let threshold = created_at.timestamp_millis() - 120_000;
        let mut newest_match: Option<(String, i64)> = None;
        let mut newest_any: Option<(String, i64)> = None;

        for meta in self.opencode_sessions() {
            if meta.directory != workdir {
                continue;
            }

            if newest_any
                .as_ref()
                .is_none_or(|(_, updated)| meta.updated > *updated)
            {
                newest_any = Some((meta.id.clone(), meta.updated));
            }
            if meta.updated >= threshold
                && newest_match
                    .as_ref()
                    .is_none_or(|(_, updated)| meta.updated > *updated)
            {
                newest_match = Some((meta.id.clone(), meta.updated));
            }
        }

        newest_match.or(newest_any).map(|(id, _)| TokenUsageSource {
            provider: TokenUsageProvider::Opencode,
            id,
        })
    }

    fn read_opencode_usage(&self, session_id: &str) -> Option<SessionTokenUsage> {
        let storage_root = self.opencode_storage_root()?;
        let message_root = storage_root.join("message").join(session_id);
        let part_root = storage_root.join("part");
        if !message_root.is_dir() {
            return None;
        }

        let mut input_tokens = 0u64;
        let mut output_tokens = 0u64;
        let mut cache_read_tokens = 0u64;
        let mut cache_write_tokens = 0u64;
        let mut reasoning_tokens = 0u64;

        for message_path in walk_files_with_extension(&message_root, "json") {
            let Some(message_id) = message_path.file_stem().and_then(|name| name.to_str()) else {
                continue;
            };
            let parts_dir = part_root.join(message_id);
            if !parts_dir.is_dir() {
                continue;
            }

            for part_path in walk_files_with_extension(&parts_dir, "json") {
                let Ok(contents) = std::fs::read_to_string(part_path) else {
                    continue;
                };
                let Ok(part) = serde_json::from_str::<OpencodePart>(&contents) else {
                    continue;
                };
                if part.session_id != session_id || part.part_type != "step-finish" {
                    continue;
                }
                let Some(tokens) = part.tokens else {
                    continue;
                };

                input_tokens = input_tokens.saturating_add(tokens.input);
                output_tokens = output_tokens.saturating_add(tokens.output);
                reasoning_tokens = reasoning_tokens.saturating_add(tokens.reasoning);
                cache_read_tokens = cache_read_tokens
                    .saturating_add(tokens.cache.as_ref().map_or(0, |cache| cache.read));
                cache_write_tokens = cache_write_tokens
                    .saturating_add(tokens.cache.as_ref().map_or(0, |cache| cache.write));
            }
        }

        Some(SessionTokenUsage {
            source: TokenUsageSource {
                provider: TokenUsageProvider::Opencode,
                id: session_id.to_string(),
            },
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            reasoning_tokens,
            total_tokens: input_tokens
                .saturating_add(output_tokens)
                .saturating_add(cache_read_tokens)
                .saturating_add(cache_write_tokens)
                .saturating_add(reasoning_tokens),
        })
    }

    fn claude_projects_dir(&self, workdir: &Path) -> Option<PathBuf> {
        self.home_dir
            .as_ref()
            .map(|home| home.join(".claude").join("projects").join(encode_path(workdir)))
    }

    fn codex_sessions_root(&self) -> Option<PathBuf> {
        self.home_dir
            .as_ref()
            .map(|home| home.join(".codex").join("sessions"))
    }

    fn opencode_storage_root(&self) -> Option<PathBuf> {
        self.data_dir
            .as_ref()
            .map(|data| data.join("opencode").join("storage"))
    }

    fn codex_sessions(&mut self) -> &[CodexSessionRecord] {
        if self
            .codex_sessions
            .as_ref()
            .is_none_or(|(loaded_at, _)| loaded_at.elapsed().as_secs() >= 30)
        {
            self.refresh_codex_sessions();
        }
        self.codex_sessions
            .as_ref()
            .map(|(_, sessions)| sessions.as_slice())
            .unwrap_or(&[])
    }

    fn opencode_sessions(&mut self) -> &[OpencodeSessionMeta] {
        if self
            .opencode_sessions
            .as_ref()
            .is_none_or(|(loaded_at, _)| loaded_at.elapsed().as_secs() >= 30)
        {
            self.refresh_opencode_sessions();
        }
        self.opencode_sessions
            .as_ref()
            .map(|(_, sessions)| sessions.as_slice())
            .unwrap_or(&[])
    }

    fn refresh_codex_sessions(&mut self) {
        let sessions = self
            .codex_sessions_root()
            .map(|root| {
                walk_files_with_extension(&root, "jsonl")
                    .into_iter()
                    .filter_map(|path| parse_codex_session_meta(&path).map(|meta| (path, meta)))
                    .map(|(path, meta)| CodexSessionRecord {
                        id: meta.id,
                        cwd: meta.cwd,
                        updated: meta.updated,
                        path,
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        self.codex_sessions = Some((Instant::now(), sessions));
    }

    fn refresh_opencode_sessions(&mut self) {
        let sessions = self
            .opencode_storage_root()
            .map(|root| {
                walk_files_with_extension(&root.join("session"), "json")
                    .into_iter()
                    .filter_map(|path| parse_opencode_session_meta(&path))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        self.opencode_sessions = Some((Instant::now(), sessions));
    }
}

pub fn provider_for_session_kind(session_kind: &SessionKind) -> Option<TokenUsageProvider> {
    match session_kind {
        SessionKind::Claude => Some(TokenUsageProvider::Claude),
        SessionKind::Opencode => Some(TokenUsageProvider::Opencode),
        SessionKind::Codex => Some(TokenUsageProvider::Codex),
        _ => None,
    }
}

/// Per-model token pricing in USD per million tokens.
/// Defaults to Claude Sonnet pricing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TokenPricingConfig {
    /// USD per million input tokens
    pub input_per_mtok: f64,
    /// USD per million output tokens
    pub output_per_mtok: f64,
    /// USD per million reasoning tokens (defaults to same as output)
    pub reasoning_per_mtok: f64,
    /// USD per million cache-read tokens
    pub cache_read_per_mtok: f64,
    /// USD per million cache-write tokens
    pub cache_write_per_mtok: f64,
    /// Whether to show the dollar cost in the session status line.
    /// Set to false to hide the cost display.
    #[serde(default = "default_true")]
    pub show_cost: bool,
}

fn default_true() -> bool {
    true
}

impl Default for TokenPricingConfig {
    fn default() -> Self {
        // Claude Sonnet 4.x pricing
        Self {
            input_per_mtok: 3.0,
            output_per_mtok: 15.0,
            reasoning_per_mtok: 15.0,
            cache_read_per_mtok: 0.30,
            cache_write_per_mtok: 3.75,
            show_cost: true,
        }
    }
}

impl TokenPricingConfig {
    fn cost_usd(&self, usage: &SessionTokenUsage) -> f64 {
        (usage.input_tokens as f64 * self.input_per_mtok
            + usage.output_tokens as f64 * self.output_per_mtok
            + usage.reasoning_tokens as f64 * self.reasoning_per_mtok
            + usage.cache_read_tokens as f64 * self.cache_read_per_mtok
            + usage.cache_write_tokens as f64 * self.cache_write_per_mtok)
            / 1_000_000.0
    }

    /// Cost-equivalent input tokens: dollar cost normalized to input token price,
    /// giving a single token count on a common scale across token types.
    fn cost_equivalent_tokens(&self, usage: &SessionTokenUsage) -> u64 {
        if self.input_per_mtok == 0.0 {
            return usage.total_tokens;
        }
        let cost = self.cost_usd(usage);
        (cost / self.input_per_mtok * 1_000_000.0).round() as u64
    }
}

pub fn format_token_usage(usage: &SessionTokenUsage, pricing: &TokenPricingConfig) -> String {
    let total_in = usage
        .input_tokens
        .saturating_add(usage.cache_read_tokens)
        .saturating_add(usage.cache_write_tokens);
    let equiv = pricing.cost_equivalent_tokens(usage);
    let base = format!(
        "{} in · {} out · {} eff",
        format_token_count(total_in),
        format_token_count(usage.output_tokens),
        format_token_count(equiv),
    );
    if pricing.show_cost {
        format!("{} · {}", base, format_dollar_cost(pricing.cost_usd(usage)))
    } else {
        base
    }
}

fn format_dollar_cost(usd: f64) -> String {
    if usd < 0.005 {
        "<$0.01".to_string()
    } else if usd < 10.0 {
        format!("${:.2}", usd)
    } else {
        format!("${:.1}", usd)
    }
}

pub fn format_token_count(tokens: u64) -> String {
    match tokens {
        0..=999 => tokens.to_string(),
        1_000..=999_999 => format!("{:.1}k", tokens as f64 / 1_000.0),
        1_000_000..=999_999_999 => format!("{:.1}M", tokens as f64 / 1_000_000.0),
        _ => format!("{:.1}B", tokens as f64 / 1_000_000_000.0),
    }
}

fn collect_jsonl_files(dir: &Path, out: &mut Vec<PathBuf>) {
    if !dir.is_dir() {
        return;
    }

    for path in walk_files_with_extension(dir, "jsonl") {
        out.push(path);
    }
}

fn walk_files_with_extension(root: &Path, extension: &str) -> Vec<PathBuf> {
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
            if path.extension().and_then(|ext| ext.to_str()) == Some(extension) {
                files.push(path);
            }
        }
    }
    files
}

fn file_modified_millis(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let elapsed = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(elapsed.as_millis() as i64)
}

fn json_u64(value: &Value, field: &str) -> u64 {
    value.get(field).and_then(|v| v.as_u64()).unwrap_or(0)
}

fn encode_path(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

#[derive(Debug)]
struct CodexSessionMeta {
    id: String,
    cwd: PathBuf,
    updated: i64,
}

#[derive(Debug, Clone)]
struct CodexSessionRecord {
    id: String,
    cwd: PathBuf,
    updated: i64,
    path: PathBuf,
}

fn parse_codex_session_meta(path: &Path) -> Option<CodexSessionMeta> {
    let contents = std::fs::read_to_string(path).ok()?;
    let mut id: Option<String> = None;
    let mut cwd: Option<PathBuf> = None;
    let mut updated = 0i64;

    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let event: CodexSessionEvent = serde_json::from_str(trimmed).ok()?;
        if let Some(ts) = event.timestamp.as_deref().and_then(parse_rfc3339_seconds) {
            updated = updated.max(ts);
        }
        if event.event_type != "session_meta" {
            continue;
        }

        let payload = event.payload?;
        id = payload.id.clone().or(id);
        cwd = payload.cwd.clone().or(cwd);
    }

    Some(CodexSessionMeta {
        id: id?,
        cwd: cwd?,
        updated,
    })
}

#[derive(Debug, Deserialize)]
struct CodexSessionEvent {
    timestamp: Option<String>,
    #[serde(rename = "type")]
    event_type: String,
    payload: Option<CodexEventPayload>,
}

#[derive(Debug, Deserialize)]
struct CodexEventPayload {
    #[serde(rename = "type")]
    payload_type: Option<String>,
    id: Option<String>,
    cwd: Option<PathBuf>,
    #[serde(rename = "info")]
    info: Option<CodexTokenInfo>,
}

#[derive(Debug, Deserialize)]
struct CodexTokenInfo {
    total_token_usage: Option<CodexTokenUsage>,
    last_token_usage: Option<CodexTokenUsage>,
}

#[derive(Debug, Deserialize)]
struct CodexTokenUsage {
    input_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    reasoning_output_tokens: Option<u64>,
    total_tokens: Option<u64>,
}

#[derive(Debug, Default)]
struct CodexUsageSnapshot {
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    reasoning_tokens: u64,
    total_tokens: u64,
}

impl CodexUsageSnapshot {
    fn saturating_add_assign(&mut self, other: &Self) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.cache_read_tokens = self.cache_read_tokens.saturating_add(other.cache_read_tokens);
        self.reasoning_tokens = self.reasoning_tokens.saturating_add(other.reasoning_tokens);
        self.total_tokens = self.total_tokens.saturating_add(other.total_tokens);
    }
}

impl From<CodexTokenUsage> for CodexUsageSnapshot {
    fn from(value: CodexTokenUsage) -> Self {
        Self {
            input_tokens: value.input_tokens.unwrap_or(0),
            output_tokens: value.output_tokens.unwrap_or(0),
            cache_read_tokens: value.cached_input_tokens.unwrap_or(0),
            reasoning_tokens: value.reasoning_output_tokens.unwrap_or(0),
            total_tokens: value.total_tokens.unwrap_or(0),
        }
    }
}

#[derive(Debug, Clone)]
struct OpencodeSessionMeta {
    id: String,
    directory: PathBuf,
    updated: i64,
}

fn parse_opencode_session_meta(path: &Path) -> Option<OpencodeSessionMeta> {
    let session: OpencodeSessionFile = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    Some(OpencodeSessionMeta {
        id: session.id,
        directory: PathBuf::from(session.directory),
        updated: session.time.updated,
    })
}

#[derive(Debug, Deserialize)]
struct OpencodeSessionFile {
    id: String,
    directory: String,
    time: OpencodeTime,
}

#[derive(Debug, Deserialize)]
struct OpencodeTime {
    updated: i64,
}

#[derive(Debug, Deserialize)]
struct OpencodePart {
    #[serde(rename = "sessionID")]
    session_id: String,
    #[serde(rename = "type")]
    part_type: String,
    tokens: Option<OpencodeTokens>,
}

#[derive(Debug, Deserialize)]
struct OpencodeTokens {
    input: u64,
    output: u64,
    #[serde(default)]
    reasoning: u64,
    #[serde(default)]
    cache: Option<OpencodeCache>,
}

#[derive(Debug, Deserialize)]
struct OpencodeCache {
    #[serde(default)]
    read: u64,
    #[serde(default)]
    write: u64,
}

fn parse_rfc3339_seconds(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.with_timezone(&Utc).timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use tempfile::TempDir;

    fn tracker_with_roots(home: &Path, data: &Path) -> SessionTokenTracker {
        SessionTokenTracker::new(Some(home.to_path_buf()), Some(data.to_path_buf()))
    }

    #[test]
    fn provider_for_session_kind_only_tracks_agent_sessions() {
        assert_eq!(
            provider_for_session_kind(&SessionKind::Claude),
            Some(TokenUsageProvider::Claude)
        );
        assert_eq!(
            provider_for_session_kind(&SessionKind::Opencode),
            Some(TokenUsageProvider::Opencode)
        );
        assert_eq!(
            provider_for_session_kind(&SessionKind::Codex),
            Some(TokenUsageProvider::Codex)
        );
        assert_eq!(provider_for_session_kind(&SessionKind::Terminal), None);
    }

    #[test]
    fn reads_claude_usage_and_dedupes_request_ids() {
        let home = TempDir::new().unwrap();
        let data = TempDir::new().unwrap();
        let workdir = PathBuf::from("/tmp/worktree");
        let encoded = encode_path(&workdir);
        let project_dir = home
            .path()
            .join(".claude")
            .join("projects")
            .join(encoded);
        std::fs::create_dir_all(project_dir.join("sess-1/subagents")).unwrap();
        std::fs::write(
            project_dir.join("sess-1.jsonl"),
            concat!(
                "{\"requestId\":\"req-1\",\"message\":{\"id\":\"msg-1\",\"usage\":{\"input_tokens\":10,\"output_tokens\":2,\"cache_read_input_tokens\":3,\"cache_creation_input_tokens\":1}}}\n",
                "{\"requestId\":\"req-1\",\"message\":{\"id\":\"msg-1b\",\"usage\":{\"input_tokens\":10,\"output_tokens\":2,\"cache_read_input_tokens\":3,\"cache_creation_input_tokens\":1}}}\n",
                "{\"requestId\":\"req-2\",\"message\":{\"id\":\"msg-2\",\"usage\":{\"input_tokens\":4,\"output_tokens\":1}}}\n"
            ),
        )
        .unwrap();
        std::fs::write(
            project_dir.join("sess-1/subagents/agent-a.jsonl"),
            "{\"requestId\":\"sub-1\",\"message\":{\"id\":\"sub-msg-1\",\"usage\":{\"input_tokens\":5,\"output_tokens\":2}}}\n",
        )
        .unwrap();

        let tracker = tracker_with_roots(home.path(), data.path());
        let usage = tracker.read_claude_usage(&workdir, "sess-1").unwrap();
        assert_eq!(usage.input_tokens, 19);
        assert_eq!(usage.output_tokens, 5);
        assert_eq!(usage.cache_read_tokens, 3);
        assert_eq!(usage.cache_write_tokens, 1);
        assert_eq!(usage.total_tokens, 28);
    }

    #[test]
    fn discovers_and_reads_codex_usage_from_local_logs() {
        let home = TempDir::new().unwrap();
        let data = TempDir::new().unwrap();
        let workdir = PathBuf::from("/tmp/codex-worktree");
        let session_dir = home.path().join(".codex").join("sessions").join("2026").join("03").join("13");
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(
            session_dir.join("rollout.jsonl"),
            concat!(
                "{\"timestamp\":\"2026-03-13T14:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"codex-1\",\"cwd\":\"/tmp/codex-worktree\"}}\n",
                "{\"timestamp\":\"2026-03-13T14:01:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":100,\"cached_input_tokens\":40,\"output_tokens\":7,\"reasoning_output_tokens\":3,\"total_tokens\":110}}}}\n",
                "{\"timestamp\":\"2026-03-13T14:02:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":180,\"cached_input_tokens\":70,\"output_tokens\":12,\"reasoning_output_tokens\":5,\"total_tokens\":197}}}}\n"
            ),
        )
        .unwrap();

        let mut tracker = tracker_with_roots(home.path(), data.path());
        let created_at = Utc.with_ymd_and_hms(2026, 3, 13, 13, 59, 30).unwrap();
        let source = tracker
            .discover_source(&SessionKind::Codex, &workdir, created_at)
            .unwrap();
        assert_eq!(source.id, "codex-1");

        let usage = tracker.read_usage(&source, &workdir).unwrap();
        assert_eq!(usage.input_tokens, 180);
        assert_eq!(usage.cache_read_tokens, 70);
        assert_eq!(usage.output_tokens, 12);
        assert_eq!(usage.reasoning_tokens, 5);
        assert_eq!(usage.total_tokens, 197);
    }

    #[test]
    fn discovers_and_reads_opencode_usage_from_storage() {
        let home = TempDir::new().unwrap();
        let data = TempDir::new().unwrap();
        let workdir = PathBuf::from("/tmp/opencode-worktree");
        let storage = data.path().join("opencode").join("storage");
        let session_dir = storage.join("session").join("project-1");
        let message_dir = storage.join("message").join("ses-1");
        let part_dir = storage.join("part").join("msg-1");
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::create_dir_all(&message_dir).unwrap();
        std::fs::create_dir_all(&part_dir).unwrap();
        std::fs::write(
            session_dir.join("ses-1.json"),
            "{\"id\":\"ses-1\",\"directory\":\"/tmp/opencode-worktree\",\"time\":{\"updated\":1773439500000}}",
        )
        .unwrap();
        std::fs::write(
            message_dir.join("msg-1.json"),
            "{\"id\":\"msg-1\",\"sessionID\":\"ses-1\"}",
        )
        .unwrap();
        std::fs::write(
            part_dir.join("part-1.json"),
            "{\"sessionID\":\"ses-1\",\"type\":\"step-finish\",\"tokens\":{\"input\":50,\"output\":4,\"reasoning\":3,\"cache\":{\"read\":7,\"write\":2}}}",
        )
        .unwrap();

        let mut tracker = tracker_with_roots(home.path(), data.path());
        let created_at = Utc.with_ymd_and_hms(2026, 3, 13, 14, 4, 0).unwrap();
        let source = tracker
            .discover_source(&SessionKind::Opencode, &workdir, created_at)
            .unwrap();
        assert_eq!(source.id, "ses-1");

        let usage = tracker.read_usage(&source, &workdir).unwrap();
        assert_eq!(usage.input_tokens, 50);
        assert_eq!(usage.output_tokens, 4);
        assert_eq!(usage.reasoning_tokens, 3);
        assert_eq!(usage.cache_read_tokens, 7);
        assert_eq!(usage.cache_write_tokens, 2);
        assert_eq!(usage.total_tokens, 66);
    }

    #[test]
    fn formats_token_counts_compactly() {
        assert_eq!(format_token_count(987), "987");
        assert_eq!(format_token_count(12_340), "12.3k");
        assert_eq!(format_token_count(9_500_000), "9.5M");
    }

    #[test]
    fn formats_token_usage_with_pricing() {
        let pricing = TokenPricingConfig::default();
        // 10k input, 2k output, 5k cache_read, 1k cache_write
        // cost = (10000*3 + 2000*15 + 5000*0.30 + 1000*3.75) / 1_000_000
        //      = (30000 + 30000 + 1500 + 3750) / 1_000_000 = 0.06525
        // cost_equiv = 0.06525 / 3.0 * 1_000_000 = 21750
        let usage = SessionTokenUsage {
            source: TokenUsageSource {
                provider: TokenUsageProvider::Claude,
                id: "test".to_string(),
            },
            input_tokens: 10_000,
            output_tokens: 2_000,
            cache_read_tokens: 5_000,
            cache_write_tokens: 1_000,
            reasoning_tokens: 0,
            total_tokens: 18_000,
        };
        let formatted = format_token_usage(&usage, &pricing);
        assert_eq!(formatted, "16.0k in · 2.0k out · 21.8k eff · $0.07");
    }

    #[test]
    fn formats_dollar_cost_ranges() {
        assert_eq!(format_dollar_cost(0.0), "<$0.01");
        assert_eq!(format_dollar_cost(0.004), "<$0.01");
        assert_eq!(format_dollar_cost(0.005), "$0.01");
        assert_eq!(format_dollar_cost(1.234), "$1.23");
        assert_eq!(format_dollar_cost(9.99), "$9.99");
        assert_eq!(format_dollar_cost(12.5), "$12.5");
    }
}
