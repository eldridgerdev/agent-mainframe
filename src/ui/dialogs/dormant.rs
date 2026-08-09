use crate::app::DormantViewState;
use crate::app::dormant::DormantFeature;
use crate::theme::Theme;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

use super::super::dashboard::centered_rect;

/// Full-screen list of features that are idle *and* unattended, with what each
/// is still holding and the keys to reclaim it.
pub fn draw_dormant_view(
    frame: &mut Frame,
    state: &DormantViewState,
    idle_minutes: u64,
    unattended_hours: u64,
    theme: &Theme,
) {
    let area = centered_rect(86, 76, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Dormant Features ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(3),
            Constraint::Length(2),
            Constraint::Length(1),
        ])
        .split(inner);

    let criteria = Paragraph::new(Line::from(Span::styled(
        format!(
            " Idle over {idle_minutes}m and untouched over {unattended_hours}h — nobody is watching these."
        ),
        Style::default().fg(theme.text_muted.to_color()),
    )))
    .wrap(Wrap { trim: false });
    frame.render_widget(criteria, chunks[0]);

    if state.features.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            " Nothing is dormant right now.",
            Style::default().fg(theme.text.to_color()),
        )));
        frame.render_widget(empty, chunks[1]);
    } else {
        let items: Vec<ListItem> = state
            .features
            .iter()
            .map(|feature| ListItem::new(row(feature, theme)))
            .collect();
        let list = List::new(items).highlight_style(
            Style::default()
                .bg(theme.selection.to_color())
                .add_modifier(Modifier::BOLD),
        );
        let mut list_state = ListState::default();
        list_state.select(Some(state.selected));
        frame.render_stateful_widget(list, chunks[1], &mut list_state);
    }

    if let Some(message) = &state.message {
        let outcome = Paragraph::new(Line::from(Span::styled(
            format!(" {message}"),
            Style::default().fg(theme.success.to_color()),
        )))
        .wrap(Wrap { trim: false });
        frame.render_widget(outcome, chunks[2]);
    }

    let hints = Paragraph::new(Line::from(vec![
        Span::styled(" Enter", key_style(theme)),
        Span::raw(" open  "),
        Span::styled("x", key_style(theme)),
        Span::raw(" stop  "),
        Span::styled("e", key_style(theme)),
        Span::raw(" close editor  "),
        Span::styled("d", key_style(theme)),
        Span::raw(" delete  "),
        Span::styled("r", key_style(theme)),
        Span::raw(" refresh  "),
        Span::styled("q", key_style(theme)),
        Span::raw(" close"),
    ]));
    frame.render_widget(hints, chunks[3]);
}

fn key_style(theme: &Theme) -> Style {
    Style::default().fg(theme.primary.to_color())
}

fn row<'a>(feature: &DormantFeature, theme: &Theme) -> Line<'a> {
    let mut spans = vec![
        Span::styled(
            format!(" {}/{}", feature.project_name, feature.feature_name),
            Style::default().fg(theme.text.to_color()),
        ),
        Span::styled(
            format!(
                "  idle {}  ·  untouched {}",
                humanize(feature.idle),
                humanize(feature.unattended)
            ),
            Style::default().fg(theme.text_muted.to_color()),
        ),
    ];
    if feature.editor_alive {
        spans.push(Span::styled(
            "  · editor open",
            Style::default().fg(theme.warning.to_color()),
        ));
    }
    if feature.is_worktree {
        spans.push(Span::styled(
            "  · worktree",
            Style::default().fg(theme.text_muted.to_color()),
        ));
    }
    Line::from(spans)
}

/// Coarse age for a list column: minutes under an hour, then hours, then days.
pub(crate) fn humanize(duration: std::time::Duration) -> String {
    let secs = duration.as_secs();
    if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn humanizes_ages_by_magnitude() {
        assert_eq!(humanize(Duration::from_secs(0)), "0m");
        assert_eq!(humanize(Duration::from_secs(90 * 60)), "1h");
        assert_eq!(humanize(Duration::from_secs(59 * 60)), "59m");
        assert_eq!(humanize(Duration::from_secs(36 * 3600)), "1d");
    }
}
