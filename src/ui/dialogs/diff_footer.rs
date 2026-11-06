use std::path::Path;

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
};
use unicode_width::UnicodeWidthStr;

use super::{cursor_comment, cursor_comment_peek_lines, effective_layout, is_new_diff_file};
use crate::{
    app::{DiffScope, DiffViewerFocus, DiffViewerLayout, DiffViewerState},
    editor::VimMode,
    highlight,
    theme::Theme,
};

pub(super) fn draw_footer(
    frame: &mut Frame,
    area: Rect,
    state: &mut DiffViewerState,
    theme: &Theme,
) {
    if state.review {
        draw_review_footer(frame, area, state, theme);
        return;
    }
    let focus = match state.focus {
        DiffViewerFocus::FileList => "files",
        DiffViewerFocus::Patch => "patch",
    };
    let new_file_selected = state
        .files
        .get(state.selected_file)
        .map(is_new_diff_file)
        .unwrap_or(false);
    let layout = match effective_layout(state) {
        DiffViewerLayout::Unified => "unified",
        DiffViewerLayout::SideBySide => "side-by-side",
    };
    let syntax_status = state
        .files
        .get(state.selected_file)
        .and_then(|file| highlight::language_install_state_for_path(Path::new(&file.path)));
    let context_level = state
        .files
        .get(state.selected_file)
        .filter(|file| file.can_expand_context())
        .map(|file| {
            state
                .context_expansion
                .get(&file.path)
                .copied()
                .unwrap_or(crate::diff::DIFF_DEFAULT_CONTEXT)
        });
    let footer = Paragraph::new(diff_footer_lines(
        focus,
        layout,
        new_file_selected,
        syntax_status,
        matches!(&state.scope, DiffScope::CurrentChanges),
        context_level,
        state.ignore_whitespace,
        state.amf_file_count > 0,
        area.width,
        theme,
    ));
    frame.render_widget(footer, area);
}

/// Whether the `E $EDITOR` hint earns its footer slot for the selected file.
/// Shared by the cursor and non-cursor footers so the two can't drift: a
/// deleted or binary file is still selectable (and, in cursor mode, still has
/// addressable removed lines), but `E` on it can only report why it won't open.
fn editor_hint_applies(state: &DiffViewerState) -> bool {
    // A PR review has no checkout to open.
    !state.is_pr_review()
        && state
            .files
            .get(state.selected_file)
            .is_some_and(|file| file.can_open_in_editor())
}

/// Ceiling on the review footer's key hints. Both rows wrapped in full can eat
/// a lot of vertical space on a narrow terminal, and the diff is what the
/// reviewer is actually there to read.
const REVIEW_HINT_MAX_ROWS: u16 = 8;

/// Rows a hint line occupies when rendered with `Wrap { trim: false }` at
/// `width`. Ratatui breaks on word boundaries, so `ceil(total / width)`
/// undercounts; this walks the same greedy word / whitespace runs the wrapper
/// does, which is what the footer has to be sized against.
pub(super) fn wrapped_line_height(line: &Line, width: u16) -> u16 {
    if width == 0 {
        return 1;
    }
    let width = width as usize;
    let text: String = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    let mut rows: usize = 1;
    let mut used: usize = 0;
    let mut rest = text.as_str();
    while !rest.is_empty() {
        let whitespace = rest.starts_with(char::is_whitespace);
        let end = rest
            .find(|c: char| c.is_whitespace() != whitespace)
            .unwrap_or(rest.len());
        let (run, tail) = rest.split_at(end);
        rest = tail;
        let run_width = run.width();
        if whitespace {
            // Whitespace that runs off the edge is absorbed there rather than
            // pushing a row of its own.
            used = (used + run_width).min(width);
            continue;
        }
        if used > 0 && used + run_width > width {
            rows += 1;
            used = 0;
        }
        if run_width > width {
            // A word wider than the row is hard-broken across rows.
            rows += (run_width - 1) / width;
            used = run_width % width;
            if used == 0 {
                used = width;
            }
        } else {
            used += run_width;
        }
    }
    rows.min(u16::MAX as usize) as u16
}

/// Rows the two hint lines want at `width`, before any clamp to the space
/// actually available.
fn hint_rows_height(first: &Line, second: &Line, width: u16) -> u16 {
    wrapped_line_height(first, width).saturating_add(wrapped_line_height(second, width))
}

/// Height the review footer's key hints need inside `inner`, clamped so a
/// heavily-wrapped footer can't crowd the diff off a short terminal.
pub(super) fn review_hint_height(state: &DiffViewerState, theme: &Theme, inner: Rect) -> u16 {
    let [first, second] = review_hint_lines(state, theme);
    // Mirror the feedback editor's reservation: header (2) + the body's Min(8).
    let cap = inner
        .height
        .saturating_sub(10)
        .clamp(2, REVIEW_HINT_MAX_ROWS);
    hint_rows_height(&first, &second, inner.width).clamp(2, cap)
}

/// Draw the two hint rows into sub-areas of their own. A single wrapping
/// `Paragraph` lets a long first row consume the whole footer and silently drop
/// the second one, taking the round-level keys (`b`, `F`, `t`, `X`, `q`, `Esc`)
/// with it — a drop that is invisible until the terminal happens to be narrow
/// enough. Sizing the second row first means the overflow lands on the first
/// row's tail instead, and that row leads with `? keys`.
fn render_hint_rows(frame: &mut Frame, area: Rect, [first, second]: [Line<'static>; 2]) {
    if area.height <= 1 {
        frame.render_widget(
            Paragraph::new(vec![first, second]).wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    let second_height = wrapped_line_height(&second, area.width).clamp(1, area.height - 1);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(area.height - second_height),
            Constraint::Length(second_height),
        ])
        .split(area);
    frame.render_widget(Paragraph::new(first).wrap(Wrap { trim: false }), chunks[0]);
    frame.render_widget(Paragraph::new(second).wrap(Wrap { trim: false }), chunks[1]);
}

fn draw_review_footer(frame: &mut Frame, area: Rect, state: &mut DiffViewerState, theme: &Theme) {
    if state.feedback_editing
        || state.editing_general
        || state.editing_line_comment
        || state.editing_file_comment
        || state.editing_suggestion
    {
        draw_feedback_editor(frame, area, state, theme);
        return;
    }

    let [first, second] = review_hint_lines(state, theme);

    // When the cursor sits on a commented line, peek the comment body in a
    // bordered box above the hints so the reviewer can read what they wrote
    // without re-opening the editor.
    if let Some(comment) = cursor_comment(state) {
        let hint_rows = hint_rows_height(&first, &second, area.width)
            .clamp(1, area.height.saturating_sub(3).max(1));
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(hint_rows)])
            .split(area);
        // A draft can never be resolved (only a kept comment can be), so the
        // resolved variants only need to branch off the non-draft arms.
        let title = match (comment.draft, comment.is_range(), comment.resolved) {
            (true, true, _) => " AI draft on these lines (a accept · d dismiss · Enter edit) ",
            (true, false, _) => " AI draft on this line (a accept · d dismiss · Enter edit) ",
            (false, true, true) => " resolved thread on these lines (Enter to edit · R reopen) ",
            (false, false, true) => " resolved thread on this line (Enter to edit · R reopen) ",
            (false, true, false) if comment.suggestion.is_some() => {
                " suggestion on these lines (x apply · Enter comment · S edit · R resolve) "
            }
            (false, false, false) if comment.suggestion.is_some() => {
                " suggestion on this line (x apply · Enter comment · S edit · R resolve) "
            }
            (false, true, false) => " comment on these lines (Enter to edit · R resolve) ",
            (false, false, false) => " comment on this line (Enter to edit · R resolve) ",
        };
        let box_color = if comment.draft {
            theme.warning.to_color()
        } else {
            theme.info.to_color()
        };
        let preview = Paragraph::new(
            cursor_comment_peek_lines(comment)
                .into_iter()
                .map(Line::from)
                .collect::<Vec<_>>(),
        )
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(box_color))
                .title(Span::styled(title, Style::default().fg(box_color))),
        )
        .wrap(Wrap { trim: false });
        frame.render_widget(preview, chunks[0]);
        render_hint_rows(frame, chunks[1], [first, second]);
        return;
    }

    render_hint_rows(frame, area, [first, second]);
}

/// The two key-hint rows the review footer shows right now: the finish
/// confirmation, the line cursor's bindings, or the standard verdict row.
/// Built separately from rendering so the footer can be *sized* to them —
/// see `review_hint_height`.
pub(super) fn review_hint_lines(state: &DiffViewerState, theme: &Theme) -> [Line<'static>; 2] {
    let key = |k: &'static str| Span::styled(k, Style::default().fg(theme.warning.to_color()));

    // A pending finish confirmation overrides the normal hints: some files have
    // no verdict and the reviewer just tried to finish.
    if state.finish_confirm {
        let (undecided, _) = state.undecided_counts();
        let amf_note = state.undecided_amf_note();
        let first = Line::from(vec![Span::styled(
            format!(" {undecided} file(s){amf_note} have no verdict — finish anyway? "),
            Style::default()
                .fg(theme.warning.to_color())
                .add_modifier(Modifier::BOLD),
        )]);
        let second = Line::from(vec![
            key(" q"),
            Span::raw("/"),
            key("y"),
            Span::raw(" finish anyway  "),
            key("u"),
            Span::raw(" next undecided  "),
            key("Esc"),
            Span::raw(" keep reviewing"),
        ]);
        return [first, second];
    }

    // While the line cursor is active, show its dedicated key hints instead of
    // the standard scroll/verdict row.
    if let Some(cursor) = state.comment_cursor {
        let comment_count = state
            .files
            .get(state.selected_file)
            .map(|file| {
                state
                    .line_comments
                    .get(&file.path)
                    .map(|c| c.len())
                    .unwrap_or(0)
            })
            .unwrap_or(0);
        // When a selection anchor is set, label the span being marked instead of
        // the lone cursor line.
        let position_label = match state.comment_anchor {
            Some(anchor) => {
                let lo = anchor.min(cursor) + 1;
                let hi = anchor.max(cursor) + 1;
                format!(" selecting {lo}-{hi} ")
            }
            None => format!(" line cursor @ {} ", cursor + 1),
        };
        // Cursor mode inverts the non-cursor footer's shape — here the first
        // line is the short one and the key row is what wraps — so the help
        // hint rides on the position label instead of leading the key row.
        let mut first_spans = vec![
            Span::styled(
                position_label,
                Style::default()
                    .fg(theme.warning.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("({comment_count} comment(s) on this file)  "),
                Style::default().fg(theme.info.to_color()),
            ),
            key("?"),
            Span::raw(" keys  "),
            key("Q"),
            Span::raw(" ask AI  "),
        ];
        // Surface a committed search so its shadowing of n/N is discoverable.
        if !state.editing_search && !state.search_query.trim().is_empty() {
            let count = if state.search_matches.is_empty() {
                "no match".to_string()
            } else {
                format!(
                    "{}/{}",
                    state.search_match_pos.map(|p| p + 1).unwrap_or(0),
                    state.search_matches.len()
                )
            };
            first_spans.push(Span::styled(
                format!("search:{} ({count})  ", state.search_query),
                Style::default().fg(theme.primary.to_color()),
            ));
            first_spans.push(key("n"));
            first_spans.push(Span::raw("/"));
            first_spans.push(key("N"));
            first_spans.push(Span::raw(" match  "));
            first_spans.push(key("Esc"));
            first_spans.push(Span::raw(" clear"));
        }
        let first = Line::from(first_spans);
        let mut second_spans = vec![
            key(" j"),
            Span::raw("/"),
            key("k"),
            Span::raw(" move  "),
            key("["),
            Span::raw("/"),
            key("]"),
            Span::raw(" hunk  "),
            key("v"),
            Span::raw(if state.comment_anchor.is_some() {
                " clear range  "
            } else {
                " select range  "
            }),
            key("Enter"),
            Span::raw(" comment  "),
            key("S"),
            Span::raw(" suggest  "),
        ];
        // A PR review's suggestions are posted to GitHub, never applied here.
        if !state.is_pr_review() {
            second_spans.extend([key("x"), Span::raw(" apply suggestion  ")]);
        }
        second_spans.extend([key("R"), Span::raw(" resolve/reopen  ")]);
        // Same gating as the non-cursor footer: a deleted or binary file has
        // nothing for an editor to open, even though its removed lines stay
        // addressable and so can still be cursored.
        if editor_hint_applies(state) {
            second_spans.extend([key("E"), Span::raw(" $EDITOR  ")]);
        }
        // Same gating as the non-cursor footer: cross-file comment navigation
        // only earns its slot once there is a comment somewhere to jump to.
        if state.line_comments.values().any(|cs| !cs.is_empty()) {
            second_spans.extend([key("{"), Span::raw("/"), key("}"), Span::raw(" comment  ")]);
        }
        second_spans.extend([
            key("n"),
            Span::raw("/"),
            key("p"),
            Span::raw(" file  "),
            key("c"),
            Span::raw("/"),
            key("Esc"),
            Span::raw(" exit cursor  "),
            key("Ctrl+T"),
            Span::raw(if state.vim_enabled {
                " editor vim: on  "
            } else {
                " editor vim: off  "
            }),
            key("q"),
            Span::raw(" finish"),
        ]);
        return [first, Line::from(second_spans)];
    }

    let mut second_line = vec![key(" j"), Span::raw("/"), key("k"), Span::raw(" scroll  ")];

    // Surface the syntax-highlight install/select affordance for the selected
    // file, mirroring the read-only diff viewer footer. Highlighting itself is
    // already applied by draw_patch_panel; `i` (handled by the shared key
    // handler) opens the language picker to install or repair the parser.
    if let Some((language, status)) = state
        .files
        .get(state.selected_file)
        .and_then(|file| highlight::language_install_state_for_path(Path::new(&file.path)))
    {
        let (label, color) = match status {
            highlight::HighlightInstallState::Installed => (
                format!("syntax:{} installed  ", language.display_name()),
                theme.info.to_color(),
            ),
            highlight::HighlightInstallState::Available => (
                format!("install {} parser  ", language.display_name()),
                theme.warning.to_color(),
            ),
            highlight::HighlightInstallState::Broken => (
                format!("repair {} parser  ", language.display_name()),
                theme.danger.to_color(),
            ),
        };
        second_line.push(key("i"));
        second_line.push(Span::raw(" "));
        second_line.push(Span::styled(label, Style::default().fg(color)));
    }

    // A PR review refuses the keys that need a local feature (a base ref, a
    // fix target, applying suggestions, finishing into an agent, AI passes run
    // in a checkout), so its footer doesn't advertise them.
    let pr_review = state.is_pr_review();
    if !pr_review {
        second_line.push(key("b"));
        second_line.push(Span::raw(" base ref  "));
    }
    second_line.push(key("F"));
    if state.file_filter == crate::app::FileFilter::All {
        second_line.push(Span::raw(" filter  "));
    } else {
        second_line.push(Span::styled(
            format!(" filter: {}  ", state.file_filter.label()),
            Style::default().fg(theme.info.to_color()),
        ));
    }
    let (target_label, target_color) = match state.fix_target {
        crate::app::pr_review::FixTarget::DedicatedReview => {
            (" target: dedicated  ", theme.info.to_color())
        }
        crate::app::pr_review::FixTarget::NewFeature => {
            (" target: new feature  ", theme.info.to_color())
        }
        crate::app::pr_review::FixTarget::ExistingFeature => {
            (" target: other feature  ", theme.info.to_color())
        }
        crate::app::pr_review::FixTarget::ExistingLive => {
            (" target: live  ", theme.text_muted.to_color())
        }
    };
    if !pr_review {
        second_line.push(key("t"));
        second_line.push(Span::styled(
            target_label,
            Style::default().fg(target_color),
        ));
    }
    let pending_suggestions = state.pending_suggestion_count();
    if pending_suggestions > 0 && !pr_review {
        second_line.push(key("X"));
        second_line.push(Span::styled(
            if state.apply_suggestions_on_finish {
                format!(" apply at finish: on ({pending_suggestions})  ")
            } else {
                format!(" apply {pending_suggestions} at finish  ")
            },
            Style::default().fg(if state.apply_suggestions_on_finish {
                theme.info.to_color()
            } else {
                theme.text_muted.to_color()
            }),
        ));
    }
    second_line.push(key("Ctrl+T"));
    second_line.push(Span::styled(
        if state.vim_enabled {
            " editor vim: on  "
        } else {
            " editor vim: off  "
        },
        Style::default().fg(if state.vim_enabled {
            theme.info.to_color()
        } else {
            theme.text_muted.to_color()
        }),
    ));
    if pr_review {
        second_line.push(key("q"));
        second_line.push(Span::raw(" submit to GitHub  "));
        second_line.push(key("Esc"));
        second_line.push(Span::raw(" pause (draft saved)"));
    } else {
        second_line.push(key("q"));
        second_line.push(Span::raw(" review summary → finish  "));
        second_line.push(key("Esc"));
        second_line.push(Span::raw(" pause (keep progress)"));
    }

    // `?` leads the row rather than joining the second one: the first line is
    // long enough to wrap into both footer rows on a narrow terminal, clipping
    // the second, and the pointer to the full key list is the one hint that
    // must never be the thing that falls off.
    let mut first_line = vec![
        key(" ?"),
        Span::raw(" keys  "),
        key("Q"),
        Span::raw(" ask AI  "),
        key("a"),
        Span::raw(" approve  "),
        key("r"),
        Span::raw(" reject  "),
        key("s"),
        Span::raw(" skip  "),
        key("f"),
        Span::raw(" general feedback  "),
        key("m"),
        Span::raw(" file comment  "),
        key("c"),
        Span::raw(" line comments  "),
        key("/"),
        Span::raw(" search"),
    ];
    if !state.general_feedback.trim().is_empty() {
        first_line.push(Span::styled(
            " ✎ note set",
            Style::default().fg(theme.info.to_color()),
        ));
    }
    if let Some(comment) = state
        .files
        .get(state.selected_file)
        .and_then(|file| state.file_comments.get(&file.path))
    {
        first_line.push(Span::styled(
            if comment.resolved {
                " ◇ file comment resolved  "
            } else {
                " ◆ file comment set  "
            },
            Style::default().fg(if comment.resolved {
                theme.text_muted.to_color()
            } else {
                theme.info.to_color()
            }),
        ));
        first_line.push(key("M"));
        first_line.push(Span::raw(if comment.resolved {
            " reopen"
        } else {
            " resolve"
        }));
    }
    first_line.extend([
        Span::raw("  "),
        key("n"),
        Span::raw("/"),
        key("p"),
        Span::raw(" file  "),
    ]);
    // Cross-file comment navigation only means something once something has
    // been commented on, so it stays out of an otherwise dense footer until then.
    if state.line_comments.values().any(|cs| !cs.is_empty()) {
        first_line.extend([key("{"), Span::raw("/"), key("}"), Span::raw(" comment  ")]);
    }
    // Likewise the undo hint: shown only while there's a verdict to take back.
    if !state.verdict_undo.is_empty() {
        first_line.extend([key("U"), Span::raw(" undo verdict  ")]);
    }
    // Tree folding only means something once the changeset spans directories.
    if state.files.iter().any(|file| file.path.contains('/')) {
        first_line.extend([key("z"), Span::raw("/"), key("Z"), Span::raw(" fold  ")]);
    }
    if state.amf_file_count > 0 {
        first_line.extend([key("."), Span::raw(" AMF files  ")]);
    }
    first_line.extend([
        key("e"),
        Span::raw(if state.notes_expanded {
            " show diff  "
        } else {
            " expand notes  "
        }),
        key("Tab"),
        Span::raw(" focus  "),
    ]);
    // Opening in `$EDITOR` needs a file that exists on disk with text in it, so
    // it stays hidden for a deletion or a binary blob rather than advertising a
    // key that can only report why it can't work.
    if editor_hint_applies(state) {
        first_line.extend([key("E"), Span::raw(" $EDITOR  ")]);
    }
    let layout_label = match effective_layout(state) {
        DiffViewerLayout::Unified => "unified",
        DiffViewerLayout::SideBySide => "side-by-side",
    };
    let new_file_selected = state
        .files
        .get(state.selected_file)
        .map(is_new_diff_file)
        .unwrap_or(false);
    if new_file_selected {
        first_line.push(Span::raw(format!(" layout:{layout_label} (new file)")));
    } else {
        first_line.push(key("v"));
        first_line.push(Span::raw(format!(" layout:{layout_label}")));
    }

    // Context expansion only means something for a file with hunks to widen —
    // an added/deleted/binary file already shows everything it can.
    if let Some(file) = state.files.get(state.selected_file)
        && file.can_expand_context()
    {
        let level = state
            .context_expansion
            .get(&file.path)
            .copied()
            .unwrap_or(crate::diff::DIFF_DEFAULT_CONTEXT);
        first_line.push(Span::raw("  "));
        first_line.push(key("+"));
        first_line.push(Span::raw("/"));
        first_line.push(key("-"));
        first_line.push(Span::styled(
            format!(" context:{}", crate::app::context_level_label(level)),
            Style::default().fg(if level == crate::diff::DIFF_DEFAULT_CONTEXT {
                theme.text_muted.to_color()
            } else {
                theme.info.to_color()
            }),
        ));
    }

    first_line.push(Span::raw("  "));
    first_line.push(key("W"));
    first_line.push(Span::styled(
        if state.ignore_whitespace {
            " ws: ignored"
        } else {
            " ws: shown"
        },
        Style::default().fg(if state.ignore_whitespace {
            theme.info.to_color()
        } else {
            theme.text_muted.to_color()
        }),
    ));

    // Offer the on-demand walkthrough only when the current file has no
    // developer note (the case where the notes panel would otherwise be empty).
    let has_note = state
        .files
        .get(state.selected_file)
        .is_some_and(|file| state.review_notes.contains_key(&file.path));
    if !pr_review {
        if !has_note {
            first_line.push(Span::raw("  "));
            first_line.push(key("w"));
            first_line.push(Span::raw(" gen walkthrough"));
        }

        // Reviewer-triggered AI co-review pass over the current file.
        first_line.push(Span::raw("  "));
        first_line.push(key("A"));
        first_line.push(Span::raw(" AI review"));

        // Reviewer-triggered whole-changeset overview / risk summary.
        first_line.push(Span::raw("  "));
        first_line.push(key("O"));
        first_line.push(Span::raw(" overview"));

        // Read-only timeline across the live review and every finished round.
        first_line.push(Span::raw("  "));
        first_line.push(key("H"));
        first_line.push(Span::raw(" history"));
    }

    // Offer the interdiff only for a file that actually changed since the
    // last review — the case where re-reading the whole diff to find the fix
    // is the exact pain this feature answers.
    let current_changed = state
        .files
        .get(state.selected_file)
        .is_some_and(|file| state.changed_since_last.contains(&file.path));
    if current_changed && !pr_review {
        first_line.push(Span::raw("  "));
        first_line.push(key("I"));
        first_line.push(Span::raw(" since last review"));
    }

    // Surface the jump-to-next-undecided affordance only while files still lack
    // a verdict.
    let undecided = state
        .files
        .iter()
        .filter(|file| !state.decisions.contains_key(&file.path))
        .count();
    if undecided > 0 {
        first_line.push(Span::raw("  "));
        first_line.push(key("u"));
        first_line.push(Span::styled(
            format!(" next undecided ({undecided})"),
            Style::default().fg(theme.text_muted.to_color()),
        ));
    }

    [Line::from(first_line), Line::from(second_line)]
}

/// Render the multi-line feedback editor (per-file rejection or general
/// feedback) into the review footer: a titled box with the editor text and a
/// key hint, mirroring the steering-prompt dialog.
fn draw_feedback_editor(frame: &mut Frame, area: Rect, state: &mut DiffViewerState, theme: &Theme) {
    let key = |k: &'static str| Span::styled(k, Style::default().fg(theme.warning.to_color()));

    let vim = state.feedback_editor.vim_mode();
    let mode_label = match vim {
        Some(VimMode::Insert) => " [Vim Insert]",
        Some(VimMode::Normal) => " [Vim Normal]",
        None => "",
    };
    // The line-comment and rejection editors carry a conventional-comments
    // severity (cycled with Ctrl+E); surface the current one in the title.
    let carries_severity =
        state.editing_line_comment || state.editing_file_comment || state.feedback_editing;
    let severity_title = if carries_severity {
        format!(" [{}]", state.comment_severity.label())
    } else {
        String::new()
    };
    let (title, border_color) = if state.editing_general {
        (
            format!(" General Feedback{mode_label} "),
            theme.info.to_color(),
        )
    } else if state.editing_file_comment {
        let path = state
            .files
            .get(state.selected_file)
            .map(|file| file.path.as_str())
            .unwrap_or("file");
        (
            format!(" File Comment — {path}{severity_title}{mode_label} "),
            theme.info.to_color(),
        )
    } else if state.editing_line_comment || state.editing_suggestion {
        let anchor = state
            .comment_cursor
            .and_then(|idx| {
                state.files.get(state.selected_file).and_then(|file| {
                    file.addressable_lines().get(idx).copied().map(|loc| {
                        match (loc.new_line, loc.old_line) {
                            (Some(new_line), _) => format!("{}:{new_line}", file.path),
                            (None, Some(old_line)) => format!("{}:{old_line} (base)", file.path),
                            (None, None) => file.path.clone(),
                        }
                    })
                })
            })
            .unwrap_or_else(|| "line".to_string());
        let label = if state.editing_suggestion {
            "Suggested change"
        } else {
            "Line Comment"
        };
        (
            format!(" {label} — {anchor}{severity_title}{mode_label} "),
            theme.warning.to_color(),
        )
    } else {
        (
            format!(" Rejection Feedback{severity_title}{mode_label} "),
            theme.danger.to_color(),
        )
    };

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));
    let editor_inner = block.inner(rows[0]);
    frame.render_widget(block, rows[0]);

    let placeholder = if state.editing_suggestion {
        "Edit the replacement code for these line(s)."
    } else {
        "Write feedback for the agent. Markdown is fine."
    };
    let editor_lines =
        crate::ui::dialogs::editor_view::editor_lines(&state.feedback_editor, theme, placeholder);
    let visible_lines = editor_inner.height as usize;
    let mut wrap_width = editor_inner.width as usize;
    let mut total_visual_lines =
        crate::ui::dialogs::editor_view::count_wrapped_editor_lines(&editor_lines, wrap_width);
    if total_visual_lines > visible_lines && wrap_width > 1 {
        wrap_width -= 1;
        total_visual_lines =
            crate::ui::dialogs::editor_view::count_wrapped_editor_lines(&editor_lines, wrap_width);
    }
    crate::ui::dialogs::editor_view::sync_editor_scroll(
        &state.feedback_editor,
        &mut state.feedback_scroll,
        &mut state.feedback_sync_to_cursor,
        visible_lines,
        wrap_width,
        total_visual_lines,
    );

    let paragraph = Paragraph::new(editor_lines)
        .wrap(Wrap { trim: false })
        .scroll((state.feedback_scroll.min(u16::MAX as usize) as u16, 0));
    frame.render_widget(paragraph, editor_inner);

    if total_visual_lines > visible_lines {
        let scrollbar = Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"));
        let mut scrollbar_state = ScrollbarState::new(total_visual_lines)
            .position(state.feedback_scroll)
            .viewport_content_length(visible_lines);
        frame.render_stateful_widget(scrollbar, rows[0], &mut scrollbar_state);
    }

    let cancel_hint = if vim.is_some() {
        key("Ctrl+Q")
    } else {
        key("Esc")
    };
    let mut hint_spans = vec![
        key(" Tab"),
        Span::raw(" submit  "),
        cancel_hint,
        Span::raw(" cancel  "),
        key("Enter"),
        Span::raw(" newline  "),
    ];
    if carries_severity {
        hint_spans.push(key("Ctrl+E"));
        hint_spans.push(Span::raw(format!(
            " severity: [{}]  ",
            state.comment_severity.label()
        )));
    }
    hint_spans.extend([
        key("Ctrl+T"),
        Span::raw(if vim.is_some() {
            " session vim off  "
        } else {
            " session vim on  "
        }),
        key("Ctrl+J/K"),
        Span::raw(" scroll"),
    ]);
    frame.render_widget(Paragraph::new(Line::from(hint_spans)), rows[1]);
}

/// Lay out one footer row from `(priority, spans)` hints so it fits `width`
/// columns: hints are dropped whole, highest priority number first (ties: the
/// later one), never cut mid-word. Priority 0 is never dropped. Hints are
/// separated by two spaces after a one-column lead.
fn fit_footer_hints(hints: Vec<(u8, Vec<Span<'static>>)>, width: usize) -> Line<'static> {
    use unicode_width::UnicodeWidthStr;
    let cost = |spans: &[Span]| -> usize {
        spans
            .iter()
            .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
            .sum()
    };
    let mut keep = vec![true; hints.len()];
    let total = |keep: &[bool]| -> usize {
        let kept: Vec<usize> = hints
            .iter()
            .zip(keep)
            .filter(|(_, k)| **k)
            .map(|((_, spans), _)| cost(spans))
            .collect();
        1 + kept.iter().sum::<usize>() + 2 * kept.len().saturating_sub(1)
    };
    while total(&keep) > width {
        let drop = hints
            .iter()
            .enumerate()
            .filter(|(i, (priority, _))| keep[*i] && *priority > 0)
            .max_by_key(|(i, (priority, _))| (*priority, *i))
            .map(|(i, _)| i);
        match drop {
            Some(i) => keep[i] = false,
            None => break,
        }
    }
    let mut spans = vec![Span::raw(" ")];
    for (n, ((_, hint), _)) in hints.into_iter().zip(keep).filter(|(_, k)| *k).enumerate() {
        if n > 0 {
            spans.push(Span::raw("  "));
        }
        spans.extend(hint);
    }
    Line::from(spans)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn diff_footer_lines(
    focus: &str,
    layout: &str,
    new_file_selected: bool,
    syntax_status: Option<(
        highlight::HighlightLanguage,
        highlight::HighlightInstallState,
    )>,
    can_change_base: bool,
    context_level: Option<usize>,
    ignore_whitespace: bool,
    has_amf_files: bool,
    width: u16,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let key = |k: &'static str| Span::styled(k, Style::default().fg(theme.warning.to_color()));
    let width = usize::from(width);

    let mut primary: Vec<(u8, Vec<Span<'static>>)> =
        vec![(0, vec![key("Tab"), Span::raw(format!(" focus:{focus}"))])];
    if let Some((language, status)) = syntax_status {
        let label = match status {
            highlight::HighlightInstallState::Installed => {
                format!(" syntax:{} installed", language.display_name())
            }
            highlight::HighlightInstallState::Available => {
                format!(" install {} parser", language.display_name())
            }
            highlight::HighlightInstallState::Broken => {
                format!(" repair {} parser", language.display_name())
            }
        };
        let color = match status {
            highlight::HighlightInstallState::Installed => theme.info.to_color(),
            highlight::HighlightInstallState::Available => theme.warning.to_color(),
            highlight::HighlightInstallState::Broken => theme.danger.to_color(),
        };
        primary.push((
            1,
            vec![key("i"), Span::styled(label, Style::default().fg(color))],
        ));
    }
    // On the first row, beside Esc, so it survives the narrowest footer: the
    // group is folded by default and this is how to see what it holds.
    if has_amf_files {
        primary.push((0, vec![key("."), Span::raw(" AMF files")]));
    }
    primary.push((0, vec![key("Esc"), Span::raw(" close")]));

    let mut secondary: Vec<(u8, Vec<Span<'static>>)> = Vec::new();
    if new_file_selected {
        secondary.push((
            1,
            vec![Span::styled(
                format!("layout:{layout} (new file)"),
                Style::default().fg(theme.info.to_color()),
            )],
        ));
    } else {
        secondary.push((1, vec![key("v"), Span::raw(format!(" layout:{layout}"))]));
    }
    secondary.push((0, vec![key("j/k"), Span::raw(" move")]));
    secondary.push((2, vec![key("z/Z"), Span::raw(" fold")]));
    secondary.push((2, vec![key("PgUp/PgDn"), Span::raw(" patch")]));
    secondary.push((3, vec![key("g/G"), Span::raw(" top/bottom")]));
    secondary.push((1, vec![key("r"), Span::raw(" refresh")]));
    if let Some(level) = context_level {
        secondary.push((
            2,
            vec![
                key("+/-"),
                Span::styled(
                    format!(" context:{}", crate::app::context_level_label(level)),
                    Style::default().fg(if level == crate::diff::DIFF_DEFAULT_CONTEXT {
                        theme.text_muted.to_color()
                    } else {
                        theme.info.to_color()
                    }),
                ),
            ],
        ));
    }
    secondary.push((
        2,
        vec![
            key("W"),
            Span::styled(
                if ignore_whitespace {
                    " ws: ignored"
                } else {
                    " ws: shown"
                },
                Style::default().fg(if ignore_whitespace {
                    theme.info.to_color()
                } else {
                    theme.text_muted.to_color()
                }),
            ),
        ],
    ));
    if can_change_base {
        secondary.push((1, vec![key("b"), Span::raw(" base ref")]));
    }

    vec![
        fit_footer_hints(primary, width),
        fit_footer_hints(secondary, width),
    ]
}
