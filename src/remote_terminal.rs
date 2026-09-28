//! Terminal access for the Remote Control PWA (Phase 3): stream a session's
//! tmux pane to the phone over a WebSocket and forward the phone's input
//! back into it.
//!
//! Which pane a session id means is decided by the main loop: `App` puts a
//! `session id → PaneTarget` table in every status snapshot it publishes
//! (`RemoteStatusSnapshot::pane_targets`), listing only tmux-backed sessions
//! of running features. The server thread never reads `App`; it looks the
//! target up in the latest snapshot and talks to tmux through [`PaneIo`].
//! Capturing and sending keys are stateless tmux calls (the persistent input
//! client is behind its own mutex), the same ones AMF's embedded view makes
//! from its own background thread — so this adds no shared state with the
//! main loop.
//!
//! Sharing is deliberately naive, per the plan's concurrent-access decision:
//! the phone and the desk type into the same pane, last keystroke wins.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use axum::extract::ws::{Message, WebSocket};
use serde::{Deserialize, Serialize};

/// A tmux pane, as `session:window`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneTarget {
    pub session: String,
    pub window: String,
}

/// One unit of input for a pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneInput {
    /// Typed text with no control characters.
    Literal(String),
    /// A tmux key name (`Enter`, `C-c`, `Up`, …) — always one that passed
    /// [`is_valid_key_name`], since it is spliced into a tmux command.
    Key(String),
    /// Multi-line text, delivered as a bracketed paste so an agent's input
    /// box receives it as one message instead of submitting line by line.
    Paste(String),
}

/// A captured screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PaneFrame {
    pub cols: u16,
    pub rows: u16,
    pub cursor_x: u16,
    pub cursor_y: u16,
    pub cursor_visible: bool,
    /// The visible screen, rows separated by `\n`, with SGR escapes.
    pub ansi: String,
}

/// How the server reaches tmux; a fake in tests.
pub trait PaneIo: Send + Sync {
    fn capture(&self, target: &PaneTarget) -> Result<PaneFrame>;
    fn send(&self, target: &PaneTarget, input: &PaneInput) -> Result<()>;
    /// The last `lines` lines of scrollback plus the screen, with SGR.
    fn history(&self, target: &PaneTarget, lines: u32) -> Result<String>;
}

/// The real thing.
pub struct TmuxPaneIo;

impl PaneIo for TmuxPaneIo {
    fn capture(&self, target: &PaneTarget) -> Result<PaneFrame> {
        use crate::tmux::TmuxManager;
        let (cols, rows, cursor_x, cursor_y, cursor_visible) =
            TmuxManager::pane_geometry(&target.session, &target.window)?;
        let ansi = TmuxManager::capture_pane_ansi(&target.session, &target.window)?;
        Ok(PaneFrame {
            cols,
            rows,
            cursor_x,
            cursor_y,
            cursor_visible,
            ansi: ansi.trim_end_matches('\n').to_string(),
        })
    }

    fn history(&self, target: &PaneTarget, lines: u32) -> Result<String> {
        let lines = i32::try_from(lines).unwrap_or(i32::MAX);
        crate::tmux::TmuxManager::capture_pane_with_history(&target.session, &target.window, lines)
            .map(|(text, _)| text.trim_end_matches('\n').to_string())
    }

    fn send(&self, target: &PaneTarget, input: &PaneInput) -> Result<()> {
        use crate::tmux::TmuxManager;
        match input {
            PaneInput::Literal(text) => {
                TmuxManager::send_literal(&target.session, &target.window, text)
            }
            PaneInput::Key(name) => {
                TmuxManager::send_key_name(&target.session, &target.window, name)
            }
            PaneInput::Paste(text) => {
                TmuxManager::paste_text(&target.session, &target.window, text)
            }
        }
    }
}

/// Whether `name` is safe to hand tmux as a key name. Key names are spliced
/// unquoted into `send-keys` commands, so anything beyond tmux's own key
/// vocabulary (letters, digits, `-`, e.g. `C-c`, `M-x`, `BSpace`, `F12`)
/// is refused rather than escaped.
pub fn is_valid_key_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 16
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && !name.starts_with('-')
}

/// Translate raw terminal input — what xterm.js's `onData` produces, the
/// bytes a real terminal would send — into tmux keys. tmux's `send-keys -l`
/// can't carry control characters through AMF's control-mode input client,
/// so every escape sequence and control byte becomes a named key and only
/// printable runs travel as literal text. Unknown escape sequences are
/// dropped rather than typed into the pane as garbage.
pub fn translate_input(data: &str) -> Vec<PaneInput> {
    let mut out = Vec::new();
    let mut literal = String::new();
    let flush = |literal: &mut String, out: &mut Vec<PaneInput>| {
        if !literal.is_empty() {
            out.push(PaneInput::Literal(std::mem::take(literal)));
        }
    };
    let key = |name: &str| PaneInput::Key(name.to_string());

    let chars: Vec<char> = data.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\x1b' {
            flush(&mut literal, &mut out);
            let (consumed, name) = escape_sequence(&chars[i..]);
            if let Some(name) = name {
                out.push(PaneInput::Key(name));
            }
            i += consumed;
            continue;
        }
        let named = match c {
            '\r' | '\n' => Some("Enter".to_string()),
            '\t' => Some("Tab".to_string()),
            '\x7f' | '\x08' => Some("BSpace".to_string()),
            '\0' => Some("C-Space".to_string()),
            '\x01'..='\x1a' => Some(format!("C-{}", (b'a' + c as u8 - 1) as char)),
            '\x1c'..='\x1f' => None, // rare; nothing sensible to map to
            _ => {
                literal.push(c);
                i += 1;
                continue;
            }
        };
        flush(&mut literal, &mut out);
        if let Some(name) = named {
            out.push(key(&name));
        }
        i += 1;
    }
    flush(&mut literal, &mut out);
    out
}

/// Decode one escape sequence at the start of `chars` (which begins with
/// ESC). Returns how many chars it spans and the tmux key, if it maps to one.
fn escape_sequence(chars: &[char]) -> (usize, Option<String>) {
    let named = |n: &str| Some(n.to_string());
    match chars.get(1) {
        None => (1, named("Escape")),
        Some('[') | Some('O') => {
            // CSI / SS3: parameters, then a final byte in @..~.
            let mut end = 2;
            while end < chars.len() && !('@'..='~').contains(&chars[end]) {
                end += 1;
            }
            if end >= chars.len() {
                return (chars.len(), None);
            }
            let params: String = chars[2..end].iter().collect();
            let key = match (params.as_str(), chars[end]) {
                ("", 'A') | ("1", 'A') => named("Up"),
                ("", 'B') | ("1", 'B') => named("Down"),
                ("", 'C') | ("1", 'C') => named("Right"),
                ("", 'D') | ("1", 'D') => named("Left"),
                ("", 'H') | ("1", 'H') | ("1", '~') | ("7", '~') => named("Home"),
                ("", 'F') | ("1", 'F') | ("4", '~') | ("8", '~') => named("End"),
                ("", 'Z') => named("BTab"),
                ("2", '~') => named("IC"),
                ("3", '~') => named("DC"),
                ("5", '~') => named("PPage"),
                ("6", '~') => named("NPage"),
                ("", 'P') => named("F1"),
                ("", 'Q') => named("F2"),
                ("", 'R') => named("F3"),
                ("", 'S') => named("F4"),
                _ => None,
            };
            (end + 1, key)
        }
        // ESC followed by a printable character is Alt/Meta+key.
        Some(&c) if c.is_ascii_alphanumeric() => (2, Some(format!("M-{c}"))),
        Some('\x1b') => (1, named("Escape")),
        Some(_) => (1, named("Escape")),
    }
}

/// Messages the phone sends on a terminal socket.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// First message on every socket: browsers can't set an
    /// `Authorization` header on a WebSocket, and a token in the URL would
    /// end up in proxy logs.
    Auth { token: String },
    /// Raw terminal input from xterm.js.
    Input { data: String },
    /// Text from the simple view's input box, optionally followed by Enter.
    Text { text: String, submit: bool },
    /// A named key from a quick-key button.
    Key { name: String },
    /// Ask for scrollback: the last `lines` lines (capped at
    /// [`MAX_HISTORY_LINES`]).
    History { lines: u32 },
}

/// The most scrollback one request can ask for.
pub const MAX_HISTORY_LINES: u32 = 5000;

/// Messages the server sends.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage<'a> {
    Frame(&'a PaneFrame),
    /// Scrollback, answering a `History` request.
    History {
        ansi: &'a str,
    },
    /// The session is gone (feature stopped, window closed) or never
    /// existed; the socket closes after this.
    Gone {
        message: &'a str,
    },
    Error {
        message: &'a str,
    },
}

/// Turn a simple-view text submission into pane input.
pub fn text_input(text: &str, submit: bool) -> Vec<PaneInput> {
    let mut out = Vec::new();
    if text.contains('\n') {
        out.push(PaneInput::Paste(text.to_string()));
    } else if !text.is_empty() {
        out.push(PaneInput::Literal(text.to_string()));
    }
    if submit {
        out.push(PaneInput::Key("Enter".to_string()));
    }
    out
}

/// How often a socket re-captures its pane while nothing is happening.
const IDLE_FRAME_INTERVAL: Duration = Duration::from_millis(250);
/// How soon after input to re-capture, so typing echoes quickly.
const ECHO_DELAY: Duration = Duration::from_millis(40);
const AUTH_TIMEOUT: Duration = Duration::from_secs(10);

/// What a terminal socket needs from the server: who is allowed in, and
/// where a session id currently points.
pub trait TerminalContext: Send + Sync + 'static {
    /// The device id for a token, if it is authorized. Counts as the
    /// device being seen.
    fn authorize(&self, token: &str) -> Option<String>;
    /// Whether a token is still authorized — checked while a socket stays
    /// open, so a revoke cuts off a live terminal too.
    fn still_authorized(&self, token: &str) -> bool;
    /// The pane a session id currently maps to, if it is live.
    fn resolve(&self, session_id: &str) -> Option<PaneTarget>;
}

async fn send_json(socket: &mut WebSocket, message: &ServerMessage<'_>) -> bool {
    let text = serde_json::to_string(message).unwrap_or_default();
    socket.send(Message::Text(text.into())).await.is_ok()
}

async fn send_inputs(
    io: &Arc<dyn PaneIo>,
    target: &PaneTarget,
    inputs: Vec<PaneInput>,
) -> Result<()> {
    if inputs.is_empty() {
        return Ok(());
    }
    let io = io.clone();
    let target = target.clone();
    tokio::task::spawn_blocking(move || {
        for input in &inputs {
            io.send(&target, input)?;
        }
        Ok(())
    })
    .await?
}

/// Serve one terminal socket until the phone disconnects or the session
/// goes away.
pub async fn run_terminal_socket(
    mut socket: WebSocket,
    session_id: String,
    context: Arc<dyn TerminalContext>,
    io: Arc<dyn PaneIo>,
) {
    // Authenticate before anything about the session is revealed.
    let token = match tokio::time::timeout(AUTH_TIMEOUT, socket.recv()).await {
        Ok(Some(Ok(Message::Text(text)))) => match serde_json::from_str(&text) {
            Ok(ClientMessage::Auth { token }) if context.authorize(&token).is_some() => Some(token),
            _ => None,
        },
        _ => None,
    };
    let Some(token) = token else {
        let _ = send_json(
            &mut socket,
            &ServerMessage::Error {
                message: "unauthorized",
            },
        )
        .await;
        return;
    };

    let mut last: Option<PaneFrame> = None;
    let mut next_capture = tokio::time::Instant::now();
    loop {
        if !context.still_authorized(&token) {
            let _ = send_json(
                &mut socket,
                &ServerMessage::Error {
                    message: "unauthorized",
                },
            )
            .await;
            return;
        }
        let Some(target) = context.resolve(&session_id) else {
            let _ = send_json(
                &mut socket,
                &ServerMessage::Gone {
                    message: "This session isn't running.",
                },
            )
            .await;
            return;
        };

        tokio::select! {
            _ = tokio::time::sleep_until(next_capture) => {
                let io = io.clone();
                let capture_target = target.clone();
                let frame = tokio::task::spawn_blocking(move || io.capture(&capture_target)).await;
                // A failed capture usually means the window just closed;
                // the next resolve() decides whether that's the end.
                if let Ok(Ok(frame)) = frame
                    && last.as_ref() != Some(&frame)
                {
                    if !send_json(&mut socket, &ServerMessage::Frame(&frame)).await {
                        return;
                    }
                    last = Some(frame);
                }
                next_capture = tokio::time::Instant::now() + IDLE_FRAME_INTERVAL;
            }
            incoming = socket.recv() => {
                let text = match incoming {
                    Some(Ok(Message::Text(text))) => text,
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                    Some(Ok(_)) => continue,
                };
                let inputs = match serde_json::from_str::<ClientMessage>(&text) {
                    Ok(ClientMessage::Input { data }) => translate_input(&data),
                    Ok(ClientMessage::Text { text, submit }) => text_input(&text, submit),
                    Ok(ClientMessage::Key { name }) if is_valid_key_name(&name) => {
                        vec![PaneInput::Key(name)]
                    }
                    Ok(ClientMessage::Key { .. }) => {
                        let _ = send_json(&mut socket, &ServerMessage::Error { message: "unknown key" }).await;
                        continue;
                    }
                    Ok(ClientMessage::Auth { .. }) => continue,
                    Ok(ClientMessage::History { lines }) => {
                        let io = io.clone();
                        let history_target = target.clone();
                        let lines = lines.min(MAX_HISTORY_LINES);
                        let history = tokio::task::spawn_blocking(move || {
                            io.history(&history_target, lines)
                        })
                        .await;
                        let sent = match history {
                            Ok(Ok(ansi)) => {
                                send_json(&mut socket, &ServerMessage::History { ansi: &ansi }).await
                            }
                            _ => {
                                send_json(
                                    &mut socket,
                                    &ServerMessage::Error { message: "Couldn't read the scrollback." },
                                )
                                .await
                            }
                        };
                        if !sent {
                            return;
                        }
                        continue;
                    }
                    Err(_) => {
                        let _ = send_json(&mut socket, &ServerMessage::Error { message: "bad message" }).await;
                        continue;
                    }
                };
                if let Err(e) = send_inputs(&io, &target, inputs).await {
                    let message = format!("Couldn't send input: {e}");
                    let _ = send_json(&mut socket, &ServerMessage::Error { message: &message }).await;
                }
                next_capture = tokio::time::Instant::now() + ECHO_DELAY;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(names: &[&str]) -> Vec<PaneInput> {
        names
            .iter()
            .map(|n| PaneInput::Key(n.to_string()))
            .collect()
    }

    #[test]
    fn printable_text_stays_literal() {
        assert_eq!(
            translate_input("hello world"),
            vec![PaneInput::Literal("hello world".into())]
        );
    }

    #[test]
    fn control_bytes_become_named_keys() {
        assert_eq!(
            translate_input("ls\r"),
            vec![
                PaneInput::Literal("ls".into()),
                PaneInput::Key("Enter".into())
            ]
        );
        assert_eq!(
            translate_input("\x03\x7f\t"),
            keys(&["C-c", "BSpace", "Tab"])
        );
    }

    #[test]
    fn escape_sequences_become_named_keys() {
        assert_eq!(
            translate_input("\x1b[A\x1b[B\x1bOC\x1b[D\x1b[3~\x1b[Z\x1b[5~"),
            keys(&["Up", "Down", "Right", "Left", "DC", "BTab", "PPage"])
        );
        assert_eq!(translate_input("\x1b"), keys(&["Escape"]));
        assert_eq!(translate_input("\x1bx"), keys(&["M-x"]));
    }

    #[test]
    fn unknown_escape_sequences_are_dropped_not_typed() {
        assert_eq!(
            translate_input("a\x1b[99;5ub"),
            vec![
                PaneInput::Literal("a".into()),
                PaneInput::Literal("b".into())
            ]
        );
    }

    #[test]
    fn key_names_are_restricted_to_tmux_vocabulary() {
        for ok in ["Enter", "C-c", "M-x", "BSpace", "F12", "Up"] {
            assert!(is_valid_key_name(ok), "{ok}");
        }
        for bad in [
            "",
            "Enter; kill-server",
            "a b",
            "-t",
            "C-c\n",
            "x".repeat(17).as_str(),
        ] {
            assert!(!is_valid_key_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn text_input_pastes_multiline_and_submits() {
        assert_eq!(
            text_input("yes", true),
            vec![
                PaneInput::Literal("yes".into()),
                PaneInput::Key("Enter".into())
            ]
        );
        assert_eq!(
            text_input("line 1\nline 2", false),
            vec![PaneInput::Paste("line 1\nline 2".into())]
        );
        assert_eq!(text_input("", true), keys(&["Enter"]));
    }
}
