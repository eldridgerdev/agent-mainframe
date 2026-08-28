use crate::app::resource_gate::{LowMemory, OverLimit};
use crate::app::{PendingStart, ResourceConfirmState};
use crate::theme::Theme;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

/// Pre-start warning: the machine is at the agent cap and/or low on memory.
/// Advisory only — confirming starts the agent anyway.
pub fn draw_resource_confirm_dialog(
    frame: &mut Frame,
    state: &ResourceConfirmState,
    theme: &Theme,
) {
    let mut body: Vec<Line> = Vec::new();
    if let Some(over) = state.over_limit {
        body.extend(over_limit_lines(&over, theme));
    }
    if state.over_limit.is_some() && state.low_memory.is_some() {
        body.push(Line::from(""));
    }
    if let Some(low) = state.low_memory {
        body.extend(low_memory_lines(&low, theme));
        body.extend(open_editor_lines(&state.open_editors, theme));
    }
    if matches!(state.pending, PendingStart::PlannedFeature(_)) {
        body.push(Line::from(""));
        body.push(Line::from(Span::styled(
            " The completed plan is saved and will be kept if you cancel.",
            Style::default().fg(theme.text.to_color()),
        )));
        body.push(Line::from(Span::styled(
            " Continuing will create and start the planned feature anyway.",
            Style::default().fg(theme.text_muted.to_color()),
        )));
    }

    // Sized to what it actually says: heading, the tripped gate(s), a blank
    // line, the question, and the key hints, inside the border. A fixed
    // percentage would leave this dialog mostly empty, since one tripped gate
    // is three short lines.
    let height = (body.len() as u16) + 7;
    let area = centered_rect_height(64, height, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Resource Check ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.warning.to_color()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let heading = Paragraph::new(Line::from(Span::styled(
        format!(" {}", pending_summary(&state.pending)),
        Style::default()
            .fg(theme.warning.to_color())
            .add_modifier(Modifier::BOLD),
    )));
    frame.render_widget(heading, chunks[0]);

    frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), chunks[1]);

    let prompt = Paragraph::new(Line::from(vec![
        Span::styled(
            " Start anyway? ",
            Style::default().fg(theme.warning.to_color()),
        ),
        Span::styled("(y/n)", Style::default().fg(theme.text_muted.to_color())),
    ]));
    frame.render_widget(prompt, chunks[2]);

    let hints = Paragraph::new(Line::from(vec![
        Span::styled(" y", Style::default().fg(theme.warning.to_color())),
        Span::raw(" start anyway  "),
        Span::styled("n/Esc", Style::default().fg(theme.warning.to_color())),
        Span::raw(" cancel"),
    ]));
    frame.render_widget(hints, chunks[3]);
}

/// A centered box `percent_x` wide and exactly `height` rows tall (clamped to
/// the screen).
fn centered_rect_height(percent_x: u16, height: u16, area: Rect) -> Rect {
    let height = height.min(area.height);
    let width = area.width * percent_x / 100;
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

fn pending_summary(pending: &PendingStart) -> String {
    match pending {
        PendingStart::Feature { .. } => "Starting a feature's agent".to_string(),
        PendingStart::BuiltinSession { label, .. } => match label {
            Some(label) => format!("Adding session '{label}'"),
            None => "Adding a session".to_string(),
        },
        PendingStart::EnterView { .. } => "Opening a stopped feature".to_string(),
        PendingStart::SwitchViewToFeature { .. } => "Jumping to a stopped feature".to_string(),
        PendingStart::PlannedFeature(pending) => {
            format!("Starting planned feature '{}'", pending.prepared.branch)
        }
    }
}

fn over_limit_lines<'a>(over: &OverLimit, theme: &Theme) -> Vec<Line<'a>> {
    vec![
        Line::from(Span::styled(
            format!(
                " {} agent{} already running (limit {}).",
                over.active,
                if over.active == 1 { "" } else { "s" },
                over.limit
            ),
            Style::default().fg(theme.text.to_color()),
        )),
        Line::from(Span::styled(
            " Counts harness sessions across all projects, plus".to_string(),
            Style::default().fg(theme.text_muted.to_color()),
        )),
        Line::from(Span::styled(
            " any headless review or plan run in flight.".to_string(),
            Style::default().fg(theme.text_muted.to_color()),
        )),
    ]
}

/// Names the editor windows AMF knows are open, under the memory figure.
///
/// They are deliberately *not* counted as agents — one language server can
/// outweigh five harnesses, so a single count could never price both — but
/// when memory is what tripped, they are usually where it went, and saying so
/// is the difference between a number and something to act on.
fn open_editor_lines<'a>(editors: &[String], theme: &Theme) -> Vec<Line<'a>> {
    if editors.is_empty() {
        return Vec::new();
    }
    // A long list would push the question off the dialog; the count carries
    // the rest.
    const NAMED: usize = 3;
    let mut lines = vec![Line::from(Span::styled(
        format!(
            " {} editor window{} open (not counted as agents):",
            editors.len(),
            if editors.len() == 1 { "" } else { "s" }
        ),
        Style::default().fg(theme.text.to_color()),
    ))];
    for editor in editors.iter().take(NAMED) {
        lines.push(Line::from(Span::styled(
            format!("   {editor}"),
            Style::default().fg(theme.text_muted.to_color()),
        )));
    }
    if editors.len() > NAMED {
        lines.push(Line::from(Span::styled(
            format!("   +{} more", editors.len() - NAMED),
            Style::default().fg(theme.text_muted.to_color()),
        )));
    }
    lines.push(Line::from(Span::styled(
        " Their language servers usually outweigh the agents.",
        Style::default().fg(theme.text_muted.to_color()),
    )));
    lines
}

fn low_memory_lines<'a>(low: &LowMemory, theme: &Theme) -> Vec<Line<'a>> {
    let mut lines = vec![Line::from(Span::styled(
        format!(
            " {} MiB memory available, below the {} MiB floor.",
            low.snapshot.available_mb, low.threshold_mb
        ),
        Style::default().fg(theme.text.to_color()),
    ))];
    if let (Some(free), Some(total)) = (low.snapshot.swap_free_mb, low.snapshot.swap_total_mb) {
        lines.push(Line::from(Span::styled(
            format!(" Swap: {free} MiB free of {total} MiB."),
            Style::default().fg(theme.text_muted.to_color()),
        )));
    }
    lines.push(Line::from(Span::styled(
        format!(" Measured from {}.", low.snapshot.source.label()),
        Style::default().fg(theme.text_muted.to_color()),
    )));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::resource_gate::LowMemory;
    use crate::resources::mem::{MemorySnapshot, MemorySource};

    fn low() -> LowMemory {
        LowMemory {
            snapshot: MemorySnapshot {
                available_mb: 900,
                total_mb: 16384,
                swap_free_mb: Some(1024),
                swap_total_mb: Some(2048),
                source: MemorySource::ProcMeminfo,
            },
            threshold_mb: 1536,
        }
    }

    fn text(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn no_open_editors_adds_nothing() {
        assert!(open_editor_lines(&[], &Theme::default()).is_empty());
    }

    #[test]
    fn open_editors_are_named_under_the_memory_figure() {
        let lines = open_editor_lines(&["VS Code — agent-limits".to_string()], &Theme::default());
        let rendered = text(&lines);
        assert!(rendered.contains("1 editor window open"), "got {rendered}");
        assert!(rendered.contains("not counted as agents"), "got {rendered}");
        assert!(
            rendered.contains("VS Code — agent-limits"),
            "got {rendered}"
        );
        assert!(rendered.contains("outweigh the agents"), "got {rendered}");
    }

    #[test]
    fn a_long_editor_list_is_summarized_so_the_question_stays_on_screen() {
        let editors: Vec<String> = (1..=6).map(|n| format!("VS Code — feat-{n}")).collect();
        let rendered = text(&open_editor_lines(&editors, &Theme::default()));
        assert!(rendered.contains("6 editor windows open"), "got {rendered}");
        assert!(rendered.contains("VS Code — feat-3"), "got {rendered}");
        assert!(!rendered.contains("VS Code — feat-4"), "got {rendered}");
        assert!(rendered.contains("+3 more"), "got {rendered}");
    }

    #[test]
    fn the_memory_figure_still_leads() {
        let rendered = text(&low_memory_lines(&low(), &Theme::default()));
        assert!(
            rendered.contains("900 MiB memory available"),
            "got {rendered}"
        );
        assert!(rendered.contains("1536 MiB floor"), "got {rendered}");
    }
}
