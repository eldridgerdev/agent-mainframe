use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use super::{CHOOSE_BASE_HINT, draw_file_list, draw_patch, effective_layout};
use crate::{
    app::{DiffScope, DiffViewerFocus, DiffViewerLayout, DiffViewerState},
    theme::Theme,
};

pub(super) fn draw_body(frame: &mut Frame, area: Rect, state: &mut DiffViewerState, theme: &Theme) {
    let current_changes = matches!(state.scope, DiffScope::CurrentChanges);
    if let Some(error) = &state.error {
        let hint = if current_changes {
            Line::from(Span::styled(
                format!(" {CHOOSE_BASE_HINT} Press r to retry."),
                Style::default().fg(theme.warning.to_color()),
            ))
        } else {
            Line::from("")
        };
        let error_widget = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                match &state.scope {
                    DiffScope::Commit(_) => " Could not load commit diff ",
                    DiffScope::PullRequest(_) => " Could not load the pull request ",
                    DiffScope::CurrentChanges => " Could not load current changes ",
                },
                Style::default()
                    .fg(theme.danger.to_color())
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                error.as_str(),
                Style::default().fg(theme.text.to_color()),
            )),
            Line::from(""),
            hint,
        ])
        .wrap(Wrap { trim: false });
        frame.render_widget(error_widget, area);
        return;
    }

    let no_base_fallback =
        current_changes && state.no_base_branch && state.override_base_ref.is_none();

    if state.files.is_empty() && no_base_fallback {
        let empty = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                " No uncommitted changes ",
                Style::default()
                    .fg(theme.success.to_color())
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                format!(
                    " No base branch was found, so this compares the working tree with HEAD. \
                     {CHOOSE_BASE_HINT} Press r to refresh after editing."
                ),
                Style::default().fg(theme.text.to_color()),
            )),
        ])
        .wrap(Wrap { trim: false });
        frame.render_widget(empty, area);
        return;
    }

    let area = if no_base_fallback && area.height > 3 {
        let [notice, rest] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(
                    " No base branch found: showing staged, unstaged and untracked changes \
                     against HEAD. {CHOOSE_BASE_HINT}"
                ),
                Style::default().fg(theme.warning.to_color()),
            ))),
            notice,
        );
        rest
    } else {
        area
    };

    if state.files.is_empty() {
        let empty = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                match &state.scope {
                    DiffScope::Commit(_) => " The selected commit has no file changes ",
                    DiffScope::PullRequest(_) => " This pull request has no file changes ",
                    DiffScope::CurrentChanges => " No changes against the selected base ",
                },
                Style::default()
                    .fg(theme.success.to_color())
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                match &state.scope {
                    DiffScope::Commit(_) => {
                        "This can happen for an empty commit or some merge commits."
                    }
                    DiffScope::PullRequest(_) => {
                        "Its head adds nothing beyond its merge-base with the base branch."
                    }
                    DiffScope::CurrentChanges => {
                        "Refresh with r after making more edits or commits."
                    }
                },
                Style::default().fg(theme.text.to_color()),
            )),
        ]);
        frame.render_widget(empty, area);
        return;
    }

    if state.review {
        draw_review_body(frame, area, state, theme);
        return;
    }

    if state.focus == DiffViewerFocus::Patch {
        draw_patch(frame, area, state, theme);
        return;
    }

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(body_constraints(area, state))
        .split(area);

    draw_file_list(frame, body[0], state, theme);
    draw_patch(frame, body[1], state, theme);
}

/// Review-mode body: file list (unless the patch is focused) on the left, and a
/// right column split into the developer-notes panel (top) over the diff. When
/// notes are expanded the panel takes the full right column.
fn draw_review_body(frame: &mut Frame, area: Rect, state: &mut DiffViewerState, theme: &Theme) {
    let content_area = if state.focus == DiffViewerFocus::Patch {
        area
    } else {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(body_constraints(area, state))
            .split(area);
        draw_file_list(frame, cols[0], state, theme);
        cols[1]
    };

    if state.notes_expanded {
        draw_notes_panel(frame, content_area, state, theme);
        return;
    }

    // Give the notes panel ~20% of the column — the diff is what the reviewer
    // is actually reading, and `e` still expands notes to full height on
    // demand. Always leave room for both.
    let notes_height = (content_area.height / 5).clamp(
        5.min(content_area.height),
        content_area.height.saturating_sub(5).max(1),
    );
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(notes_height), Constraint::Min(3)])
        .split(content_area);
    draw_notes_panel(frame, split[0], state, theme);
    draw_patch(frame, split[1], state, theme);
}

fn draw_notes_panel(frame: &mut Frame, area: Rect, state: &mut DiffViewerState, theme: &Theme) {
    let path = state
        .files
        .get(state.selected_file)
        .map(|file| file.path.clone());
    let note = path
        .as_ref()
        .and_then(|p| state.review_notes.get(p))
        .cloned();
    let generating = path.as_ref().is_some_and(|p| {
        state.walkthrough_child.is_some() && state.walkthrough_file.as_deref() == Some(p.as_str())
    });
    let generated = path
        .as_ref()
        .and_then(|p| state.generated_notes.get(p))
        .cloned();

    // A PR review has no developer notes (those live in a feature's
    // `.claude/`); its panel reports what the PR's updates did to this file
    // and to the reviewer's comments instead.
    let pr_notes = path
        .as_deref()
        .filter(|_| state.is_pr_review())
        .map(|path| pr_review_notes_markdown(state, path));

    // The panel titles itself after whatever it is showing, so a generated
    // walkthrough isn't mistaken for a hand-written developer note.
    let title = if pr_notes.is_some() {
        " PR Review Notes "
    } else if note.is_none() && (generating || generated.is_some()) {
        " AI Walkthrough "
    } else {
        " Developer Notes "
    };

    let border = if state.notes_expanded {
        theme.warning.to_color()
    } else {
        theme.primary.to_color()
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let notes_scroll = state.notes_scroll;
    // Render markdown pre-wrapped to the panel width so the scroll offset maps
    // to rendered visual lines.
    let render_md = |text: &str| {
        let rendered =
            crate::markdown::render_markdown(text, theme, inner.width.max(1) as usize, None);
        let line_count = rendered.lines.len();
        (
            Paragraph::new(rendered.lines).scroll((notes_scroll as u16, 0)),
            line_count,
        )
    };
    // On a re-review, the feature agent's replies to last round's items for this
    // file (parsed from the feedback file). Rendered as a markdown section under
    // the developer note / walkthrough so the reviewer sees what the agent said.
    let responses_md = path
        .as_ref()
        .and_then(|p| state.prior_agent_responses.get(p))
        .filter(|v| !v.is_empty())
        .map(|responses| {
            let mut section = String::from("### Agent replies (last round)\n\n");
            for r in responses {
                section.push_str(&format!("**{}**\n\n{}\n\n", r.anchor, r.response));
            }
            section
        });

    let (paragraph, rendered_lines) = if let Some(pr_notes) = pr_notes {
        match pr_notes {
            Some(text) => render_md(&text),
            None => (
                Paragraph::new("Nothing to note for this file.")
                    .wrap(Wrap { trim: false })
                    .style(Style::default().fg(theme.text_muted.to_color())),
                0,
            ),
        }
    } else if generating {
        (
            Paragraph::new("Generating walkthrough…")
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(theme.text_muted.to_color())),
            0,
        )
    } else {
        // A developer note wins over a generated walkthrough as the base body;
        // the agent-replies section (if any) follows either, separated by a rule.
        let base = note.as_deref().or(generated.as_deref());
        match (base, responses_md.as_deref()) {
            (Some(text), Some(resp)) => render_md(&format!("{text}\n\n---\n\n{resp}")),
            (Some(text), None) => render_md(text),
            (None, Some(resp)) => render_md(resp),
            (None, None) => (
                Paragraph::new(
                    "No developer note for this file.\n\nPress w to generate an AI walkthrough of \
                     this file's diff. Review mode records per-file reasoning in \
                     .claude/review-notes.md as changes are made.",
                )
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(theme.text_muted.to_color())),
                0,
            ),
        }
    };

    // Record the rendered (wrapped) line count and viewport height so the scroll
    // clamp can reach the visual bottom of soft-wrapped / markdown notes.
    state.notes_rendered_lines = rendered_lines;
    state.notes_view_height = inner.height as usize;

    frame.render_widget(paragraph, inner);
}

/// The PR review notes panel for `path`, as markdown. `None` when there is
/// nothing to say.
/// - The file changed since the reviewer's draft (its verdict was cleared).
/// - This file's outdated comments: kept, but their code is gone from the diff.
/// - Comments on files the PR no longer touches (PR-wide, so on every file).
pub(super) fn pr_review_notes_markdown(state: &DiffViewerState, path: &str) -> Option<String> {
    fn comment_line(anchor: String, comment: &crate::app::LineComment) -> String {
        let text = comment.text.trim();
        let text = if text.is_empty() && comment.suggestion.is_some() {
            "(suggested change)"
        } else {
            text
        };
        format!("- **{anchor}** [{}] {text}\n", comment.severity.label())
    }
    let was = |comment: &crate::app::LineComment| {
        comment
            .location
            .new_line
            .or(comment.location.old_line)
            .map(|line| format!("was L{line}"))
            .unwrap_or_else(|| "was unanchored".to_string())
    };

    let mut sections = Vec::new();
    if state.changed_since_last.contains(path) {
        sections.push(
            "**Changed since your draft.** Its changes differ from what you reviewed, so its \
             verdict was cleared. Take another look."
                .to_string(),
        );
    }
    let outdated: Vec<&crate::app::LineComment> = state
        .line_comments
        .get(path)
        .into_iter()
        .flatten()
        .filter(|c| c.anchor_lost)
        .collect();
    if !outdated.is_empty() {
        let mut section = String::from(
            "### Outdated comments\n\nThe code these were on is no longer in the PR's diff. \
             They stay in your draft and go in the review summary when you post.\n\n",
        );
        for comment in outdated {
            section.push_str(&comment_line(was(comment), comment));
        }
        sections.push(section);
    }
    let detached = state.pr_detached_line_comments.len() + state.pr_detached_file_comments.len();
    if detached > 0 {
        let mut section = String::from(
            "### Comments on files no longer in this PR\n\nKept in your draft; they go in the \
             review summary when you post.\n\n",
        );
        let mut paths: Vec<&String> = state
            .pr_detached_line_comments
            .keys()
            .chain(state.pr_detached_file_comments.keys())
            .collect();
        paths.sort();
        paths.dedup();
        for detached_path in paths {
            for comment in state
                .pr_detached_line_comments
                .get(detached_path)
                .into_iter()
                .flatten()
            {
                section.push_str(&comment_line(
                    format!("{detached_path} ({})", was(comment)),
                    comment,
                ));
            }
            if let Some(comment) = state.pr_detached_file_comments.get(detached_path) {
                section.push_str(&format!(
                    "- **{detached_path}** (whole file) [{}] {}\n",
                    comment.severity.label(),
                    comment.text.trim()
                ));
            }
        }
        sections.push(section);
    }
    (!sections.is_empty()).then(|| sections.join("\n\n"))
}

fn body_constraints(area: Rect, state: &DiffViewerState) -> [Constraint; 2] {
    match (&effective_layout(state), &state.focus) {
        (DiffViewerLayout::SideBySide, DiffViewerFocus::Patch) => {
            let file_width = area.width.saturating_mul(22) / 100;
            [Constraint::Length(file_width.max(24)), Constraint::Min(40)]
        }
        (DiffViewerLayout::SideBySide, DiffViewerFocus::FileList) => {
            let file_width = area.width.saturating_mul(30) / 100;
            [Constraint::Length(file_width.max(30)), Constraint::Min(34)]
        }
        _ => [Constraint::Percentage(32), Constraint::Percentage(68)],
    }
}
