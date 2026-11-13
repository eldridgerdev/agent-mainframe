//! Vetting of staff-written Mermaid: extract the block, parse it with the
//! renderer, retry once with the parser's reason, then fall back to a list.
//!
//! The packet stores Mermaid source, never rendered text.

use std::sync::OnceLock;

use regex::Regex;

use crate::mermaid as m;

/// The renderer's limits as prompt text, built from the constants in
/// `src/mermaid.rs` so the prompt cannot drift from the renderer.
pub fn renderer_limits() -> String {
    format!(
        "- Diagram kinds: flowchart/graph and sequenceDiagram only.\n\
         - Flowchart: at most {} nodes, {} edges and {} subgraphs; plain \
         `-->` arrows only (no dotted, thick or other arrow heads); no links, \
         icons, images or click callbacks.\n\
         - Sequence: at most {} participants and {} events; no notes, boxes, \
         activations, autonumber, or participant creation or destruction.\n\
         - Labels: one line, at most {} characters, no `<` or `>`, no control \
         characters; plain single-width characters only.\n\
         - No `%%{{` directives or frontmatter. Source at most {} bytes and \
         {} lines.",
        m::MAX_NODES,
        m::MAX_EDGES,
        m::MAX_GROUPS,
        m::MAX_PARTICIPANTS,
        m::MAX_EVENTS,
        m::MAX_LABEL_CHARS,
        m::MAX_SOURCE_BYTES,
        m::MAX_SOURCE_LINES,
    )
}

/// The body of the first ```mermaid fence in `text`.
pub fn extract_mermaid(text: &str) -> Option<String> {
    let mut lines = text.lines();
    lines.find(|line| {
        let t = line.trim_start();
        t.starts_with("```") && t[3..].trim().eq_ignore_ascii_case("mermaid")
    })?;
    let mut body = Vec::new();
    for line in lines {
        if line.trim_start().starts_with("```") {
            let src = body.join("\n");
            return (!src.trim().is_empty()).then_some(src);
        }
        body.push(line);
    }
    // An unterminated fence is a malformed response, not a diagram.
    None
}

/// `Ok` when the viewer would draw `src`; otherwise the renderer's reason.
pub fn validate(src: &str) -> Result<(), String> {
    m::validate_source(src)
}

/// Extract and validate a model response.
pub fn check(response: &str) -> Result<String, String> {
    let src = extract_mermaid(response)
        .ok_or_else(|| "The reply contained no complete ```mermaid block".to_owned())?;
    validate(&src)?;
    Ok(src)
}

/// The one retry: the original prompt plus the parser's reason.
pub fn retry_prompt(original_prompt: &str, reason: &str) -> String {
    format!(
        "{original_prompt}\n\nYour previous diagram was rejected by the renderer: {reason}\n\
         Reply with the corrected diagram as exactly one fenced ```mermaid block, \
         within the limits above, and nothing else."
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagramOutcome {
    /// Mermaid source the renderer accepts.
    Valid(String),
    /// A Markdown bullet list standing in for a diagram that still fails.
    Fallback(String),
}

/// One retry, then a list. Pass `None` for `retry_response` when no retry was
/// made (or it produced nothing); the first response then decides alone.
/// Callers retry only when [`check`] on the first response is an `Err`.
pub fn accept(first_response: &str, retry_response: Option<&str>) -> DiagramOutcome {
    if let Ok(src) = check(first_response) {
        return DiagramOutcome::Valid(src);
    }
    if let Some(retry) = retry_response {
        if let Ok(src) = check(retry) {
            return DiagramOutcome::Valid(src);
        }
        if extract_mermaid(retry).is_some() {
            return DiagramOutcome::Fallback(fallback_list(retry));
        }
    }
    DiagramOutcome::Fallback(fallback_list(first_response))
}

fn arrow_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"-->|---|==>|-\.->").unwrap())
}

fn node_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"^([A-Za-z0-9_]+)\s*(?:[\[\(\{>]+\s*"?([^\]\)\}"]*?)"?\s*[\]\)\}]*)?$"#)
            .unwrap()
    })
}

fn message_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^(\S+?)\s*(?:-->>|->>|-->|->|--x|-x|--\)|-\))[+-]?\s*(\S+?)\s*:\s*(.*)$")
            .unwrap()
    })
}

/// A bulleted Markdown list of the diagram's connections (and any node that
/// has none). Accepts a mermaid source or a response containing a fence.
/// Best effort: it reads lines, it does not parse the grammar.
pub fn fallback_list(src_or_text: &str) -> String {
    let src = extract_mermaid(src_or_text).unwrap_or_else(|| src_or_text.to_owned());
    let mut labels: Vec<(String, String)> = Vec::new(); // id -> label, in order
    let mut edges: Vec<String> = Vec::new();
    let mut used: Vec<String> = Vec::new();
    let mut sequence = false;

    let note = |labels: &mut Vec<(String, String)>, id: &str, label: &str| match labels
        .iter_mut()
        .find(|(i, _)| i == id)
    {
        Some(entry) => {
            if entry.1 == entry.0 && !label.is_empty() {
                entry.1 = label.to_owned();
            }
        }
        None => labels.push((
            id.to_owned(),
            if label.is_empty() { id } else { label }.to_owned(),
        )),
    };

    for raw in src.lines() {
        let line = raw.trim();
        let lower = line.to_ascii_lowercase();
        if line.is_empty()
            || line.starts_with("%%")
            || [
                "subgraph",
                "end",
                "direction",
                "classdef",
                "style",
                "class ",
            ]
            .iter()
            .any(|k| lower == *k || lower.starts_with(&format!("{k} ")))
        {
            continue;
        }
        if lower.starts_with("sequencediagram") {
            sequence = true;
            continue;
        }
        if lower.starts_with("flowchart") || lower.starts_with("graph") {
            continue;
        }
        if sequence {
            for kw in ["participant ", "actor "] {
                if let Some(rest) = lower.strip_prefix(kw) {
                    let rest = &line[line.len() - rest.len()..];
                    match rest.split_once(" as ") {
                        Some((id, label)) => note(&mut labels, id.trim(), label.trim()),
                        None => note(&mut labels, rest.trim(), ""),
                    }
                }
            }
            if let Some(c) = message_re().captures(line) {
                note(&mut labels, &c[1], "");
                note(&mut labels, &c[2], "");
                used.push(c[1].to_owned());
                used.push(c[2].to_owned());
                edges.push(format!("{}\u{1}{}\u{1}{}", &c[1], &c[2], c[3].trim()));
            }
            continue;
        }
        // Flowchart: `A[x] -->|t| B --> C`.
        let mut rest = line;
        let mut prev: Option<String> = None;
        loop {
            let (segment, next) = match arrow_re().find(rest) {
                Some(found) => (&rest[..found.start()], Some(&rest[found.end()..])),
                None => (rest, None),
            };
            let mut segment = segment.trim();
            let mut edge_label = String::new();
            if prev.is_some()
                && segment.starts_with('|')
                && let Some(end) = segment[1..].find('|')
            {
                edge_label = segment[1..=end].trim().to_owned();
                segment = segment[end + 2..].trim();
            }
            if let Some(c) = node_re().captures(segment) {
                let id = c[1].to_owned();
                note(&mut labels, &id, c.get(2).map_or("", |m| m.as_str()).trim());
                if let Some(p) = &prev {
                    used.push(p.clone());
                    used.push(id.clone());
                    edges.push(format!("{p}\u{1}{id}\u{1}{edge_label}"));
                }
                prev = Some(id);
            }
            match next {
                Some(n) => rest = n,
                None => break,
            }
        }
    }

    let label_of = |id: &str| {
        labels
            .iter()
            .find(|(i, _)| i == id)
            .map_or(id, |(_, l)| l.as_str())
            .to_owned()
    };
    let mut out = Vec::new();
    for edge in &edges {
        let mut parts = edge.splitn(3, '\u{1}');
        let (from, to, text) = (
            parts.next().unwrap(),
            parts.next().unwrap(),
            parts.next().unwrap(),
        );
        out.push(if text.is_empty() {
            format!("- {} \u{2192} {}", label_of(from), label_of(to))
        } else {
            format!("- {} \u{2192} {}: {text}", label_of(from), label_of(to))
        });
    }
    for (id, label) in &labels {
        if !used.contains(id) {
            out.push(format!("- {label}"));
        }
    }
    if out.is_empty() {
        "- Diagram unavailable".to_owned()
    } else {
        out.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLOW: &str = "flowchart TD\n A[Start] --> B{Ready?}\n B -->|Yes| C[Deploy]\n";
    const SEQ: &str =
        "sequenceDiagram\n participant U as User\n participant A as AMF\n U->>A: Open plan\n";

    fn fenced(src: &str) -> String {
        format!("Here you go.\n```mermaid\n{src}\n```\nDone.")
    }

    #[test]
    fn limits_come_from_the_renderer_constants() {
        let text = renderer_limits();
        for n in [
            m::MAX_NODES,
            m::MAX_EDGES,
            m::MAX_GROUPS,
            m::MAX_PARTICIPANTS,
            m::MAX_EVENTS,
            m::MAX_LABEL_CHARS,
        ] {
            assert!(text.contains(&n.to_string()), "{n}");
        }
        assert!(text.contains("flowchart") && text.contains("sequenceDiagram"));
    }

    #[test]
    fn extracts_first_mermaid_fence_only() {
        let text = format!("```text\nx\n```\n{}\n```mermaid\nother\n```", fenced(FLOW));
        assert_eq!(extract_mermaid(&text).unwrap().trim(), FLOW.trim());
        assert_eq!(extract_mermaid("no fence"), None);
        assert_eq!(extract_mermaid("```mermaid\nflowchart TD\n A-->B"), None);
        assert_eq!(extract_mermaid("```mermaid\n```"), None);
    }

    #[test]
    fn validates_flowchart_and_sequence() {
        assert_eq!(validate(FLOW), Ok(()));
        assert_eq!(validate(SEQ), Ok(()));
    }

    #[test]
    fn rejects_invalid_unsupported_and_over_limit() {
        assert!(validate("not a diagram").is_err());
        assert!(validate("pie title x\n \"a\": 1").is_err());
        let mut big = String::from("flowchart TD\n");
        for i in 0..=m::MAX_NODES {
            big.push_str(&format!(" N{i}[Node {i}] --> N{}[Node {}]\n", i + 1, i + 1));
        }
        let reason = validate(&big).unwrap_err();
        assert!(reason.contains("limit"), "{reason}");
    }

    #[test]
    fn retry_prompt_carries_original_and_reason() {
        let p = retry_prompt("ORIGINAL", "too many nodes");
        assert!(p.starts_with("ORIGINAL"));
        assert!(p.contains("too many nodes"));
        assert!(p.contains("```mermaid"));
    }

    #[test]
    fn accept_valid_first_needs_no_retry() {
        assert_eq!(
            accept(&fenced(FLOW.trim_end()), None),
            DiagramOutcome::Valid(FLOW.trim_end().to_owned())
        );
    }

    #[test]
    fn accept_uses_retry_when_first_fails() {
        let bad = fenced("pie title x\n \"a\": 1");
        assert!(check(&bad).is_err());
        assert!(matches!(
            accept(&bad, Some(&fenced(FLOW))),
            DiagramOutcome::Valid(_)
        ));
    }

    #[test]
    fn accept_falls_back_to_a_list_after_one_retry() {
        let bad = fenced("pie title x\n \"a\": 1");
        assert!(matches!(
            accept(&bad, Some("no diagram at all")),
            DiagramOutcome::Fallback(_)
        ));
        // No retry made.
        let DiagramOutcome::Fallback(list) =
            accept(&fenced("flowchart TD\n A[One] -.-> B[Two]"), None)
        else {
            panic!("expected fallback");
        };
        assert!(list.starts_with("- "));
    }

    #[test]
    fn fallback_lists_flowchart_edges_and_loose_nodes() {
        let list = fallback_list(
            "flowchart TD\n A[Start] --> B{Ready?}\n B -->|Yes| C[Deploy]\n D[Alone]\n",
        );
        assert_eq!(
            list,
            "- Start \u{2192} Ready?\n- Ready? \u{2192} Deploy: Yes\n- Alone"
        );
    }

    #[test]
    fn fallback_lists_chains_and_sequence_messages() {
        assert_eq!(
            fallback_list("graph LR\n A[a] --> B[b] --> C[c]"),
            "- a \u{2192} b\n- b \u{2192} c"
        );
        assert_eq!(fallback_list(SEQ), "- User \u{2192} AMF: Open plan");
        assert_eq!(fallback_list("???"), "- Diagram unavailable");
    }
}
