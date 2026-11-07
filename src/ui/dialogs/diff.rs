use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use std::path::Path;

#[cfg(test)]
use crate::app::{DiffPickerState, ReviewDecision};

use crate::{
    app::{DiffScope, DiffViewerFocus, DiffViewerLayout, DiffViewerState},
    diff::{DiffFile, DiffFileStatus, DiffHunk, DiffLine, DiffLineKind, DiffLineLocation},
    highlight,
    theme::Theme,
};

use super::super::dashboard::centered_rect;

#[path = "diff_patch.rs"]
mod diff_patch;
pub(super) use diff_patch::{
    PatchPanelOptions, draw_patch, draw_patch_panel, draw_patch_with_gutter, effective_layout,
    is_new_diff_file, status_color, status_label,
};
#[cfg(test)]
use diff_patch::{
    added_emphasis_style, added_row_style, hunk_intra_line_ranges, indexed_patch_lines,
    new_file_added_row_style, patch_lines,
};
#[path = "diff_picker.rs"]
mod diff_picker;
pub use diff_picker::draw_diff_picker;
/// Base-selection guidance for a current-changes view without a usable base.
/// Either the diff fell back to `HEAD` or the selected base could not be read.
pub(super) const CHOOSE_BASE_HINT: &str = "Press b to choose a base ref.";

#[path = "diff_file_tree.rs"]
mod diff_file_tree;
pub(in crate::ui::dialogs) use diff_file_tree::draw_file_list;
#[cfg(test)]
use diff_file_tree::{changeset_has_test_changes, dir_row_summary, file_risk_marker};
#[path = "diff_text.rs"]
mod diff_text;
use diff_text::{
    IntraLineEmphasis, StyledChunk, chunks_to_spans, diff_chunks, diff_chunks_emphasized,
    file_highlights, highlighted_line, pad_chunks_to_width, plain_chunks, wrap_chunks,
};
#[path = "diff_overlays.rs"]
mod diff_overlays;
#[cfg(test)]
use diff_overlays::{REVIEW_HELP_SECTIONS, review_help_sections};
use diff_overlays::{
    draw_base_ref_prompt, draw_changeset_overview_modal, draw_interdiff_modal,
    draw_pr_submit_modal, draw_review_help_modal, draw_review_history_modal,
    draw_review_summary_modal, draw_search_prompt,
};
#[path = "diff_comments.rs"]
mod diff_comments;
#[cfg(test)]
use diff_comments::cursor_comment_text;
use diff_comments::{
    ReviewLineMarkers, cursor_comment, cursor_comment_peek_lines, cursor_comment_preview_rows,
    review_cursor_info,
};
#[path = "diff_body.rs"]
mod diff_body;
use diff_body::draw_body;
#[cfg(test)]
use diff_body::pr_review_notes_markdown;
#[path = "diff_header.rs"]
mod diff_header;
#[cfg(test)]
use diff_header::amf_split_count_label;
use diff_header::draw_header;
#[path = "diff_footer.rs"]
mod diff_footer;
#[cfg(test)]
use diff_footer::{diff_footer_lines, review_hint_lines, wrapped_line_height};
use diff_footer::{draw_footer, review_hint_height};

pub fn draw_diff_viewer(frame: &mut Frame, state: &mut DiffViewerState, theme: &Theme) {
    if state.questions.open {
        super::review_questions::draw(frame, &mut state.questions, theme);
        return;
    }
    let area = centered_rect(96, 90, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let border_color = if state.error.is_some() {
        theme.danger.to_color()
    } else {
        theme.primary.to_color()
    };

    let block = Block::default()
        .title(if matches!(&state.scope, DiffScope::PullRequest(_)) {
            " PR Review "
        } else if state.review {
            " Final Review "
        } else if matches!(&state.scope, DiffScope::Commit(_)) {
            " Commit Diff "
        } else {
            " Current Changes "
        })
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(border_color));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // The review footer grows to host the multi-line feedback editor while it
    // is open; otherwise it is a two-row key hint.
    let footer_height = if state.review
        && (state.feedback_editing
            || state.editing_general
            || state.editing_line_comment
            || state.editing_file_comment
            || state.editing_suggestion)
    {
        inner.height.saturating_sub(10).clamp(4, 12)
    } else if state.review {
        // Grow to fit the hints themselves — the verdict row is dense enough to
        // wrap past two rows on an ordinary terminal — plus the line-comment
        // peek box when the cursor is parked on a line that carries a comment.
        review_hint_height(state, theme, inner) + cursor_comment_preview_rows(state)
    } else {
        2
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(8),
            Constraint::Length(footer_height),
        ])
        .split(inner);

    draw_header(frame, chunks[0], state, theme);
    draw_body(frame, chunks[1], state, theme);
    draw_footer(frame, chunks[2], state, theme);

    if state.editing_base_ref {
        draw_base_ref_prompt(frame, state, theme);
    }
    if state.editing_search {
        draw_search_prompt(frame, state, theme);
    }
    if state.changeset_overview_open {
        draw_changeset_overview_modal(frame, state, theme);
    }
    if state.interdiff_open {
        draw_interdiff_modal(frame, state, theme);
    }
    if state.summary_open {
        draw_review_summary_modal(frame, state, theme);
    }
    if state.review_history.is_some() {
        draw_review_history_modal(frame, state, theme);
    }
    // Drawn last so it sits on top of anything else the reviewer left open.
    if state.help_open {
        draw_review_help_modal(frame, state, theme);
    }
    // The destination picker and the companion-feature setup it can open sit on
    // top of everything (they capture every key while shown).
    if let Some(pick) = &state.destination_pick {
        super::draw_review_destination_pick(frame, pick, theme);
    }
    if let Some(setup) = &state.review_feature_setup {
        super::draw_review_feature_setup(frame, setup, theme);
    }
    // Hidden while its summary is being edited, so the editor is visible.
    if state.pr_submit.is_some() && !state.editing_general {
        draw_pr_submit_modal(frame, state, theme);
    }
}

pub fn draw_diff_viewer_loading(
    frame: &mut Frame,
    state: &DiffViewerState,
    throbber_state: &throbber_widgets_tui::ThrobberState,
    theme: &Theme,
) {
    let area = centered_rect(54, 28, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let branch = if state.branch.is_empty() {
        state.from_view.feature_name.as_str()
    } else {
        state.branch.as_str()
    };
    let (title, loading_label, detail) = match &state.scope {
        DiffScope::Commit(commit) => (
            " Commit Diff ",
            " Loading commit diff...",
            format!("{}  {}", commit.short_hash, commit.subject),
        ),
        DiffScope::CurrentChanges => (
            " Current Changes ",
            " Loading current changes...",
            format!("Comparing all changes for {branch}"),
        ),
        DiffScope::PullRequest(target) => (
            " PR Review ",
            " Loading pull request...",
            format!("#{}  {}", target.pr.number, target.pr.title),
        ),
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let throbber = throbber_widgets_tui::Throbber::default()
        .style(Style::default().fg(theme.warning.to_color()));
    let spinner = throbber.to_symbol_span(throbber_state);

    let loading = Paragraph::new(vec![
        Line::from(""),
        Line::from(vec![
            spinner,
            Span::styled(
                loading_label,
                Style::default()
                    .fg(theme.text.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            detail,
            Style::default().fg(theme.text_muted.to_color()),
        )),
    ])
    .wrap(Wrap { trim: false });

    frame.render_widget(loading, inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_text(line: &Line<'static>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    fn changed_pair_hunk(old: &str, new: &str) -> DiffHunk {
        DiffHunk {
            header: "@@ -1 +1 @@".into(),
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 1,
            lines: vec![
                DiffLine {
                    kind: DiffLineKind::Removed,
                    text: format!("-{old}"),
                },
                DiffLine {
                    kind: DiffLineKind::Added,
                    text: format!("+{new}"),
                },
            ],
        }
    }

    #[test]
    fn intra_line_ranges_pair_a_removal_with_its_replacement() {
        let hunk = changed_pair_hunk("let x = foo(1);", "let x = foo(2);");
        let ranges = hunk_intra_line_ranges(&hunk);

        // Index 0 is the removal, index 1 its paired addition.
        let old = ranges[0].as_ref().expect("removal has emphasis");
        let new = ranges[1].as_ref().expect("addition has emphasis");
        assert_eq!(&"let x = foo(1);"[old[0].clone()], "1");
        assert_eq!(&"let x = foo(2);"[new[0].clone()], "2");
    }

    #[test]
    fn intra_line_ranges_skip_unpaired_and_context_lines() {
        // Two removals, one addition: only the first pair has a counterpart.
        let hunk = DiffHunk {
            header: "@@ -1,3 +1,2 @@".into(),
            old_start: 1,
            old_lines: 3,
            new_start: 1,
            new_lines: 2,
            lines: vec![
                DiffLine {
                    kind: DiffLineKind::Context,
                    text: " unchanged".into(),
                },
                DiffLine {
                    kind: DiffLineKind::Removed,
                    text: "-value = 1;".into(),
                },
                DiffLine {
                    kind: DiffLineKind::Removed,
                    text: "-dropped();".into(),
                },
                DiffLine {
                    kind: DiffLineKind::Added,
                    text: "+value = 2;".into(),
                },
            ],
        };
        let ranges = hunk_intra_line_ranges(&hunk);

        assert!(ranges[0].is_none(), "context lines are never emphasised");
        assert!(ranges[1].is_some(), "first removal pairs with the addition");
        assert!(ranges[2].is_none(), "second removal has no counterpart");
        assert!(ranges[3].is_some());
    }

    #[test]
    fn emphasis_splits_chunks_without_losing_or_reordering_text() {
        let theme = Theme::default();
        let row = added_row_style(&theme);
        let emphasis = IntraLineEmphasis {
            // Two disjoint runs inside "abcdef": "bc" and "e".
            ranges: Vec::from([1..3, 4..5]),
            style: added_emphasis_style(&theme),
        };
        let chunks = diff_chunks_emphasized("+abcdef", row, &theme, None, Some(&emphasis));

        let text: String = chunks.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(text, "+abcdef", "no text may be lost or reordered");

        // Only the emphasised slices carry the brighter background; the rest
        // keeps the plain row background.
        let emphasised: String = chunks
            .iter()
            .filter(|c| c.style.bg == added_emphasis_style(&theme).bg)
            .map(|c| c.text.as_str())
            .collect();
        assert_eq!(emphasised, "bce");
    }

    #[test]
    fn emphasis_offsets_are_measured_past_the_diff_prefix() {
        let theme = Theme::default();
        // Offsets are into the *content*, not the raw line — starting a range
        // at 0 must highlight `a`, never the `+` prefix.
        let emphasis = IntraLineEmphasis {
            ranges: Vec::from([0..3, 5..6]),
            style: added_emphasis_style(&theme),
        };
        let chunks = diff_chunks_emphasized(
            "+abcdef",
            added_row_style(&theme),
            &theme,
            None,
            Some(&emphasis),
        );

        let emphasised: String = chunks
            .iter()
            .filter(|c| c.style.bg == added_emphasis_style(&theme).bg)
            .map(|c| c.text.as_str())
            .collect();
        assert_eq!(emphasised, "abcf");
    }

    #[test]
    fn the_amf_split_count_reads_cleanly_for_singular_plural_and_none() {
        assert_eq!(
            amf_split_count_label(1, 4),
            "5 files · 1 is yours · 4 generated by AMF"
        );
        assert_eq!(
            amf_split_count_label(3, 1),
            "4 files · 3 are yours · 1 generated by AMF"
        );
        assert_eq!(
            amf_split_count_label(0, 2),
            "2 files · none are yours · 2 generated by AMF"
        );
        assert_eq!(
            amf_split_count_label(0, 1),
            "1 file · none are yours · 1 generated by AMF"
        );
    }

    #[test]
    fn diff_footer_shows_the_ignore_whitespace_state() {
        let theme = Theme::default();
        let shown = diff_footer_lines(
            "files", "unified", false, None, true, None, false, false, 200, &theme,
        );
        assert!(line_text(&shown[1]).contains("ws: shown"));

        let ignored = diff_footer_lines(
            "files", "unified", false, None, true, None, true, false, 200, &theme,
        );
        assert!(line_text(&ignored[1]).contains("ws: ignored"));
    }

    /// The second row used to wrap at 120 columns with every hint present,
    /// pushing `b base ref` onto a third row the two-row footer never draws.
    #[test]
    fn diff_footer_rows_fit_whole_hints_at_80_and_120_columns() {
        use unicode_width::UnicodeWidthStr;
        let theme = Theme::default();
        for width in [80u16, 120] {
            let lines = diff_footer_lines(
                "files",
                "side-by-side",
                false,
                Some((
                    highlight::HighlightLanguage::Tsx,
                    highlight::HighlightInstallState::Available,
                )),
                true,
                Some(crate::diff::DIFF_DEFAULT_CONTEXT + 7),
                true,
                true,
                width,
                &theme,
            );
            for line in &lines {
                let text = line_text(line);
                assert!(
                    UnicodeWidthStr::width(text.as_str()) <= usize::from(width),
                    "{width}: {text:?} overflows"
                );
            }
            let first = line_text(&lines[0]);
            assert!(first.contains(". AMF files"), "{width}: {first}");
            assert!(first.contains("Esc close"), "{width}: {first}");
            let second = line_text(&lines[1]);
            assert!(second.contains("j/k move"), "{width}: {second}");
            assert!(second.contains("r refresh"), "{width}: {second}");
            assert!(second.contains("b base ref"), "{width}: {second}");
            // Whatever is dropped goes whole: no hint is left without its label.
            assert!(!second.trim_end().ends_with('b'), "{width}: {second}");
        }
        let wide = diff_footer_lines(
            "files",
            "unified",
            false,
            None,
            true,
            Some(3),
            false,
            false,
            200,
            &theme,
        );
        assert!(line_text(&wide[1]).contains("g/G top/bottom"));
    }

    #[test]
    fn diff_footer_prioritizes_syntax_install_hint() {
        let theme = Theme::default();
        let lines = diff_footer_lines(
            "files",
            "unified",
            true,
            Some((
                highlight::HighlightLanguage::Tsx,
                highlight::HighlightInstallState::Available,
            )),
            true,
            None,
            false,
            false,
            200,
            &theme,
        );

        assert_eq!(lines.len(), 2);
        assert!(line_text(&lines[0]).contains("install tsx parser"));
        assert!(line_text(&lines[0]).contains("Esc close"));
        assert!(line_text(&lines[1]).contains("layout:unified (new file)"));
    }

    #[test]
    fn file_comment_editor_expands_footer_and_renders_edit_box() {
        use ratatui::{Terminal, backend::TestBackend};

        let (mut state, _) = single_added_line_review_state();
        state.editing_file_comment = true;
        let backend = TestBackend::new(120, 36);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| draw_diff_viewer(frame, &mut state, &Theme::default()))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(rendered.contains("File Comment — a.rs"));
        assert!(rendered.contains("Write feedback for the agent. Markdown is fine."));
    }

    #[test]
    fn review_footer_reports_session_vim_state() {
        let (mut state, _) = single_added_line_review_state();
        let theme = Theme::default();

        let plain = review_hint_lines(&state, &theme);
        assert!(line_text(&plain[1]).contains("editor vim: off"));

        state.toggle_feedback_vim();
        let vim = review_hint_lines(&state, &theme);
        assert!(line_text(&vim[1]).contains("editor vim: on"));
    }

    #[test]
    fn active_review_editor_reports_normal_mode_and_session_toggle() {
        use ratatui::{Terminal, backend::TestBackend};

        let (mut state, _) = single_added_line_review_state();
        state.editing_file_comment = true;
        state.toggle_feedback_vim();
        let backend = TestBackend::new(120, 36);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| draw_diff_viewer(frame, &mut state, &Theme::default()))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(rendered.contains("[Vim Normal]"));
        assert!(rendered.contains("session vim off"));
    }

    #[test]
    fn review_help_documents_session_vim_submit_and_cancel_controls() {
        let comments = REVIEW_HELP_SECTIONS
            .iter()
            .find(|(title, _)| *title == "Comments")
            .expect("Comments help section")
            .1;

        assert!(comments.iter().any(|(key, text)| {
            *key == "Ctrl+T" && text.contains("every editor in this review session")
        }));
        assert!(
            comments
                .iter()
                .any(|(key, text)| *key == "Tab" && text.contains("either keymap"))
        );
        assert!(
            comments
                .iter()
                .any(|(key, text)| *key == "Ctrl+Q" && text.contains("Cancel"))
        );
        assert!(
            comments
                .iter()
                .any(|(_, text)| text.contains("enabling Vim enters Normal mode"))
        );
    }

    #[test]
    fn new_file_patch_lines_preserve_syntax_coloring_for_indented_javascript() {
        if highlight::HighlightLanguage::JavaScript.install_state()
            != highlight::HighlightInstallState::Installed
        {
            return;
        }

        crate::highlight::reload_runtime_state();

        let theme = Theme::default();
        let file = DiffFile {
            old_path: None,
            path: "syntax-test.js".to_string(),
            status: DiffFileStatus::Added,
            additions: 3,
            deletions: 0,
            is_binary: false,
            old_content: None,
            new_content: Some("const palette = {\n  primary: \"#0f172a\",\n};\n".to_string()),
            patch: "\
diff --git a/syntax-test.js b/syntax-test.js
new file mode 100644
index 0000000..1111111
--- /dev/null
+++ b/syntax-test.js
@@ -0,0 +1,3 @@
+const palette = {
+  primary: \"#0f172a\",
+};
"
            .to_string(),
            hunks: vec![crate::diff::DiffHunk {
                header: "@@ -0,0 +1,3 @@".to_string(),
                old_start: 0,
                old_lines: 0,
                new_start: 1,
                new_lines: 3,
                lines: vec![
                    crate::diff::DiffLine {
                        kind: crate::diff::DiffLineKind::Added,
                        text: "+const palette = {".to_string(),
                    },
                    crate::diff::DiffLine {
                        kind: crate::diff::DiffLineKind::Added,
                        text: "+  primary: \"#0f172a\",".to_string(),
                    },
                    crate::diff::DiffLine {
                        kind: crate::diff::DiffLineKind::Added,
                        text: "+};".to_string(),
                    },
                ],
            }],
        };

        let lines = patch_lines(
            &file,
            100,
            &theme,
            false,
            true,
            None,
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &mut None,
        );
        let indented_code_line = &lines[2];
        let default_added_fg = new_file_added_row_style(&theme).fg;
        let has_syntax_colored_token = indented_code_line.spans.iter().any(|span| {
            !span.content.trim().is_empty()
                && span.content.contains("primary")
                && span.style.fg != default_added_fg
        });

        assert!(
            has_syntax_colored_token,
            "expected indented JavaScript property to keep syntax coloring in new-file diff rows"
        );
    }

    #[test]
    fn new_file_patch_lines_preserve_syntax_coloring_for_typescript() {
        if highlight::HighlightLanguage::TypeScript.install_state()
            != highlight::HighlightInstallState::Installed
        {
            return;
        }

        crate::highlight::reload_runtime_state();

        let theme = Theme::default();
        let file = DiffFile {
            old_path: None,
            path: "docs/syntax-tests/syntax-test-highlight.ts".to_string(),
            status: DiffFileStatus::Added,
            additions: 74,
            deletions: 0,
            is_binary: false,
            old_content: None,
            new_content: Some(
                include_str!("../../../docs/syntax-tests/syntax-test-highlight.ts").to_string(),
            ),
            patch: "\
diff --git a/docs/syntax-tests/syntax-test-highlight.ts b/docs/syntax-tests/syntax-test-highlight.ts
new file mode 100644
index 0000000..1111111
--- /dev/null
+++ b/docs/syntax-tests/syntax-test-highlight.ts
@@ -0,0 +1,74 @@
"
            .to_string(),
            hunks: vec![crate::diff::DiffHunk {
                header: "@@ -0,0 +1,74 @@".to_string(),
                old_start: 0,
                old_lines: 0,
                new_start: 1,
                new_lines: 74,
                lines: include_str!("../../../docs/syntax-tests/syntax-test-highlight.ts")
                    .lines()
                    .map(|line| crate::diff::DiffLine {
                        kind: crate::diff::DiffLineKind::Added,
                        text: format!("+{line}"),
                    })
                    .collect(),
            }],
        };

        let lines = patch_lines(
            &file,
            120,
            &theme,
            false,
            true,
            None,
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &mut None,
        );
        let has_syntax_colored_token =
            lines.iter().flat_map(|line| line.spans.iter()).any(|span| {
                !span.content.trim().is_empty()
                    && span.content.contains("JsonPrimitive")
                    && span.style.fg != new_file_added_row_style(&theme).fg
            });

        assert!(
            has_syntax_colored_token,
            "expected TypeScript diff rows to keep syntax coloring in new-file diff rows"
        );
    }

    #[test]
    fn pr_review_notes_list_comments_on_files_that_left_the_pr() {
        let (mut state, _) = single_added_line_review_state();
        // Nothing changed, nothing outdated, nothing detached: nothing to say.
        assert_eq!(pr_review_notes_markdown(&state, "a.rs"), None);
        state.pr_detached_line_comments.insert(
            "gone.rs".to_string(),
            vec![
                serde_json::from_value(serde_json::json!({
                    "location": {"old_line": null, "new_line": 4},
                    "text": "about removed code"
                }))
                .unwrap(),
            ],
        );
        state.pr_detached_file_comments.insert(
            "gone.rs".to_string(),
            serde_json::from_value(serde_json::json!({"text": "whole-file note"})).unwrap(),
        );

        let notes = pr_review_notes_markdown(&state, "a.rs").unwrap();

        assert!(
            notes.contains("Comments on files no longer in this PR"),
            "{notes}"
        );
        assert!(notes.contains("gone.rs (was L4)"), "{notes}");
        assert!(notes.contains("about removed code"), "{notes}");
        assert!(notes.contains("gone.rs** (whole file)"), "{notes}");
        assert!(!notes.contains("Outdated comments"), "{notes}");
    }

    fn single_added_line_review_state() -> (DiffViewerState, DiffLineLocation) {
        let mut state = DiffViewerState::new(
            crate::app::ViewState::new(
                "proj".into(),
                "feat".into(),
                "sess".into(),
                "claude".into(),
                "Claude".into(),
                crate::project::SessionKind::Claude,
                crate::project::VibeMode::Vibeless,
                true,
            ),
            std::path::PathBuf::from("/tmp"),
        );
        state.review = true;
        let file = DiffFile {
            old_path: None,
            path: "a.rs".to_string(),
            status: DiffFileStatus::Added,
            additions: 1,
            deletions: 0,
            is_binary: false,
            old_content: None,
            new_content: Some("x\n".to_string()),
            patch: String::new(),
            hunks: vec![crate::diff::DiffHunk {
                header: "@@ -0,0 +1,1 @@".to_string(),
                old_start: 0,
                old_lines: 0,
                new_start: 1,
                new_lines: 1,
                lines: vec![crate::diff::DiffLine {
                    kind: crate::diff::DiffLineKind::Added,
                    text: "+x".to_string(),
                }],
            }],
        };
        let loc = file.addressable_lines()[0];
        state.files = vec![file];
        state.selected_file = 0;
        (state, loc)
    }

    fn render_viewer(state: &mut DiffViewerState) -> String {
        use ratatui::{Terminal, backend::TestBackend};
        let mut terminal = Terminal::new(TestBackend::new(160, 36)).unwrap();
        terminal
            .draw(|frame| draw_diff_viewer(frame, state, &Theme::default()))
            .unwrap();
        normalized_screen(&terminal)
    }

    #[test]
    fn head_fallback_is_labelled_and_the_body_offers_b() {
        for review in [false, true] {
            let (mut state, _) = single_added_line_review_state();
            state.review = review;
            state.branch = "main".into();
            state.base_ref = "HEAD".into();
            state.base_commit = "0123456789abcdef".into();
            state.no_base_branch = true;
            let screen = render_viewer(&mut state);
            assert!(
                screen.contains("vs HEAD (no base branch found)"),
                "review={review}: {screen}"
            );
            assert!(
                screen.contains("Press b to choose a base ref."),
                "review={review}"
            );
            assert!(!screen.contains("(unknown branch)"));
        }

        // Nothing to show yet: the empty state carries the same hint.
        let (mut state, _) = single_added_line_review_state();
        state.review = false;
        state.files.clear();
        state.no_base_branch = true;
        let screen = render_viewer(&mut state);
        assert!(screen.contains("No uncommitted changes"), "{screen}");
        assert!(screen.contains("Press b to choose a base ref."));
    }

    #[test]
    fn a_resolved_base_shows_no_fallback_notice() {
        let (mut state, _) = single_added_line_review_state();
        state.branch = "feature".into();
        state.base_ref = "main".into();
        let screen = render_viewer(&mut state);
        assert!(screen.contains("vs main"), "{screen}");
        assert!(!screen.contains("no base branch found"));
    }

    #[test]
    fn a_failed_current_changes_load_offers_b_in_the_body() {
        let (mut state, _) = single_added_line_review_state();
        state.review = false;
        state.files.clear();
        state.error = Some("Could not determine a base branch for 'x'".into());
        let screen = render_viewer(&mut state);
        assert!(
            screen.contains("Could not load current changes"),
            "{screen}"
        );
        assert!(screen.contains("Press b to choose a base ref. Press r to retry."));
    }

    #[test]
    fn scope_picker_explains_missing_commits_in_one_line() {
        use ratatui::{Terminal, backend::TestBackend};
        let (viewer, _) = single_added_line_review_state();
        let mut picker = DiffPickerState {
            from_view: viewer.from_view.clone(),
            workdir: "/tmp".into(),
            commits: Vec::new(),
            selected: 0,
            error: None,
            no_base_branch: true,
            base_unresolved: false,
        };
        let render = |picker: &DiffPickerState| {
            let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
            terminal
                .draw(|frame| draw_diff_picker(frame, picker, &Theme::default()))
                .unwrap();
            normalized_screen(&terminal)
        };
        let screen = render(&picker);
        assert!(screen.contains("No base branch found"), "{screen}");
        assert!(screen.contains("vs HEAD"));
        assert!(!screen.contains("0 commit(s)"));

        picker.no_base_branch = false;
        picker.error = Some("Detached HEAD: commits are listed only on a named branch.".into());
        let screen = render(&picker);
        assert!(screen.contains("Detached HEAD"), "{screen}");
        assert!(!screen.contains("Could not list feature commits"));
        assert!(!screen.contains("0 commit(s)"));
    }

    #[test]
    fn review_history_modal_renders_current_state_and_lazy_archive_tail() {
        use ratatui::{Terminal, backend::TestBackend};

        let (mut state, _) = single_added_line_review_state();
        state
            .decisions
            .insert("a.rs".to_string(), ReviewDecision::Approve);
        state.review_history = Some(crate::app::ReviewHistoryState {
            rounds: vec![crate::app::ReviewHistoryRound {
                title: "Review — r1".to_string(),
                markdown: "## Review — r1\n\n**Approved:** 1\n".to_string(),
                carried_unresolved: 0,
            }],
            selected: 0,
            scroll: 0,
            rendered_lines: 0,
            view_height: 0,
            archive_available: true,
            archive_loaded: false,
            error: None,
            archive_error: None,
        });
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_diff_viewer(frame, &mut state, &Theme::default()))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(rendered.contains("Review Timeline"));
        assert!(rendered.contains("Current"));
        assert!(rendered.contains("Last review"));
        assert!(rendered.contains("Older"));
        assert!(rendered.contains("Current Review"));
        assert!(rendered.contains("Approved: 1"));
        assert!(rendered.contains("press Enter to return to editing"));
    }

    #[test]
    fn help_modal_renders_grouped_keys_and_records_its_scroll_extent() {
        use ratatui::{Terminal, backend::TestBackend};

        let (mut state, _) = single_added_line_review_state();
        state.help_open = true;

        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_diff_viewer(frame, &mut state, &Theme::default()))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(rendered.contains("Final Review"));
        assert!(rendered.contains("Verdicts"));
        assert!(rendered.contains("Approve the current file"));
        assert!(rendered.contains("Line cursor"));

        // The overlay is taller than its viewport, and the renderer has to
        // report both so `G` can clamp to the real bottom.
        assert!(state.help_rendered_lines > state.help_view_height);
        assert!(state.help_view_height > 0);
    }

    /// Structural guard on the help table: a section that renders as a bare
    /// heading, or a continuation line with nothing above it to continue, is a
    /// rendering bug that only shows up on screen.
    #[test]
    fn help_sections_are_non_empty_and_uniquely_titled() {
        // Both variants: the PR review's filtering must not leave a bare
        // heading or an orphaned continuation line behind.
        for pr_review in [false, true] {
            let mut titles = std::collections::HashSet::new();
            for (title, binds) in review_help_sections(pr_review) {
                assert!(titles.insert(title), "duplicate help section: {title}");
                assert!(!binds.is_empty(), "empty help section: {title}");
                // A blank key column is a continuation line, so it must follow
                // a real binding rather than lead a section.
                assert!(
                    !binds[0].0.is_empty(),
                    "section {title} starts with a continuation line"
                );
                for (_, desc) in &binds {
                    assert!(!desc.is_empty(), "empty help description in {title}");
                }
            }
        }
    }

    #[test]
    fn pr_review_help_leads_with_submitting_and_omits_refused_keys() {
        let sections = review_help_sections(true);
        assert_eq!(sections[0].0, "Submitting (PR review)");
        let keys: Vec<&str> = sections
            .iter()
            .flat_map(|(_, binds)| binds.iter().map(|(key, _)| *key))
            .collect();
        for refused in ["x", "E", "b", "w", "A", "O", "I", "H", "t", "X"] {
            assert!(!keys.contains(&refused), "help still lists {refused}");
        }
        let descriptions: String = sections
            .iter()
            .flat_map(|(_, binds)| binds.iter().map(|(_, desc)| *desc))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!descriptions.contains("worktree"), "{descriptions}");
        assert!(!descriptions.contains("dispatches fixes"), "{descriptions}");
        // The Final Review overlay is unchanged.
        assert_eq!(
            review_help_sections(false).len(),
            REVIEW_HELP_SECTIONS.len()
        );
    }

    #[test]
    fn review_footer_always_advertises_the_help_key() {
        use ratatui::{Terminal, backend::TestBackend};

        let (mut state, _) = single_added_line_review_state();

        // Both footer shapes — the cursor-off hints and the cursor-mode hints —
        // point at `?`, since it's the only way to see the keys they omit. The
        // narrow width is the case that matters: the dense key row wraps into
        // both footer rows there, so a hint on the wrong line is clipped away.
        for width in [200u16, 90] {
            for cursor in [None, Some(0)] {
                state.comment_cursor = cursor;
                let mut terminal = Terminal::new(TestBackend::new(width, 40)).unwrap();
                terminal
                    .draw(|frame| draw_diff_viewer(frame, &mut state, &Theme::default()))
                    .unwrap();
                let rendered = terminal
                    .backend()
                    .buffer()
                    .content()
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect::<String>();
                assert!(
                    rendered.contains("? keys"),
                    "footer missing the help hint (width: {width}, cursor: {cursor:?})"
                );
            }
        }
    }

    /// The whole screen as one whitespace-normalized string. Rows are joined and
    /// the panel border is blanked out, so a hint the wrapper split across two
    /// rows still reads as the phrase it is — which a raw cell-by-cell
    /// concatenation (trailing padding and border glyphs and all) does not.
    fn normalized_screen(terminal: &ratatui::Terminal<ratatui::backend::TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        let area = *buffer.area();
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| match buffer[(x, y)].symbol() {
                        "│" | "┃" | "║" => " ",
                        symbol => symbol,
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The round-level keys live on the footer's *second* row, and the first row
    /// is dense enough to wrap into two rows on its own at an ordinary terminal
    /// width — which used to silently swallow the second row whole. The footer
    /// now grows to fit and renders each row into its own area, so the width has
    /// to stop mattering.
    #[test]
    fn review_footer_second_row_survives_every_terminal_width() {
        use ratatui::{Terminal, backend::TestBackend};

        let (mut state, loc) = single_added_line_review_state();
        // A realistic mid-review footer: every conditional hint on the first row
        // is showing, which is exactly when it wraps.
        state.general_feedback = "looks close".to_string();
        state.verdict_undo.push(crate::app::VerdictUndo {
            path: "a.rs".to_string(),
            previous: None,
            previous_auto_rejected: false,
        });
        state.changed_since_last.insert("a.rs".to_string());

        for width in [200u16, 160, 120, 100, 80] {
            for cursor in [None, Some(0)] {
                state.comment_cursor = cursor;
                let mut terminal = Terminal::new(TestBackend::new(width, 40)).unwrap();
                terminal
                    .draw(|frame| draw_diff_viewer(frame, &mut state, &Theme::default()))
                    .unwrap();
                let rendered = normalized_screen(&terminal);

                // The first row's pointer to the full key list, and hints that
                // only ever appear on the second row.
                assert!(
                    rendered.contains("? keys"),
                    "footer dropped the help hint (width: {width}, cursor: {cursor:?})"
                );
                if cursor.is_none() {
                    for hint in ["b base ref", "F filter", "target: live", "Esc pause"] {
                        assert!(
                            rendered.contains(hint),
                            "footer dropped {hint:?} (width: {width})"
                        );
                    }
                } else {
                    for hint in ["c/Esc exit cursor", "R resolve/reopen"] {
                        assert!(
                            rendered.contains(hint),
                            "cursor footer dropped {hint:?} (width: {width})"
                        );
                    }
                }
            }
        }

        // The peek box must not squeeze the hints out either: with the cursor on
        // a commented line the footer hosts both.
        state.comment_cursor = Some(0);
        state.line_comments.insert(
            "a.rs".to_string(),
            vec![crate::app::LineComment {
                location: loc,
                start: None,
                text: "needs a guard".to_string(),
                draft: false,
                suggestion: None,
                severity: crate::app::Severity::default(),
                anchor_context: None,
                start_anchor_context: None,
                anchor_lost: false,
                resolved: false,
                carried: false,
            }],
        );
        for width in [200u16, 120, 80] {
            let mut terminal = Terminal::new(TestBackend::new(width, 40)).unwrap();
            terminal
                .draw(|frame| draw_diff_viewer(frame, &mut state, &Theme::default()))
                .unwrap();
            let rendered = normalized_screen(&terminal);
            assert!(
                rendered.contains("needs a guard"),
                "peek box lost its body (width: {width})"
            );
            assert!(
                rendered.contains("c/Esc exit cursor"),
                "peek box crowded out the hint rows (width: {width})"
            );
        }
    }

    /// The wrapper breaks on word boundaries, so the height has to be measured
    /// the same way — a `ceil(total / width)` estimate undercounts and puts the
    /// footer right back to clipping a row.
    #[test]
    fn wrapped_line_height_counts_word_wrapping_not_raw_length() {
        let line = Line::from("aaaa bbbb cccc");
        assert_eq!(wrapped_line_height(&line, 14), 1);
        // "aaaa bbbb" fits in 10; "cccc" moves down whole rather than splitting
        // at the 10th column the way a raw division would assume.
        assert_eq!(wrapped_line_height(&line, 10), 2);
        assert_eq!(wrapped_line_height(&line, 5), 3);
        // A word wider than the row is hard-broken instead of overflowing.
        assert_eq!(wrapped_line_height(&Line::from("aaaaaaaaa"), 3), 3);
        // Degenerate widths must still report a drawable row.
        assert_eq!(wrapped_line_height(&Line::from(""), 20), 1);
        assert_eq!(wrapped_line_height(&line, 0), 1);
    }

    #[test]
    fn cursor_comment_preview_surfaces_only_when_cursor_lands_on_a_comment() {
        let (mut state, loc) = single_added_line_review_state();

        // No cursor → nothing to peek.
        assert!(cursor_comment_text(&state).is_none());
        assert_eq!(cursor_comment_preview_rows(&state), 0);

        // Cursor on an un-commented line → still nothing.
        state.comment_cursor = Some(0);
        assert!(cursor_comment_text(&state).is_none());
        assert_eq!(cursor_comment_preview_rows(&state), 0);

        // A comment on the cursored line surfaces, sized to its body + border.
        state.line_comments.insert(
            "a.rs".to_string(),
            vec![crate::app::LineComment {
                location: loc,
                start: None,
                text: "needs a guard\nfor None".to_string(),
                draft: false,
                suggestion: None,
                severity: crate::app::Severity::default(),
                anchor_context: None,
                start_anchor_context: None,
                anchor_lost: false,
                resolved: false,
                carried: false,
            }],
        );
        assert_eq!(cursor_comment_text(&state), Some("needs a guard\nfor None"));
        // 2 body lines + the severity header + 2 border rows.
        assert_eq!(cursor_comment_preview_rows(&state), 5);
    }

    #[test]
    fn cursor_comment_preview_caps_long_bodies() {
        let (mut state, loc) = single_added_line_review_state();
        state.comment_cursor = Some(0);
        let body = (0..20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        state.line_comments.insert(
            "a.rs".to_string(),
            vec![crate::app::LineComment {
                location: loc,
                start: None,
                text: body,
                draft: false,
                suggestion: None,
                severity: crate::app::Severity::default(),
                anchor_context: None,
                start_anchor_context: None,
                anchor_lost: false,
                resolved: false,
                carried: false,
            }],
        );
        // 20 body lines (+ severity header) clamp to 6 visible + 2 border rows.
        assert_eq!(cursor_comment_preview_rows(&state), 8);
    }

    #[test]
    fn editor_hint_hides_for_a_deleted_file_in_both_footers() {
        use ratatui::{Terminal, backend::TestBackend};

        fn footer_text(state: &mut DiffViewerState) -> String {
            let backend = TestBackend::new(200, 40);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| draw_diff_viewer(frame, state, &Theme::default()))
                .unwrap();
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>()
        }

        let (mut state, _) = single_added_line_review_state();
        // A live file advertises `E` whether or not the line cursor is up.
        assert!(footer_text(&mut state).contains("$EDITOR"));
        state.comment_cursor = Some(0);
        assert!(footer_text(&mut state).contains("$EDITOR"));

        // A deletion keeps its removed lines addressable, so the cursor can
        // still sit on one — but there is nothing on disk for `E` to open.
        state.files[0].status = DiffFileStatus::Deleted;
        state.files[0].hunks[0].lines[0] = crate::diff::DiffLine {
            kind: crate::diff::DiffLineKind::Removed,
            text: "-x".to_string(),
        };
        assert!(!state.files[0].addressable_lines().is_empty());
        assert!(!footer_text(&mut state).contains("$EDITOR"));
        state.comment_cursor = None;
        assert!(!footer_text(&mut state).contains("$EDITOR"));

        // Same for a binary blob, which has nothing an editor can show.
        state.files[0].status = DiffFileStatus::Modified;
        state.files[0].is_binary = true;
        assert!(!footer_text(&mut state).contains("$EDITOR"));
        state.comment_cursor = Some(0);
        assert!(!footer_text(&mut state).contains("$EDITOR"));
    }

    #[test]
    fn file_risk_marker_flags_large_no_note_and_no_tests() {
        let (mut state, _) = single_added_line_review_state();
        let theme = Theme::default();
        // Make the single existing file large and give it no note, so it picks
        // up all three flags (the changeset also has no test-looking file).
        state.files[0].additions = 500;
        let changeset_has_tests = changeset_has_test_changes(&state.files);
        assert!(!changeset_has_tests);
        let marker = file_risk_marker(&state.files[0], &state, changeset_has_tests, &theme)
            .expect("expected a risk marker");
        assert_eq!(marker.content.as_ref(), " [L,N,T]");

        // A small file with a developer note, in a changeset that now includes
        // a test file, gets no marker at all.
        state.files[0].additions = 1;
        state
            .review_notes
            .insert("a.rs".to_string(), "reasoning".to_string());
        state.files.push(DiffFile {
            old_path: None,
            path: "tests/a_test.rs".to_string(),
            status: DiffFileStatus::Added,
            additions: 1,
            deletions: 0,
            is_binary: false,
            old_content: None,
            new_content: None,
            patch: String::new(),
            hunks: vec![],
        });
        let changeset_has_tests = changeset_has_test_changes(&state.files);
        assert!(changeset_has_tests);
        assert!(file_risk_marker(&state.files[0], &state, changeset_has_tests, &theme).is_none());
    }
    #[test]
    fn file_list_renders_a_directory_tree_and_folds_it() {
        use ratatui::{Terminal, backend::TestBackend};

        let (mut state, _) = single_added_line_review_state();
        state.files = ["src/app/mod.rs", "src/app/state.rs", "src/ui/diff.rs"]
            .iter()
            .map(|path| DiffFile {
                old_path: Some((*path).to_string()),
                path: (*path).to_string(),
                status: DiffFileStatus::Modified,
                additions: 2,
                deletions: 1,
                is_binary: false,
                old_content: None,
                new_content: None,
                patch: String::new(),
                hunks: vec![],
            })
            .collect();
        state.selected_file = 0;

        // Only the file-list column: the patch panel beside it still titles
        // itself with the full path, which would mask what the rows show.
        const WIDTH: u16 = 120;
        const FILE_LIST_COLS: usize = 34;
        let render = |state: &mut DiffViewerState| {
            let backend = TestBackend::new(WIDTH, 30);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| draw_diff_viewer(frame, state, &Theme::default()))
                .unwrap();
            let cells: Vec<String> = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol().to_string())
                .collect();
            cells
                .chunks(WIDTH as usize)
                .map(|row| row[..FILE_LIST_COLS].concat())
                .collect::<Vec<String>>()
                .join("\n")
        };

        let rendered = render(&mut state);
        // Directory headers replace the repeated path prefixes, and file rows
        // show only their basename.
        assert!(
            rendered.contains("│ ▾ src/"),
            "top-level directories should only reserve the selection gutter: {rendered}"
        );
        assert!(
            rendered.contains("│>  · M mod.rs"),
            "file marks should follow nesting without extra leading padding: {rendered}"
        );
        assert!(
            rendered.contains("  ▾ app/"),
            "expected a nested, indented app directory row: {rendered}"
        );
        assert!(rendered.contains("mod.rs"));
        assert!(
            !rendered.contains("src/app/mod.rs"),
            "file rows should drop the prefix their directory rows already show"
        );

        // Folding hides the files beneath and summarises what it swallowed.
        state.toggle_dir_collapsed("src/app");
        let folded = render(&mut state);
        assert!(!folded.contains("mod.rs"));
        assert!(folded.contains("▸ app/"));
        assert!(
            folded.contains("(2)"),
            "collapsed row should count its files"
        );
    }

    #[test]
    fn collapsed_dir_summary_only_counts_files_the_filter_shows() {
        let (mut state, _) = single_added_line_review_state();
        state.files = ["src/app/mod.rs", "src/app/state.rs", "src/app/ui.rs"]
            .iter()
            .map(|path| DiffFile {
                old_path: Some((*path).to_string()),
                path: (*path).to_string(),
                status: DiffFileStatus::Modified,
                additions: 2,
                deletions: 1,
                is_binary: false,
                old_content: None,
                new_content: None,
                patch: String::new(),
                hunks: vec![],
            })
            .collect();
        state.selected_file = 0;
        // Only mod.rs stays undecided; the other two are decided and one of them
        // also changed since the last round.
        state
            .decisions
            .insert("src/app/state.rs".to_string(), ReviewDecision::Approve);
        state.decisions.insert(
            "src/app/ui.rs".to_string(),
            ReviewDecision::Reject {
                feedback: "no".to_string(),
                severity: crate::app::Severity::default(),
            },
        );
        state
            .changed_since_last
            .insert("src/app/state.rs".to_string());
        state.file_filter = crate::app::FileFilter::Undecided;
        state.toggle_dir_collapsed("src/app");

        let rows = state.file_tree_rows();
        let files = rows
            .iter()
            .find_map(|row| match row {
                crate::app::FileTreeRow::Dir { path, files, .. } if path == "src/app" => {
                    Some(*files)
                }
                _ => None,
            })
            .expect("the collapsed src/app row");
        let visible = state.visible_file_indices();
        let summary: String =
            dir_row_summary("src/app", files, &visible, &state, &Theme::default())
                .iter()
                .map(|span| span.content.as_ref())
                .collect();

        // The badges describe the one file the row is actually hiding — not the
        // decisions and Δ of the two the filter dropped.
        assert_eq!(summary.trim(), "(1) ·1", "summary was {summary:?}");
    }

    /// The patch panel renders from a cursor-free cache and paints the cursor
    /// on afterwards; the result must match rendering every row with the
    /// cursor built in, including a cursor line that wraps and one scrolled
    /// partly out of view.
    #[test]
    fn cached_patch_panel_matches_a_full_render_with_the_cursor_built_in() {
        use ratatui::{Terminal, backend::TestBackend};
        use std::collections::HashSet;

        let long = "x".repeat(70);
        let file = DiffFile {
            old_path: None,
            path: "notes.txt".into(),
            status: DiffFileStatus::Modified,
            additions: 2,
            deletions: 1,
            is_binary: false,
            old_content: None,
            new_content: None,
            patch: "diff --git a/notes.txt b/notes.txt\n".into(),
            hunks: vec![DiffHunk {
                header: "@@ -1,3 +1,4 @@".into(),
                old_start: 1,
                old_lines: 3,
                new_start: 1,
                new_lines: 4,
                lines: vec![
                    DiffLine {
                        kind: DiffLineKind::Context,
                        text: " alpha".into(),
                    },
                    DiffLine {
                        kind: DiffLineKind::Removed,
                        text: "-beta".into(),
                    },
                    DiffLine {
                        kind: DiffLineKind::Added,
                        text: format!("+{long}"),
                    },
                    DiffLine {
                        kind: DiffLineKind::Added,
                        text: "+gamma".into(),
                    },
                    DiffLine {
                        kind: DiffLineKind::Context,
                        text: " delta".into(),
                    },
                ],
            }],
        };
        let theme = Theme::default();
        let wrapped = DiffLineLocation {
            old_line: None,
            new_line: Some(2),
        };
        let removed = DiffLineLocation {
            old_line: Some(2),
            new_line: None,
        };
        let context = DiffLineLocation {
            old_line: Some(3),
            new_line: Some(4),
        };
        let set = |locs: &[DiffLineLocation]| locs.iter().copied().collect::<HashSet<_>>();

        let cases = [
            (wrapped, set(&[]), set(&[]), set(&[context])),
            (wrapped, set(&[wrapped]), set(&[]), set(&[])),
            (removed, set(&[]), set(&[removed]), set(&[])),
            (context, set(&[removed]), set(&[wrapped]), set(&[context])),
        ];
        for (compact_gutter, (cursor, commented, draft, matched)) in [false, true]
            .into_iter()
            .flat_map(|compact| cases.iter().map(move |case| (compact, case)))
        {
            for scroll in 0..6 {
                let render = |cached: bool| {
                    let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
                    terminal
                        .draw(|frame| {
                            let area = frame.area();
                            if cached {
                                draw_patch_panel(
                                    frame,
                                    area,
                                    Some(&file),
                                    PatchPanelOptions {
                                        title: "Patch".into(),
                                        scroll,
                                        compact_gutter,
                                        cursor: Some(*cursor),
                                        commented: commented.clone(),
                                        draft: draft.clone(),
                                        matched: matched.clone(),
                                        ..Default::default()
                                    },
                                    &theme,
                                );
                            } else {
                                let lines = indexed_patch_lines(
                                    &file,
                                    area.width - 2,
                                    &theme,
                                    true,
                                    false,
                                    compact_gutter,
                                    Some(*cursor),
                                    commented,
                                    draft,
                                    &HashSet::new(),
                                    &HashSet::new(),
                                    matched,
                                    &mut std::collections::HashMap::new(),
                                );
                                let block = Block::default()
                                    .title(" Patch [unified] ")
                                    .borders(Borders::ALL)
                                    .border_style(Style::default().fg(Color::Reset));
                                frame.render_widget(
                                    Paragraph::new(lines)
                                        .block(block)
                                        .scroll((scroll as u16, 0))
                                        .wrap(Wrap { trim: false }),
                                    area,
                                );
                            }
                        })
                        .unwrap();
                    terminal.backend().buffer().clone()
                };
                assert_eq!(
                    render(true),
                    render(false),
                    "cursor {cursor:?} at scroll {scroll}, compact gutter {compact_gutter}"
                );
            }
        }
    }

    /// The compact gutter keeps one number column: the new line number, or
    /// the old one for a removed line, and hands the freed columns to code.
    #[test]
    fn compact_gutter_shows_one_number_column_and_widens_the_code() {
        use std::collections::{HashMap, HashSet};

        let file = crate::diff::parse_unified_diff(
            "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -10,3 +10,3 @@\n keep\n-old_name\n+new_name\n tail\n",
        )
        .unwrap()
        .remove(0);
        let theme = Theme::default();
        let render = |compact: bool, width: u16| {
            let mut rows = HashMap::new();
            let lines = indexed_patch_lines(
                &file,
                width,
                &theme,
                false,
                false,
                compact,
                None,
                &HashSet::new(),
                &HashSet::new(),
                &HashSet::new(),
                &HashSet::new(),
                &HashSet::new(),
                &mut rows,
            );
            let text: Vec<String> = lines.iter().map(line_text).collect();
            (text, rows)
        };

        let (full, _) = render(false, 40);
        let (compact, rows) = render(true, 40);
        assert!(
            full.iter().any(|row| row.starts_with("10 10 │  keep")),
            "{full:?}"
        );
        let row = |loc: DiffLineLocation| compact[rows[&loc].start].clone();
        assert!(
            row(DiffLineLocation {
                old_line: Some(10),
                new_line: Some(10),
            })
            .starts_with("10 │  keep"),
            "{compact:?}"
        );
        assert!(
            row(DiffLineLocation {
                old_line: Some(11),
                new_line: None,
            })
            .starts_with("11 │ -old_name"),
            "a removed line keeps its old number: {compact:?}"
        );
        assert!(
            row(DiffLineLocation {
                old_line: None,
                new_line: Some(11),
            })
            .starts_with("11 │ +new_name"),
            "{compact:?}"
        );

        // A line that wraps in the two-column gutter fits on one row once
        // the second number column is gone.
        // At 16 columns the code gets 11 compact and 8 full: "+new_name" is 9.
        let (full, _) = render(false, 16);
        let (compact, _) = render(true, 16);
        assert!(
            compact.iter().any(|row| row.contains("+new_name")),
            "{compact:?}"
        );
        assert!(full.len() > compact.len(), "{full:?} vs {compact:?}");
    }
}
