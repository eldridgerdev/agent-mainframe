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
    let toast_height: u16 = 3;
    let gap: u16 = 1;
    let step = toast_height + gap;

    for (i, toast) in toasts.iter().enumerate() {
        let offset = i as u16 * step;
        let y = match area.bottom().saturating_sub(2 + offset + toast_height) {
            y if y < area.top() => break,
            y => y,
        };
        let x = area.right().saturating_sub(toast_width + 1);
        let toast_rect = Rect::new(x, y, toast_width, toast_height);

        let border_color = match toast.kind {
            ToastKind::Success => theme.success.to_color(),
            ToastKind::Info => theme.info.to_color(),
            ToastKind::Warning => theme.warning.to_color(),
            ToastKind::Error => theme.danger.to_color(),
        };

        let label = match toast.kind {
            ToastKind::Success => " ✓ ",
            ToastKind::Info => " i ",
            ToastKind::Warning => " ! ",
            ToastKind::Error => " ✕ ",
        };

        frame.render_widget(Clear, toast_rect);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .title(Span::styled(label, Style::default().fg(border_color)));

        let inner = block.inner(toast_rect);
        frame.render_widget(block, toast_rect);

        let max_chars = inner.width.saturating_sub(1) as usize;
        let display = if toast.message.len() > max_chars && max_chars > 1 {
            format!("{}…", &toast.message[..max_chars.saturating_sub(1)])
        } else {
            toast.message.clone()
        };

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
