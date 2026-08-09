use anyhow::Result;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use std::time::Instant;

use crate::app::{App, AppMode, CreateFeatureStep, Selection, VisibleItem};
use crate::project::AgentKind;
use crate::tmux::TmuxManager;

static mut LAST_CLICK_TIME: Option<Instant> = None;
static mut LAST_CLICK_ROW: Option<u16> = None;
const VIEW_MOUSE_SCROLL_LINES: usize = 3;
const DEBUG_LOG_MOUSE_SCROLL_LINES: usize = 3;
const MARKDOWN_MOUSE_SCROLL_LINES: usize = 3;
const HELP_MOUSE_SCROLL_LINES: usize = 3;

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
        MouseEventKind::Moved => {
            handle_move(app, mouse.row, visible_rows);
        }
        _ => {}
    }
    Ok(())
}

fn handle_move(app: &mut App, row: u16, visible_rows: u16) {
    let AppMode::CreatingFeature(state) = &mut app.mode else {
        return;
    };
    if state.step != CreateFeatureStep::Mode {
        return;
    }

    let full_height = visible_rows.saturating_add(3);
    let dialog_top = full_height.saturating_mul(5) / 100;
    let inner_top = dialog_top.saturating_add(1);
    let relative_row = row.saturating_sub(inner_top);

    state.mode_focus = match relative_row {
        8..=11 => 0,
        13..=17 => 1,
        19 => 2,
        20 => 3,
        21 if state.agent == AgentKind::Claude => 4,
        22 if state.agent == AgentKind::Claude => 5,
        21 => 4,
        23 if state.agent == AgentKind::Claude => 6,
        _ => state.mode_focus,
    };
}

fn handle_scroll_up(app: &mut App, visible_rows: u16) {
    if matches!(
        app.mode,
        AppMode::DiffPicker(_) | AppMode::DiffViewer(_) | AppMode::DiffViewerLoading(_)
    ) {
        return;
    }
    if matches!(app.mode, AppMode::MarkdownLoading(_)) {
        return;
    }
    if matches!(app.mode, AppMode::DiffReviewPrompt(_)) {
        app.diff_review_scroll_patch_up(VIEW_MOUSE_SCROLL_LINES);
        return;
    }
    if let AppMode::DebugLog(state) = &mut app.mode {
        state.scroll_offset = state
            .scroll_offset
            .saturating_sub(DEBUG_LOG_MOUSE_SCROLL_LINES);
        return;
    }
    if let AppMode::MarkdownViewer(state) = &mut app.mode {
        state.scroll_offset = state
            .scroll_offset
            .saturating_sub(MARKDOWN_MOUSE_SCROLL_LINES);
        return;
    }
    if let AppMode::Help(state) = &mut app.mode {
        state.scroll_offset = state.scroll_offset.saturating_sub(HELP_MOUSE_SCROLL_LINES);
        return;
    }
    if matches!(app.mode, AppMode::Viewing(_)) {
        handle_view_scroll(app, ScrollDirection::Up, visible_rows);
        return;
    }
    app.select_prev();
}

fn handle_scroll_down(app: &mut App, visible_rows: u16) {
    if matches!(
        app.mode,
        AppMode::DiffPicker(_) | AppMode::DiffViewer(_) | AppMode::DiffViewerLoading(_)
    ) {
        return;
    }
    if matches!(app.mode, AppMode::MarkdownLoading(_)) {
        return;
    }
    if matches!(app.mode, AppMode::DiffReviewPrompt(_)) {
        app.diff_review_scroll_patch_down(VIEW_MOUSE_SCROLL_LINES);
        return;
    }
    if let AppMode::DebugLog(state) = &mut app.mode {
        state.scroll_offset = state
            .scroll_offset
            .saturating_add(DEBUG_LOG_MOUSE_SCROLL_LINES);
        return;
    }
    if let AppMode::MarkdownViewer(state) = &mut app.mode {
        state.scroll_offset = state
            .scroll_offset
            .saturating_add(MARKDOWN_MOUSE_SCROLL_LINES);
        return;
    }
    if let AppMode::Help(state) = &mut app.mode {
        state.scroll_offset = state.scroll_offset.saturating_add(HELP_MOUSE_SCROLL_LINES);
        return;
    }
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
            if app.pane_content_cols == 0 || col >= app.pane_content_cols {
                return Ok(());
            }
            app.message = None;
            let Some((content_row, content_col)) =
                selection_coords_for_view(view, row - 1, col, app.pane_content_cols)
            else {
                return Ok(());
            };
            view.selection.start_row = content_row;
            view.selection.start_col = content_col;
            view.selection.end_row = content_row;
            view.selection.end_col = content_col;
            view.selection.is_selecting = true;
            view.selection.has_selection = false;
        }
        return Ok(());
    }

    if matches!(app.mode, AppMode::Help(_)) {
        app.mode = AppMode::Normal;
        return Ok(());
    }

    if row == 1 && !app.pending_inputs.is_empty() {
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
            | AppMode::StoppedSessionDialog(_)
            | AppMode::SessionPicker(_)
            | AppMode::NamingNewSession(_)
            | AppMode::BookmarkPicker(_)
            | AppMode::DiffPicker(_)
            | AppMode::DiffViewerLoading(_)
            | AppMode::DiffViewer(_)
            | AppMode::MarkdownLoading(_)
            | AppMode::SessionSwitcher(_)
            | AppMode::RenamingSession(_)
            | AppMode::RenamingFeature(_)
            | AppMode::NotificationPicker(_, _)
            | AppMode::DiffReviewPrompt(_)
            | AppMode::RunningHook(_)
            | AppMode::ConfirmResourceStart(_)
    ) {
        return Ok(());
    }

    let list_start_row = 4;
    let list_visible_height = visible_rows.saturating_sub(5);
    let list_end_row = list_start_row + list_visible_height;

    if row >= list_start_row && row < list_end_row {
        let clicked_in_list = row - list_start_row;
        if let Some(item_index) =
            app.item_index_at_visible_row(clicked_in_list as usize, list_visible_height as usize)
        {
            let visible = app.visible_items();
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
        && app.pane_content_cols > 0
    {
        let Some((content_row, content_col)) =
            selection_coords_for_view(view, row - 1, col, app.pane_content_cols)
        else {
            return Ok(());
        };
        view.selection.end_row = content_row;
        view.selection.end_col = content_col;
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
            if row > 0 && app.pane_content_cols > 0 {
                let Some((content_row, content_col)) =
                    selection_coords_for_view(view, row - 1, col, app.pane_content_cols)
                else {
                    return Ok(());
                };
                view.selection.end_row = content_row;
                view.selection.end_col = content_col;
            }

            let (content, rows) =
                selection_source_for_view(&app.pane_content, app.pane_content_rows, view);
            let text = extract_selected_text(content, &view.selection, rows, app.pane_content_cols);

            if !text.is_empty() {
                copy_to_clipboard_osc52(&text);
                app.push_toast_success(format!("Copied {} chars", text.len()));
            }
        }
    }
    Ok(())
}

fn selection_coords_for_view(
    view: &crate::app::ViewState,
    row: u16,
    col: u16,
    pane_content_cols: u16,
) -> Option<(u16, u16)> {
    if view.scroll_mode && !view.scroll_passthrough {
        let scrollbar_width = crate::ui::SCROLLBAR_WIDTH;
        let content_width = pane_content_cols.saturating_sub(scrollbar_width);
        if col >= content_width {
            return None;
        }

        if content_width == 0 {
            return None;
        }

        Some((
            (row as usize)
                .saturating_add(view.scroll_offset)
                .min(u16::MAX as usize) as u16,
            col,
        ))
    } else {
        Some((row, col.min(pane_content_cols.saturating_sub(1))))
    }
}

fn selection_source_for_view<'a>(
    pane_content: &'a str,
    pane_content_rows: u16,
    view: &'a crate::app::ViewState,
) -> (&'a str, u16) {
    if view.scroll_mode && !view.scroll_passthrough {
        (
            &view.scroll_content,
            view.scroll_total_lines.min(u16::MAX as usize) as u16,
        )
    } else {
        (pane_content, pane_content_rows)
    }
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
    let mut result = String::with_capacity(data.len().div_ceil(3) * 4);
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
    let normalized = crate::ui::normalize_captured_pane(content);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, AppMode, DebugLogState};
    use crate::project::ProjectStore;
    use crate::traits::{MockTmuxOps, MockWorktreeOps};
    use crossterm::event::KeyModifiers;
    use std::collections::HashMap;

    fn test_app() -> App {
        App::new_for_test(
            ProjectStore {
                version: 5,
                projects: vec![],
                session_bookmarks: vec![],
                available_harnesses: vec![],
                prompt_templates: Vec::new(),
                extra: HashMap::new(),
            },
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        )
    }

    #[test]
    fn mouse_scroll_down_moves_debug_log() {
        let mut app = test_app();
        app.mode = AppMode::DebugLog(DebugLogState {
            scroll_offset: 1,
            from_view: None,
            hide_perf_logs: false,
        });

        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: 0,
                row: 0,
                modifiers: KeyModifiers::NONE,
            },
            20,
        )
        .unwrap();

        match &app.mode {
            AppMode::DebugLog(state) => {
                assert_eq!(state.scroll_offset, 1 + DEBUG_LOG_MOUSE_SCROLL_LINES)
            }
            _ => panic!("expected debug log to stay open"),
        }
    }

    #[test]
    fn mouse_scroll_up_clamps_debug_log_at_top() {
        let mut app = test_app();
        app.mode = AppMode::DebugLog(DebugLogState {
            scroll_offset: 1,
            from_view: None,
            hide_perf_logs: false,
        });

        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::ScrollUp,
                column: 0,
                row: 0,
                modifiers: KeyModifiers::NONE,
            },
            20,
        )
        .unwrap();

        match &app.mode {
            AppMode::DebugLog(state) => assert_eq!(state.scroll_offset, 0),
            _ => panic!("expected debug log to stay open"),
        }
    }
}
