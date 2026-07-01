use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Upper bound on the dedupe-key tail kept in incremental parse state.
/// Duplicate request ids appear on adjacent or near-adjacent transcript
/// lines, so a bounded recent tail is sufficient and keeps the state
/// (and its DB serialization) from growing with session length.
const DEDUPE_TAIL_CAP: usize = 1024;

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

/// Entry exchanged between `SessionTokenTracker` and the DB cache table.
pub struct DbTokenCacheEntry {
    pub source: TokenUsageSource,
    pub signature: Option<u64>,
    pub usage: Option<SessionTokenUsage>,
    pub parse_state: Option<TranscriptParseState>,
}

/// Incremental parse state for append-only JSONL transcripts. On each
/// refresh only bytes past the per-file offset are read and parsed; a
/// full reparse happens only when a file shrinks (truncation/rotation).
/// Persisted in the DB token cache so restarts stay cheap.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TranscriptParseState {
    /// Bytes parsed so far per transcript file (through the last
    /// complete line).
    #[serde(default)]
    pub offsets: HashMap<PathBuf, u64>,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cache_write_tokens: u64,
    /// Recent dedupe keys (bounded tail, oldest first).
    #[serde(default)]
    pub recent_dedupe: VecDeque<String>,
    #[serde(default)]
    pub codex_latest_total: Option<CodexUsageSnapshot>,
    #[serde(default)]
    pub codex_accumulated: CodexUsageSnapshot,
}

#[derive(Debug, Clone)]
struct UsageCacheEntry {
    refreshed_at: Instant,
    signature: Option<u64>,
    usage: Option<SessionTokenUsage>,
    parse_state: TranscriptParseState,
}

#[derive(Debug, Clone)]
pub struct SessionTokenTracker {
    home_dir: Option<PathBuf>,
    data_dir: Option<PathBuf>,
    codex_sessions: Option<(Instant, Vec<CodexSessionRecord>)>,
    opencode_sessions: Option<(Instant, Vec<OpencodeSessionMeta>)>,
    usage_cache: HashMap<TokenUsageSource, UsageCacheEntry>,
    /// Set when a cache entry actually changed since the last DB flush.
    dirty: bool,
}

impl Default for SessionTokenTracker {
    fn default() -> Self {
        Self {
            home_dir: dirs::home_dir(),
            data_dir: dirs::data_dir(),
            codex_sessions: None,
            opencode_sessions: None,
            usage_cache: HashMap::new(),
            dirty: false,
        }
    }
}

impl SessionTokenTracker {
    #[allow(dead_code)] // exercised only by unit tests
    pub fn new(home_dir: Option<PathBuf>, data_dir: Option<PathBuf>) -> Self {
        Self {
            home_dir,
            data_dir,
            codex_sessions: None,
            opencode_sessions: None,
            usage_cache: HashMap::new(),
            dirty: false,
        }
    }

    /// Returns whether any cache entry changed since the last call, and
    /// resets the flag. Lets the DB flush skip unchanged syncs.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// Pre-populate the in-memory cache from DB-persisted entries.
    /// Each entry is inserted with a stale `refreshed_at` so the first
    /// access after startup re-validates the signature without doing a
    /// full file re-read when the signature is unchanged.
    pub fn seed_from_db_cache(&mut self, entries: Vec<DbTokenCacheEntry>) {
        let stale = Instant::now()
            .checked_sub(Duration::from_secs(31))
            .unwrap_or_else(Instant::now);
        for entry in entries {
            self.usage_cache.insert(
                entry.source,
                UsageCacheEntry {
                    refreshed_at: stale,
                    signature: entry.signature,
                    usage: entry.usage,
                    parse_state: entry.parse_state.unwrap_or_default(),
                },
            );
        }
    }

    /// Snapshot all current cache entries for flushing to DB.
    pub fn all_cache_entries(&self) -> Vec<DbTokenCacheEntry> {
        self.usage_cache
            .iter()
            .map(|(source, entry)| DbTokenCacheEntry {
                source: source.clone(),
                signature: entry.signature,
                usage: entry.usage.clone(),
                parse_state: Some(entry.parse_state.clone()),
            })
            .collect()
    }

    pub fn discover_source(
        &mut self,
        session_kind: &SessionKind,
        workdir: &Path,
        created_at: DateTime<Utc>,
    ) -> Option<TokenUsageSource> {
        self.discover_source_excluding(session_kind, workdir, created_at, &HashSet::new())
    }

    pub fn discover_source_excluding(
        &mut self,
        session_kind: &SessionKind,
        workdir: &Path,
        created_at: DateTime<Utc>,
        excluded_ids: &HashSet<String>,
    ) -> Option<TokenUsageSource> {
        match provider_for_session_kind(session_kind)? {
            TokenUsageProvider::Claude => {
                self.discover_claude_source(workdir, created_at, excluded_ids)
            }
            TokenUsageProvider::Opencode => {
                self.discover_opencode_source(workdir, created_at, excluded_ids)
            }
            TokenUsageProvider::Codex => {
                self.discover_codex_source(workdir, created_at, excluded_ids)
            }
        }
    }

    pub fn read_usage(
        &mut self,
        source: &TokenUsageSource,
        workdir: &Path,
    ) -> Option<SessionTokenUsage> {
        if let Some(entry) = self.usage_cache.get(source)
            && entry.refreshed_at.elapsed().as_secs() < 30
        {
            return entry.usage.clone();
        }

        let current_signature = self.current_usage_signature(source, workdir);
        if let Some(entry) = self.usage_cache.get_mut(source)
            && current_signature.is_some()
            && entry.signature == current_signature
        {
            entry.refreshed_at = Instant::now();
            return entry.usage.clone();
        }

        // Take the incremental state out so the readers can resume from
        // the last parsed offset instead of re-reading whole transcripts.
        let mut parse_state = self
            .usage_cache
            .get_mut(source)
            .map(|entry| std::mem::take(&mut entry.parse_state))
            .unwrap_or_default();

        let usage = match source.provider {
            TokenUsageProvider::Claude => {
                self.read_claude_usage(workdir, &source.id, &mut parse_state)
            }
            TokenUsageProvider::Opencode => self.read_opencode_usage(&source.id),
            TokenUsageProvider::Codex => {
                self.read_codex_usage(workdir, &source.id, &mut parse_state)
            }
        };
        self.usage_cache.insert(
            source.clone(),
            UsageCacheEntry {
                refreshed_at: Instant::now(),
                signature: current_signature,
                usage: usage.clone(),
                parse_state,
            },
        );
        self.dirty = true;
        usage
    }

    fn current_usage_signature(
        &mut self,
        source: &TokenUsageSource,
        workdir: &Path,
    ) -> Option<u64> {
        match source.provider {
            TokenUsageProvider::Claude => self.claude_usage_signature(workdir, &source.id),
            TokenUsageProvider::Opencode => self.opencode_usage_signature(&source.id),
            TokenUsageProvider::Codex => self.codex_usage_signature(workdir, &source.id),
        }
    }

    fn discover_claude_source(
        &self,
        workdir: &Path,
        created_at: DateTime<Utc>,
        excluded_ids: &HashSet<String>,
    ) -> Option<TokenUsageSource> {
        let projects_dir = self.claude_projects_dir(workdir)?;
        if !is_real_dir(&projects_dir) {
            return None;
        }
        let threshold = created_at.timestamp_millis() - 120_000;
        let created_at_millis = created_at.timestamp_millis();
        let mut closest_match: Option<(String, u64)> = None;
        let mut newest_any: Option<(String, i64)> = None;

        for entry in std::fs::read_dir(projects_dir).ok()? {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                continue;
            }

            let id = path.file_stem()?.to_str()?.to_string();
            if excluded_ids.contains(&id) {
                continue;
            }
            let modified = file_modified_millis(&path)?;
            if newest_any.as_ref().is_none_or(|(_, ts)| modified > *ts) {
                newest_any = Some((id.clone(), modified));
            }
            let distance = modified.abs_diff(created_at_millis);
            if modified >= threshold
                && closest_match
                    .as_ref()
                    .is_none_or(|(_, current)| distance < *current)
            {
                closest_match = Some((id, distance));
            }
        }

        closest_match
            .or(newest_any.map(|(id, _)| (id, 0)))
            .map(|(id, _)| TokenUsageSource {
                provider: TokenUsageProvider::Claude,
                id,
            })
    }

    fn read_claude_usage(
        &self,
        workdir: &Path,
        session_id: &str,
        state: &mut TranscriptParseState,
    ) -> Option<SessionTokenUsage> {
        let projects_dir = self.claude_projects_dir(workdir)?;
        let root = projects_dir.join(format!("{session_id}.jsonl"));
        if !root.exists() {
            return None;
        }

        let mut paths = vec![root];
        let subagents_dir = projects_dir.join(session_id).join("subagents");
        collect_jsonl_files(&subagents_dir, &mut paths);

        // A shrunken or vanished file means the offsets no longer
        // describe the content — recompute everything from scratch.
        let needs_reset = state.offsets.iter().any(|(path, &offset)| {
            std::fs::metadata(path)
                .map(|metadata| metadata.len() < offset)
                .unwrap_or(true)
        });
        if needs_reset {
            *state = TranscriptParseState::default();
        }

        let mut seen_requests: HashSet<String> = state.recent_dedupe.iter().cloned().collect();

        for path in paths {
            let offset = state.offsets.get(&path).copied().unwrap_or(0);
            let Ok((new_offset, bytes)) = read_appended_lines(&path, offset) else {
                continue;
            };

            for line in bytes.split(|&byte| byte == b'\n') {
                let Ok(value) = serde_json::from_slice::<Value>(line) else {
                    continue;
                };
                let usage = value
                    .get("message")
                    .and_then(|message| message.get("usage"));
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
                if !seen_requests.insert(dedupe_key.clone()) {
                    continue;
                }
                state.recent_dedupe.push_back(dedupe_key);
                if state.recent_dedupe.len() > DEDUPE_TAIL_CAP {
                    state.recent_dedupe.pop_front();
                }

                state.input_tokens = state
                    .input_tokens
                    .saturating_add(json_u64(usage, "input_tokens"));
                state.output_tokens = state
                    .output_tokens
                    .saturating_add(json_u64(usage, "output_tokens"));
                state.cache_read_tokens = state
                    .cache_read_tokens
                    .saturating_add(json_u64(usage, "cache_read_input_tokens"));
                state.cache_write_tokens = state
                    .cache_write_tokens
                    .saturating_add(json_u64(usage, "cache_creation_input_tokens"));
            }

            state.offsets.insert(path, new_offset);
        }

        let total_tokens = state
            .input_tokens
            .saturating_add(state.output_tokens)
            .saturating_add(state.cache_read_tokens)
            .saturating_add(state.cache_write_tokens);

        Some(SessionTokenUsage {
            source: TokenUsageSource {
                provider: TokenUsageProvider::Claude,
                id: session_id.to_string(),
            },
            input_tokens: state.input_tokens,
            output_tokens: state.output_tokens,
            cache_read_tokens: state.cache_read_tokens,
            cache_write_tokens: state.cache_write_tokens,
            reasoning_tokens: 0,
            total_tokens,
        })
    }

    fn claude_usage_signature(&self, workdir: &Path, session_id: &str) -> Option<u64> {
        let paths = self.claude_usage_paths(workdir, session_id)?;
        metadata_signature_for_paths(paths)
    }

    fn claude_usage_paths(&self, workdir: &Path, session_id: &str) -> Option<Vec<PathBuf>> {
        let projects_dir = self.claude_projects_dir(workdir)?;
        let root = projects_dir.join(format!("{session_id}.jsonl"));
        if !root.exists() {
            return None;
        }

        let mut paths = vec![root];
        let subagents_dir = projects_dir.join(session_id).join("subagents");
        collect_jsonl_files(&subagents_dir, &mut paths);
        Some(paths)
    }

    fn discover_codex_source(
        &mut self,
        workdir: &Path,
        created_at: DateTime<Utc>,
        excluded_ids: &HashSet<String>,
    ) -> Option<TokenUsageSource> {
        let threshold = created_at.timestamp();
        let created_at_seconds = created_at.timestamp();
        let mut closest_match: Option<(String, u64)> = None;
        let mut newest_any: Option<(String, i64)> = None;

        let sessions = self
            .home_dir
            .as_deref()
            .and_then(|home| crate::codex::indexed_sessions_in(home, workdir))
            .map(|sessions| {
                sessions
                    .into_iter()
                    .map(|session| (session.id, session.updated_at))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| {
                self.codex_sessions()
                    .iter()
                    .filter(|meta| meta.cwd == workdir)
                    .map(|meta| (meta.id.clone(), meta.updated))
                    .collect()
            });

        for (id, updated) in sessions {
            if excluded_ids.contains(&id) {
                continue;
            }
            if newest_any
                .as_ref()
                .is_none_or(|(_, current)| updated > *current)
            {
                newest_any = Some((id.clone(), updated));
            }
            let distance = updated.abs_diff(created_at_seconds);
            if updated >= threshold
                && closest_match
                    .as_ref()
                    .is_none_or(|(_, current)| distance < *current)
            {
                closest_match = Some((id, distance));
            }
        }

        closest_match
            .or(newest_any.map(|(id, _)| (id, 0)))
            .map(|(id, _)| TokenUsageSource {
                provider: TokenUsageProvider::Codex,
                id,
            })
    }

    fn read_codex_usage(
        &mut self,
        workdir: &Path,
        session_id: &str,
        state: &mut TranscriptParseState,
    ) -> Option<SessionTokenUsage> {
        let session_path = self.codex_session_path(workdir, session_id)?;

        let offset = state.offsets.get(&session_path).copied().unwrap_or(0);
        let shrunk = std::fs::metadata(&session_path)
            .map(|metadata| metadata.len() < offset)
            .unwrap_or(true);
        if shrunk {
            *state = TranscriptParseState::default();
        }

        let offset = state.offsets.get(&session_path).copied().unwrap_or(0);
        if let Ok((new_offset, bytes)) = read_appended_lines(&session_path, offset) {
            for line in bytes.split(|&byte| byte == b'\n') {
                let Ok(event) = serde_json::from_slice::<CodexSessionEvent>(line) else {
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
                    state.codex_latest_total = Some(total.into());
                } else if let Some(last) = info.last_token_usage {
                    state.codex_accumulated.saturating_add_assign(&last.into());
                }
            }
            state.offsets.insert(session_path, new_offset);
        }

        let snapshot = state
            .codex_latest_total
            .clone()
            .unwrap_or_else(|| state.codex_accumulated.clone());
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

    fn codex_usage_signature(&mut self, workdir: &Path, session_id: &str) -> Option<u64> {
        metadata_signature_for_paths(vec![self.codex_session_path(workdir, session_id)?])
    }

    fn codex_session_path(&mut self, workdir: &Path, session_id: &str) -> Option<PathBuf> {
        self.home_dir
            .as_deref()
            .and_then(|home| crate::codex::indexed_session_in(home, workdir, session_id))
            .map(|session| session.rollout_path)
            .or_else(|| {
                self.codex_sessions()
                    .iter()
                    .find(|record| record.id == session_id && record.cwd == workdir)
                    .map(|record| record.path.clone())
            })
            .or_else(|| {
                self.refresh_codex_sessions();
                self.codex_sessions()
                    .iter()
                    .find(|record| record.id == session_id && record.cwd == workdir)
                    .map(|record| record.path.clone())
            })
    }

    fn discover_opencode_source(
        &mut self,
        workdir: &Path,
        created_at: DateTime<Utc>,
        excluded_ids: &HashSet<String>,
    ) -> Option<TokenUsageSource> {
        let threshold = created_at.timestamp_millis() - 120_000;
        let created_at_millis = created_at.timestamp_millis();
        let mut closest_match: Option<(String, u64)> = None;
        let mut newest_any: Option<(String, i64)> = None;

        for meta in self.opencode_sessions() {
            if meta.directory != workdir {
                continue;
            }
            if excluded_ids.contains(&meta.id) {
                continue;
            }

            if newest_any
                .as_ref()
                .is_none_or(|(_, updated)| meta.updated > *updated)
            {
                newest_any = Some((meta.id.clone(), meta.updated));
            }
            let distance = meta.updated.abs_diff(created_at_millis);
            if meta.updated >= threshold
                && closest_match
                    .as_ref()
                    .is_none_or(|(_, updated)| distance < *updated)
            {
                closest_match = Some((meta.id.clone(), distance));
            }
        }

        closest_match
            .or(newest_any.map(|(id, _)| (id, 0)))
            .map(|(id, _)| TokenUsageSource {
                provider: TokenUsageProvider::Opencode,
                id,
            })
    }

    fn read_opencode_usage(&self, session_id: &str) -> Option<SessionTokenUsage> {
        let storage_root = self.opencode_storage_root()?;
        let message_root = storage_root.join("message").join(session_id);
        let part_root = storage_root.join("part");
        if !is_real_dir(&message_root) {
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
            if !is_real_dir(&parts_dir) {
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

    fn opencode_usage_signature(&self, session_id: &str) -> Option<u64> {
        let paths = self.opencode_usage_paths(session_id)?;
        metadata_signature_for_paths(paths)
    }

    fn opencode_usage_paths(&self, session_id: &str) -> Option<Vec<PathBuf>> {
        let storage_root = self.opencode_storage_root()?;
        let message_root = storage_root.join("message").join(session_id);
        let part_root = storage_root.join("part");
        if !is_real_dir(&message_root) {
            return None;
        }

        let mut paths = walk_files_with_extension(&message_root, "json");
        if paths.is_empty() {
            return Some(paths);
        }

        let message_ids = paths
            .iter()
            .filter_map(|path| path.file_stem().and_then(|name| name.to_str()))
            .map(str::to_string)
            .collect::<Vec<_>>();
        for message_id in message_ids {
            let parts_dir = part_root.join(message_id);
            if is_real_dir(&parts_dir) {
                paths.extend(walk_files_with_extension(&parts_dir, "json"));
            }
        }

        Some(paths)
    }

    fn claude_projects_dir(&self, workdir: &Path) -> Option<PathBuf> {
        self.home_dir.as_ref().map(|home| {
            home.join(".claude")
                .join("projects")
                .join(encode_path(workdir))
        })
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

/// Read complete lines appended past `offset`. Returns the new offset
/// (end of the last complete line) and those lines' bytes. A trailing
/// partial line (mid-append, no newline yet) is left for the next call
/// so a line is never half-parsed or skipped.
pub(crate) fn read_appended_lines(path: &Path, offset: u64) -> std::io::Result<(u64, Vec<u8>)> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    if len <= offset {
        return Ok((offset, Vec::new()));
    }

    file.seek(SeekFrom::Start(offset))?;
    let mut buf = Vec::with_capacity((len - offset) as usize);
    file.take(len - offset).read_to_end(&mut buf)?;

    match buf.iter().rposition(|&byte| byte == b'\n') {
        Some(pos) => {
            let parsed_through = offset + pos as u64 + 1;
            buf.truncate(pos + 1);
            Ok((parsed_through, buf))
        }
        None => Ok((offset, Vec::new())),
    }
}

fn collect_jsonl_files(dir: &Path, out: &mut Vec<PathBuf>) {
    if !is_real_dir(dir) {
        return;
    }

    for path in walk_files_with_extension(dir, "jsonl") {
        out.push(path);
    }
}

fn walk_files_with_extension(root: &Path, extension: &str) -> Vec<PathBuf> {
    if !is_real_dir(root) {
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

fn is_real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false)
}

fn metadata_signature_for_paths<I>(paths: I) -> Option<u64>
where
    I: IntoIterator<Item = PathBuf>,
{
    let mut paths = paths.into_iter().collect::<Vec<_>>();
    if paths.is_empty() {
        return Some(0);
    }

    paths.sort();
    let mut hasher = DefaultHasher::new();
    for path in paths {
        path.hash(&mut hasher);
        let metadata = std::fs::metadata(&path).ok()?;
        metadata.len().hash(&mut hasher);
        metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos())
            .hash(&mut hasher);
    }
    Some(hasher.finish())
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
    // Use file mtime for `updated` — avoids reading the whole file just for
    // timestamps and is accurate enough for "newest session" comparisons.
    let updated = file_modified_millis(path).map(|ms| ms / 1000).unwrap_or(0);

    let contents = std::fs::read_to_string(path).ok()?;
    let mut id: Option<String> = None;
    let mut cwd: Option<PathBuf> = None;

    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let Ok(event) = serde_json::from_str::<CodexSessionEvent>(trimmed) else {
            continue;
        };
        if event.event_type != "session_meta" {
            continue;
        }

        let payload = event.payload?;
        id = payload.id.clone().or(id);
        cwd = payload.cwd.clone().or(cwd);

        // session_meta is typically near the top; stop once we have both fields.
        if id.is_some() && cwd.is_some() {
            break;
        }
    }

    Some(CodexSessionMeta {
        id: id?,
        cwd: cwd?,
        updated,
    })
}

#[derive(Debug, Deserialize)]
struct CodexSessionEvent {
    #[allow(dead_code)] // deserialized but not read yet
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CodexUsageSnapshot {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_tokens: u64,
    #[serde(default)]
    reasoning_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
}

impl CodexUsageSnapshot {
    fn saturating_add_assign(&mut self, other: &Self) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(other.cache_read_tokens);
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
    let session: OpencodeSessionFile =
        serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
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
        let project_dir = home.path().join(".claude").join("projects").join(encoded);
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
        let mut state = TranscriptParseState::default();
        let usage = tracker
            .read_claude_usage(&workdir, "sess-1", &mut state)
            .unwrap();
        assert_eq!(usage.input_tokens, 19);
        assert_eq!(usage.output_tokens, 5);
        assert_eq!(usage.cache_read_tokens, 3);
        assert_eq!(usage.cache_write_tokens, 1);
        assert_eq!(usage.total_tokens, 28);
    }

    #[test]
    fn claude_usage_parses_only_appended_bytes_incrementally() {
        let home = TempDir::new().unwrap();
        let data = TempDir::new().unwrap();
        let workdir = PathBuf::from("/tmp/worktree");
        let encoded = encode_path(&workdir);
        let project_dir = home.path().join(".claude").join("projects").join(encoded);
        std::fs::create_dir_all(&project_dir).unwrap();
        let transcript = project_dir.join("sess-1.jsonl");
        std::fs::write(
            &transcript,
            "{\"requestId\":\"req-1\",\"message\":{\"usage\":{\"input_tokens\":10,\"output_tokens\":2}}}\n",
        )
        .unwrap();

        let tracker = tracker_with_roots(home.path(), data.path());
        let mut state = TranscriptParseState::default();
        let usage = tracker
            .read_claude_usage(&workdir, "sess-1", &mut state)
            .unwrap();
        assert_eq!(usage.input_tokens, 10);
        let offset_after_first = *state.offsets.get(&transcript).unwrap();
        assert!(offset_after_first > 0);

        // Append: only the new line should be parsed; the duplicate
        // request id must still be deduped across calls.
        let mut contents = std::fs::read(&transcript).unwrap();
        contents.extend_from_slice(
            b"{\"requestId\":\"req-1\",\"message\":{\"usage\":{\"input_tokens\":10,\"output_tokens\":2}}}\n",
        );
        contents.extend_from_slice(
            b"{\"requestId\":\"req-2\",\"message\":{\"usage\":{\"input_tokens\":4,\"output_tokens\":1}}}\n",
        );
        std::fs::write(&transcript, contents).unwrap();

        let usage = tracker
            .read_claude_usage(&workdir, "sess-1", &mut state)
            .unwrap();
        assert_eq!(usage.input_tokens, 14);
        assert_eq!(usage.output_tokens, 3);
        assert!(*state.offsets.get(&transcript).unwrap() > offset_after_first);

        // Truncation forces a full reparse from scratch.
        std::fs::write(
            &transcript,
            "{\"requestId\":\"req-9\",\"message\":{\"usage\":{\"input_tokens\":7,\"output_tokens\":3}}}\n",
        )
        .unwrap();
        let usage = tracker
            .read_claude_usage(&workdir, "sess-1", &mut state)
            .unwrap();
        assert_eq!(usage.input_tokens, 7);
        assert_eq!(usage.output_tokens, 3);
    }

    #[test]
    fn claude_usage_leaves_partial_trailing_line_for_next_read() {
        let home = TempDir::new().unwrap();
        let data = TempDir::new().unwrap();
        let workdir = PathBuf::from("/tmp/worktree");
        let encoded = encode_path(&workdir);
        let project_dir = home.path().join(".claude").join("projects").join(encoded);
        std::fs::create_dir_all(&project_dir).unwrap();
        let transcript = project_dir.join("sess-1.jsonl");
        let complete = "{\"requestId\":\"req-1\",\"message\":{\"usage\":{\"input_tokens\":10,\"output_tokens\":2}}}\n";
        let partial = "{\"requestId\":\"req-2\",\"message\":{\"usage\":{\"input_tok";
        std::fs::write(&transcript, format!("{complete}{partial}")).unwrap();

        let tracker = tracker_with_roots(home.path(), data.path());
        let mut state = TranscriptParseState::default();
        let usage = tracker
            .read_claude_usage(&workdir, "sess-1", &mut state)
            .unwrap();
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(
            *state.offsets.get(&transcript).unwrap(),
            complete.len() as u64
        );

        // Completing the line later picks it up in full.
        let mut contents = std::fs::read(&transcript).unwrap();
        contents.extend_from_slice(b"ens\":4,\"output_tokens\":1}}}\n");
        std::fs::write(&transcript, contents).unwrap();
        let usage = tracker
            .read_claude_usage(&workdir, "sess-1", &mut state)
            .unwrap();
        assert_eq!(usage.input_tokens, 14);
        assert_eq!(usage.output_tokens, 3);
    }

    #[test]
    fn codex_usage_resumes_from_offset_and_tracks_latest_total() {
        let home = TempDir::new().unwrap();
        let data = TempDir::new().unwrap();
        let session_dir = home
            .path()
            .join(".codex")
            .join("sessions")
            .join("2026")
            .join("03")
            .join("13");
        std::fs::create_dir_all(&session_dir).unwrap();
        let transcript = session_dir.join("rollout.jsonl");
        std::fs::write(
            &transcript,
            concat!(
                "{\"timestamp\":\"2026-03-13T14:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"codex-1\",\"cwd\":\"/tmp/codex-worktree\"}}\n",
                "{\"timestamp\":\"2026-03-13T14:01:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":100,\"output_tokens\":7,\"total_tokens\":107}}}}\n"
            ),
        )
        .unwrap();

        let mut tracker = tracker_with_roots(home.path(), data.path());
        let mut state = TranscriptParseState::default();
        let workdir = Path::new("/tmp/codex-worktree");
        let usage = tracker
            .read_codex_usage(workdir, "codex-1", &mut state)
            .unwrap();
        assert_eq!(usage.total_tokens, 107);

        let mut contents = std::fs::read(&transcript).unwrap();
        contents.extend_from_slice(
            b"{\"timestamp\":\"2026-03-13T14:02:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"total_token_usage\":{\"input_tokens\":180,\"output_tokens\":12,\"total_tokens\":192}}}}\n",
        );
        std::fs::write(&transcript, contents).unwrap();

        let usage = tracker
            .read_codex_usage(workdir, "codex-1", &mut state)
            .unwrap();
        assert_eq!(usage.input_tokens, 180);
        assert_eq!(usage.total_tokens, 192);
    }

    #[test]
    fn transcript_parse_state_round_trips_through_json() {
        let mut state = TranscriptParseState::default();
        state.offsets.insert(PathBuf::from("/tmp/a.jsonl"), 42);
        state.input_tokens = 10;
        state.recent_dedupe.push_back("request:req-1".to_string());
        state.codex_latest_total = Some(CodexUsageSnapshot {
            input_tokens: 1,
            output_tokens: 2,
            cache_read_tokens: 3,
            reasoning_tokens: 4,
            total_tokens: 10,
        });

        let json = serde_json::to_string(&state).unwrap();
        let restored: TranscriptParseState = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.offsets.get(Path::new("/tmp/a.jsonl")), Some(&42));
        assert_eq!(restored.input_tokens, 10);
        assert_eq!(restored.recent_dedupe.len(), 1);
        assert_eq!(restored.codex_latest_total.unwrap().total_tokens, 10);
    }

    #[test]
    fn discovers_and_reads_codex_usage_from_local_logs() {
        let home = TempDir::new().unwrap();
        let data = TempDir::new().unwrap();
        let workdir = PathBuf::from("/tmp/codex-worktree");
        let session_dir = home
            .path()
            .join(".codex")
            .join("sessions")
            .join("2026")
            .join("03")
            .join("13");
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
