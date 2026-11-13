//! Windowed reader of Claude transcript usage entries, one per message, each
//! with its own model so subagents and mid-session model switches are priced
//! correctly.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::cost::{Priced, TokenCounts, price};
use super::pricing::{PricedModel, lookup_with};
use crate::token_tracking::{SessionTokenTracker, TokenPricingConfig};

#[derive(Debug, Clone, PartialEq)]
pub struct UsageEntry {
    pub model: String,
    pub timestamp: DateTime<Utc>,
    pub counts: TokenCounts,
    /// Read from a `subagents/` transcript.
    pub from_subagent: bool,
}

/// Transcript paths for a session: the root jsonl and every subagent jsonl.
/// None when the root transcript does not exist.
pub fn session_paths(
    tracker: &SessionTokenTracker,
    workdir: &Path,
    session_id: &str,
) -> Option<Vec<PathBuf>> {
    tracker.claude_usage_paths(workdir, session_id)
}

/// Same key order as the sidebar reader: requestId, message.id, uuid. Unlike
/// the sidebar reader (first line wins, #863), lines sharing a key are merged
/// by field-wise max in `read_window`.
fn dedupe_key(value: &Value) -> Option<String> {
    let str_at = |v: Option<&Value>| v.and_then(Value::as_str).map(str::to_owned);
    let message = value.get("message");
    str_at(value.get("requestId"))
        .map(|id| format!("request:{id}"))
        .or_else(|| str_at(message.and_then(|m| m.get("id"))).map(|id| format!("message:{id}")))
        .or_else(|| str_at(value.get("uuid")).map(|id| format!("uuid:{id}")))
}

fn count(usage: &Value, field: &str) -> Option<u64> {
    usage.get(field).and_then(Value::as_u64)
}

fn counts_from_usage(usage: &Value) -> TokenCounts {
    let creation = usage.get("cache_creation");
    let split_5m = creation.and_then(|c| count(c, "ephemeral_5m_input_tokens"));
    let split_1h = creation.and_then(|c| count(c, "ephemeral_1h_input_tokens"));
    let (cache_write_5m, cache_write_1h) = if split_5m.is_some() || split_1h.is_some() {
        (split_5m, split_1h)
    } else {
        // No split reported: the lifetime is unknown, so count it as 5m.
        (count(usage, "cache_creation_input_tokens"), None)
    };
    TokenCounts {
        input: count(usage, "input_tokens"),
        output: count(usage, "output_tokens"),
        cache_read: count(usage, "cache_read_input_tokens"),
        cache_write_5m,
        cache_write_1h,
        reasoning: None,
    }
}

fn parse_line(line: &[u8], from_subagent: bool) -> Option<(String, UsageEntry)> {
    let value: Value = serde_json::from_slice(line).ok()?;
    let message = value.get("message")?;
    let usage = message.get("usage")?;
    let key = dedupe_key(&value)?;
    let timestamp = DateTime::parse_from_rfc3339(value.get("timestamp")?.as_str()?)
        .ok()?
        .with_timezone(&Utc);
    let model = message
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    Some((
        key,
        UsageEntry {
            model,
            timestamp,
            counts: counts_from_usage(usage),
            from_subagent,
        },
    ))
}

fn max_opt(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (None, None) => None,
        _ => Some(a.unwrap_or(0).max(b.unwrap_or(0))),
    }
}

/// Folds a later line of the same response into `into`: field-wise max of
/// each count (earlier lines can carry a partial `output_tokens`), earliest
/// timestamp, first model that is not "unknown".
fn merge(into: &mut UsageEntry, other: UsageEntry) {
    let (a, b) = (into.counts, other.counts);
    into.counts = TokenCounts {
        input: max_opt(a.input, b.input),
        output: max_opt(a.output, b.output),
        cache_read: max_opt(a.cache_read, b.cache_read),
        cache_write_5m: max_opt(a.cache_write_5m, b.cache_write_5m),
        cache_write_1h: max_opt(a.cache_write_1h, b.cache_write_1h),
        reasoning: max_opt(a.reasoning, b.reasoning),
    };
    into.timestamp = into.timestamp.min(other.timestamp);
    if into.model == "unknown" {
        into.model = other.model;
    }
}

/// Entries with `start <= timestamp < end`, in first-seen order. Lines that
/// share a dedupe key (across all files) are merged into one entry before
/// the window is applied. Malformed lines and lines without usage, a dedupe
/// key or a timestamp are skipped.
pub fn read_window(paths: &[PathBuf], start: DateTime<Utc>, end: DateTime<Utc>) -> Vec<UsageEntry> {
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut entries: Vec<UsageEntry> = Vec::new();
    for path in paths {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let from_subagent = path.components().any(|c| c.as_os_str() == "subagents");
        for line in bytes.split(|&b| b == b'\n') {
            let Some((key, entry)) = parse_line(line, from_subagent) else {
                continue;
            };
            match index.get(&key) {
                Some(&i) => merge(&mut entries[i], entry),
                None => {
                    index.insert(key, entries.len());
                    entries.push(entry);
                }
            }
        }
    }
    entries.retain(|e| e.timestamp >= start && e.timestamp < end);
    entries
}

/// Prices each entry at its own model's rates.
pub fn price_entries(
    entries: &[UsageEntry],
    fallback: &TokenPricingConfig,
) -> Vec<(String, Priced)> {
    entries
        .iter()
        .map(|e| {
            let model: PricedModel = lookup_with(&e.model, fallback);
            (e.model.clone(), price(&e.counts, &model.rates))
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelTotal {
    pub counts: TokenCounts,
    pub cost_usd: f64,
    /// Sum of per-entry effective tokens; None when no entry had any.
    pub effective_tokens: Option<u64>,
    pub entries: usize,
    pub price_assumed: bool,
}

/// Per-model totals, each entry priced on its own (so the long-prompt tier
/// applies per call, not to the sum).
pub fn aggregate_by_model(
    entries: &[UsageEntry],
    fallback: &TokenPricingConfig,
) -> BTreeMap<String, ModelTotal> {
    let mut totals: BTreeMap<String, ModelTotal> = BTreeMap::new();
    for entry in entries {
        let model = lookup_with(&entry.model, fallback);
        let priced = price(&entry.counts, &model.rates);
        let total = totals.entry(entry.model.clone()).or_insert(ModelTotal {
            counts: TokenCounts::default(),
            cost_usd: 0.0,
            effective_tokens: None,
            entries: 0,
            price_assumed: model.price_assumed,
        });
        total.counts = total.counts.plus(&entry.counts);
        total.cost_usd += priced.cost_usd;
        total.effective_tokens = match (total.effective_tokens, priced.effective_tokens) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
        };
        total.entries += 1;
    }
    totals
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::fs;

    fn ts(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 13, h, m, 0).unwrap()
    }

    fn line(req: &str, model: &str, time: &str, usage: &str) -> String {
        format!(
            r#"{{"type":"assistant","requestId":"{req}","uuid":"u-{req}","timestamp":"{time}","message":{{"id":"m-{req}","model":"{model}","usage":{usage}}}}}"#
        )
    }

    fn fixture(dir: &Path) -> Vec<PathBuf> {
        let root = dir.join("s1.jsonl");
        let sub_dir = dir.join("s1").join("subagents");
        fs::create_dir_all(&sub_dir).unwrap();
        let sub = sub_dir.join("agent-a.jsonl");
        let root_lines = [
            line(
                "r1",
                "claude-fable-5-2",
                "2026-11-13T10:00:00Z",
                r#"{"input_tokens":10,"output_tokens":200,"cache_read_input_tokens":5000,"cache_creation_input_tokens":300,"cache_creation":{"ephemeral_5m_input_tokens":100,"ephemeral_1h_input_tokens":200}}"#,
            ),
            // Same requestId: merged by max, so smaller counts change nothing.
            line(
                "r1",
                "claude-fable-5-2",
                "2026-11-13T10:00:01Z",
                r#"{"input_tokens":5,"output_tokens":50}"#,
            ),
            "{ this is not json".to_string(),
            r#"{"type":"user","message":{"content":"hi"}}"#.to_string(),
            // Model switch, no split: all creation counts as 5m.
            line(
                "r2",
                "claude-opus-5-5",
                "2026-11-13T11:00:00Z",
                r#"{"input_tokens":20,"output_tokens":30,"cache_creation_input_tokens":400}"#,
            ),
            // Outside the window.
            line(
                "r3",
                "claude-opus-5-5",
                "2026-11-13T13:00:00Z",
                r#"{"input_tokens":1,"output_tokens":1}"#,
            ),
        ];
        fs::write(&root, root_lines.join("\n")).unwrap();
        fs::write(
            &sub,
            line(
                "r4",
                "claude-haiku-5-5",
                "2026-11-13T10:30:00Z",
                r#"{"input_tokens":1000,"output_tokens":100}"#,
            ),
        )
        .unwrap();
        vec![root, sub]
    }

    #[test]
    fn reads_models_split_dedupe_and_window() {
        let dir = tempfile::tempdir().unwrap();
        let paths = fixture(dir.path());
        let entries = read_window(&paths, ts(10, 0), ts(12, 0));
        assert_eq!(entries.len(), 3);

        let fable = &entries[0];
        assert_eq!(fable.model, "claude-fable-5-2");
        assert!(!fable.from_subagent);
        assert_eq!(fable.counts.input, Some(10));
        assert_eq!(fable.counts.output, Some(200));
        assert_eq!(fable.counts.cache_read, Some(5000));
        assert_eq!(fable.counts.cache_write_5m, Some(100));
        assert_eq!(fable.counts.cache_write_1h, Some(200));

        let opus = &entries[1];
        assert_eq!(opus.model, "claude-opus-5-5");
        assert_eq!(opus.counts.cache_write_5m, Some(400));
        assert_eq!(opus.counts.cache_write_1h, None);

        let haiku = &entries[2];
        assert_eq!(haiku.model, "claude-haiku-5-5");
        assert!(haiku.from_subagent);
    }

    #[test]
    fn same_request_lines_merge_by_max_with_earliest_timestamp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        let lines = [
            line(
                "r9",
                "claude-opus-5-5",
                "2026-11-13T10:00:00Z",
                r#"{"input_tokens":10,"output_tokens":5}"#,
            ),
            line(
                "r9",
                "claude-opus-5-5",
                "2026-11-13T10:00:05Z",
                r#"{"input_tokens":10,"output_tokens":400,"cache_read_input_tokens":7}"#,
            ),
        ];
        fs::write(&path, lines.join("\n")).unwrap();
        let paths = vec![path];
        // The window applies to the merged entry's earliest timestamp.
        assert!(read_window(&paths, ts(10, 0), ts(10, 0)).is_empty());
        let entries = read_window(&paths, ts(10, 0), ts(11, 0));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].counts.output, Some(400));
        assert_eq!(entries[0].counts.input, Some(10));
        assert_eq!(entries[0].counts.cache_read, Some(7));
        assert_eq!(entries[0].timestamp, ts(10, 0));
    }

    #[test]
    fn window_end_is_exclusive_and_start_inclusive() {
        let dir = tempfile::tempdir().unwrap();
        let paths = fixture(dir.path());
        let entries = read_window(&paths, ts(10, 0), ts(11, 0));
        assert_eq!(entries.len(), 2);
        let all = read_window(&paths, ts(0, 0), ts(23, 0));
        assert_eq!(all.len(), 4);
    }

    #[test]
    fn prices_each_entry_at_its_own_model() {
        let dir = tempfile::tempdir().unwrap();
        let paths = fixture(dir.path());
        let entries = read_window(&paths, ts(10, 0), ts(12, 0));
        let fallback = TokenPricingConfig::default();
        let priced = price_entries(&entries, &fallback);
        assert_eq!(priced.len(), 3);
        // Haiku subagent: 1000 in * 0.10 + 100 out * 0.50, per MTok.
        let haiku = &priced[2].1;
        assert!((haiku.cost_usd - (100.0 + 50.0) / 1e6).abs() < 1e-12);
        // Opus: 20 in, 30 out, 400 5m writes.
        let opus = &priced[1].1;
        assert!((opus.cost_usd - (20.0 * 4.0 + 30.0 * 20.0 + 400.0 * 5.0) / 1e6).abs() < 1e-12);

        let totals = aggregate_by_model(&entries, &fallback);
        assert_eq!(totals.len(), 3);
        assert_eq!(totals["claude-haiku-5-5"].entries, 1);
        assert!(!totals["claude-opus-5-5"].price_assumed);
    }

    #[test]
    fn unknown_model_entry_is_flagged_in_totals() {
        let entries = vec![UsageEntry {
            model: "claude-sonnet-4-5".into(),
            timestamp: ts(1, 0),
            counts: TokenCounts::default(),
            from_subagent: false,
        }];
        let totals = aggregate_by_model(&entries, &TokenPricingConfig::default());
        assert!(totals["claude-sonnet-4-5"].price_assumed);
        assert_eq!(totals["claude-sonnet-4-5"].effective_tokens, None);
    }
}
