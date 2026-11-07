use super::*;

#[test]
fn a_pr_update_flags_only_changed_files_and_marks_comments_on_removed_code_outdated() {
    let repo = repo_with_two_file_pr();
    let dir = repo.path();
    let db_file = NamedTempFile::new().unwrap();
    let mut app = review_tab_with_db(dir, &db_file);
    open_selected_pr(&mut app);

    // Review v1: approve both files, comment on code in each.
    select_file(&mut app, "edit.rs");
    comment_on_line_containing(&mut app, "old_a", "rename this");
    select_file(&mut app, "keep.rs");
    comment_on_line_containing(&mut app, "kept_line", "nice");
    for path in ["edit.rs", "keep.rs"] {
        select_file(&mut app, path);
        press(&mut app, KeyCode::Char('a'));
    }
    assert_eq!(viewer(&app).decisions.len(), 2);
    press(&mut app, KeyCode::Esc);

    // The author replaces the lines the edit.rs comment was on.
    push_to_pr(dir, "edit.rs", "fn edit() {}\nfn new_c() {}\n");
    open_selected_pr(&mut app);
    let state = viewer(&app);

    // Only the changed file is flagged, filtered to, and loses its verdict.
    assert_eq!(
        state.changed_since_last,
        std::collections::HashSet::from(["edit.rs".to_string()])
    );
    assert!(state.has_prior_review);
    assert_eq!(state.file_filter, FileFilter::Changed);
    assert_eq!(state.files[state.selected_file].path, "edit.rs");
    assert!(
        !state.decisions.contains_key("edit.rs"),
        "changed code is re-reviewed"
    );
    assert_eq!(
        state.decisions.get("keep.rs"),
        Some(&ReviewDecision::Approve)
    );

    // Both comments are kept: the one on removed code is outdated, the one on
    // untouched code still sits on its line.
    assert!(comment(&app, "edit.rs", "rename this").anchor_lost);
    let kept = comment(&app, "keep.rs", "nice");
    assert!(!kept.anchor_lost);
    let keep_file = state.files.iter().find(|f| f.path == "keep.rs").unwrap();
    let kept_index = keep_file
        .addressable_lines()
        .iter()
        .position(|loc| *loc == kept.location)
        .unwrap();
    assert!(keep_file.addressable_line_texts()[kept_index].contains("kept_line"));

    let message = app.message.clone().unwrap_or_default();
    assert!(message.contains("1 of 2 file(s) changed"), "{message}");
    assert!(
        message.contains("1 verdict(s) on them cleared"),
        "{message}"
    );
    assert!(message.contains("1 comment(s) are outdated"), "{message}");

    // The outdated comment survives the next save too.
    press(&mut app, KeyCode::Esc);
    let saved = saved_draft(&app).unwrap();
    assert!(saved.progress.contains("rename this"));
    assert_eq!(
        saved.head_oid,
        git_out(dir, &["rev-parse", "teammate-pr"]),
        "the draft now describes the revision just reviewed"
    );
}

#[test]
fn a_new_push_that_changes_no_file_keeps_every_verdict() {
    let repo = repo_with_two_file_pr();
    let dir = repo.path();
    let db_file = NamedTempFile::new().unwrap();
    let mut app = review_tab_with_db(dir, &db_file);
    open_selected_pr(&mut app);
    for path in ["edit.rs", "keep.rs"] {
        select_file(&mut app, path);
        press(&mut app, KeyCode::Char('a'));
    }
    press(&mut app, KeyCode::Esc);

    // A new, empty commit: the head moves, but no file's diff changes.
    let empty = git_out(
        dir,
        &[
            "commit-tree",
            "-m",
            "empty push",
            "-p",
            "teammate-pr",
            "teammate-pr^{tree}",
        ],
    );
    git_out(dir, &["update-ref", "refs/heads/teammate-pr", &empty]);

    open_selected_pr(&mut app);
    let state = viewer(&app);
    assert!(state.changed_since_last.is_empty());
    assert_eq!(state.decisions.len(), 2);
    assert!(
        app.message
            .as_deref()
            .is_some_and(|m| m.contains("none of its files' changes differ")),
        "{:?}",
        app.message
    );
}

#[test]
fn the_pr_notes_panel_shows_changes_and_outdated_comments_and_the_footer_only_usable_keys() {
    use ratatui::{Terminal, backend::TestBackend};

    let repo = repo_with_two_file_pr();
    let dir = repo.path();
    let db_file = NamedTempFile::new().unwrap();
    let mut app = review_tab_with_db(dir, &db_file);
    open_selected_pr(&mut app);
    select_file(&mut app, "edit.rs");
    comment_on_line_containing(&mut app, "old_a", "rename this");
    press(&mut app, KeyCode::Esc);
    push_to_pr(dir, "edit.rs", "fn edit() {}\nfn new_c() {}\n");
    open_selected_pr(&mut app);
    select_file(&mut app, "edit.rs");
    press(&mut app, KeyCode::Char('e')); // expand the notes panel

    let mut terminal = Terminal::new(TestBackend::new(160, 50)).unwrap();
    terminal
        .draw(|frame| crate::ui::draw(frame, &mut app))
        .unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(screen.contains("PR Review Notes"), "{screen}");
    assert!(screen.contains("Changed since your draft"), "{screen}");
    assert!(screen.contains("Outdated comments"), "{screen}");
    assert!(screen.contains("rename this"), "{screen}");
    assert!(
        !screen.contains("review-notes.md"),
        "no feature-notes placeholder"
    );

    // The footer offers only what a PR review can do.
    press(&mut app, KeyCode::Char('e'));
    terminal
        .draw(|frame| crate::ui::draw(frame, &mut app))
        .unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    for refused in [
        "$EDITOR",
        "gen walkthrough",
        "AI review",
        " overview",
        " history",
        "since last review",
        "base ref",
        "target:",
        "→ finish",
    ] {
        assert!(!screen.contains(refused), "footer still offers {refused:?}");
    }
    assert!(screen.contains("pause (draft saved)"), "{screen}");
}

// ── Submitting a PR review ──────────────────────────────────────────────────
