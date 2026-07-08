//! Detection and surfacing of Claude Code Remote Control state from an
//! embedded Claude session's captured pane content.
//!
//! Remote Control (research preview, Claude Code v2.1.51+) bridges a local
//! session to claude.ai/code and the Claude mobile app. A connected session
//! shows a footer indicator — `/rc active` on v2.1.172+, or the older
//! `Remote Control active` — that links to the session on claude.ai. The
//! embedded TUI cannot click that link, so AMF parses the captured pane to
//! surface the state (a badge) and, when the URL appears as plain text, to
//! copy / open it.
//!
//! Note: tmux `capture-pane -e` preserves colour escapes but not OSC 8
//! hyperlink targets, so the footer link's URL is usually *not* recoverable
//! from the grid. URL extraction is therefore best-effort: it succeeds when
//! Claude prints the URL as visible text and degrades gracefully otherwise.

/// Remote Control state parsed from a captured Claude pane.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemoteControlStatus {
    /// The `/rc active` (or legacy `Remote Control active`) indicator is
    /// present — i.e. the session is bridged to claude.ai.
    pub active: bool,
    /// The claude.ai session URL, if one is visible in the pane text.
    pub url: Option<String>,
}

/// Strip ANSI / OSC escape sequences so plain-text scanning is reliable on
/// content captured with `capture-pane -e`.
fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            // CSI sequence: ESC [ ... <final byte 0x40..=0x7e>
            Some('[') => {
                chars.next();
                for d in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&d) {
                        break;
                    }
                }
            }
            // OSC sequence: ESC ] ... terminated by BEL or ST (ESC \).
            Some(']') => {
                chars.next();
                while let Some(d) = chars.next() {
                    if d == '\u{07}' {
                        break;
                    }
                    if d == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            // Other two-char escapes (e.g. ESC \, ESC =): drop the next char.
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    out
}

/// Find the first claude.ai URL in plain text, trimming trailing punctuation
/// that commonly abuts a URL in prose.
fn find_claude_url(text: &str) -> Option<String> {
    const MARKERS: [&str; 2] = ["https://claude.ai/", "http://claude.ai/"];
    let start = MARKERS.iter().filter_map(|m| text.find(m)).min()?;
    let rest = &text[start..];
    let end = rest
        .find(|c: char| {
            c.is_whitespace() || c == '"' || c == '\'' || c == '`' || c == '<' || c == '>'
        })
        .unwrap_or(rest.len());
    let url = rest[..end].trim_end_matches(['.', ',', ')', ']', '}', ';', ':']);
    if url.len() > "https://claude.ai/".len() {
        Some(url.to_string())
    } else {
        None
    }
}

/// Parse Remote Control state from captured pane text (ANSI or plain).
pub fn detect_remote_control(pane: &str) -> RemoteControlStatus {
    let text = strip_ansi(pane);
    let lower = text.to_lowercase();
    let active = lower.contains("/rc active") || lower.contains("remote control active");
    RemoteControlStatus {
        active,
        url: find_claude_url(&text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_modern_indicator() {
        let status = detect_remote_control("some output\n  ⎿ /rc active\n");
        assert!(status.active);
    }

    #[test]
    fn detects_legacy_indicator() {
        let status = detect_remote_control("Remote Control active — connected");
        assert!(status.active);
    }

    #[test]
    fn no_indicator_means_inactive() {
        let status = detect_remote_control("just a normal claude session\n> ");
        assert!(!status.active);
        assert!(status.url.is_none());
    }

    #[test]
    fn detects_indicator_through_ansi_colour_codes() {
        // `/rc active` wrapped in SGR colour sequences, as captured with -e.
        let raw = "\u{1b}[2m\u{1b}[38;5;245m/rc active\u{1b}[0m";
        assert!(detect_remote_control(raw).active);
    }

    #[test]
    fn extracts_plain_url() {
        let status = detect_remote_control(
            "Open this session at https://claude.ai/code/abc-123-def to continue.",
        );
        assert_eq!(
            status.url.as_deref(),
            Some("https://claude.ai/code/abc-123-def")
        );
    }

    #[test]
    fn extracts_url_trimming_trailing_punctuation() {
        let status = detect_remote_control("see (https://claude.ai/code/xyz).");
        assert_eq!(status.url.as_deref(), Some("https://claude.ai/code/xyz"));
    }

    #[test]
    fn extracts_url_through_ansi() {
        let raw = "\u{1b}[34mhttps://claude.ai/code/zzz\u{1b}[0m\n";
        assert_eq!(
            detect_remote_control(raw).url.as_deref(),
            Some("https://claude.ai/code/zzz")
        );
    }

    #[test]
    fn ignores_bare_domain_without_path() {
        let status = detect_remote_control("visit https://claude.ai/ for info");
        assert!(status.url.is_none());
    }

    #[test]
    fn strips_osc_hyperlink_wrapper_but_keeps_label() {
        // OSC 8 hyperlink: ESC ]8;;URL ST  label  ESC ]8;; ST
        let raw = "\u{1b}]8;;https://claude.ai/code/osc\u{1b}\\/rc active\u{1b}]8;;\u{1b}\\";
        let status = detect_remote_control(raw);
        // The label survives stripping, so the indicator is detected.
        assert!(status.active);
    }
}
