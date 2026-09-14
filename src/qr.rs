//! Minimal QR-code-to-terminal-text rendering for the Remote Control
//! pairing dialog (`docs/backlog/remote-control-companion-app-plan.md`,
//! Epic 4). Half-block glyphs pack two QR module rows into one terminal
//! cell row, keeping the code roughly square despite non-square terminal
//! cells — the same approach sketched in
//! `docs/backlog/remote-control-qr-overlay-plan.md` for the (separate)
//! session-URL QR overlay; this module is the first real use of it.

use qrcode::QrCode;
use qrcode::render::unicode;

/// Render `data` as a QR code using half-block Unicode glyphs (one `String`
/// per output row), with the mandatory quiet zone included. Returns `None`
/// if the crate can't encode `data` at all — practically unreachable for
/// our short pairing payloads, but the caller degrades to showing the
/// numeric code alone rather than panicking or showing a blank box.
pub fn render_qr_lines(data: &str) -> Option<Vec<String>> {
    let code = QrCode::new(data.as_bytes()).ok()?;
    let rendered = code
        .render::<unicode::Dense1x2>()
        .quiet_zone(true)
        .module_dimensions(1, 1)
        .build();
    Some(rendered.lines().map(str::to_string).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_a_nonempty_square_ish_block_for_a_short_payload() {
        let lines = render_qr_lines("amf-pair://127.0.0.1:54321?code=123456")
            .expect("short payload should always encode");
        assert!(!lines.is_empty());
        // Half-block rendering packs 2 module rows per line, so lines are
        // shorter than they are numerous relative to module count, but all
        // rows should still share one width.
        let width = lines[0].chars().count();
        assert!(width > 0);
        for line in &lines {
            assert_eq!(line.chars().count(), width);
        }
    }

    #[test]
    fn empty_payload_still_encodes() {
        assert!(render_qr_lines("").is_some());
    }
}
