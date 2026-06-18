# Remote Control — QR code overlay

- **Status:** Backlog
- **Owner:** unassigned
- **Relates to:** interactive Remote Control, shipped in v0.24.0 (see
  `CHANGELOG.md`); [server mode](remote-control-server-mode-plan.md)
  (the QR pays off most there); `app::remote_control` (URL detection)

## Why / problem

When a Claude session is bridged to claude.ai via Remote Control, the
fastest way to pick it up on a phone is to scan a code — that is exactly
what standalone `claude remote-control` server mode offers (press space
for a QR). AMF's embedded TUI can't click the footer link, and copying a
URL to then send it to a phone is clumsy. A QR overlay rendered in the
TUI lets you point your phone at the screen and open the session
directly.

This was noted as a stretch goal during the Phase 3 runtime-control work
and deferred; this doc captures it as its own item.

## Proposed design

- **URL source** (the gating dependency):
  - Server mode prints a stable session URL as plain text — the reliable
    source, and why the QR pays off most alongside
    [server mode](remote-control-server-mode-plan.md).
  - Interactive mode: `app::remote_control::detect_remote_control()`
    already does a best-effort scan, but tmux `capture-pane` usually
    drops the OSC 8 hyperlink target, so the URL is often unavailable.
    The overlay must degrade gracefully (toast "no URL to encode yet")
    rather than show an empty/oversized code.
- **Encoding:** add a small pure-Rust QR dependency (e.g. `qrcode` or
  `qrcodegen`) to turn the URL into a module matrix. No network, no
  system deps.
- **Rendering:** draw the matrix in a centered overlay using half-block
  glyphs (`▀` / `▄` / `█` / space) so two QR rows map to one terminal
  cell row, keeping the code roughly square given terminal cell aspect.
  Include the mandatory quiet-zone (≥4 modules) and pick the smallest
  size that stays scannable in the available area; show the URL as text
  beneath for fallback.
- **Trigger / mode:** a new `AppMode::RemoteControlQr(String)` overlay
  opened from a leader command in a Claude RC view (and later from the
  server-mode row). `Esc` / `q` closes. Reuses the URL-resolution path
  behind the existing copy/open commands (`c` / `O`).

## Progress

Nothing started — design only. Implementation items:

- [ ] Choose + add a pure-Rust QR-encoding dependency
- [ ] `AppMode::RemoteControlQr(url)` + key handler (open/close)
- [ ] Half-block QR renderer with quiet zone + size-to-fit
- [ ] Leader command to open the overlay from a Claude RC view
- [ ] Reuse URL resolution; graceful toast when no URL is available
- [ ] Wire into the server-mode row once that lands
- [ ] Tests: matrix-to-glyph rendering against known fixtures

## Open questions

- Which QR crate (size, license, no-deps, error-correction control)?
- Minimum module size / quiet zone that reliably scans from a phone
  pointed at a terminal, across themes and font sizes.
- Full-block vs half-block trade-off given non-square terminal cells.
- In interactive mode, is there any reliable way to recover the session
  URL when `capture-pane` drops the OSC 8 target? If not, gate the
  overlay to contexts where the URL is known (server mode, or a pasted
  URL).

## Reasoning / when to build

Build alongside [server mode](remote-control-server-mode-plan.md), where
a stable plain-text URL makes the QR reliably useful. In interactive
mode alone the value is limited by URL availability, so it is not worth
the dependency + render mode on its own. If a reliable interactive URL
source appears, it becomes worthwhile independently.
