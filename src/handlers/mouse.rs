use anyhow::Result;
use crossterm::event::{MouseEvent, MouseEventKind};
use std::time::Instant;

use crate::app::{App, AppMode, Selection, VisibleItem};

static mut LAST_CLICK_TIME: Option<Instant> = None;
static mut LAST_CLICK_ROW: Option<u16> = None;

pub fn handle_mouse(app: &mut App, mouse: MouseEvent, visible_rows: u16) -> Result<()> {
    match mouse.kind {
        MouseEventKind::ScrollUp => {
            handle_scroll_up(app);
        }
        MouseEventKind::ScrollDown => {
            handle_scroll_down(app);
        }
        MouseEventKind::Down(button) => {
            handle_click(app, mouse.column, mouse.row, button, visible_rows)?;
        }
        _ => {}
    }
    Ok(())
}

fn handle_scroll_up(app: &mut App) {
    if matches!(app.mode, AppMode::Viewing(_)) {
        return;
    }
    app.select_prev();
}

fn handle_scroll_down(app: &mut App) {
    if matches!(app.mode, AppMode::Viewing(_)) {
        return;
    }
    app.select_next();
}

fn handle_click(
    app: &mut App,
    col: u16,
    row: u16,
    _button: crossterm::event::MouseButton,
    visible_rows: u16,
) -> Result<()> {
    if let AppMode::Viewing(view) = &app.mode {
        if row == 0 {
            let name_start = 2;
            let name_end = name_start + view.project_name.len() as u16 + 1;
            if col >= name_start && col <= name_end {
                app.exit_view();
                return Ok(());
            }

            let pending = app.pending_inputs.len();
            if pending > 0 {
                let inputs_text = format!(" | {} input{}", pending, if pending == 1 { "" } else { "s" });
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
        let badge_text = format!("  [{} input request{}]", pending, if pending == 1 { "" } else { "s" });
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
            | AppMode::SessionPicker(_)
            | AppMode::SessionSwitcher(_)
            | AppMode::RenamingSession(_)
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
