//! Shared rendering for compact companion-feature setup dialogs.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::app::{FeatureSetupRow, FeatureSetupState};
use crate::theme::Theme;

/// Render the common settings rows used by PR Triage and Final Review. The
/// owning dialog supplies its own title, explanation, error, and key hints.
pub(super) fn draw_rows(frame: &mut Frame, area: Rect, setup: &FeatureSetupState, theme: &Theme) {
    let value_for = |row: FeatureSetupRow| -> String {
        match row {
            FeatureSetupRow::Preset => setup.preset_label(),
            FeatureSetupRow::Harness => setup.agent().display_name().to_string(),
            FeatureSetupRow::Mode => {
                format!(
                    "{} — {}",
                    setup.mode.display_name(),
                    setup.mode.description()
                )
            }
            FeatureSetupRow::Review => if setup.review { "on" } else { "off" }.to_string(),
            FeatureSetupRow::Chrome => if setup.enable_chrome { "on" } else { "off" }.to_string(),
            FeatureSetupRow::Branch => setup.branch.clone(),
        }
    };

    let lines: Vec<Line> = FeatureSetupRow::ALL
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let is_selected = i == setup.row;
            let marker = if is_selected { ">" } else { " " };
            let value_style = if is_selected {
                Style::default()
                    .fg(theme.text.to_color())
                    .bg(theme.effective_selection_bg())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text.to_color())
            };
            let mut value = value_for(*row);
            if is_selected && *row == FeatureSetupRow::Branch {
                value.push('▏');
            }
            Line::from(vec![
                Span::styled(
                    format!("  {marker} "),
                    Style::default().fg(theme.warning.to_color()),
                ),
                Span::styled(
                    format!("{:<13}", row.label()),
                    Style::default().fg(theme.text_muted.to_color()),
                ),
                Span::styled(value, value_style),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}
