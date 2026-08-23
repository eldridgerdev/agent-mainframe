//! Harness-specific context-window telemetry collectors.

#![allow(dead_code)] // Wired into the five-second sync in the next plan task.

use crate::context_tracking::{
    ContextProvenance, ContextResetEvent, ContextResetMetadata, ContextResetReason,
    ContextUsageSample,
};
use crate::project::SessionKind;
use crate::token_tracking::SessionTokenUsage;
use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const CLAUDE_DEFAULT_CONTEXT_LIMIT: u64 = 200_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextUnavailableReason {
    UnsupportedSessionKind,
    MissingSession,
    MissingTelemetry,
    MalformedTelemetry,
    AwaitingPostResetSample,
}

/// Collector failures are data, not status-sync errors. Callers can retain a
/// previous snapshot and classify it as stale without aborting the sync pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextCollectionResult {
    Collected(ContextUsageSample),
    Unavailable(ContextUnavailableReason),
}

pub struct ContextCollectionTarget<'a> {
    pub session_kind: &'a SessionKind,
    pub workdir: &'a Path,
    pub conversation_id: Option<&'a str>,
    pub provider_id: Option<&'a str>,
    pub model_id: Option<&'a str>,
    /// Optional direct runtime JSON, such as Claude status-line input or Pi's
    /// `get_session_stats` response. File collectors remain the fallback.
    pub runtime_payload: Option<&'a Value>,
    pub fallback_usage: Option<&'a SessionTokenUsage>,
    pub fallback_context_limit: Option<u64>,
    pub collected_at: DateTime<Utc>,
}

pub trait ContextCollector {
    fn collect(&mut self, target: ContextCollectionTarget<'_>) -> ContextCollectionResult;
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ModelLimitKey {
    harness: &'static str,
    provider: String,
    model: String,
}

pub struct SessionContextCollector {
    home_dir: Option<PathBuf>,
    data_dir: Option<PathBuf>,
    cache_dir: Option<PathBuf>,
    model_limits: HashMap<ModelLimitKey, Option<u64>>,
    allow_model_commands: bool,
}

impl Default for SessionContextCollector {
    fn default() -> Self {
        Self {
            home_dir: dirs::home_dir(),
            data_dir: dirs::data_dir(),
            cache_dir: dirs::cache_dir(),
            model_limits: HashMap::new(),
            allow_model_commands: true,
        }
    }
}

impl SessionContextCollector {
    #[cfg(test)]
    fn with_roots(home: &Path, data: &Path, cache: &Path) -> Self {
        Self {
            home_dir: Some(home.to_path_buf()),
            data_dir: Some(data.to_path_buf()),
            cache_dir: Some(cache.to_path_buf()),
            model_limits: HashMap::new(),
            allow_model_commands: false,
        }
    }

    fn collect_claude(&mut self, target: &ContextCollectionTarget<'_>) -> ContextCollectionResult {
        if let Some(payload) = target.runtime_payload {
            match claude_runtime_sample(payload, target) {
                Some(sample) => return ContextCollectionResult::Collected(sample),
                None if claude_runtime_is_post_reset(payload) => {
                    return ContextCollectionResult::Unavailable(
                        ContextUnavailableReason::AwaitingPostResetSample,
                    );
                }
                None => {}
            }
        }

        let Some(home) = self.home_dir.as_deref() else {
            return self.fallback(target, FallbackFormula::Claude);
        };
        let project_dir = home
            .join(".claude")
            .join("projects")
            .join(encode_claude_path(target.workdir));
        let transcript = target
            .conversation_id
            .map(|id| project_dir.join(format!("{id}.jsonl")))
            .filter(|path| path.is_file())
            .or_else(|| newest_file_with_extension(&project_dir, "jsonl"));
        let Some(transcript) = transcript else {
            return self.fallback(target, FallbackFormula::Claude);
        };

        match parse_claude_transcript(&transcript, target) {
            Some(sample) => ContextCollectionResult::Collected(sample),
            None => self.fallback_or(
                target,
                FallbackFormula::Claude,
                ContextUnavailableReason::MalformedTelemetry,
            ),
        }
    }

    fn collect_codex(&mut self, target: &ContextCollectionTarget<'_>) -> ContextCollectionResult {
        if let Some(payload) = target.runtime_payload
            && let Some(sample) = codex_runtime_sample(payload, target)
        {
            return ContextCollectionResult::Collected(sample);
        }

        let Some(home) = self.home_dir.as_deref() else {
            return self.fallback(target, FallbackFormula::Codex);
        };
        let rollout = target
            .conversation_id
            .and_then(|id| crate::codex::indexed_session_in(home, target.workdir, id))
            .map(|session| session.rollout_path)
            .or_else(|| match target.conversation_id {
                Some(_) => None,
                None => crate::codex::indexed_sessions_in(home, target.workdir)
                    .and_then(|sessions| sessions.into_iter().next())
                    .map(|session| session.rollout_path),
            })
            .or_else(|| find_codex_rollout(home, target.workdir, target.conversation_id));
        let Some(rollout) = rollout else {
            return self.fallback(target, FallbackFormula::Codex);
        };

        match parse_codex_rollout(&rollout, target) {
            Some(sample) => ContextCollectionResult::Collected(sample),
            None => self.fallback_or(
                target,
                FallbackFormula::Codex,
                ContextUnavailableReason::MalformedTelemetry,
            ),
        }
    }

    fn collect_opencode(
        &mut self,
        target: &ContextCollectionTarget<'_>,
    ) -> ContextCollectionResult {
        let record = self
            .data_dir
            .as_deref()
            .and_then(|data| read_opencode_sqlite(data, target))
            .or_else(|| {
                self.data_dir
                    .as_deref()
                    .and_then(|data| read_opencode_legacy(data, target))
            });

        let Some(mut record) = record else {
            return self.fallback(target, FallbackFormula::Opencode);
        };
        if record.awaiting_post_reset {
            return ContextCollectionResult::Unavailable(
                ContextUnavailableReason::AwaitingPostResetSample,
            );
        }
        let Some(used_tokens) = record.used_tokens else {
            return self.fallback_or(
                target,
                FallbackFormula::Opencode,
                ContextUnavailableReason::MalformedTelemetry,
            );
        };
        let provider = record
            .provider_id
            .take()
            .or_else(|| target.provider_id.map(str::to_string));
        let model = record
            .model_id
            .take()
            .or_else(|| target.model_id.map(str::to_string));
        let context_limit = target.fallback_context_limit.or_else(|| {
            provider
                .as_deref()
                .zip(model.as_deref())
                .and_then(|(provider, model)| self.opencode_model_limit(provider, model))
        });
        let provenance = if record.direct_limit.is_some() {
            ContextProvenance::Direct
        } else {
            ContextProvenance::Estimated
        };

        ContextCollectionResult::Collected(make_sample(
            used_tokens,
            record.direct_limit.or(context_limit),
            provenance,
            record.sampled_at.unwrap_or(target.collected_at),
            target.collected_at,
            record
                .conversation_id
                .or_else(|| target.conversation_id.map(str::to_string)),
            record.last_reset,
        ))
    }

    fn collect_pi(&mut self, target: &ContextCollectionTarget<'_>) -> ContextCollectionResult {
        if let Some(payload) = target.runtime_payload {
            match pi_runtime_sample(payload, target) {
                Some(sample) => return ContextCollectionResult::Collected(sample),
                None if pi_runtime_is_post_reset(payload) => {
                    return ContextCollectionResult::Unavailable(
                        ContextUnavailableReason::AwaitingPostResetSample,
                    );
                }
                None => {}
            }
        }

        let Some(home) = self.home_dir.as_deref() else {
            return ContextCollectionResult::Unavailable(ContextUnavailableReason::MissingSession);
        };
        let Some(path) = find_pi_session(home, target.workdir, target.conversation_id) else {
            return ContextCollectionResult::Unavailable(ContextUnavailableReason::MissingSession);
        };
        let Some(record) = parse_pi_session(&path, target) else {
            return ContextCollectionResult::Unavailable(
                ContextUnavailableReason::MalformedTelemetry,
            );
        };
        if record.awaiting_post_reset {
            return ContextCollectionResult::Unavailable(
                ContextUnavailableReason::AwaitingPostResetSample,
            );
        }
        let Some(used_tokens) = record.used_tokens else {
            return ContextCollectionResult::Unavailable(
                ContextUnavailableReason::MissingTelemetry,
            );
        };
        let provider = record.provider_id.as_deref().or(target.provider_id);
        let model = record.model_id.as_deref().or(target.model_id);
        let context_limit = target.fallback_context_limit.or_else(|| {
            provider
                .zip(model)
                .and_then(|(provider, model)| self.pi_model_limit(provider, model))
        });

        ContextCollectionResult::Collected(make_sample(
            used_tokens,
            context_limit,
            ContextProvenance::Estimated,
            record.sampled_at.unwrap_or(target.collected_at),
            target.collected_at,
            record
                .conversation_id
                .or_else(|| target.conversation_id.map(str::to_string)),
            record.last_reset,
        ))
    }

    fn fallback(
        &self,
        target: &ContextCollectionTarget<'_>,
        formula: FallbackFormula,
    ) -> ContextCollectionResult {
        self.fallback_or(
            target,
            formula,
            if target.conversation_id.is_some() {
                ContextUnavailableReason::MissingTelemetry
            } else {
                ContextUnavailableReason::MissingSession
            },
        )
    }

    fn fallback_or(
        &self,
        target: &ContextCollectionTarget<'_>,
        formula: FallbackFormula,
        unavailable: ContextUnavailableReason,
    ) -> ContextCollectionResult {
        let Some(usage) = target.fallback_usage else {
            return ContextCollectionResult::Unavailable(unavailable);
        };
        let used_tokens = match formula {
            FallbackFormula::Claude => usage
                .input_tokens
                .saturating_add(usage.cache_read_tokens)
                .saturating_add(usage.cache_write_tokens),
            // Codex cached input is a subset of input and must not be added
            // twice. Existing AMF usage is cumulative, hence Estimated.
            FallbackFormula::Codex => usage.input_tokens,
            FallbackFormula::Opencode => usage.total_tokens,
        };
        let context_limit = target.fallback_context_limit.or(match formula {
            FallbackFormula::Claude => Some(CLAUDE_DEFAULT_CONTEXT_LIMIT),
            _ => None,
        });

        ContextCollectionResult::Collected(make_sample(
            used_tokens,
            context_limit,
            ContextProvenance::Estimated,
            target.collected_at,
            target.collected_at,
            target.conversation_id.map(str::to_string),
            None,
        ))
    }

    fn opencode_model_limit(&mut self, provider: &str, model: &str) -> Option<u64> {
        let key = ModelLimitKey {
            harness: "opencode",
            provider: provider.to_string(),
            model: model.to_string(),
        };
        if let Some(limit) = self.model_limits.get(&key) {
            return *limit;
        }

        let limit = self
            .home_dir
            .as_deref()
            .and_then(|home| read_opencode_config_limit(home, provider, model))
            .or_else(|| {
                self.cache_dir
                    .as_deref()
                    .and_then(|cache| read_opencode_catalog_limit(cache, provider, model))
            });
        self.model_limits.insert(key, limit);
        limit
    }

    fn pi_model_limit(&mut self, provider: &str, model: &str) -> Option<u64> {
        let key = ModelLimitKey {
            harness: "pi",
            provider: provider.to_string(),
            model: model.to_string(),
        };
        if let Some(limit) = self.model_limits.get(&key) {
            return *limit;
        }

        let limit = self
            .home_dir
            .as_deref()
            .and_then(|home| read_pi_custom_model_limit(home, provider, model))
            .or_else(|| {
                self.allow_model_commands
                    .then(|| query_pi_model_limit(provider, model))?
            });
        self.model_limits.insert(key, limit);
        limit
    }
}

impl ContextCollector for SessionContextCollector {
    fn collect(&mut self, target: ContextCollectionTarget<'_>) -> ContextCollectionResult {
        match target.session_kind {
            SessionKind::Claude => self.collect_claude(&target),
            SessionKind::Codex => self.collect_codex(&target),
            SessionKind::Opencode => self.collect_opencode(&target),
            SessionKind::Pi => self.collect_pi(&target),
            _ => ContextCollectionResult::Unavailable(
                ContextUnavailableReason::UnsupportedSessionKind,
            ),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum FallbackFormula {
    Claude,
    Codex,
    Opencode,
}

#[derive(Default)]
struct ArtifactContextRecord {
    used_tokens: Option<u64>,
    direct_limit: Option<u64>,
    provider_id: Option<String>,
    model_id: Option<String>,
    conversation_id: Option<String>,
    sampled_at: Option<DateTime<Utc>>,
    last_reset: Option<ContextResetEvent>,
    awaiting_post_reset: bool,
}

fn make_sample(
    used_tokens: u64,
    context_limit: Option<u64>,
    provenance: ContextProvenance,
    sampled_at: DateTime<Utc>,
    checked_at: DateTime<Utc>,
    conversation_id: Option<String>,
    last_reset: Option<ContextResetEvent>,
) -> ContextUsageSample {
    ContextUsageSample {
        used_tokens,
        context_limit,
        provenance,
        sampled_at,
        checked_at,
        reset: ContextResetMetadata {
            generation: 0,
            conversation_id,
            last_reset,
        },
    }
}

fn claude_runtime_sample(
    payload: &Value,
    target: &ContextCollectionTarget<'_>,
) -> Option<ContextUsageSample> {
    let context = payload.get("context_window")?;
    // Claude reports `current_usage: null` before the first response and
    // immediately after compaction. `total_input_tokens` may simultaneously
    // be zero, but that is an unknown sample rather than fresh 0% telemetry.
    let current = context
        .get("current_usage")
        .filter(|usage| !usage.is_null())?;
    let limit = json_u64(context, "context_window_size")?;
    let used = json_u64(context, "total_input_tokens").or_else(|| {
        Some(
            json_u64(current, "input_tokens")?
                .saturating_add(json_u64(current, "cache_read_input_tokens").unwrap_or(0))
                .saturating_add(json_u64(current, "cache_creation_input_tokens").unwrap_or(0)),
        )
    })?;
    let conversation_id = payload
        .get("session_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| target.conversation_id.map(str::to_string));

    Some(make_sample(
        used,
        Some(limit),
        ContextProvenance::Direct,
        target.collected_at,
        target.collected_at,
        conversation_id,
        None,
    ))
}

fn claude_runtime_is_post_reset(payload: &Value) -> bool {
    payload
        .pointer("/context_window/current_usage")
        .is_some_and(Value::is_null)
}

fn parse_claude_transcript(
    path: &Path,
    target: &ContextCollectionTarget<'_>,
) -> Option<ContextUsageSample> {
    let content = fs::read_to_string(path).ok()?;
    let mut latest: Option<(u64, DateTime<Utc>, Option<String>)> = None;
    let mut last_reset = None;

    for line in content.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let timestamp = json_timestamp(&value, "timestamp").unwrap_or(target.collected_at);
        let subtype = value.get("subtype").and_then(Value::as_str);
        if matches!(subtype, Some("compact_boundary" | "microcompact_boundary")) {
            last_reset = Some(ContextResetEvent {
                reason: if subtype == Some("microcompact_boundary") {
                    ContextResetReason::Summarization
                } else {
                    ContextResetReason::Compaction
                },
                detected_at: timestamp,
            });
        }
        if value.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(usage) = value.pointer("/message/usage") else {
            continue;
        };
        let Some(input) = json_u64(usage, "input_tokens") else {
            continue;
        };
        let used = input
            .saturating_add(json_u64(usage, "cache_read_input_tokens").unwrap_or(0))
            .saturating_add(json_u64(usage, "cache_creation_input_tokens").unwrap_or(0));
        let conversation = value
            .get("sessionId")
            .or_else(|| value.get("session_id"))
            .and_then(Value::as_str)
            .map(str::to_string);
        latest = Some((used, timestamp, conversation));
    }

    let (used, sampled_at, conversation_id) = latest?;
    Some(make_sample(
        used,
        target
            .fallback_context_limit
            .or(Some(CLAUDE_DEFAULT_CONTEXT_LIMIT)),
        ContextProvenance::Estimated,
        sampled_at,
        target.collected_at,
        conversation_id.or_else(|| target.conversation_id.map(str::to_string)),
        last_reset,
    ))
}

fn codex_runtime_sample(
    payload: &Value,
    target: &ContextCollectionTarget<'_>,
) -> Option<ContextUsageSample> {
    let token_usage = payload
        .pointer("/tokenUsage")
        .or_else(|| payload.pointer("/params/tokenUsage"))?;
    let last = token_usage.get("last")?;
    let used = json_u64(last, "totalTokens")
        .or_else(|| json_u64(last, "total_tokens"))
        .or_else(|| {
            Some(
                json_u64(last, "inputTokens")?
                    .saturating_add(json_u64(last, "outputTokens").unwrap_or(0)),
            )
        })?;
    let limit = json_u64(token_usage, "modelContextWindow")
        .or_else(|| json_u64(token_usage, "model_context_window"))?;
    let conversation_id = payload
        .get("threadId")
        .or_else(|| payload.get("thread_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| target.conversation_id.map(str::to_string));

    Some(make_sample(
        used,
        Some(limit),
        ContextProvenance::Direct,
        target.collected_at,
        target.collected_at,
        conversation_id,
        None,
    ))
}

fn parse_codex_rollout(
    path: &Path,
    target: &ContextCollectionTarget<'_>,
) -> Option<ContextUsageSample> {
    let content = fs::read_to_string(path).ok()?;
    let mut conversation_id = target.conversation_id.map(str::to_string);
    let mut latest: Option<(u64, u64, DateTime<Utc>)> = None;
    let mut last_reset = None;

    for line in content.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let event_type = value.get("type").and_then(Value::as_str);
        let payload_type = value.pointer("/payload/type").and_then(Value::as_str);
        let timestamp = json_timestamp(&value, "timestamp").unwrap_or(target.collected_at);
        if event_type == Some("session_meta") {
            conversation_id = value
                .pointer("/payload/id")
                .or_else(|| value.pointer("/payload/session_id"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .or(conversation_id);
        }
        if event_type == Some("compacted") || payload_type == Some("context_compacted") {
            last_reset = Some(ContextResetEvent {
                reason: ContextResetReason::Compaction,
                detected_at: timestamp,
            });
        }
        if event_type != Some("event_msg") || payload_type != Some("token_count") {
            continue;
        }
        let Some(info) = value.pointer("/payload/info") else {
            continue;
        };
        let Some(last) = info.get("last_token_usage") else {
            continue;
        };
        let Some(used) = json_u64(last, "total_tokens").or_else(|| {
            Some(
                json_u64(last, "input_tokens")?
                    .saturating_add(json_u64(last, "output_tokens").unwrap_or(0)),
            )
        }) else {
            continue;
        };
        let Some(limit) = json_u64(info, "model_context_window") else {
            continue;
        };
        latest = Some((used, limit, timestamp));
    }

    let (used, limit, sampled_at) = latest?;
    Some(make_sample(
        used,
        Some(limit),
        ContextProvenance::Direct,
        sampled_at,
        target.collected_at,
        conversation_id,
        last_reset,
    ))
}

fn pi_runtime_sample(
    payload: &Value,
    target: &ContextCollectionTarget<'_>,
) -> Option<ContextUsageSample> {
    let data = payload.get("data").unwrap_or(payload);
    let context = data.get("contextUsage")?;
    let used = context.get("tokens")?.as_u64()?;
    let limit = json_u64(context, "contextWindow")?;
    let conversation_id = data
        .get("sessionId")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| target.conversation_id.map(str::to_string));

    Some(make_sample(
        used,
        Some(limit),
        ContextProvenance::Direct,
        target.collected_at,
        target.collected_at,
        conversation_id,
        None,
    ))
}

fn pi_runtime_is_post_reset(payload: &Value) -> bool {
    let data = payload.get("data").unwrap_or(payload);
    data.pointer("/contextUsage/tokens")
        .is_some_and(Value::is_null)
}

fn read_opencode_sqlite(
    data_dir: &Path,
    target: &ContextCollectionTarget<'_>,
) -> Option<ArtifactContextRecord> {
    let db_path = data_dir.join("opencode").join("opencode.db");
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let workdir = target.workdir.to_string_lossy();
    let session_id = if let Some(id) = target.conversation_id {
        conn.query_row(
            "SELECT id FROM session WHERE id = ?1 AND directory = ?2",
            params![id, workdir.as_ref()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .ok()??
    } else {
        conn.query_row(
            "SELECT id FROM session WHERE directory = ?1 AND time_archived IS NULL ORDER BY time_updated DESC LIMIT 1",
            params![workdir.as_ref()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .ok()??
    };

    let mut stmt = conn
        .prepare(
            "SELECT data, time_updated FROM message WHERE session_id = ?1 ORDER BY time_updated ASC",
        )
        .ok()?;
    let rows = stmt
        .query_map(params![&session_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .ok()?;
    let mut record = ArtifactContextRecord {
        conversation_id: Some(session_id.clone()),
        ..ArtifactContextRecord::default()
    };
    let mut last_usage_millis = None;
    let mut last_reset_millis = None;

    for row in rows.flatten() {
        let Ok(value) = serde_json::from_str::<Value>(&row.0) else {
            continue;
        };
        if value.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        if value.get("summary").and_then(Value::as_bool) == Some(true) {
            last_reset_millis = Some(row.1);
            record.last_reset = Some(ContextResetEvent {
                reason: ContextResetReason::Compaction,
                detected_at: millis_timestamp(row.1).unwrap_or(target.collected_at),
            });
            continue;
        }
        if value.get("error").is_some_and(|error| !error.is_null()) {
            continue;
        }
        let Some(tokens) = value.get("tokens") else {
            continue;
        };
        let Some(used) = opencode_context_tokens(tokens) else {
            continue;
        };
        record.used_tokens = Some(used);
        record.provider_id = value
            .get("providerID")
            .and_then(Value::as_str)
            .map(str::to_string);
        record.model_id = value
            .get("modelID")
            .and_then(Value::as_str)
            .map(str::to_string);
        record.sampled_at = millis_timestamp(row.1);
        last_usage_millis = Some(row.1);
    }
    record.awaiting_post_reset = last_reset_millis
        .zip(last_usage_millis)
        .is_some_and(|(reset, usage)| reset >= usage)
        || (last_reset_millis.is_some() && last_usage_millis.is_none());
    Some(record)
}

fn read_opencode_legacy(
    data_dir: &Path,
    target: &ContextCollectionTarget<'_>,
) -> Option<ArtifactContextRecord> {
    let storage = data_dir.join("opencode").join("storage");
    let session_root = storage.join("session");
    let sessions = walk_files_with_extension(&session_root, "json");
    let mut selected: Option<(String, i64)> = None;
    for path in sessions {
        let Ok(value) = read_json(&path) else {
            continue;
        };
        let Some(id) = value.get("id").and_then(Value::as_str) else {
            continue;
        };
        if target.conversation_id.is_some_and(|wanted| wanted != id) {
            continue;
        }
        if value.get("directory").and_then(Value::as_str)
            != Some(target.workdir.to_string_lossy().as_ref())
        {
            continue;
        }
        let updated = value
            .pointer("/time/updated")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        if selected
            .as_ref()
            .is_none_or(|(_, current)| updated > *current)
        {
            selected = Some((id.to_string(), updated));
        }
    }
    let (session_id, _) = selected?;
    let message_root = storage.join("message").join(&session_id);
    let mut messages = walk_files_with_extension(&message_root, "json");
    messages.sort_by_key(|path| modified_millis(path).unwrap_or(0));
    let mut record = ArtifactContextRecord {
        conversation_id: Some(session_id.clone()),
        ..ArtifactContextRecord::default()
    };
    let mut latest_message_id = None;
    for path in messages {
        let Ok(value) = read_json(&path) else {
            continue;
        };
        if value.get("role").and_then(Value::as_str) != Some("assistant")
            || value.get("summary").and_then(Value::as_bool) == Some(true)
            || value.get("error").is_some_and(|error| !error.is_null())
        {
            continue;
        }
        if let Some(tokens) = value.get("tokens")
            && let Some(used) = opencode_context_tokens(tokens)
        {
            record.used_tokens = Some(used);
            record.sampled_at = value
                .pointer("/time/completed")
                .or_else(|| value.pointer("/time/created"))
                .and_then(Value::as_i64)
                .and_then(millis_timestamp);
            record.provider_id = value
                .get("providerID")
                .and_then(Value::as_str)
                .map(str::to_string);
            record.model_id = value
                .get("modelID")
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        latest_message_id = path
            .file_stem()
            .and_then(|name| name.to_str())
            .map(str::to_string);
    }
    if record.used_tokens.is_none()
        && let Some(message_id) = latest_message_id
    {
        let mut parts = walk_files_with_extension(&storage.join("part").join(message_id), "json");
        parts.sort_by_key(|path| modified_millis(path).unwrap_or(0));
        for path in parts {
            let Ok(value) = read_json(&path) else {
                continue;
            };
            if value.get("type").and_then(Value::as_str) == Some("step-finish")
                && let Some(used) = value.get("tokens").and_then(opencode_context_tokens)
            {
                record.used_tokens = Some(used);
                record.sampled_at = modified_millis(&path).and_then(millis_timestamp);
            }
        }
    }
    Some(record)
}

fn opencode_context_tokens(tokens: &Value) -> Option<u64> {
    if let Some(total) = json_u64(tokens, "total")
        && total > 0
    {
        return Some(total);
    }
    Some(
        json_u64(tokens, "input")?
            .saturating_add(json_u64(tokens, "output").unwrap_or(0))
            .saturating_add(
                tokens
                    .pointer("/cache/read")
                    .and_then(value_u64)
                    .unwrap_or(0),
            )
            .saturating_add(
                tokens
                    .pointer("/cache/write")
                    .and_then(value_u64)
                    .unwrap_or(0),
            ),
    )
}

fn parse_pi_session(
    path: &Path,
    target: &ContextCollectionTarget<'_>,
) -> Option<ArtifactContextRecord> {
    let content = fs::read_to_string(path).ok()?;
    let mut record = ArtifactContextRecord::default();
    let mut saw_header = false;
    let mut usage_after_reset = false;
    let mut saw_reset = false;

    for line in content.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match value.get("type").and_then(Value::as_str) {
            Some("session") => {
                if value.get("cwd").and_then(Value::as_str)
                    != Some(target.workdir.to_string_lossy().as_ref())
                {
                    return None;
                }
                record.conversation_id =
                    value.get("id").and_then(Value::as_str).map(str::to_string);
                saw_header = true;
            }
            Some("compaction") => {
                saw_reset = true;
                usage_after_reset = false;
                record.last_reset = Some(ContextResetEvent {
                    reason: ContextResetReason::Compaction,
                    detected_at: json_timestamp(&value, "timestamp").unwrap_or(target.collected_at),
                });
            }
            Some("branch_summary") => {
                saw_reset = true;
                usage_after_reset = false;
                record.last_reset = Some(ContextResetEvent {
                    reason: ContextResetReason::Summarization,
                    detected_at: json_timestamp(&value, "timestamp").unwrap_or(target.collected_at),
                });
            }
            Some("message") => {
                let Some(message) = value.get("message") else {
                    continue;
                };
                if message.get("role").and_then(Value::as_str) != Some("assistant")
                    || matches!(
                        message.get("stopReason").and_then(Value::as_str),
                        Some("aborted" | "error")
                    )
                {
                    continue;
                }
                let Some(usage) = message.get("usage") else {
                    continue;
                };
                let Some(used) = json_u64(usage, "totalTokens")
                    .filter(|total| *total > 0)
                    .or_else(|| {
                        Some(
                            json_u64(usage, "input")?
                                .saturating_add(json_u64(usage, "output").unwrap_or(0))
                                .saturating_add(json_u64(usage, "cacheRead").unwrap_or(0))
                                .saturating_add(json_u64(usage, "cacheWrite").unwrap_or(0)),
                        )
                    })
                else {
                    continue;
                };
                record.used_tokens = Some(used);
                record.provider_id = message
                    .get("provider")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                record.model_id = message
                    .get("model")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                record.sampled_at = message
                    .get("timestamp")
                    .and_then(Value::as_i64)
                    .and_then(millis_timestamp)
                    .or_else(|| json_timestamp(&value, "timestamp"));
                if saw_reset {
                    usage_after_reset = true;
                }
            }
            _ => {}
        }
    }
    if !saw_header {
        return None;
    }
    record.awaiting_post_reset = saw_reset && !usage_after_reset;
    Some(record)
}

fn find_pi_session(home: &Path, workdir: &Path, id: Option<&str>) -> Option<PathBuf> {
    let root = home.join(".pi").join("agent").join("sessions");
    let mut selected: Option<(PathBuf, i64)> = None;
    for path in walk_files_with_extension(&root, "jsonl") {
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        let Some(header) = content
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|value| value.get("type").and_then(Value::as_str) == Some("session"))
        else {
            continue;
        };
        if header.get("cwd").and_then(Value::as_str) != Some(workdir.to_string_lossy().as_ref()) {
            continue;
        }
        let session_id = header.get("id").and_then(Value::as_str);
        if id.is_some_and(|wanted| session_id != Some(wanted)) {
            continue;
        }
        let modified = modified_millis(&path).unwrap_or(0);
        if selected
            .as_ref()
            .is_none_or(|(_, current)| modified > *current)
        {
            selected = Some((path, modified));
        }
    }
    selected.map(|(path, _)| path)
}

fn find_codex_rollout(home: &Path, workdir: &Path, id: Option<&str>) -> Option<PathBuf> {
    let root = home.join(".codex").join("sessions");
    let mut selected: Option<(PathBuf, i64)> = None;
    for path in walk_files_with_extension(&root, "jsonl") {
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        let mut matched = false;
        for line in content.lines() {
            let Ok(value) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if value.get("type").and_then(Value::as_str) != Some("session_meta") {
                continue;
            }
            let session_id = value
                .pointer("/payload/id")
                .or_else(|| value.pointer("/payload/session_id"))
                .and_then(Value::as_str);
            let cwd = value.pointer("/payload/cwd").and_then(Value::as_str);
            matched = cwd == Some(workdir.to_string_lossy().as_ref())
                && id.is_none_or(|wanted| session_id == Some(wanted));
            break;
        }
        if !matched {
            continue;
        }
        let modified = modified_millis(&path).unwrap_or(0);
        if selected
            .as_ref()
            .is_none_or(|(_, current)| modified > *current)
        {
            selected = Some((path, modified));
        }
    }
    selected.map(|(path, _)| path)
}

fn read_opencode_config_limit(home: &Path, provider: &str, model: &str) -> Option<u64> {
    let config_dir = home.join(".config").join("opencode");
    for name in ["opencode.json", "opencode.jsonc"] {
        let Ok(value) = read_json(&config_dir.join(name)) else {
            continue;
        };
        if let Some(limit) = value
            .pointer(&format!(
                "/provider/{}/{}/{}",
                escape_json_pointer(provider),
                "models",
                escape_json_pointer(model)
            ))
            .and_then(|model| model.pointer("/limit/context"))
            .and_then(value_u64)
        {
            return Some(limit);
        }
    }
    None
}

fn read_opencode_catalog_limit(cache: &Path, provider: &str, model: &str) -> Option<u64> {
    let value = read_json(&cache.join("opencode").join("models.json")).ok()?;
    value
        .get(provider)?
        .get("models")?
        .get(model)?
        .pointer("/limit/context")
        .and_then(value_u64)
}

fn read_pi_custom_model_limit(home: &Path, provider: &str, model: &str) -> Option<u64> {
    let value = read_json(&home.join(".pi").join("agent").join("models.json")).ok()?;
    let models = value
        .pointer(&format!(
            "/providers/{}/models",
            escape_json_pointer(provider)
        ))?
        .as_array()?;
    models.iter().find_map(|entry| {
        if entry.get("id").and_then(Value::as_str) == Some(model) {
            entry.get("contextWindow").and_then(value_u64)
        } else {
            None
        }
    })
}

fn query_pi_model_limit(provider: &str, model: &str) -> Option<u64> {
    let query = format!("{provider}/{model}");
    let output = Command::new("pi")
        .args(["--list-models", &query])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    stdout.lines().skip(1).find_map(|line| {
        let columns = line.split_whitespace().collect::<Vec<_>>();
        if columns.first().copied() == Some(provider) && columns.get(1).copied() == Some(model) {
            columns.get(2).and_then(|value| parse_token_count(value))
        } else {
            None
        }
    })
}

fn parse_token_count(value: &str) -> Option<u64> {
    let (number, multiplier) = match value.as_bytes().last().copied()? {
        b'K' | b'k' => (&value[..value.len() - 1], 1_000f64),
        b'M' | b'm' => (&value[..value.len() - 1], 1_000_000f64),
        _ => (value, 1f64),
    };
    let parsed = number.parse::<f64>().ok()?;
    (parsed.is_finite() && parsed > 0.0).then_some((parsed * multiplier).round() as u64)
}

fn read_json(path: &Path) -> Result<Value, ()> {
    fs::read_to_string(path)
        .map_err(|_| ())
        .and_then(|content| serde_json::from_str(&content).map_err(|_| ()))
}

fn json_u64(value: &Value, field: &str) -> Option<u64> {
    value.get(field).and_then(value_u64)
}

fn value_u64(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        let number = value.as_f64()?;
        (number.is_finite() && number >= 0.0 && number.fract() == 0.0).then_some(number as u64)
    })
}

fn json_timestamp(value: &Value, field: &str) -> Option<DateTime<Utc>> {
    let raw = value.get(field)?.as_str()?;
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|timestamp| timestamp.with_timezone(&Utc))
}

fn millis_timestamp(millis: i64) -> Option<DateTime<Utc>> {
    Utc.timestamp_millis_opt(millis).single()
}

fn modified_millis(path: &Path) -> Option<i64> {
    fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis() as i64)
}

fn newest_file_with_extension(root: &Path, extension: &str) -> Option<PathBuf> {
    walk_files_with_extension(root, extension)
        .into_iter()
        .filter_map(|path| modified_millis(&path).map(|modified| (path, modified)))
        .max_by_key(|(_, modified)| *modified)
        .map(|(path, _)| path)
}

fn walk_files_with_extension(root: &Path, extension: &str) -> Vec<PathBuf> {
    if !root.is_dir() {
        return Vec::new();
    }
    let mut files = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some(extension) {
                files.push(path);
            }
        }
    }
    files
}

fn encode_claude_path(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect()
}

fn escape_json_pointer(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context_tracking::{ContextBand, calculate_context_snapshot};
    use crate::token_tracking::{TokenUsageProvider, TokenUsageSource};
    use std::fs;
    use tempfile::TempDir;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 23, 12, 0, 0)
            .single()
            .unwrap()
    }

    fn target<'a>(
        kind: &'a SessionKind,
        workdir: &'a Path,
        conversation_id: Option<&'a str>,
    ) -> ContextCollectionTarget<'a> {
        ContextCollectionTarget {
            session_kind: kind,
            workdir,
            conversation_id,
            provider_id: None,
            model_id: None,
            runtime_payload: None,
            fallback_usage: None,
            fallback_context_limit: None,
            collected_at: now(),
        }
    }

    fn collector(roots: &TempDir) -> SessionContextCollector {
        SessionContextCollector::with_roots(roots.path(), roots.path(), roots.path())
    }

    fn write(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn claude_status_line_is_direct_and_transcript_fallback_is_estimated() {
        let roots = TempDir::new().unwrap();
        let workdir = roots.path().join("repo");
        let kind = SessionKind::Claude;
        let payload = serde_json::json!({
            "session_id": "claude-1",
            "context_window": {
                "total_input_tokens": 140_000,
                "context_window_size": 200_000,
                "current_usage": {
                    "input_tokens": 10_000,
                    "cache_read_input_tokens": 130_000,
                    "cache_creation_input_tokens": 0
                }
            }
        });
        let mut direct_target = target(&kind, &workdir, Some("claude-1"));
        direct_target.runtime_payload = Some(&payload);
        let ContextCollectionResult::Collected(sample) = collector(&roots).collect(direct_target)
        else {
            panic!("expected direct Claude sample");
        };
        assert_eq!(sample.provenance, ContextProvenance::Direct);
        assert_eq!(sample.used_tokens, 140_000);
        assert_eq!(
            calculate_context_snapshot(sample).unwrap().band,
            ContextBand::Warning
        );

        let post_reset_payload = serde_json::json!({
            "session_id": "claude-1",
            "context_window": {
                "total_input_tokens": 0,
                "context_window_size": 200_000,
                "current_usage": null
            }
        });
        let mut post_reset_target = target(&kind, &workdir, Some("claude-1"));
        post_reset_target.runtime_payload = Some(&post_reset_payload);
        assert_eq!(
            collector(&roots).collect(post_reset_target),
            ContextCollectionResult::Unavailable(ContextUnavailableReason::AwaitingPostResetSample)
        );

        let transcript = roots
            .path()
            .join(".claude/projects")
            .join(encode_claude_path(&workdir))
            .join("claude-1.jsonl");
        write(
            &transcript,
            concat!(
                "not json\n",
                "{\"type\":\"assistant\",\"timestamp\":\"2026-08-23T11:59:00Z\",\"sessionId\":\"claude-1\",\"message\":{\"model\":\"claude-sonnet-5\",\"usage\":{\"input_tokens\":2,\"cache_read_input_tokens\":100000,\"cache_creation_input_tokens\":1000}}}\n"
            ),
        );
        let ContextCollectionResult::Collected(sample) =
            collector(&roots).collect(target(&kind, &workdir, Some("claude-1")))
        else {
            panic!("expected transcript fallback");
        };
        assert_eq!(sample.used_tokens, 101_002);
        assert_eq!(sample.context_limit, Some(200_000));
        assert_eq!(sample.provenance, ContextProvenance::Estimated);
    }

    #[test]
    fn codex_rollout_uses_last_usage_and_direct_window() {
        let roots = TempDir::new().unwrap();
        let workdir = roots.path().join("repo");
        let rollout = roots
            .path()
            .join(".codex/sessions/2026/08/23/rollout.jsonl");
        write(
            &rollout,
            &format!(
                "{{\"timestamp\":\"2026-08-23T11:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"codex-1\",\"cwd\":{}}}}}\n\
                 {{malformed\n\
                 {{\"timestamp\":\"2026-08-23T11:59:00Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"total_tokens\":900000}},\"last_token_usage\":{{\"input_tokens\":179000,\"cached_input_tokens\":170000,\"output_tokens\":1000,\"total_tokens\":180000}},\"model_context_window\":200000}}}}}}\n",
                serde_json::to_string(workdir.to_string_lossy().as_ref()).unwrap()
            ),
        );
        let kind = SessionKind::Codex;
        let ContextCollectionResult::Collected(sample) =
            collector(&roots).collect(target(&kind, &workdir, Some("codex-1")))
        else {
            panic!("expected Codex sample");
        };
        assert_eq!(sample.used_tokens, 180_000);
        assert_eq!(sample.context_limit, Some(200_000));
        assert_eq!(sample.provenance, ContextProvenance::Direct);
        assert_eq!(
            calculate_context_snapshot(sample).unwrap().band,
            ContextBand::Critical
        );
    }

    #[test]
    fn opencode_sqlite_uses_latest_assistant_and_cached_model_limit() {
        let roots = TempDir::new().unwrap();
        let workdir = roots.path().join("repo");
        let db_path = roots.path().join("opencode/opencode.db");
        fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT NOT NULL, time_updated INTEGER NOT NULL, time_archived INTEGER);
             CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session VALUES (?1, ?2, 1, NULL)",
            params!["open-1", workdir.to_string_lossy()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message VALUES ('m1', 'open-1', 1000, ?1)",
            params![
                serde_json::json!({
                    "role": "assistant",
                    "providerID": "anthropic",
                    "modelID": "claude-test",
                    "tokens": {"input": 10, "output": 5, "cache": {"read": 74_985, "write": 0}}
                })
                .to_string()
            ],
        )
        .unwrap();
        write(
            &roots.path().join("opencode/models.json"),
            &serde_json::json!({
                "anthropic": {"models": {"claude-test": {"limit": {"context": 100_000}}}}
            })
            .to_string(),
        );
        let kind = SessionKind::Opencode;
        let ContextCollectionResult::Collected(sample) =
            collector(&roots).collect(target(&kind, &workdir, Some("open-1")))
        else {
            panic!("expected OpenCode sample");
        };
        assert_eq!(sample.used_tokens, 75_000);
        assert_eq!(sample.context_limit, Some(100_000));
        assert_eq!(sample.provenance, ContextProvenance::Estimated);
        assert_eq!(
            calculate_context_snapshot(sample).unwrap().band,
            ContextBand::Warning
        );
    }

    #[test]
    fn pi_session_uses_latest_usage_and_custom_model_limit() {
        let roots = TempDir::new().unwrap();
        let workdir = roots.path().join("repo");
        let session = roots
            .path()
            .join(".pi/agent/sessions/--repo--/session.jsonl");
        write(
            &session,
            &format!(
                "{{\"type\":\"session\",\"version\":3,\"id\":\"pi-1\",\"timestamp\":\"2026-08-23T11:00:00Z\",\"cwd\":{}}}\n\
                 {{\"type\":\"message\",\"id\":\"a1\",\"parentId\":null,\"timestamp\":\"2026-08-23T11:59:00Z\",\"message\":{{\"role\":\"assistant\",\"provider\":\"anthropic\",\"model\":\"claude-test\",\"timestamp\":1787486340000,\"stopReason\":\"stop\",\"usage\":{{\"input\":70000,\"output\":1000,\"cacheRead\":0,\"cacheWrite\":0,\"totalTokens\":71000}}}}}}\n",
                serde_json::to_string(workdir.to_string_lossy().as_ref()).unwrap()
            ),
        );
        write(
            &roots.path().join(".pi/agent/models.json"),
            &serde_json::json!({
                "providers": {"anthropic": {"models": [{"id": "claude-test", "contextWindow": 100_000}]}}
            })
            .to_string(),
        );
        let kind = SessionKind::Pi;
        let ContextCollectionResult::Collected(sample) =
            collector(&roots).collect(target(&kind, &workdir, Some("pi-1")))
        else {
            panic!("expected Pi sample");
        };
        assert_eq!(sample.used_tokens, 71_000);
        assert_eq!(sample.context_limit, Some(100_000));
        assert_eq!(sample.provenance, ContextProvenance::Estimated);
    }

    #[test]
    fn malformed_or_unsupported_telemetry_is_recoverable() {
        let roots = TempDir::new().unwrap();
        let workdir = roots.path().join("repo");
        let kind = SessionKind::Terminal;
        assert_eq!(
            collector(&roots).collect(target(&kind, &workdir, None)),
            ContextCollectionResult::Unavailable(ContextUnavailableReason::UnsupportedSessionKind)
        );

        let kind = SessionKind::Claude;
        let transcript = roots
            .path()
            .join(".claude/projects")
            .join(encode_claude_path(&workdir))
            .join("broken.jsonl");
        write(&transcript, "{not-json\n");
        assert_eq!(
            collector(&roots).collect(target(&kind, &workdir, Some("broken"))),
            ContextCollectionResult::Unavailable(ContextUnavailableReason::MalformedTelemetry)
        );
    }

    #[test]
    fn cumulative_fallback_is_explicitly_estimated() {
        let roots = TempDir::new().unwrap();
        let workdir = roots.path().join("repo");
        let kind = SessionKind::Claude;
        let usage = SessionTokenUsage {
            source: TokenUsageSource {
                provider: TokenUsageProvider::Claude,
                id: "claude-1".to_string(),
            },
            input_tokens: 50_000,
            output_tokens: 1_000,
            cache_read_tokens: 20_000,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
            total_tokens: 71_000,
        };
        let mut request = target(&kind, &workdir, Some("claude-1"));
        request.fallback_usage = Some(&usage);
        let ContextCollectionResult::Collected(sample) = collector(&roots).collect(request) else {
            panic!("expected fallback sample");
        };
        assert_eq!(sample.used_tokens, 70_000);
        assert_eq!(sample.provenance, ContextProvenance::Estimated);
    }

    #[test]
    fn explicit_pi_compaction_waits_for_a_post_reset_sample() {
        let roots = TempDir::new().unwrap();
        let workdir = roots.path().join("repo");
        let session = roots
            .path()
            .join(".pi/agent/sessions/--repo--/session.jsonl");
        write(
            &session,
            &format!(
                "{{\"type\":\"session\",\"version\":3,\"id\":\"pi-1\",\"timestamp\":\"2026-08-23T11:00:00Z\",\"cwd\":{}}}\n\
                 {{\"type\":\"message\",\"id\":\"a1\",\"parentId\":null,\"message\":{{\"role\":\"assistant\",\"provider\":\"x\",\"model\":\"y\",\"timestamp\":1,\"stopReason\":\"stop\",\"usage\":{{\"input\":90000,\"output\":0,\"cacheRead\":0,\"cacheWrite\":0,\"totalTokens\":90000}}}}}}\n\
                 {{\"type\":\"compaction\",\"id\":\"c1\",\"parentId\":\"a1\",\"timestamp\":\"2026-08-23T11:59:00Z\",\"summary\":\"summary\",\"firstKeptEntryId\":\"a1\",\"tokensBefore\":90000}}\n",
                serde_json::to_string(workdir.to_string_lossy().as_ref()).unwrap()
            ),
        );
        let kind = SessionKind::Pi;
        assert_eq!(
            collector(&roots).collect(target(&kind, &workdir, Some("pi-1"))),
            ContextCollectionResult::Unavailable(ContextUnavailableReason::AwaitingPostResetSample)
        );
    }
}
