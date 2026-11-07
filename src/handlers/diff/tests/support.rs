use super::*;

pub(super) fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

pub(super) fn ctrl(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::CONTROL)
}

/// Finish a fully-decided review: the first `q` opens the pre-finish
/// summary, the second (`q` again, now handled by the summary modal)
/// actually writes/dispatches. Tests that only care about the end state
/// use this instead of a single `q` press.
pub(super) fn finish_review(app: &mut App) {
    handle_diff_viewer_key(app, key(KeyCode::Char('q'))).unwrap();
    assert!(
        matches!(&app.mode, AppMode::DiffViewer(s) if s.summary_open),
        "expected the pre-finish summary to open"
    );
    handle_diff_viewer_key(app, key(KeyCode::Char('q'))).unwrap();
}

pub(super) fn make_review_app(workdir: &Path, paths: &[&str]) -> App {
    let mut app = crate::app::App::new_for_test(
        ProjectStore {
            version: 5,
            projects: vec![],
            session_bookmarks: vec![],
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        },
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let mut state = DiffViewerState::new(
        ViewState::new(
            "proj".into(),
            "feat".into(),
            "sess".into(),
            "claude".into(),
            "Claude".into(),
            crate::project::SessionKind::Claude,
            VibeMode::Vibeless,
            true,
        ),
        workdir.to_path_buf(),
    );
    state.review = true;
    state.files = paths
        .iter()
        .map(|p| DiffFile {
            old_path: Some((*p).to_string()),
            path: (*p).to_string(),
            status: DiffFileStatus::Modified,
            additions: 1,
            deletions: 1,
            is_binary: false,
            old_content: None,
            new_content: None,
            patch: String::new(),
            hunks: vec![],
        })
        .collect();
    app.mode = AppMode::DiffViewer(state);
    app
}

/// A review app whose single file carries real hunks parsed from `patch`, so
/// `addressable_lines()` is non-empty (needed to exercise the line cursor and
/// diff search).
pub(super) fn make_review_app_with_patch(workdir: &Path, patch: &str) -> App {
    let mut app = make_review_app(workdir, &["a.rs"]);
    let files = crate::diff::parse_unified_diff(patch).unwrap();
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files = files;
    }
    app
}

/// Matches `SEARCH_PATCH` in the review.rs tests: addressable lines are
/// 0 alpha, 1 beta, 2 beta_two, 3 gamma_alpha, 4 delta.
pub(super) const SEARCH_PATCH: &str = "\
diff --git a/a.rs b/a.rs
index 1111111..2222222 100644
--- a/a.rs
+++ b/a.rs
@@ -1,3 +1,4 @@
 fn alpha() {}
-fn beta() {}
+fn beta_two() {}
+fn gamma_alpha() {}
 fn delta() {}
";

/// A single-file review app with `lines` added lines, focused on the patch.
pub(super) fn make_long_patch_app(workdir: &Path, lines: usize) -> App {
    let mut patch =
        format!("diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -0,0 +1,{lines} @@\n");
    for i in 0..lines {
        patch.push_str(&format!("+line {i}\n"));
    }
    let mut app = make_review_app_with_patch(workdir, &patch);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.focus = DiffViewerFocus::Patch;
    }
    app
}

pub(super) fn patch_scroll_and_cursor(app: &App) -> (usize, Option<usize>) {
    match &app.mode {
        AppMode::DiffViewer(state) => (state.patch_scroll, state.comment_cursor),
        _ => unreachable!(),
    }
}

pub(super) fn help_open(app: &App) -> bool {
    matches!(&app.mode, AppMode::DiffViewer(state) if state.help_open)
}

/// Give the review app's single file a real hunk (one context + one added
/// line) so it has addressable lines to put a cursor on.
pub(super) fn set_single_hunk(app: &mut App) {
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files[0].hunks = vec![DiffHunk {
            header: "@@ -1,1 +1,2 @@".into(),
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 2,
            lines: vec![
                DiffLine {
                    kind: DiffLineKind::Context,
                    text: " ctx".into(),
                },
                DiffLine {
                    kind: DiffLineKind::Added,
                    text: "+added line".into(),
                },
            ],
        }];
    }
}

pub(super) fn run_git(repo: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Two files, each with the same shape: addressable lines are
/// 0 context, 1 removed, 2 added, 3 context.
pub(super) const TWO_FILE_PATCH: &str = "\
diff --git a/a.rs b/a.rs
index 1111111..2222222 100644
--- a/a.rs
+++ b/a.rs
@@ -1,3 +1,3 @@
 fn alpha() {}
-fn beta() {}
+fn beta_two() {}
 fn delta() {}
diff --git a/b.rs b/b.rs
index 3333333..4444444 100644
--- a/b.rs
+++ b/b.rs
@@ -1,3 +1,3 @@
 fn one() {}
-fn two() {}
+fn two_prime() {}
 fn three() {}
";
