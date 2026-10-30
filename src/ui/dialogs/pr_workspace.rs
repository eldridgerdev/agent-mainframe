//! Shared split presentation. All pane content comes from the existing review
//! renderers and their real state; this module never fetches or posts data.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use crate::{
    app::{
        AiReviewState, App, AppMode,
        pr_workspace::{PrWorkspace, WorkspacePane, WorkspaceTab, engine_modal},
    },
    theme::Theme,
};

/// Shared with pointer routing so visual layout and input geometry agree.
pub(crate) fn pane_areas(
    area: Rect,
    focus: WorkspacePane,
    expanded: bool,
) -> Vec<(WorkspacePane, Rect)> {
    let rows = Layout::vertical([
        Constraint::Length(5),
        Constraint::Min(1),
        Constraint::Length(3),
    ])
    .split(area);
    let body = rows[1];
    if expanded || body.width < 85 {
        return vec![(focus, body)];
    }
    if body.width >= 125 {
        let cols = Layout::horizontal([
            Constraint::Percentage(24),
            Constraint::Percentage(44),
            Constraint::Percentage(32),
        ])
        .split(body);
        vec![
            (WorkspacePane::Navigator, cols[0]),
            (WorkspacePane::Diff, cols[1]),
            (WorkspacePane::Context, cols[2]),
        ]
    } else {
        let cols = Layout::horizontal([Constraint::Percentage(56), Constraint::Percentage(44)])
            .split(body);
        let mut panes = vec![
            (WorkspacePane::Diff, cols[0]),
            (WorkspacePane::Context, cols[1]),
        ];
        if focus == WorkspacePane::Navigator {
            panes.push((
                WorkspacePane::Navigator,
                Rect {
                    width: body.width.min(42),
                    ..body
                },
            ));
        }
        panes
    }
}

/// Shared by the displayed control and pointer routing.
pub(crate) fn cancel_button_area(area: Rect, focus: WorkspacePane, expanded: bool) -> Option<Rect> {
    let (_, navigator) = pane_areas(area, focus, expanded)
        .into_iter()
        .find(|(pane, rect)| *pane == WorkspacePane::Navigator && rect.height >= 8)?;
    Some(Rect::new(
        navigator.x + 1,
        navigator.y + 3,
        navigator.width.saturating_sub(2).min(15),
        1,
    ))
}

pub(crate) fn draw(frame: &mut Frame, app: &mut App) -> bool {
    let Some(mut workspace) = app.pr_workspace.take() else {
        return false;
    };
    if workspace.pr.is_none() {
        let drawn = if let AppMode::PrReviewList(state) = &app.mode {
            super::pr_review::draw_pr_workspace_list(frame, state, &app.theme);
            true
        } else {
            false
        };
        app.pr_workspace = Some(workspace);
        return drawn;
    }
    // Existing full-screen editors/confirmations retain their rendering and
    // precedence. The split workspace resumes with the same state on close.
    if !workspace.owns_mode(&app.mode)
        || WorkspaceTab::for_mode(&app.mode).is_none()
        || engine_modal(&app.mode)
    {
        app.pr_workspace = Some(workspace);
        return false;
    }
    let area = frame.area();
    let theme = &app.theme;
    let ai_running = workspace.ai_running(&app.ai_review_run);
    frame.render_widget(
        Block::default().style(
            Style::default()
                .bg(theme.effective_bg())
                .fg(theme.text.to_color()),
        ),
        area,
    );
    let rows = Layout::vertical([
        Constraint::Length(5),
        Constraint::Min(1),
        Constraint::Length(3),
    ])
    .split(area);
    draw_header(frame, rows[0], &workspace, &app.mode, theme, ai_running);
    for (pane, rect) in pane_areas(area, workspace.focus, workspace.expanded) {
        clear_with_bg(frame, rect, theme);
        let content = if pane == WorkspacePane::Navigator && ai_running && rect.height >= 8 {
            let sections =
                Layout::vertical([Constraint::Length(5), Constraint::Min(3)]).split(rect);
            draw_ai_progress(
                frame,
                sections[0],
                app.ai_review_run.progress().as_ref(),
                &app.throbber_state,
                theme,
            );
            sections[1]
        } else {
            rect
        };
        draw_pane(
            frame,
            content,
            pane,
            &mut workspace,
            &mut app.mode,
            theme,
            ai_running,
        );
        let title = match pane {
            WorkspacePane::Navigator => match workspace.tab {
                WorkspaceTab::Files => " Files ",
                WorkspaceTab::Threads => " GitHub threads ",
                WorkspaceTab::Ai => " AI findings ",
            }
            .to_owned(),
            WorkspacePane::Diff => workspace
                .files(&app.mode)
                .and_then(|state| state.files.get(state.selected_file))
                .map(|file| format!(" {} ", file.path))
                .unwrap_or_else(|| " Code ".into()),
            WorkspacePane::Context => match workspace.tab {
                WorkspaceTab::Files => " Your review ",
                WorkspaceTab::Threads => " GitHub discussion ",
                WorkspaceTab::Ai => " AI finding detail ",
            }
            .to_owned(),
        };
        // A border alone preserves renderer content while making focus
        // consistent across all three independently reused renderers.
        frame.render_widget(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(if pane == workspace.focus {
                    theme.warning.to_color()
                } else {
                    theme.text_muted.to_color()
                })),
            rect,
        );
    }
    let action_tab = if workspace.focus == WorkspacePane::Diff {
        WorkspaceTab::Files
    } else {
        workspace.tab
    };
    let actions: &[(&str, &str)] = match action_tab {
        WorkspaceTab::Files => &[
            ("c", "line cursor"),
            ("Enter", "comment"),
            ("m", "file comment"),
            ("f", "summary"),
            ("a", "approve"),
            ("s", "skip"),
        ],
        WorkspaceTab::Threads => &[
            ("f", "fix"),
            ("v", "investigate"),
            ("R", "reply"),
            ("M", "add to memory"),
            ("m", "mark/resolve"),
            ("h", "hide/show resolved"),
            ("o", "sort"),
            ("B", "fix marked"),
            ("r", "refresh"),
            ("P", "fix session"),
        ],
        WorkspaceTab::Ai => &[
            ("s", "skip"),
            ("e", "edit"),
            ("f", "fix"),
            ("Space", "mark"),
            ("B", "fix marked"),
            ("W", "preview/post AI review"),
        ],
    };
    let ai_hint = if ai_running {
        ("Ctrl+X", "cancel AI")
    } else {
        ("A", "run AI")
    };
    let navigation = [
        ("1/2/3", "tabs"),
        ("Tab", "focus"),
        ("z", "expand"),
        ai_hint,
        ("p", "submit review"),
        ("?", "help"),
        ("q", "close"),
    ];
    let message = workspace.errors[workspace.tab.index()]
        .as_deref()
        .or(app.message.as_deref())
        .unwrap_or("F5 reopen this PR at its latest head · leader+U / W open the legacy workflows");
    frame.render_widget(
        Paragraph::new(vec![
            hint_line("Go", theme.primary.to_color(), &navigation, theme),
            hint_line(action_tab.label(), theme.warning.to_color(), actions, theme),
            Line::styled(
                format!(" {message}"),
                Style::default().fg(theme.text_muted.to_color()),
            ),
        ]),
        rows[2],
    );
    if workspace.help {
        draw_help(frame, theme, workspace.help_scroll);
    }
    app.pr_workspace = Some(workspace);
    true
}

/// One footer row: a coloured group label, then `key label` pairs with the
/// key as a badge and a dim divider between pairs.
fn hint_line(
    group: &str,
    group_color: ratatui::style::Color,
    hints: &[(&str, &str)],
    theme: &Theme,
) -> Line<'static> {
    let muted = Style::default().fg(theme.text_muted.to_color());
    let mut spans = vec![Span::styled(
        format!(" {group:<7}"),
        Style::default()
            .fg(group_color)
            .add_modifier(Modifier::BOLD),
    )];
    for (i, (key, label)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" │ ", muted));
        }
        spans.push(Span::styled(
            format!(" {key} "),
            Style::default()
                .fg(theme.shortcut_text.to_color())
                .bg(theme.shortcut_background.to_color())
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(format!(" {label}")));
    }
    Line::from(spans)
}

/// `Clear` resets cells to the terminal default, which drops the theme
/// background; repaint it so cleared panes and overlays keep their backdrop.
fn clear_with_bg(frame: &mut Frame, area: Rect, theme: &Theme) {
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.effective_bg())),
        area,
    );
}

fn ai_state(mode: Option<&AppMode>) -> Option<&AiReviewState> {
    match mode? {
        AppMode::AiReview(state) => Some(state),
        AppMode::AiReviewRunning(state) => Some(&state.origin),
        _ => None,
    }
}

fn draw_ai_progress(
    frame: &mut Frame,
    area: Rect,
    progress: Option<&crate::app::AiReviewRunProgress>,
    throbber: &throbber_widgets_tui::ThrobberState,
    theme: &Theme,
) {
    let accent = Style::default().fg(theme.warning.to_color());
    let detail = progress.map_or_else(
        || " Working in background".to_owned(),
        |progress| {
            let stage = match progress.stage {
                crate::app::ai_review::AiReviewStage::PreparingDiff => "Fetching diff",
                crate::app::ai_review::AiReviewStage::Reviewing { .. } => "Reviewing code",
            };
            let seconds = progress.started_at.elapsed().as_secs();
            format!(" {stage} · {}m {:02}s", seconds / 60, seconds % 60)
        },
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                throbber_widgets_tui::Throbber::default()
                    .style(accent)
                    .to_symbol_span(throbber),
                Span::styled(" AI review running", accent.add_modifier(Modifier::BOLD)),
            ]),
            Line::styled(detail, Style::default().fg(theme.text_muted.to_color())),
            Line::styled(
                "[Ctrl+X Cancel]",
                accent
                    .bg(theme.effective_selection_bg())
                    .add_modifier(Modifier::BOLD),
            ),
        ])
        .block(Block::default().borders(Borders::ALL).border_style(accent)),
        area,
    );
}

fn draw_header(
    frame: &mut Frame,
    area: Rect,
    workspace: &PrWorkspace,
    active: &AppMode,
    theme: &Theme,
    ai_running: bool,
) {
    let Some(pr) = &workspace.pr else {
        return;
    };
    let files = workspace.files(active);
    let title = files
        .and_then(|files| match &files.scope {
            crate::app::DiffScope::PullRequest(target) => Some(target.pr.title.as_str()),
            _ => None,
        })
        .unwrap_or("");
    let threads = match workspace.mode(WorkspaceTab::Threads, active) {
        Some(AppMode::PrReview(state)) => format!(
            "{} open threads",
            state
                .review
                .comments
                .iter()
                .filter(|c| !c.is_resolved && c.in_reply_to.is_none())
                .count()
        ),
        _ if workspace.errors[WorkspaceTab::Threads.index()].is_some() => {
            "comments unavailable · 2 then r to retry".into()
        }
        _ => "loading comments…".into(),
    };
    let ai = if ai_running {
        "AI running…".into()
    } else {
        ai_state(workspace.mode(WorkspaceTab::Ai, active))
            .map(|state| {
                if state.last_run.is_none() && state.findings.is_empty() {
                    return "AI: not run · A generate".into();
                }
                if matches!(
                    state.last_run.as_ref().map(|run| &run.outcome),
                    Some(crate::app::ai_review::AiReviewRunOutcome::Error(_))
                ) {
                    return "AI: failed · A retry".into();
                }
                format!(
                    "AI: {} findings · {} unpublished",
                    state.findings.len(),
                    state
                        .findings
                        .iter()
                        .filter(|f| !f.published && !f.skipped)
                        .count()
                )
            })
            .unwrap_or_else(|| "AI: not run".into())
    };
    let progress = files
        .map(|files| {
            format!(
                "{} drafts · {}/{} files decided",
                files.line_comments.values().map(Vec::len).sum::<usize>()
                    + files.file_comments.len(),
                files.decisions.len(),
                files.files.len()
            )
        })
        .unwrap_or_default();
    let status = if workspace.observed_head_moved(active) {
        "PR head changed · press F5 to reopen this PR before comparing code or sending fixes"
            .to_string()
    } else if let Some(AppMode::PrReview(state)) = workspace.mode(WorkspaceTab::Threads, active)
        && let Some(branch) = state.branch_mismatch()
    {
        format!(
            "Fix target branch: {branch} · PR branch: {} · fix confirmation requires acknowledgement",
            pr.head_ref
        )
    } else {
        progress
    };
    let muted = Style::default().fg(theme.text_muted.to_color());
    let sep = || Span::styled("  │  ", muted);
    let tabs = [
        (WorkspaceTab::Files, "1 Files"),
        (WorkspaceTab::Threads, "2 Threads"),
        (WorkspaceTab::Ai, "3 AI"),
    ]
    .into_iter()
    .flat_map(|(tab, label)| {
        let style = if workspace.tab == tab {
            Style::default()
                .fg(theme.shortcut_text.to_color())
                .bg(theme.warning.to_color())
                .add_modifier(Modifier::BOLD)
        } else {
            muted
        };
        [Span::styled(format!(" {label} "), style), Span::raw("   ")]
    })
    .collect::<Vec<_>>();
    let ai_color = if ai_running {
        theme.warning.to_color()
    } else {
        theme.info.to_color()
    };
    let status_color = if workspace.observed_head_moved(active) {
        theme.danger.to_color()
    } else {
        theme.text_muted.to_color()
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(
                    format!(" #{} ", pr.number),
                    Style::default()
                        .fg(theme.shortcut_text.to_color())
                        .bg(theme.primary.to_color())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" {title}"),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled(
                    format!(" {}", pr.head_ref),
                    Style::default().fg(theme.secondary.to_color()),
                ),
                Span::styled(
                    format!(" @ {}", pr.head_sha.get(..8).unwrap_or(&pr.head_sha)),
                    muted,
                ),
                sep(),
                Span::styled(threads, Style::default().fg(theme.success.to_color())),
                sep(),
                Span::styled(ai, Style::default().fg(ai_color)),
            ]),
            Line::styled(format!(" {status}"), Style::default().fg(status_color)),
            Line::raw(""),
            Line::from(tabs),
        ]),
        area,
    );
}

fn draw_pane(
    frame: &mut Frame,
    area: Rect,
    pane: WorkspacePane,
    workspace: &mut PrWorkspace,
    active: &mut AppMode,
    theme: &Theme,
    ai_running: bool,
) {
    let tab = workspace.tab;
    if pane == WorkspacePane::Diff {
        if let Some(message) = &workspace.anchor_message {
            placeholder(frame, area, " Code context ", message, theme);
        } else if let Some(AppMode::DiffViewer(files)) =
            workspace.mode_mut(WorkspaceTab::Files, active)
        {
            if let Some(error) = &files.error {
                placeholder(frame, area, " Diff unavailable ", error, theme);
            } else if files.files.is_empty() {
                placeholder(
                    frame,
                    area,
                    " Changes ",
                    "This PR has no file changes.",
                    theme,
                );
            } else {
                // Side-by-side halves an already narrow pane and clips long
                // lines; the unified layout wraps them, so nothing runs off
                // the edge.
                // Switch once per width change (not per frame) so scroll limits
                // and input handling see the layout actually drawn.
                if area.width < 110 {
                    if files.narrow_layout_restore.is_none() {
                        files.narrow_layout_restore = Some(files.layout.clone());
                        files.layout = crate::app::DiffViewerLayout::Unified;
                    }
                } else if let Some(layout) = files.narrow_layout_restore.take() {
                    files.layout = layout;
                }
                super::diff::draw_patch(frame, area, files, theme);
            }
        }
        return;
    }
    if let Some(error) = &workspace.errors[tab.index()] {
        placeholder(
            frame,
            area,
            " Could not load ",
            &format!("{error}\n\nPress r to retry comments, or 1 to continue reviewing files."),
            theme,
        );
        return;
    }
    match workspace.mode_mut(tab, active) {
        Some(AppMode::DiffViewer(files)) => {
            if pane == WorkspacePane::Navigator {
                super::diff::draw_file_list(frame, area, files, theme);
            } else {
                draw_manual_context(frame, area, files, theme);
            }
        }
        Some(AppMode::PrReview(state)) => {
            if pane == WorkspacePane::Navigator {
                super::pr_review::draw_comment_list(frame, area, state, theme);
            } else {
                let investigation = state.selected_comment().and_then(|comment| {
                    state
                        .investigations
                        .iter()
                        .find(|run| run.comment_id == comment.id)
                });
                state.detail_content_lines = super::pr_review::draw_comment_detail(
                    frame,
                    area,
                    state.selected_comment(),
                    &state.review.comments,
                    investigation,
                    state.detail_scroll,
                    theme,
                );
            }
        }
        Some(AppMode::AiReview(state)) => draw_ai_pane(frame, area, pane, state, theme, ai_running),
        Some(AppMode::AiReviewRunning(state)) => {
            draw_ai_pane(frame, area, pane, &mut state.origin, theme, ai_running)
        }
        _ => placeholder(
            frame,
            area,
            " Loading ",
            "Loading this PR's feedback… You can keep reading in Files.",
            theme,
        ),
    }
}

fn draw_manual_context(
    frame: &mut Frame,
    area: Rect,
    state: &mut crate::app::DiffViewerState,
    theme: &Theme,
) {
    let Some(file) = state.files.get(state.selected_file) else {
        placeholder(frame, area, " Your review ", "No file selected.", theme);
        return;
    };
    let path = &file.path;
    let mut text = format!("**{path}**\n\nHuman review · unpublished\n\n");
    if state.changed_since_last.contains(path) {
        text.push_str("**Changed since your draft.** Review this file again.\n\n");
    }
    match state.decisions.get(path) {
        Some(crate::app::ReviewDecision::Approve) => text.push_str("File verdict: approved\n\n"),
        Some(crate::app::ReviewDecision::Reject { feedback, .. }) => {
            text.push_str(&format!("### Changes requested\n\n{feedback}\n\n"))
        }
        None => text.push_str("No file verdict yet.\n\n"),
    }
    if let Some(comment) = state.file_comments.get(path) {
        text.push_str(&format!(
            "### File comment{}\n\n{}\n\n",
            if comment.resolved {
                " (resolved locally)"
            } else {
                ""
            },
            comment.text
        ));
    }
    if let Some(comments) = state.line_comments.get(path) {
        for comment in comments {
            let label = if comment.draft {
                "AI draft · accept before posting"
            } else if comment.resolved {
                "resolved locally"
            } else if comment.anchor_lost {
                "outdated · goes in summary"
            } else {
                "unpublished"
            };
            text.push_str(&format!(
                "### {}\n\n{label}\n\n{}\n\n",
                crate::app::review::comment_anchor_label(path, comment),
                comment.text
            ));
            if let Some(suggestion) = &comment.suggestion {
                text.push_str(&format!("```suggestion\n{suggestion}\n```\n\n"));
            }
        }
    }
    if !state.general_feedback.trim().is_empty() {
        text.push_str(&format!(
            "### Review summary\n\n{}\n\n",
            state.general_feedback
        ));
    }
    let detached = state.pr_detached_line_comments.len() + state.pr_detached_file_comments.len();
    if detached > 0 {
        text.push_str(&format!("{detached} removed file(s) have comments kept for the submission summary. Press p to preview them.\n\n"));
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Your review ");
    let inner = block.inner(area);
    let mut rendered =
        crate::markdown::render_markdown(&text, theme, inner.width.max(1) as usize, None);
    rendered.lines.push(Line::raw(""));
    rendered.lines.push(Line::styled(
        "Keys",
        Style::default()
            .fg(theme.warning.to_color())
            .add_modifier(Modifier::BOLD),
    ));
    for (key, label) in [
        ("c", "line cursor"),
        ("Enter", "comment on line"),
        ("m", "file comment"),
        ("f", "review summary"),
        ("p", "preview and submit"),
    ] {
        rendered.lines.push(Line::from(vec![
            Span::styled(
                format!(" {key:<5} "),
                Style::default()
                    .fg(theme.shortcut_text.to_color())
                    .bg(theme.shortcut_background.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(" {label}")),
        ]));
    }
    state.notes_rendered_lines = rendered.lines.len();
    state.notes_view_height = inner.height as usize;
    state.notes_scroll = state.notes_scroll.min(
        state
            .notes_rendered_lines
            .saturating_sub(state.notes_view_height),
    );
    frame.render_widget(
        Paragraph::new(rendered.lines)
            .scroll((state.notes_scroll.min(u16::MAX as usize) as u16, 0))
            .block(block),
        area,
    );
}

fn draw_ai_pane(
    frame: &mut Frame,
    area: Rect,
    pane: WorkspacePane,
    state: &mut AiReviewState,
    theme: &Theme,
    ai_running: bool,
) {
    if pane == WorkspacePane::Navigator {
        super::ai_review::draw_finding_list(frame, area, state, theme, &[]);
    } else if state.findings.is_empty() {
        placeholder(
            frame,
            area,
            " AI Review ",
            if ai_running {
                "AI review in progress.\n\nYou can browse Files and Threads while it runs. Findings will appear here automatically."
            } else {
                "Press A to generate an AI review. You can browse Files while it runs.\n\nGenerating does not post findings to GitHub."
            },
            theme,
        );
    } else {
        state.detail_content_lines = super::ai_review::draw_finding_detail(
            frame,
            area,
            state.findings.get(state.selected),
            None,
            state.detail_scroll,
            theme,
        );
    }
}

fn placeholder(frame: &mut Frame, area: Rect, title: &str, message: &str, theme: &Theme) {
    placeholder_scrolled(frame, area, title, message, theme, 0);
}

fn placeholder_scrolled(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    message: &str,
    theme: &Theme,
    scroll: u16,
) {
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    let paragraph = Paragraph::new(message).wrap(Wrap { trim: false });
    let max_scroll = paragraph
        .line_count(inner.width)
        .saturating_sub(inner.height as usize)
        .min(u16::MAX as usize) as u16;
    frame.render_widget(
        paragraph
            .scroll((scroll.min(max_scroll), 0))
            .block(block)
            .style(Style::default().fg(theme.text_muted.to_color())),
        area,
    );
}

fn draw_help(frame: &mut Frame, theme: &Theme, scroll: u16) {
    let area = super::super::dashboard::centered_rect(88, 90, frame.area());
    clear_with_bg(frame, area, theme);
    placeholder_scrolled(
        frame,
        area,
        " PR Workspace · experimental ",
        "1 / 2 / 3    Files / GitHub threads / AI findings\nTab          Focus next pane (Shift+Tab goes back)\nj / k        Move in the navigator; scroll the focused pane\nEnter        Focus code; in line-cursor mode, write a comment\nz            Expand / restore the focused pane\n             (on a Files folder row, z / Enter fold that folder)\nEsc          Restore an expanded pane, otherwise save draft and close\n\nA            Generate AI review (uses existing harness/model pickers)\nCtrl+X       Cancel the AI review (also clickable in the navigator)\np            Preview and submit your human review\nW            In AI: preview and post kept AI findings\nF5           Reopen this same PR at the latest head\n\nFiles/code: c line cursor · m file comment · f summary\n            a approve file · s skip · Enter write line comment\nThreads:    f fix · v investigate · e investigation context\n            a investigation actions · R reply · M add to memory\n            m mark/resolve\n            r refresh comments · P visit fix session\nAI:         s skip · e edit · f fix · Space mark · B fix marked\n\nEditors and confirmation dialogs retain their existing controls.\nHuman drafts and AI findings are posted separately with attribution.\nUnpublished AI findings can be fixed without posting them.\n\nResize: 125+ columns = three panes; 85–124 = navigator drawer;\nunder 85 = focused pane only. z expands any pane.\n\n? / Esc closes help. leader+U / leader+W open the legacy PR Triage / AI Review workflows.",
        theme,
        scroll,
    );
}
