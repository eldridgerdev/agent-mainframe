# Toast Notification System — Implementation Plan

## Context

The app already has `app.message: Option<String>` — a single sticky string shown on
the bottom line in Viewing mode and cleared by nav keys. It has no expiry and holds
only one message at a time.

The goal is a parallel `Vec<Toast>` system that:

- Renders as floating overlays in the bottom-right corner
- Auto-expires after a configurable duration
- Stacks multiple simultaneous messages
- Is themed by severity using the existing `Theme` struct

`app.message` stays for its current use cases (vim mode indicator, steering prompt
state — sticky, mode-linked feedback). Transient "action completed" messages migrate
to toasts.

## Codebase Overview

- **Language**: Rust 2024 edition
- **TUI**: ratatui 0.29 / crossterm 0.28
- **Key structs**: `App` in `src/app/mod.rs`, `Theme` in `src/theme.rs`
- **Render entry**: `ui::draw(frame, app)` → `dashboard::draw` in
  `src/ui/dashboard.rs`
- **Event loop**: `run_loop()` in `src/main.rs` — 5 ms poll (Viewing), 125 ms
  (animating), 250 ms (Normal)
- **Animation gate**: `app.has_visible_animation()` — returning true forces 125 ms
  redraws
- **Redraw gate**: `app.redraw_signature()` — a hash of relevant app state; changes
  trigger redraws

## Theme Colors Available

The `Theme` struct (`src/theme.rs:120`) has these relevant fields, all `ColorDef`
with a `.to_color() -> Color` method:

```rust
pub success: ColorDef,
pub warning: ColorDef,
pub danger: ColorDef,   // use for Error
pub info: ColorDef,
pub background: ColorDef,
pub text: ColorDef,
pub border: ColorDef,
```

## Step 1 — Data model: `src/app/toast.rs` (new file)

Create the module with `ToastKind`, `Toast`, and `impl Toast`:

```rust
use std::time::{Duration, Instant};

pub enum ToastKind {
    Success,
    Info,
    Warning,
    Error,
}

impl ToastKind {
    pub fn default_duration(&self) -> Duration {
        match self {
            ToastKind::Success => Duration::from_secs(3),
            ToastKind::Info    => Duration::from_secs(4),
            ToastKind::Warning => Duration::from_secs(6),
            ToastKind::Error   => Duration::from_secs(8),
        }
    }
}

pub struct Toast {
    pub message: String,
    pub kind: ToastKind,
    created_at: Instant,
    duration: Duration,
}

impl Toast {
    pub fn new(message: impl Into<String>, kind: ToastKind) -> Self {
        let duration = kind.default_duration();
        Self {
            message: message.into(),
            kind,
            created_at: Instant::now(),
            duration,
        }
    }

    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed() >= self.duration
    }

    /// 0.0 = just created, 1.0 = expired. Use to dim text near expiry.
    pub fn age_fraction(&self) -> f32 {
        let elapsed = self.created_at.elapsed().as_secs_f32();
        let total = self.duration.as_secs_f32();
        (elapsed / total).clamp(0.0, 1.0)
    }
}
```

Then add an `impl App` block in the same file (which is declared as a submodule of
`src/app/mod.rs`):

```rust
impl App {
    pub fn push_toast(&mut self, msg: impl Into<String>, kind: ToastKind) {
        self.toasts.push(Toast::new(msg, kind));
    }

    pub fn push_toast_success(&mut self, msg: impl Into<String>) {
        self.push_toast(msg, ToastKind::Success);
    }

    pub fn push_toast_info(&mut self, msg: impl Into<String>) {
        self.push_toast(msg, ToastKind::Info);
    }

    pub fn push_toast_warning(&mut self, msg: impl Into<String>) {
        self.push_toast(msg, ToastKind::Warning);
    }

    pub fn push_toast_error(&mut self, msg: impl Into<String>) {
        self.push_toast(msg, ToastKind::Error);
    }

    /// Remove expired toasts. Returns true if any were removed (caller sets
    /// force_redraw).
    pub fn tick_toasts(&mut self) -> bool {
        let before = self.toasts.len();
        self.toasts.retain(|t| !t.is_expired());
        self.toasts.len() != before
    }

    pub fn has_active_toasts(&self) -> bool {
        !self.toasts.is_empty()
    }
}
```

## Step 2 — App struct additions: `src/app/mod.rs`

1. Add `pub(crate) mod toast;` near the other submodule declarations at the top.
2. Add `pub use toast::{Toast, ToastKind};` to the re-exports block.
3. Add field to the `App` struct:

```rust
pub toasts: Vec<Toast>,
```

4. Initialize in `App::new()`:

```rust
toasts: Vec::new(),
```

5. Extend `has_visible_animation()` — add `|| self.has_active_toasts()` to the
   return expression so the event loop uses the 125 ms animation interval while
   toasts are on screen. Without this, Normal mode runs at 250 ms and toast expiry
   won't update the screen in a timely way.

6. Extend `redraw_signature()` — add `self.toasts.len().hash(&mut hasher);` so any
   toast appearing or expiring triggers a redraw.

## Step 3 — Event loop: `src/main.rs`

At the very top of the `run_loop` body (just after `app.throbber_state.calc_next()`),
add:

```rust
if app.tick_toasts() {
    force_redraw = true;
}
```

That is the only change to `main.rs`.

## Step 4 — Render module: `src/ui/toast.rs` (new file)

```rust
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

use crate::app::toast::{Toast, ToastKind};
use crate::theme::Theme;

pub(crate) fn draw_toasts(frame: &mut Frame, toasts: &[Toast], theme: &Theme) {
    if toasts.is_empty() {
        return;
    }

    let area = frame.area();
    let toast_width = 50_u16.min(area.width.saturating_sub(4));
    let toast_height: u16 = 3; // top border + 1 content line + bottom border
    let gap: u16 = 1;
    let step = toast_height + gap;

    // Render oldest at bottom, newest above (LIFO visual stack).
    for (i, toast) in toasts.iter().enumerate() {
        let offset = i as u16 * step;
        let y = match area.bottom().saturating_sub(2 + offset + toast_height) {
            y if y < area.top() => break, // clipped off screen — stop rendering
            y => y,
        };
        let x = area.right().saturating_sub(toast_width + 1);
        let toast_rect = Rect::new(x, y, toast_width, toast_height);

        let border_color = match toast.kind {
            ToastKind::Success => theme.success.to_color(),
            ToastKind::Info    => theme.info.to_color(),
            ToastKind::Warning => theme.warning.to_color(),
            ToastKind::Error   => theme.danger.to_color(),
        };

        let label = match toast.kind {
            ToastKind::Success => " ✓ ",
            ToastKind::Info    => " i ",
            ToastKind::Warning => " ! ",
            ToastKind::Error   => " ✕ ",
        };

        frame.render_widget(Clear, toast_rect);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .title(Span::styled(label, Style::default().fg(border_color)));

        let inner = block.inner(toast_rect);
        frame.render_widget(block, toast_rect);

        // Truncate message with ellipsis if needed.
        let max_chars = inner.width.saturating_sub(1) as usize;
        let display = if toast.message.len() > max_chars && max_chars > 1 {
            format!("{}…", &toast.message[..max_chars.saturating_sub(1)])
        } else {
            toast.message.clone()
        };

        // Dim text when close to expiry (age_fraction > 0.75).
        let mut style = Style::default().fg(theme.text.to_color());
        if toast.age_fraction() > 0.75 {
            style = style.add_modifier(Modifier::DIM);
        }

        let paragraph = Paragraph::new(Line::from(Span::styled(
            format!(" {display}"),
            style,
        )));
        frame.render_widget(paragraph, inner);
    }
}
```

## Step 5 — Wire into `src/ui/mod.rs`

Add:

```rust
mod toast;
pub(crate) use toast::draw_toasts;
```

## Step 6 — Wire into `src/ui/dashboard.rs`

Add as the **absolute last line** of the `draw()` function (after all dialog/overlay
renders, currently ending around line 944):

```rust
super::draw_toasts(frame, &app.toasts, &app.theme);
```

Because it renders last it sits on top of every other overlay — modals, pickers,
the viewing pane — with no z-order conflicts.

## Step 7 — Migrate `app.message` call sites

Replace transient `app.message = Some(...)` assignments with `push_toast_*()` calls.
Keep `app.message` only for sticky, mode-linked feedback (vim mode indicator,
steering prompt state).

### Sites to migrate

**`src/handlers/view.rs`**

```
"Refreshed statuses"          → push_toast_success
"Stopped session"             → push_toast_success
"No pending input requests"   → push_toast_warning
```

**`src/handlers/picker.rs`** (~15 sites)

```
"Applied '...'"               → push_toast_success
"No Codex session selected"   → push_toast_warning
"Sent '...'"                  → push_toast_success
"No active session..."        → push_toast_warning
"Wait for syntax operation"   → push_toast_warning
"Input request deleted"       → push_toast_success
"Cannot start: ..."           → push_toast_warning
"Added '...'"                 → push_toast_success
"Error: ..."                  → push_toast_error
"No bookmarks yet"            → push_toast_info
"No bookmarks to remove"      → push_toast_info
```

**`src/handlers/normal.rs`**

```
"No pending input requests"   → push_toast_warning
"Filter: ..."                 → push_toast_info
```

**`src/handlers/mouse.rs`**

```
"Copied N chars"              → push_toast_success
```

**`src/handlers/harness.rs`**

```
Error/status messages         → push_toast_error / push_toast_info
```

**`src/handlers/fork.rs`**

```
"Branch name cannot be empty" → push_toast_warning
```

**`src/handlers/feature_creation.rs`**

```
"No available worktrees"      → push_toast_warning
"Branch name cannot be empty" → push_toast_warning
```

**`src/handlers/dialog.rs`**

```
"Vim mode enabled/disabled"   → KEEP in app.message (mode-linked)
"Steering prompt cleared"     → push_toast_success
```

**`src/main.rs`**

```
debug alert                   → push_toast_info
```

### The app.message = None clearings

The `app.message = None` calls in `handlers/normal.rs` (lines 21, 26, 249, 253)
and `handlers/mouse.rs:168` guard the sticky cases (vim mode indicator). Leave them
as-is — they clear `app.message` on navigation, which is the correct behaviour for
the remaining sticky uses.

### Viewing-mode bottom line

After all handler sites are migrated, evaluate whether the `app.message` render block
in `dashboard.rs:615-632` (bottom line of Viewing pane) should be removed. If the
only remaining `app.message` uses are the vim/steering cases (which only occur in
`AppMode::SteeringPrompt`, not `AppMode::Viewing`), the block can be removed. If any
Viewing-mode sites still use `app.message`, keep it.

## Step 8 — Verify

1. `cargo check` — no compile errors
2. `cargo build` — clean build
3. Manual smoke tests:
   - "Copied N chars" (mouse select in Viewing mode) → green toast, 3 s, bottom-right
   - "No pending input requests" → yellow toast, 6 s
   - Two rapid actions → two stacked toasts
   - Let toasts expire → screen updates cleanly without flicker
   - Both Normal and Viewing mode — toasts visible in both
   - Terminal resize with active toasts — position recalculates correctly

## File Change Summary

| File | Change | Status |
|---|---|---|
| `src/app/toast.rs` | **New** — `Toast`, `ToastKind`, `impl Toast`, `impl App` | ✅ Done |
| `src/app/mod.rs` | Add `mod toast`, `pub toasts: Vec<Toast>`, extend `has_visible_animation`, `redraw_signature` | ✅ Done |
| `src/main.rs` | Add `tick_toasts()` call at top of run loop | ✅ Done |
| `src/ui/toast.rs` | **New** — `draw_toasts()` render function | ✅ Done |
| `src/ui/mod.rs` | Add `mod toast`, re-export `draw_toasts` | ✅ Done |
| `src/ui/dashboard.rs` | Add `draw_toasts` call as last line of `draw()` | ✅ Done |
| `src/handlers/view.rs` | Migrate 3 sites | ✅ Done |
| `src/handlers/picker.rs` | Migrate ~15 sites | ✅ Done |
| `src/handlers/normal.rs` | Migrate 2 sites | ✅ Done |
| `src/handlers/mouse.rs` | Migrate 1 site | ✅ Done |
| `src/handlers/harness.rs` | Migrate 3 sites | ✅ Done |
| `src/handlers/fork.rs` | Migrate 1 site | ✅ Done |
| `src/handlers/feature_creation.rs` | Migrate 2 sites | ✅ Done |
| `src/handlers/dialog.rs` | Migrate 1 site, keep 1 | ✅ Done |

## Implementation Notes

- `ToastKind` not re-exported from `app/mod.rs` — all callers use the
  `push_toast_{success,info,warning,error}` convenience methods instead
- `handlers/picker.rs` notification-delete case restructured to avoid
  double mutable borrow of `app` through `app.mode`
- `app/` module files (`feature_ops.rs`, `harpoon.rs`, `view.rs`, etc.)
  still use `app.message` — those were out of scope for this pass
