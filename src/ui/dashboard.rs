use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
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
        );
        return;
    }

    if let AppMode::SessionSwitcher(state) = &app.mode {
        let temp_view = crate::app::ViewState {
            project_name: state.project_name.clone(),
            feature_name: state.feature_name.clone(),
            session: state.tmux_session.clone(),
            window: state.return_window.clone(),
            session_label: state.return_label.clone(),
            vibe_mode: state.vibe_mode.clone(),
        };
        super::pane::draw(
            frame,
            &temp_view,
            &app.pane_content,
            false,
            app.pending_inputs.len(),
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
        );
        super::picker::draw_command_picker(frame, state);
        return;
    }

    if let AppMode::RenamingSession(state) = &app.mode
        && let RenameReturnTo::SessionSwitcher(ref sw) =
            state.return_to
    {
        let temp_view = crate::app::ViewState {
            project_name: sw.project_name.clone(),
            feature_name: sw.feature_name.clone(),
            session: sw.tmux_session.clone(),
            window: sw.return_window.clone(),
            session_label: sw.return_label.clone(),
            vibe_mode: sw.vibe_mode.clone(),
        };
        super::pane::draw(
            frame,
            &temp_view,
            &app.pane_content,
            false,
            app.pending_inputs.len(),
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

    super::header::draw(frame, chunks[0], app.pending_inputs.len());
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
                super::dialogs::draw_create_feature_dialog(frame, state);
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

    if let AppMode::OpencodeSessionPicker(state) = &app.mode {
        super::picker::draw_opencode_session_picker(frame, state);
    }

    if matches!(app.mode, AppMode::ConfirmingOpencodeSession { .. }) {
        super::picker::draw_opencode_session_confirm(frame);
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
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // header
            Constraint::Min(1),   // pane content
        ])
        .split(frame.area());

    // Header bar with project/feature/session info
    let mut header_spans = vec![
        Span::styled(
            format!(" {} ", view.project_name),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("/ {} ", view.feature_name),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("/ {} ", view.session_label),
            Style::default().fg(Color::DarkGray),
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
    };

    if leader_active {
        header_spans.push(Span::styled(
            "| LEADER ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
        header_spans.push(Span::styled(
            " q:exit t/T:cycle w:switcher n/p:feature /:commands i:inputs s:attach x:stop ?:help",
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
            Style::default().fg(Color::DarkGray),
        ));
    }

    if pending_count > 0 {
        header_spans.push(Span::styled(
            format!(
                " | {} input{}",
                pending_count,
                if pending_count == 1 { "" } else { "s" },
            ),
            Style::default()
                .fg(Color::Red)
                .add_modifier(Modifier::BOLD),
        ));
    }

    let border_color = if leader_active {
        Color::Yellow
    } else {
        Color::Cyan
    };

    let header =
        Paragraph::new(Line::from(header_spans)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(
                    Style::default().fg(border_color),
                ),
        );
    frame.render_widget(header, chunks[0]);

    // Parse ANSI content through vt100 and render
    // Catppuccin Frappé base background (#303446)
    let content_area = chunks[1];
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

            if style != current_style
                && !current_text.is_empty()
            {
                spans.push(Span::styled(
                    std::mem::take(&mut current_text),
                    current_style,
                ));
            }
            current_style = style;
            current_text.push_str(&cell.contents());
        }

        if !current_text.is_empty() {
            spans.push(Span::styled(
                current_text,
                current_style,
            ));
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
