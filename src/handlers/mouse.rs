use anyhow::Result;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use std::time::Instant;

use crate::app::{App, AppMode, Selection, VisibleItem};
use crate::tmux::TmuxManager;

static mut LAST_CLICK_TIME: Option<Instant> = None;
static mut LAST_CLICK_ROW: Option<u16> = None;
const VIEW_MOUSE_SCROLL_LINES: usize = 3;

pub fn handle_mouse(app: &mut App, mouse: MouseEvent, visible_rows: u16) -> Result<()> {
    match mouse.kind {
        MouseEventKind::ScrollUp => {
            handle_scroll_up(app, visible_rows);
        }
        MouseEventKind::ScrollDown => {
            handle_scroll_down(app, visible_rows);
        }
        MouseEventKind::Down(button) => {
            handle_click(app, mouse.column, mouse.row, button, visible_rows)?;
        }
        MouseEventKind::Drag(button) => {
            handle_drag(app, mouse.column, mouse.row, button)?;
        }
        MouseEventKind::Up(button) => {
            handle_release(app, mouse.column, mouse.row, button)?;
        }
        _ => {}
    }
    Ok(())
}

fn handle_scroll_up(app: &mut App, visible_rows: u16) {
    if matches!(app.mode, AppMode::Viewing(_)) {
        handle_view_scroll(app, ScrollDirection::Up, visible_rows);
        return;
    }
    app.select_prev();
}

fn handle_scroll_down(app: &mut App, visible_rows: u16) {
    if matches!(app.mode, AppMode::Viewing(_)) {
        handle_view_scroll(app, ScrollDirection::Down, visible_rows);
        return;
    }
    app.select_next();
}

enum ScrollDirection {
    Up,
    Down,
}

fn handle_view_scroll(app: &mut App, direction: ScrollDirection, visible_rows: u16) {
    let needs_scroll_mode = matches!(&app.mode, AppMode::Viewing(view) if !view.scroll_mode);
    if needs_scroll_mode {
        app.deactivate_leader();
        app.toggle_scroll_mode(visible_rows);
    }

    let (session, window, passthrough) = match &app.mode {
        AppMode::Viewing(view) if view.scroll_mode => (
            view.session.clone(),
            view.window.clone(),
            view.scroll_passthrough,
        ),
        _ => return,
    };

    if passthrough {
        let key_name = match direction {
            ScrollDirection::Up => "PPage",
            ScrollDirection::Down => "NPage",
        };
        if let Err(err) = TmuxManager::send_key_name(&session, &window, key_name) {
            app.show_error(err);
        }
        return;
    }

    match direction {
        ScrollDirection::Up => app.scroll_up(VIEW_MOUSE_SCROLL_LINES),
        ScrollDirection::Down => app.scroll_down(VIEW_MOUSE_SCROLL_LINES, visible_rows),
    }
}

fn handle_click(
    app: &mut App,
    col: u16,
    row: u16,
    button: crossterm::event::MouseButton,
    visible_rows: u16,
) -> Result<()> {
    if let AppMode::Viewing(view) = &mut app.mode {
        if row == 0 {
            let name_start = 2;
            let name_end = name_start + view.project_name.len() as u16 + 1;
            if col >= name_start && col <= name_end {
                app.exit_view();
                return Ok(());
            }

            let pending = app.pending_inputs.len();
            if pending > 0 {
                let inputs_text = format!(
                    " | {} input{}",
                    pending,
                    if pending == 1 { "" } else { "s" }
                );
                let inputs_len = inputs_text.len() as u16;

                let mut header_len = 2;
                header_len += view.project_name.len() as u16 + 1;
                header_len += view.feature_name.len() as u16 + 2;
                header_len += view.session_label.len() as u16 + 2;
                header_len += match view.vibe_mode {
                    crate::project::VibeMode::Vibeless => 11,
                    crate::project::VibeMode::Vibe => 7,
                    crate::project::VibeMode::SuperVibe => 11,
                    crate::project::VibeMode::Review => 9,
                };
                if view.review {
                    header_len += 9;
                }
                header_len += 17;

                let inputs_start = header_len;
                let inputs_end = inputs_start + inputs_len;
                if col >= inputs_start && col < inputs_end {
                    app.mode = AppMode::NotificationPicker(0, None);
                    return Ok(());
                }
            }
            return Ok(());
        }

        if button == MouseButton::Left && row > 0 {
            app.message = None;
            let content_row = row - 1;
            view.selection.start_row = content_row;
            view.selection.start_col = col;
            view.selection.end_row = content_row;
            view.selection.end_col = col;
            view.selection.is_selecting = true;
            view.selection.has_selection = false;
        }
        return Ok(());
    }

    if matches!(app.mode, AppMode::Help(_)) {
        app.mode = AppMode::Normal;
        return Ok(());
    }

    if row == 1 && app.pending_inputs.len() > 0 {
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let prefix_len = 19 + cwd.len() as u16;
        let pending = app.pending_inputs.len();
        let badge_text = format!(
            "  [{} input request{}]",
            pending,
            if pending == 1 { "" } else { "s" }
        );
        let badge_start = prefix_len;
        let badge_end = badge_start + badge_text.len() as u16;
        if col >= badge_start && col < badge_end {
            app.mode = AppMode::NotificationPicker(0, None);
            return Ok(());
        }
    }

    if matches!(
        app.mode,
        AppMode::CreatingProject(_)
            | AppMode::CreatingFeature(_)
            | AppMode::DeletingProject(_)
            | AppMode::DeletingFeature(_, _)
            | AppMode::BrowsingPath(_)
            | AppMode::CommandPicker(_)
            | AppMode::Searching(_)
            | AppMode::OpencodeSessionPicker(_)
            | AppMode::ConfirmingOpencodeSession { .. }
            | AppMode::ClaudeSessionPicker(_)
            | AppMode::ConfirmingClaudeSession { .. }
            | AppMode::CodexSessionPicker(_)
            | AppMode::ConfirmingCodexSession { .. }
            | AppMode::SessionPicker(_)
            | AppMode::BookmarkPicker(_)
            | AppMode::SessionSwitcher(_)
            | AppMode::RenamingSession(_)
            | AppMode::RenamingFeature(_)
            | AppMode::NotificationPicker(_, _)
            | AppMode::ChangeReasonPrompt(_)
            | AppMode::RunningHook(_)
    ) {
        return Ok(());
    }

    let list_start_row = 4;
    let list_end_row = list_start_row + visible_rows;

    if row >= list_start_row && row < list_end_row {
        let clicked_in_list = row - list_start_row;
        let item_index = app.scroll_offset + clicked_in_list as usize;

        let visible = app.visible_items();
        if item_index < visible.len() {
            let clicked_item = visible[item_index].clone();

            let is_double_click = unsafe {
                let now = Instant::now();
                let is_double = LAST_CLICK_TIME
                    .map(|t| now.duration_since(t).as_millis() < 400)
                    .unwrap_or(false)
                    && LAST_CLICK_ROW == Some(row);
                LAST_CLICK_TIME = Some(now);
                LAST_CLICK_ROW = Some(row);
                is_double
            };

            if is_double_click {
                handle_double_click(app, &clicked_item, col)?;
            } else {
                app.selection = match clicked_item {
                    VisibleItem::Project(pi) => Selection::Project(pi),
                    VisibleItem::Feature(pi, fi) => Selection::Feature(pi, fi),
                    VisibleItem::Session(pi, fi, si) => Selection::Session(pi, fi, si),
                };
                app.reload_extension_config();
            }
        }
    }

    Ok(())
}

fn handle_double_click(app: &mut App, item: &VisibleItem, col: u16) -> Result<()> {
    match item {
        VisibleItem::Project(pi) => {
            if let Some(project) = app.store.projects.get_mut(*pi) {
                project.collapsed = !project.collapsed;
            }
        }
        VisibleItem::Feature(_pi, _fi) => {
            if col < 10 {
                if let Some(project) = app.store.projects.get_mut(*_pi)
                    && let Some(feature) = project.features.get_mut(*_fi)
                {
                    feature.collapsed = !feature.collapsed;
                }
            } else {
                app.enter_view()?;
            }
        }
        VisibleItem::Session(_pi, _fi, _si) => {
            app.selection = match item {
                VisibleItem::Session(pi, fi, si) => Selection::Session(*pi, *fi, *si),
                _ => unreachable!(),
            };
            app.enter_view()?;
        }
    }
    Ok(())
}

fn handle_drag(app: &mut App, col: u16, row: u16, button: MouseButton) -> Result<()> {
    if let AppMode::Viewing(view) = &mut app.mode
        && button == MouseButton::Left
        && view.selection.is_selecting
        && row > 0
    {
        let content_row = row - 1;
        view.selection.end_row = content_row;
        view.selection.end_col = col;
        view.selection.has_selection = true;
    }
    Ok(())
}

fn handle_release(app: &mut App, col: u16, row: u16, button: MouseButton) -> Result<()> {
    if let AppMode::Viewing(view) = &mut app.mode
        && button == MouseButton::Left
        && view.selection.is_selecting
    {
        view.selection.is_selecting = false;

        if view.selection.has_selection {
            if row > 0 {
                let content_row = row - 1;
                view.selection.end_row = content_row;
                view.selection.end_col = col;
            }

            let text = extract_selected_text(
                &app.pane_content,
                &view.selection,
                app.pane_content_rows,
                app.pane_content_cols,
            );

            if !text.is_empty() {
                copy_to_clipboard_osc52(&text);
                app.message = Some(format!("Copied {} chars", text.len()));
            }
        }
    }
    Ok(())
}

/// Copy text to clipboard using OSC 52 escape sequence.
/// This works in terminals that support it (most modern terminals).
fn copy_to_clipboard_osc52(text: &str) {
    use std::io::Write;
    let encoded = base64_encode(text.as_bytes());
    let _ = std::io::stdout().write_all(format!("\x1b]52;c;{}\x07", encoded).as_bytes());
    let _ = std::io::stdout().flush();
}

fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        result.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(CHARS[(triple & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

fn extract_selected_text(
    content: &str,
    selection: &crate::app::TextSelection,
    rows: u16,
    cols: u16,
) -> String {
    let (start_row, start_col, end_row, end_col) = selection.normalized();

    if rows == 0 || cols == 0 {
        return String::new();
    }

    let mut parser = vt100::Parser::new(rows, cols, 0);
    let normalized = content.replace('\n', "\r\n");
    parser.process(normalized.as_bytes());
    let screen = parser.screen();

    let mut result = String::new();

    for row in start_row..=end_row.min(rows.saturating_sub(1)) {
        let col_start = if row == start_row { start_col } else { 0 };
        let col_end = if row == end_row {
            end_col.min(cols)
        } else {
            cols
        };

        let mut line_text = String::new();
        for col in col_start..col_end {
            if let Some(cell) = screen.cell(row, col) {
                line_text.push_str(&cell.contents());
            }
        }

        let trimmed = line_text.trim_end();
        if !trimmed.is_empty() || row != end_row.min(rows.saturating_sub(1)) {
            result.push_str(trimmed);
            if row != end_row.min(rows.saturating_sub(1)) {
                result.push('\n');
            }
        }
    }

    result
}
