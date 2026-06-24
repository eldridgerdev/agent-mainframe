use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::{
    app::pr_review::{CommentKind, PrComment},
    app::{PrNumberPromptState, PrReviewLoadState, PrReviewState},
    theme::Theme,
};

/// Modal prompt for a manual PR number, shown when the branch has no
/// auto-detectable open PR. Collects digits and surfaces resolve errors inline.
pub fn draw_pr_number_prompt(frame: &mut Frame, state: &PrNumberPromptState, theme: &Theme) {
    let area = super::super::dashboard::centered_rect(50, 25, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Review PR by number ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // hint
            Constraint::Length(2), // input
            Constraint::Min(0),    // error
        ])
        .split(inner);

    let hint = Paragraph::new(Line::from(Span::styled(
        " No open PR detected for this branch — enter a number:",
        Style::default().fg(theme.text_muted.to_color()),
    )));
    frame.render_widget(hint, chunks[0]);

    let input = Paragraph::new(Line::from(vec![
        Span::styled(" PR #", Style::default().fg(theme.primary.to_color())),
        Span::styled(&state.input, Style::default().fg(theme.text.to_color())),
        Span::styled("\u{2588}", Style::default().fg(theme.primary.to_color())),
    ]));
    frame.render_widget(input, chunks[1]);

    if let Some(err) = &state.error {
        let error = Paragraph::new(Line::from(Span::styled(
            format!(" {err}"),
            Style::default().fg(theme.danger.to_color()),
        )))
        .wrap(Wrap { trim: false });
        frame.render_widget(error, chunks[2]);
    }
}

/// Full-screen loading frame shown while a PR's comments are fetched.
pub fn draw_pr_review_loading(
    frame: &mut Frame,
    state: &PrReviewLoadState,
    throbber_state: &throbber_widgets_tui::ThrobberState,
    theme: &Theme,
) {
    let area = frame.area();
    let block = pane_block(theme).title(format!(" PR #{} ", state.pr.number));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let throbber = throbber_widgets_tui::Throbber::default()
        .style(Style::default().fg(theme.warning.to_color()));
    let spinner = throbber.to_symbol_span(throbber_state);

    let body = Paragraph::new(vec![
        Line::from(""),
        Line::from(vec![
            spinner,
            Span::styled(
                " Fetching PR comments...",
                Style::default()
                    .fg(theme.text.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            format!("PR #{}  ·  {}", state.pr.number, state.pr.url),
            Style::default().fg(theme.text_muted.to_color()),
        )),
        Line::from(Span::styled(
            "esc to cancel",
            Style::default().fg(theme.text_muted.to_color()),
        )),
    ])
    .wrap(Wrap { trim: false });
    frame.render_widget(body, inner);
}

/// Full-screen PR comment-review pane: comment list on the left, detail on the
/// right.
pub fn draw_pr_review(frame: &mut Frame, state: &PrReviewState, theme: &Theme) {
    let area = frame.area();
    let review = &state.review;

    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Min(1),    // body
            Constraint::Length(1), // footer
        ])
        .split(area);

    // Header.
    let header = Line::from(vec![
        Span::styled(
            format!(" PR #{} ", review.pr.number),
            Style::default()
                .fg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "{} comments ({} open)",
                review.comments.len(),
                review.open_count()
            ),
            Style::default().fg(theme.text_muted.to_color()),
        ),
    ]);
    frame.render_widget(Paragraph::new(header), outer[0]);

    // Body: list | detail.
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
        .split(outer[1]);

    draw_comment_list(frame, body[0], state, theme);
    draw_comment_detail(frame, body[1], state.selected_comment(), state.detail_scroll, theme);

    // Footer.
    let toggle_hint = if state.hide_resolved {
        "h show-resolved"
    } else {
        "h hide-resolved"
    };
    let footer = Paragraph::new(Line::from(Span::styled(
        format!(" j/k move   ^d/^u scroll   {toggle_hint}   r refresh   g other-PR   esc/q close"),
        Style::default().fg(theme.text_muted.to_color()),
    )));
    frame.render_widget(footer, outer[2]);
}

fn draw_comment_list(frame: &mut Frame, area: Rect, state: &PrReviewState, theme: &Theme) {
    let block = pane_block(theme).title(" Comments ");

    if state.review.comments.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "No comments on this PR.",
            Style::default().fg(theme.text_muted.to_color()),
        )))
        .block(block);
        frame.render_widget(empty, area);
        return;
    }

    let visible = state.visible_indices();
    if visible.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "All comments resolved (h to show).",
            Style::default().fg(theme.text_muted.to_color()),
        )))
        .block(block);
        frame.render_widget(empty, area);
        return;
    }

    let mut items: Vec<ListItem> = visible
        .iter()
        .map(|&i| ListItem::new(comment_list_line(&state.review.comments[i], theme)))
        .collect();

    let hidden = state.hidden_resolved_count();
    if hidden > 0 {
        items.push(ListItem::new(Line::from(Span::styled(
            format!("  ─ {hidden} resolved hidden (h to show) ─"),
            Style::default().fg(theme.text_muted.to_color()),
        ))));
    }

    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .bg(theme.primary.to_color())
            .fg(theme.effective_bg())
            .add_modifier(Modifier::BOLD),
    );

    // The list renders only visible comments, so translate the absolute
    // selection index into its position within the visible slice.
    let highlight = visible.iter().position(|&i| i == state.selected);
    let mut list_state = ListState::default();
    list_state.select(highlight);
    frame.render_stateful_widget(list, area, &mut list_state);
}

/// One row in the comment list: a resolution marker, location, author, snippet.
fn comment_list_line<'a>(c: &'a PrComment, theme: &Theme) -> Line<'a> {
    let marker = if c.is_resolved { "✓" } else { " " };
    let location = match (&c.path, c.line) {
        (Some(path), Some(line)) => format!("{path}:{line}"),
        (Some(path), None) => path.clone(),
        (None, _) => kind_label(&c.kind).to_string(),
    };

    let location_style = if c.is_resolved {
        Style::default().fg(theme.text_muted.to_color())
    } else {
        Style::default().fg(theme.text.to_color())
    };

    Line::from(vec![
        Span::styled(
            format!("{marker} "),
            Style::default().fg(theme.success.to_color()),
        ),
        Span::styled(location, location_style),
        Span::styled(
            format!("  @{}", c.author),
            Style::default().fg(theme.secondary.to_color()),
        ),
        Span::styled(
            format!("  {}", c.snippet),
            Style::default().fg(theme.text_muted.to_color()),
        ),
    ])
}

fn draw_comment_detail(
    frame: &mut Frame,
    area: Rect,
    comment: Option<&PrComment>,
    scroll: usize,
    theme: &Theme,
) {
    let block = pane_block(theme).title(" Detail ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(c) = comment else {
        return;
    };

    let mut lines: Vec<Line> = Vec::new();

    // Location + flags.
    let mut header_spans = vec![Span::styled(
        match (&c.path, c.line) {
            (Some(path), Some(line)) => format!("{path}:{line}"),
            (Some(path), None) => path.clone(),
            (None, _) => kind_label(&c.kind).to_string(),
        },
        Style::default()
            .fg(theme.primary.to_color())
            .add_modifier(Modifier::BOLD),
    )];
    if c.outdated {
        header_spans.push(Span::styled(
            "  [outdated]",
            Style::default().fg(theme.warning.to_color()),
        ));
    }
    if c.is_resolved {
        header_spans.push(Span::styled(
            "  [resolved]",
            Style::default().fg(theme.success.to_color()),
        ));
    }
    lines.push(Line::from(header_spans));

    lines.push(Line::from(vec![
        Span::styled(
            format!("@{}", c.author),
            Style::default().fg(theme.secondary.to_color()),
        ),
        Span::styled(
            if c.is_bot { "  (bot)" } else { "" }.to_string(),
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled(
            format!("  ·  {}", kind_label(&c.kind)),
            Style::default().fg(theme.text_muted.to_color()),
        ),
    ]));

    // Diff hunk, if present.
    if let Some(hunk) = &c.diff_hunk {
        lines.push(Line::from(""));
        for hl in hunk.lines() {
            lines.push(Line::from(Span::styled(
                hl.to_string(),
                Style::default().fg(theme.text_muted.to_color()),
            )));
        }
    }

    // Body.
    lines.push(Line::from(""));
    for bl in c.body.lines() {
        lines.push(Line::from(Span::styled(
            bl.to_string(),
            Style::default().fg(theme.text.to_color()),
        )));
    }

    let body = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll as u16, 0));
    frame.render_widget(body, inner);
}

fn kind_label(kind: &CommentKind) -> &'static str {
    match kind {
        CommentKind::Inline => "inline comment",
        CommentKind::ReviewSummary { .. } => "review summary",
        CommentKind::Conversation => "conversation",
    }
}

fn pane_block(theme: &Theme) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()))
}
