//! A decision record and the outbox line that carries it.
//!
//! Every agent-written field has a cap. Oversized text is truncated on a char
//! boundary with a trailing `…`, never rejected: a retry costs the agent more
//! than the truncation loses.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const TITLE_CAP: usize = 80;
pub const WHY_CAP: usize = 400;
pub const REJECTED_ITEM_CAP: usize = 160;
pub const REJECTED_MAX: usize = 3;
pub const ASK_CAP: usize = 200;
/// A developer's reply (`amf review reply`).
pub const REPLY_CAP: usize = 400;
/// `--supersedes` names an earlier record (`d4`); anything longer is noise.
pub const SUPERSEDES_CAP: usize = 32;
pub const PATHS_CAP: usize = 40;
/// Best-effort cap on the per-record dirty snapshot.
pub const DIRTY_CAP: usize = 200;

/// What kind of choice a record explains.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum DecisionKind {
    Architecture,
    Data,
    Dependency,
    Tradeoff,
    Deviation,
    Risk,
}

impl DecisionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DecisionKind::Architecture => "architecture",
            DecisionKind::Data => "data",
            DecisionKind::Dependency => "dependency",
            DecisionKind::Tradeoff => "tradeoff",
            DecisionKind::Deviation => "deviation",
            DecisionKind::Risk => "risk",
        }
    }
}

/// The agent-written fields of one decision, as given on the command line.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DecisionText {
    pub title: String,
    pub why: String,
    pub rejected: Vec<String>,
    pub ask: Option<String>,
    pub supersedes: Option<String>,
}

/// Trim `s` and cap it at `cap` chars. Over the cap, keeps `cap - 1` chars and
/// appends `…`, so the result is exactly `cap` chars. Returns whether it cut.
pub fn truncate_chars(s: &str, cap: usize) -> (String, bool) {
    let trimmed = s.trim();
    if trimmed.chars().count() <= cap {
        return (trimmed.to_string(), false);
    }
    let keep = cap.saturating_sub(1);
    let end = trimmed
        .char_indices()
        .nth(keep)
        .map_or(trimmed.len(), |(i, _)| i);
    let mut out = trimmed[..end].trim_end().to_string();
    out.push('…');
    (out, true)
}

/// Apply every cap. Empty optional fields become `None`; extra `rejected`
/// entries beyond [`REJECTED_MAX`] are dropped. Returns whether anything the
/// agent wrote was cut.
pub fn apply_caps(input: DecisionText) -> (DecisionText, bool) {
    let mut truncated = false;
    let mut cap = |s: &str, n: usize| {
        let (out, cut) = truncate_chars(s, n);
        truncated |= cut;
        out
    };
    let title = cap(&input.title, TITLE_CAP);
    let why = cap(&input.why, WHY_CAP);
    let mut rejected: Vec<String> = input
        .rejected
        .iter()
        .map(|r| cap(r, REJECTED_ITEM_CAP))
        .filter(|r| !r.is_empty())
        .collect();
    let ask = input
        .ask
        .as_deref()
        .map(|a| cap(a, ASK_CAP))
        .filter(|a| !a.is_empty());
    let supersedes = input
        .supersedes
        .as_deref()
        .map(|s| cap(s, SUPERSEDES_CAP))
        .filter(|s| !s.is_empty());
    if rejected.len() > REJECTED_MAX {
        rejected.truncate(REJECTED_MAX);
        truncated = true;
    }
    (
        DecisionText {
            title,
            why,
            rejected,
            ask,
            supersedes,
        },
        truncated,
    )
}

/// One decision as written to the outbox.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub uid: String,
    pub seq: u32,
    pub kind: DecisionKind,
    pub title: String,
    pub why: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rejected: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    #[serde(default)]
    pub paths: Vec<String>,
    /// Uncommitted files at record time (path to `git hash-object` blob id,
    /// or `"deleted"`), capped at [`DIRTY_CAP`]. Capture-only: the next record
    /// diffs against it so a file is attributed to the decision that
    /// introduced or changed it, not to every later one. Not stored in the DB.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dirty: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub harness: String,
    /// RFC 3339.
    pub ts: String,
}

/// The sequence number a `--supersedes` value names: `d4`, `D4` or `4`.
pub fn parse_decision_ref(text: &str) -> Option<i64> {
    let text = text.trim();
    let digits = text.strip_prefix(['d', 'D']).unwrap_or(text);
    if digits.is_empty() || digits.len() > 9 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().filter(|n| *n > 0)
}

/// How the developer answers one brief item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReplyKind {
    /// A change item was done.
    Done,
    /// A change item was declined, with the reason.
    Pushback,
    /// A question item was answered.
    Answer,
}

impl ReplyKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ReplyKind::Done => "done",
            ReplyKind::Pushback => "pushback",
            ReplyKind::Answer => "answer",
        }
    }
}

/// A developer's reply to one brief item, as written to the outbox. The CLI
/// does not know the round, so ingest matches the newest round that has
/// `label`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplyRecord {
    pub uid: String,
    /// `C<n>` or `Q<n>`, upper case.
    pub label: String,
    pub kind: ReplyKind,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// RFC 3339.
    pub ts: String,
}

/// One line of `outbox.jsonl`, tagged by `type`: decisions
/// (`amf decision add`) and replies (`amf review reply`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum OutboxLine {
    Decision(DecisionRecord),
    Reply(ReplyRecord),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_trimmed_not_truncated() {
        assert_eq!(
            truncate_chars("  hello  ", 80),
            ("hello".to_string(), false)
        );
    }

    #[test]
    fn long_text_is_cut_to_the_cap_with_an_ellipsis() {
        let (out, cut) = truncate_chars(&"a".repeat(100), 80);
        assert!(cut);
        assert_eq!(out.chars().count(), 80);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn multibyte_text_is_cut_on_a_char_boundary() {
        let text = "é".repeat(10) + &"日本".repeat(50);
        let (out, cut) = truncate_chars(&text, 15);
        assert!(cut);
        assert_eq!(out.chars().count(), 15);
        assert!(out.starts_with("éééééééééé日本"));
        assert!(out.ends_with('…'));
        // Exactly at the cap is not truncated.
        let (out, cut) = truncate_chars(&"日".repeat(80), 80);
        assert!(!cut);
        assert_eq!(out.chars().count(), 80);
    }

    #[test]
    fn caps_apply_to_every_field_and_drop_extra_rejected() {
        let (out, truncated) = apply_caps(DecisionText {
            title: "t".repeat(81),
            why: "w".repeat(401),
            rejected: vec!["r".repeat(161), "two".into(), "three".into(), "four".into()],
            ask: Some("a".repeat(201)),
            supersedes: Some(" d4 ".into()),
        });
        assert!(truncated);
        assert_eq!(out.title.chars().count(), TITLE_CAP);
        assert_eq!(out.why.chars().count(), WHY_CAP);
        assert_eq!(out.rejected.len(), REJECTED_MAX);
        assert_eq!(out.rejected[0].chars().count(), REJECTED_ITEM_CAP);
        assert_eq!(out.rejected[2], "three");
        assert_eq!(out.ask.unwrap().chars().count(), ASK_CAP);
        assert_eq!(out.supersedes.as_deref(), Some("d4"));
    }

    #[test]
    fn within_caps_reports_no_truncation_and_drops_empty_options() {
        let (out, truncated) = apply_caps(DecisionText {
            title: "Outbox".into(),
            why: "No DB writes from the agent".into(),
            rejected: vec!["  ".into()],
            ask: Some("".into()),
            supersedes: None,
        });
        assert!(!truncated);
        assert!(out.rejected.is_empty());
        assert_eq!(out.ask, None);
    }

    #[test]
    fn outbox_line_is_tagged_as_a_decision() {
        let line = OutboxLine::Decision(DecisionRecord {
            uid: "u1".into(),
            seq: 1,
            kind: DecisionKind::Tradeoff,
            title: "t".into(),
            why: "w".into(),
            rejected: vec![],
            ask: None,
            supersedes: None,
            paths: vec!["src/a.rs".into()],
            dirty: BTreeMap::new(),
            head: Some("abc".into()),
            session_id: None,
            harness: "claude".into(),
            ts: "2026-11-13T00:00:00Z".into(),
        });
        let json = serde_json::to_string(&line).unwrap();
        assert!(json.starts_with(r#"{"type":"decision","#), "{json}");
        assert!(json.contains(r#""kind":"tradeoff""#));
        assert!(!json.contains("dirty"), "{json}");
        assert_eq!(serde_json::from_str::<OutboxLine>(&json).unwrap(), line);
    }

    #[test]
    fn decision_refs_parse_with_or_without_the_d() {
        assert_eq!(parse_decision_ref("d4"), Some(4));
        assert_eq!(parse_decision_ref(" D12 "), Some(12));
        assert_eq!(parse_decision_ref("7"), Some(7));
        for bad in ["", "d", "d0", "x4", "d4x", "d-1"] {
            assert_eq!(parse_decision_ref(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn reply_line_round_trips_with_a_lowercase_kind() {
        let line = OutboxLine::Reply(ReplyRecord {
            uid: "r1".into(),
            label: "C1".into(),
            kind: ReplyKind::Pushback,
            text: "not worth it".into(),
            session_id: None,
            ts: "2026-11-13T00:00:00Z".into(),
        });
        let json = serde_json::to_string(&line).unwrap();
        assert!(json.starts_with(r#"{"type":"reply","#), "{json}");
        assert!(json.contains(r#""kind":"pushback""#));
        assert_eq!(serde_json::from_str::<OutboxLine>(&json).unwrap(), line);
    }
}
