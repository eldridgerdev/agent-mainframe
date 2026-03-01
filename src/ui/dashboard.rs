use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
    Frame,
};

use crate::app::{App, AppMode, CreateFeatureStep, RenameReturnTo};
use crate::project::VibeMode;
use crate::ui::list::rainbow_spans;

pub fn draw(frame: &mut Frame, app: &mut App) {
    if let AppMode::Viewing(view) = &app.mode {
        super::pane::draw(
            frame,
            view,
            &app.pane_content,
            app.leader_active,
            app.pending_inputs.len(),
            app.tmux_cursor,
        );
        return;
    }

    if let AppMode::SessionSwitcher(state) = &app.mode {
        let temp_view = crate::app::ViewState::new(
            state.project_name.clone(),
            state.feature_name.clone(),
            state.tmux_session.clone(),
            state.return_window.clone(),
            state.return_label.clone(),
            state.vibe_mode.clone(),
            state.review,
        );
        super::pane::draw(
            frame,
            &temp_view,
            &app.pane_content,
            false,
            app.pending_inputs.len(),
            app.tmux_cursor,
        );
        super::picker::draw_session_switcher(
            frame,
            state,
            app.config.nerd_font,
        );
        return;
    }

    if let AppMode::CommandPicker(state) = &app.mode
        && state.from_view.is_some()
    {
        let view = state.from_view.as_ref().unwrap();
        super::pane::draw(
            frame,
            view,
            &app.pane_content,
            false,
            app.pending_inputs.len(),
            app.tmux_cursor,
        );
        super::picker::draw_command_picker(frame, state);
        return;
    }

    if let AppMode::RenamingSession(state) = &app.mode
        && let RenameReturnTo::SessionSwitcher(ref sw) =
            state.return_to
    {
        let temp_view = crate::app::ViewState::new(
            sw.project_name.clone(),
            sw.feature_name.clone(),
            sw.tmux_session.clone(),
            sw.return_window.clone(),
            sw.return_label.clone(),
            sw.vibe_mode.clone(),
            sw.review,
        );
        super::pane::draw(
            frame,
            &temp_view,
            &app.pane_content,
            false,
            app.pending_inputs.len(),
            app.tmux_cursor,
        );
        super::dialogs::draw_rename_session_dialog(frame, state);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(3),
        ])
        .split(frame.area());

    super::header::draw(frame, chunks[0], &std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(), app.pending_inputs.len());
    super::list::draw(frame, app, chunks[1]);
    super::status::draw(frame, app, chunks[2]);

    match &app.mode {
        AppMode::CreatingProject(state) => {
            super::dialogs::draw_create_project_dialog(frame, state);
        }
        AppMode::CreatingFeature(state) => {
            if state.step == CreateFeatureStep::ConfirmSuperVibe {
                super::dialogs::draw_confirm_supervibe_dialog(frame);
            } else {
                let presets =
                    app.active_extension.feature_presets.as_slice();
                super::dialogs::draw_create_feature_dialog(
                    frame, state, presets,
                );
            }
        }
        AppMode::DeletingProject(name) => {
            super::dialogs::draw_delete_project_confirm(frame, name);
        }
        AppMode::DeletingFeature(
            project_name,
            feature_name,
        ) => {
            super::dialogs::draw_delete_feature_confirm(
                frame,
                project_name,
                feature_name,
            );
        }
        AppMode::BrowsingPath(state) => {
            super::dialogs::draw_browse_path_dialog(frame, state);
        }
        _ => {}
    }

    if let AppMode::RenamingSession(state) = &app.mode {
        super::dialogs::draw_rename_session_dialog(frame, state);
    }

    if matches!(app.mode, AppMode::Help) {
        super::dialogs::draw_help(frame);
    }

    if let AppMode::NotificationPicker(selected) = &app.mode
    {
        super::picker::draw_notification_picker(
            frame,
            &app.pending_inputs,
            *selected,
        );
    }

    if let AppMode::CommandPicker(state) = &app.mode {
        super::picker::draw_command_picker(frame, state);
    }

    if let AppMode::Searching(state) = &app.mode {
        super::dialogs::draw_search_dialog(frame, state);
    }

    if let AppMode::OpencodeSessionPicker(state) = &app.mode {
        super::picker::draw_opencode_session_picker(frame, state);
    }

    if matches!(app.mode, AppMode::ConfirmingOpencodeSession { .. }) {
        super::picker::draw_opencode_session_confirm(frame);
    }

    if let AppMode::SessionPicker(state) = &app.mode {
        super::picker::draw_session_picker(frame, state, app.config.nerd_font);
    }

    if let AppMode::ChangeReasonPrompt(state) = &app.mode {
        super::dialogs::draw_change_reason_dialog(frame, state);
    }

    if let AppMode::RunningHook(state) = &app.mode {
        super::dialogs::draw_running_hook_dialog(frame, state, &app.throbber_state);
    }

    if let AppMode::DeletingFeatureInProgress(state) = &app.mode {
        super::dialogs::draw_deleting_feature_dialog(frame, state, &app.throbber_state);
    }

    if let AppMode::HookPrompt(state) = &app.mode {
        super::dialogs::draw_hook_prompt_dialog(frame, state);
    }
}

pub fn centered_rect(
    percent_x: u16,
    percent_y: u16,
    area: Rect,
) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn draw_pane_view(
    frame: &mut Frame,
    view: &crate::app::ViewState,
    pane_content: &str,
    leader_active: bool,
    pending_count: usize,
) {
    let area = frame.area();
    let header_area = Rect::new(area.x, area.y, area.width, 1);
    let content_area = Rect::new(
        area.x,
        area.y + 1,
        area.width,
        area.height.saturating_sub(1),
    );

    // Header bar - single line with essential info
    let mut header_spans = vec![
        Span::raw("  "),
        Span::styled(
            format!("{} ", view.project_name),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("/{} ", view.feature_name),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("/{} ", view.session_label),
            Style::default().fg(Color::White),
        ),
    ];
    match view.vibe_mode {
        VibeMode::Vibeless => header_spans.push(Span::styled(
            "[vibeless] ",
            Style::default().fg(Color::Green),
        )),
        VibeMode::Vibe => header_spans.push(Span::styled(
            "[vibe] ",
            Style::default().fg(Color::Yellow),
        )),
        VibeMode::SuperVibe => {
            header_spans.push(Span::raw("["));
            header_spans.extend(rainbow_spans("supervibe"));
            header_spans.push(Span::raw("] "));
        }
        VibeMode::Review => header_spans.push(Span::styled(
            "[review] ",
            Style::default().fg(Color::Magenta),
        )),
    };
    if view.review {
        header_spans.push(Span::styled(
            "[review] ",
            Style::default().fg(Color::Cyan),
        ));
    }

    if leader_active {
        header_spans.push(Span::styled(
            "|LEADER ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
        header_spans.push(Span::styled(
            "q:exit t/T:cycle w:switcher n/p:feature /:commands i:inputs s:attach x:stop ?:help",
            Style::default().fg(Color::Yellow),
        ));
    } else {
        header_spans.push(Span::styled(
            "| ",
            Style::default().fg(Color::DarkGray),
        ));
        header_spans.push(Span::styled(
            "Ctrl+Space",
            Style::default().fg(Color::Yellow),
        ));
        header_spans.push(Span::styled(
            " command palette",
            Style::default().fg(Color::White),
        ));
    }

    if pending_count > 0 {
        header_spans.push(Span::styled(
            format!(" | {} input{}",
                pending_count,
                if pending_count == 1 { "" } else { "s" },
            ),
            Style::default()
                .fg(Color::Red)
                .add_modifier(Modifier::BOLD),
        ));
    }

    let header = Paragraph::new(Line::from(header_spans))
        .style(Style::default().bg(Color::Rgb(76, 79, 105)));
    frame.render_widget(header, header_area);

    // Parse ANSI content through vt100 and render
    // Catppuccin Frappé base background (#303446)
    let bg_color = Color::Rgb(48, 52, 70);
    let text = ansi_to_ratatui_text(
        pane_content,
        content_area.width,
        content_area.height,
        bg_color,
    );
    let paragraph = Paragraph::new(text).block(
        Block::default().style(Style::default().bg(bg_color)),
    );
    frame.render_widget(paragraph, content_area);
}

fn ansi_to_ratatui_text<'a>(
    raw: &str,
    cols: u16,
    rows: u16,
    bg_color: Color,
) -> Vec<Line<'a>> {
    let mut parser = vt100::Parser::new(rows, cols, 0);
    let normalized = raw.replace("\r\n", "\n").replace('\n', "\r\n");
    parser.process(normalized.as_bytes());
    let screen = parser.screen();

    let mut lines = Vec::with_capacity(rows as usize);

    for row in 0..rows {
        let mut spans: Vec<Span<'a>> = Vec::new();
        let mut current_text = String::new();
        let mut current_style = Style::default().bg(bg_color);

        for col in 0..cols {
            let cell = screen.cell(row, col);
            let cell = match cell {
                Some(c) => c,
                None => continue,
            };

            let style = vt100_cell_to_style(cell, bg_color);

            if style != current_style && !current_text.is_empty() {
                spans.push(Span::styled(
                    std::mem::take(&mut current_text),
                    current_style,
                ));
            }
            current_style = style;
            current_text.push_str(&cell.contents());
        }

        if !current_text.is_empty() {
            spans.push(Span::styled(current_text, current_style));
        }

        lines.push(Line::from(spans));
    }

    lines
}

fn vt100_cell_to_style(cell: &vt100::Cell, bg_color: Color) -> Style {
    let mut style = Style::default().bg(bg_color);

    if cell.fgcolor() != vt100::Color::Default {
        style = style.fg(vt100_color_to_ratatui(cell.fgcolor()));
    }

    if cell.bold() {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.italic() {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.underline() {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if cell.inverse() {
        style = style.add_modifier(Modifier::REVERSED);
    }

    style
}

fn vt100_color_to_ratatui(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    // ── centered_rect ─────────────────────────────────────────

    #[test]
    fn centered_rect_50_percent() {
        let area = Rect::new(0, 0, 100, 100);
        let result = centered_rect(50, 50, area);
        // Middle slice should be 50% of 100 = 50 in each dim
        assert_eq!(result.width, 50);
        assert_eq!(result.height, 50);
        // Should start at 25% offset
        assert_eq!(result.x, 25);
        assert_eq!(result.y, 25);
    }

    #[test]
    fn centered_rect_80_60_percent() {
        let area = Rect::new(0, 0, 100, 100);
        let result = centered_rect(80, 60, area);
        assert_eq!(result.width, 80);
        assert_eq!(result.height, 60);
        assert_eq!(result.x, 10);
        assert_eq!(result.y, 20);
    }

    #[test]
    fn centered_rect_fits_within_area() {
        let area = Rect::new(10, 5, 80, 40);
        let result = centered_rect(60, 50, area);
        // Result must be inside the original area
        assert!(result.x >= area.x);
        assert!(result.y >= area.y);
        assert!(result.x + result.width <= area.x + area.width);
        assert!(result.y + result.height <= area.y + area.height);
    }

    #[test]
    fn centered_rect_100_percent_fills_area() {
        let area = Rect::new(0, 0, 80, 40);
        let result = centered_rect(100, 100, area);
        assert_eq!(result.width, area.width);
        assert_eq!(result.height, area.height);
    }
}
