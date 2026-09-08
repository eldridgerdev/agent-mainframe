use crate::app::{
    App, AppMode, BrowseScope, LearningAnchor, LearningAnchorDrift, LearningAnchorLoss,
    LearningFocus, LearningLevel, LearningListEntry, LearningListGroup, LearningQa,
    LearningQaIntent, LearningViewState, Selection,
};
use crate::project::AgentKind;
use anyhow::Result;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::follow_up::{
    MAX_TODO_TITLE, anchor_locator, escalation_seed, learning_session_label, todo_body,
    todo_title_seed,
};
use super::navigation::{
    AnchorTarget, MAX_DIR_CHILDREN, MAX_FILE_BYTES, all_dir_paths, anchor_for_cursor,
    build_changed_entries, build_repo_tree_entries, cap_repo_entries, check_anchor_drift,
    default_expanded_dirs, expected_block, flatten_tree, hunk_index_for_line, load_file_lines,
    selection_text, start_here_candidates, walk_files_capped,
};
use super::workers::{
    LearningAnswer, LearningPromptContext, MAX_FOLLOW_UP_DEPTH, MAX_SELECTION_LINES, ParentTurn,
    STARTER_QUESTIONS, StarterScope, build_prompt, headless_failure_message, starter_questions_for,
    thread_insert_index, thread_rows,
};
use crate::app::{ProjectStatus, ProjectStore};
use crate::project::{Feature, Project, VibeMode};
use crate::traits::{MockTmuxOps, MockWorktreeOps};
use std::process::Command;
use tempfile::TempDir;

/// A store with one project/feature rooted at `workdir`.
fn store_at(workdir: &Path, is_git: bool) -> ProjectStore {
    let now = chrono::Utc::now();
    let feature = Feature {
        id: "feat-1".to_string(),
        name: "my-feat".to_string(),
        branch: "my-feat".to_string(),
        workdir: workdir.to_path_buf(),
        is_worktree: false,
        tmux_session: "amf-my-feat".to_string(),
        sessions: vec![],
        collapsed: false,
        mode: VibeMode::default(),
        review: false,
        plan_mode: false,
        agent: AgentKind::default(),
        enable_chrome: false,
        remote_control: false,
        pending_worktree_script: false,
        ready: false,
        status: ProjectStatus::Stopped,
        created_at: now,
        last_accessed: now,
        summary: None,
        summary_updated_at: None,
        nickname: None,
        selected_plan_path: None,
        triage_source: None,
        review_source: None,
    };
    ProjectStore {
        version: 2,
        projects: vec![Project {
            id: "proj-1".to_string(),
            name: "my-project".to_string(),
            repo: workdir.to_path_buf(),
            collapsed: false,
            features: vec![feature],
            created_at: now,
            preferred_agent: AgentKind::default(),
            is_git,
        }],
        session_bookmarks: vec![],
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
        extra: std::collections::HashMap::new(),
    }
}

fn app_at(workdir: &Path, is_git: bool) -> App {
    App::new_for_test(
        store_at(workdir, is_git),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    )
}

fn git(repo: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A repo on `main` with a README, a source file, and a branch carrying one
/// changed file.
fn repo_with_branch_change() -> TempDir {
    let repo = TempDir::new().unwrap();
    git(repo.path(), &["init", "--initial-branch=main"]);
    git(repo.path(), &["config", "user.name", "AMF Test"]);
    git(repo.path(), &["config", "user.email", "amf@example.com"]);
    std::fs::write(repo.path().join("README.md"), "# my-project\n").unwrap();
    std::fs::create_dir_all(repo.path().join("src")).unwrap();
    std::fs::write(repo.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(repo.path().join("src/util.rs"), "pub fn ok() {}\n").unwrap();
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-m", "initial"]);
    git(repo.path(), &["checkout", "-b", "my-feat"]);
    std::fs::write(
        repo.path().join("src/main.rs"),
        "fn main() {\n    ok();\n}\n",
    )
    .unwrap();
    git(repo.path(), &["commit", "-am", "call ok"]);
    repo
}

fn learning(app: &App) -> &LearningViewState {
    match &app.mode {
        AppMode::Learning(state) => state,
        _ => panic!("expected Learning mode"),
    }
}

fn state_with_content(lines: &[&str]) -> LearningViewState {
    let mut state = LearningViewState::new(
        "proj-1".to_string(),
        0,
        0,
        "amf".to_string(),
        "learning-mode".to_string(),
        PathBuf::from("/tmp/does-not-matter"),
        true,
        AgentKind::Claude,
        LearningLevel::Newcomer,
        "sess-1".to_string(),
    );
    state.content = lines.iter().map(|l| (*l).to_string()).collect();
    state.content_path = Some("src/app/learning.rs".to_string());
    state
}

#[test]
fn start_here_lists_only_files_that_exist() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("README.md"), "hi").unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();

    let found = start_here_candidates(dir.path());
    assert_eq!(found, vec!["README.md", "src/main.rs", "Cargo.toml"]);
}

#[test]
fn start_here_is_empty_for_a_project_following_no_conventions() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("thing.xyz"), "?").unwrap();
    assert!(start_here_candidates(dir.path()).is_empty());
}

/// A directory named like a candidate isn't a reading suggestion.
#[test]
fn start_here_ignores_directories() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("README.md")).unwrap();
    assert!(start_here_candidates(dir.path()).is_empty());
}

/// Shorthand for the tests below, which care about rows rather than the
/// root-overflow count.
fn tree_entries(
    files: &[String],
    start_here: &[String],
    collapsed: bool,
    expanded: &BTreeSet<String>,
) -> Vec<LearningListEntry> {
    build_repo_tree_entries(files, start_here, collapsed, expanded).0
}

fn expanded_set(dirs: &[&str]) -> BTreeSet<String> {
    dirs.iter().map(|d| (*d).to_string()).collect()
}

#[test]
fn repo_tree_entries_pin_the_orientation_group_on_top() {
    let entries = tree_entries(
        &["src/app/learning.rs".to_string(), "README.md".to_string()],
        &["README.md".to_string()],
        false,
        &expanded_set(&["src", "src/app"]),
    );
    assert!(matches!(entries[0], LearningListEntry::StartHereHeader));
    assert!(matches!(entries[1], LearningListEntry::ProjectTour));
    assert_eq!(entries[2].path(), Some("README.md"));
    // The pinned copy doesn't remove the file from the tree below: `src`,
    // `src/app`, the file inside it, and `README.md` in its real place.
    assert_eq!(entries.len(), 7);
    assert_eq!(entries[3].dir_path(), Some("src"));
    assert_eq!(entries[4].dir_path(), Some("src/app"));
    assert_eq!(entries[5].path(), Some("src/app/learning.rs"));
    assert_eq!(entries[6].path(), Some("README.md"));
}

#[test]
fn collapsing_the_group_keeps_only_its_header() {
    let entries = tree_entries(
        &["src/main.rs".to_string()],
        &["README.md".to_string()],
        true,
        &expanded_set(&["src"]),
    );
    assert!(matches!(entries[0], LearningListEntry::StartHereHeader));
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[1].dir_path(), Some("src"));
    assert_eq!(entries[2].path(), Some("src/main.rs"));
}

#[test]
fn no_orientation_group_when_no_candidate_exists() {
    let entries = tree_entries(&["a.rs".to_string()], &[], false, &BTreeSet::new());
    assert_eq!(entries.len(), 1);
    assert!(entries[0].is_file());
}

// ── Epic 7: the tree ─────────────────────────────────────

/// Directories come before files at each level, each in name order, and a
/// closed directory contributes exactly one row.
#[test]
fn flattening_puts_directories_before_files_at_each_level() {
    let files = vec![
        "zebra.md".to_string(),
        "src/main.rs".to_string(),
        "Cargo.toml".to_string(),
        "docs/guide.md".to_string(),
    ];
    let (rows, _) = flatten_tree(&files, &BTreeSet::new());
    let labels: Vec<&str> = rows
        .iter()
        .map(|r| r.dir_path().or_else(|| r.path()).unwrap())
        .collect();
    assert_eq!(labels, vec!["docs", "src", "Cargo.toml", "zebra.md"]);
    // Nothing is expanded, so neither directory shows its contents.
    assert!(rows.iter().all(|r| r.depth() == 0));
}

/// Expanding a directory reveals its children one level deeper, and does
/// not open its subdirectories with it.
#[test]
fn expanding_a_directory_reveals_one_level() {
    let files = vec![
        "src/main.rs".to_string(),
        "src/app/learning.rs".to_string(),
        "src/app/todos.rs".to_string(),
    ];
    let (rows, _) = flatten_tree(&files, &expanded_set(&["src"]));
    assert_eq!(rows[0].dir_path(), Some("src"));
    assert_eq!(rows[0].depth(), 0);
    assert_eq!(rows[1].dir_path(), Some("src/app"));
    assert_eq!(rows[1].depth(), 1);
    // `src/app` is closed, so its two files are not rows.
    assert_eq!(rows[2].path(), Some("src/main.rs"));
    assert_eq!(rows[2].depth(), 1);
    assert_eq!(rows.len(), 3);

    let (deeper, _) = flatten_tree(&files, &expanded_set(&["src", "src/app"]));
    assert_eq!(deeper[2].path(), Some("src/app/learning.rs"));
    assert_eq!(deeper[2].depth(), 2);
    assert_eq!(deeper.len(), 5);
}

/// Collapsing and re-expanding returns exactly the rows you started with —
/// the tree is derived from `expanded_dirs`, so this is what says the
/// derivation has no memory of its own.
#[test]
fn collapse_and_expand_round_trips() {
    let files = vec![
        "src/app/learning.rs".to_string(),
        "src/main.rs".to_string(),
        "README.md".to_string(),
    ];
    let open = expanded_set(&["src", "src/app"]);
    let (before, _) = flatten_tree(&files, &open);
    let (collapsed, _) = flatten_tree(&files, &BTreeSet::new());
    assert_eq!(collapsed.len(), 2);
    let (after, _) = flatten_tree(&files, &open);
    assert_eq!(before, after);
}

/// A closed folder says how many files it holds, counting everything below
/// it rather than only its immediate children — the number is there to
/// answer "is it worth opening this".
#[test]
fn a_closed_directory_counts_everything_beneath_it() {
    let files = vec![
        "src/a.rs".to_string(),
        "src/app/b.rs".to_string(),
        "src/app/deep/c.rs".to_string(),
    ];
    let (rows, _) = flatten_tree(&files, &BTreeSet::new());
    assert!(matches!(
        &rows[0],
        LearningListEntry::Dir {
            path,
            file_count: 3,
            expanded: false,
            ..
        } if path == "src"
    ));
}

/// The opening state: the path down to each `Start here` file is open, and
/// nothing else is — so `src/main.rs` is on screen without the whole repo
/// being.
#[test]
fn the_tree_opens_at_the_start_here_files() {
    let start_here = vec![
        "README.md".to_string(),
        "src/main.rs".to_string(),
        "Cargo.toml".to_string(),
    ];
    let expanded = default_expanded_dirs(&start_here);
    // `README.md` and `Cargo.toml` are at the root and open nothing.
    assert_eq!(expanded, expanded_set(&["src"]));

    let files = vec![
        "src/main.rs".to_string(),
        "src/app/learning.rs".to_string(),
        "docs/guide.md".to_string(),
    ];
    let (rows, _) = flatten_tree(&files, &expanded);
    assert!(rows.iter().any(|r| r.path() == Some("src/main.rs")));
    // `docs` is visible as structure but not opened.
    assert!(rows.iter().any(|r| r.dir_path() == Some("docs")));
    assert!(rows.iter().all(|r| r.path() != Some("docs/guide.md")));
}

/// A nested candidate opens every directory above it, not just the last.
#[test]
fn the_opening_state_opens_the_whole_path_down() {
    let expanded = default_expanded_dirs(&["a/b/c/main.rs".to_string()]);
    assert_eq!(expanded, expanded_set(&["a", "a/b", "a/b/c"]));
}

/// One directory over the cap is truncated and *says so* on its own row,
/// rather than the listing looking complete. This is the honesty duty the
/// old whole-listing cap carried, moved to where it now applies.
#[test]
fn a_directory_over_the_cap_says_what_it_is_not_showing() {
    let files: Vec<String> = (0..MAX_DIR_CHILDREN + 25)
        .map(|i| format!("generated/file{i:06}.rs"))
        .collect();
    let (rows, root_overflow) = flatten_tree(&files, &expanded_set(&["generated"]));
    assert_eq!(root_overflow, 0, "only one directory sits at the root");
    assert!(matches!(
        &rows[0],
        LearningListEntry::Dir { truncated: 25, .. }
    ));
    // The row for the directory itself, plus its capped children.
    assert_eq!(rows.len(), MAX_DIR_CHILDREN + 1);
}

/// The root has no row to be truthful on, so its overflow comes back to
/// the caller for the banner.
#[test]
fn root_level_overflow_is_reported_to_the_caller() {
    let files: Vec<String> = (0..MAX_DIR_CHILDREN + 7)
        .map(|i| format!("file{i:06}.rs"))
        .collect();
    let (rows, root_overflow) = flatten_tree(&files, &BTreeSet::new());
    assert_eq!(root_overflow, 7);
    assert_eq!(rows.len(), MAX_DIR_CHILDREN);
}

/// "Expand all" needs every directory in the repo, including ones with no
/// row on screen yet — that is the whole difference between it and pressing
/// `l` repeatedly.
#[test]
fn every_directory_is_reachable_by_expand_all() {
    let files = vec![
        "src/app/deep/x.rs".to_string(),
        "docs/guide.md".to_string(),
        "README.md".to_string(),
    ];
    assert_eq!(
        all_dir_paths(&files),
        expanded_set(&["docs", "src", "src/app", "src/app/deep"])
    );
    let (rows, _) = flatten_tree(&files, &all_dir_paths(&files));
    assert!(rows.iter().any(|r| r.path() == Some("src/app/deep/x.rs")));
}

/// Branch-changes scope keeps the flat list the plan left it with: a
/// handful of changed paths needs no structure, and the paths are the point.
#[test]
fn branch_changes_stay_flat() {
    let changed = |path: &str| crate::diff::DiffFile {
        old_path: None,
        path: path.to_string(),
        status: crate::diff::DiffFileStatus::Modified,
        additions: 1,
        deletions: 0,
        is_binary: false,
        old_content: None,
        new_content: None,
        patch: String::new(),
        hunks: Vec::new(),
    };
    let rows = build_changed_entries(&[changed("src/app/learning.rs"), changed("README.md")]);
    assert!(rows.iter().all(|r| r.depth() == 0));
    assert!(rows.iter().all(|r| r.dir_path().is_none()));
    assert_eq!(rows[0].path(), Some("src/app/learning.rs"));
}

#[test]
fn line_anchor_is_one_based_and_clamped() {
    let mut state = state_with_content(&["a", "b", "c"]);
    state.cursor_line = 0;
    assert_eq!(
        anchor_for_cursor(&state),
        LearningAnchor::Lines { start: 1, end: 1 }
    );

    state.cursor_line = 2;
    state.selection_anchor = Some(0);
    assert_eq!(
        anchor_for_cursor(&state),
        LearningAnchor::Lines { start: 1, end: 3 }
    );

    // A cursor past the end (file reloaded shorter) clamps rather than
    // producing an anchor that points off the end of the file.
    state.cursor_line = 99;
    state.selection_anchor = None;
    assert_eq!(
        anchor_for_cursor(&state),
        LearningAnchor::Lines { start: 3, end: 3 }
    );
}

#[test]
fn empty_file_anchors_to_the_file() {
    let state = state_with_content(&[]);
    assert_eq!(anchor_for_cursor(&state), LearningAnchor::File);
}

#[test]
fn selection_text_covers_the_anchored_lines_only() {
    let mut state = state_with_content(&["one", "two", "three"]);
    state.cursor_line = 1;
    state.selection_anchor = Some(2);
    state.anchor = anchor_for_cursor(&state);
    assert_eq!(selection_text(&state), "two\nthree");

    state.anchor = LearningAnchor::File;
    assert_eq!(selection_text(&state), "one\ntwo\nthree");

    state.anchor = LearningAnchor::Project;
    assert_eq!(selection_text(&state), "");
}

/// Repo-tree browsing has no diff, so there is no hunk to select.
#[test]
fn hunk_selection_is_unavailable_in_repo_tree_scope() {
    let mut state = state_with_content(&["a"]);
    state.scope = BrowseScope::RepoTree;
    assert!(!state.hunk_selection_available());

    // Nor is it available in branch-changes scope with nothing selected.
    state.scope = BrowseScope::BranchChanges;
    assert!(!state.hunk_selection_available());
}

#[test]
fn hunk_lookup_finds_the_enclosing_hunk() {
    let starts = vec![0usize, 5, 12];
    assert_eq!(hunk_index_for_line(&starts, 0), Some(0));
    assert_eq!(hunk_index_for_line(&starts, 4), Some(0));
    assert_eq!(hunk_index_for_line(&starts, 5), Some(1));
    assert_eq!(hunk_index_for_line(&starts, 11), Some(1));
    assert_eq!(hunk_index_for_line(&starts, 40), Some(2));
    assert_eq!(hunk_index_for_line(&[], 3), None);
}

#[test]
fn binary_and_oversized_files_are_skipped_with_a_reason() {
    let dir = TempDir::new().unwrap();
    let binary = dir.path().join("thing.bin");
    std::fs::write(&binary, [0x7f, 0x45, 0x00, 0x01]).unwrap();
    let err = load_file_lines(&binary, "thing.bin").unwrap_err();
    assert!(err.contains("binary"), "{err}");

    let big = dir.path().join("huge.txt");
    std::fs::write(&big, vec![b'a'; (MAX_FILE_BYTES + 1) as usize]).unwrap();
    let err = load_file_lines(&big, "huge.txt").unwrap_err();
    assert!(err.contains("too big"), "{err}");

    let missing = dir.path().join("nope.txt");
    assert!(load_file_lines(&missing, "nope.txt").is_err());
}

/// A file that won't open is the one failure met by simply moving the
/// cursor, so it has to reach both the pane (with a next step) and the
/// debug log (with the path, which the pane's message alone doesn't give
/// someone reading the log later).
#[test]
fn a_file_that_vanished_says_what_to_do_and_reaches_the_debug_log() {
    let repo = repo_with_branch_change();
    let mut app = app_at(repo.path(), true);
    app.open_learning_mode(0, 0).unwrap();

    // Select README.md, then delete it out from under the listing.
    let idx = learning(&app)
        .entries
        .iter()
        .position(|e| e.path() == Some("README.md"))
        .expect("README.md should be listed");
    if let AppMode::Learning(state) = &mut app.mode {
        state.selected_entry = idx;
    }
    std::fs::remove_file(repo.path().join("README.md")).unwrap();
    app.learning_load_selected_content();

    let reason = learning(&app)
        .content_error
        .clone()
        .expect("a missing file should say so");
    assert!(
        reason.contains("moved or deleted") && reason.contains("pick another file"),
        "should say what to do next, got {reason}"
    );
    // The repo-relative path, not the absolute one: a workdir prefix is
    // long enough to push the advice off the end of the line, which is
    // exactly how this read the first time it was captured.
    assert!(
        reason.contains("Couldn't open README.md:"),
        "should name the file the way the list does, got {reason}"
    );
    assert!(
        !reason.contains(&repo.path().display().to_string()),
        "the workdir prefix is noise the pane title already carries, got {reason}"
    );

    let logged = app
        .debug_log
        .entries()
        .iter()
        .any(|e| e.context == "learning" && e.message.contains("README.md"));
    assert!(logged, "the load failure should name the file in the log");
}

/// Opening the cursored file is a focus change, not a second read: the
/// cursor move already loaded it. Caught by seeing the same failure logged
/// twice for one `Enter`.
#[test]
fn opening_the_file_already_under_the_cursor_does_not_read_it_again() {
    let repo = repo_with_branch_change();
    let mut app = app_at(repo.path(), true);
    app.open_learning_mode(0, 0).unwrap();

    let idx = learning(&app)
        .entries
        .iter()
        .position(|e| e.path() == Some("src/util.rs"))
        .expect("src/util.rs should be listed");
    if let AppMode::Learning(state) = &mut app.mode {
        state.selected_entry = idx;
    }
    app.learning_load_selected_content();
    assert_eq!(learning(&app).content_path.as_deref(), Some("src/util.rs"));

    // Change the file on disk, then press Enter. A reload would pick the
    // new text up; a focus change leaves what was already read alone.
    std::fs::write(repo.path().join("src/util.rs"), "pub fn changed() {}\n").unwrap();
    app.learning_activate_selection();

    assert_eq!(learning(&app).focus, LearningFocus::Content);
    assert_eq!(learning(&app).content, vec!["pub fn ok() {}"]);
}

/// ...but a file that failed still reloads, because `Enter` is the only
/// retry there is.
#[test]
fn opening_a_file_that_failed_to_load_tries_it_again() {
    let repo = repo_with_branch_change();
    let mut app = app_at(repo.path(), true);
    app.open_learning_mode(0, 0).unwrap();

    let idx = learning(&app)
        .entries
        .iter()
        .position(|e| e.path() == Some("src/util.rs"))
        .expect("src/util.rs should be listed");
    if let AppMode::Learning(state) = &mut app.mode {
        state.selected_entry = idx;
    }
    // Fail the load, then make the file readable again.
    std::fs::remove_file(repo.path().join("src/util.rs")).unwrap();
    app.learning_load_selected_content();
    assert!(learning(&app).content_error.is_some());

    std::fs::write(repo.path().join("src/util.rs"), "pub fn back() {}\n").unwrap();
    app.learning_activate_selection();

    assert!(learning(&app).content_error.is_none());
    assert_eq!(learning(&app).content, vec!["pub fn back() {}"]);
}

#[test]
fn text_files_load_as_lines() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("a.txt");
    std::fs::write(&path, "one\ntwo\n").unwrap();
    assert_eq!(load_file_lines(&path, "a.txt").unwrap(), vec!["one", "two"]);
}

// ── prompt builders ──────────────────────────────────────

fn sample_context() -> LearningPromptContext {
    LearningPromptContext {
        project_name: "my-project".to_string(),
        feature_name: "learning-mode".to_string(),
        file_path: Some("src/app/learning.rs".to_string()),
        anchor: LearningAnchor::Lines { start: 3, end: 4 },
        selection_text: "let x = 1;\nlet y = 2;".to_string(),
        selection_is_diff: false,
        file_lines: (1..=10).map(|i| format!("line {i}")).collect(),
        selection_start_line: Some(3),
        question: "What does this do?".to_string(),
        intent: LearningQaIntent::Explain,
        level: LearningLevel::Newcomer,
        run_mode: crate::app::LearningRunMode::NoTools,
        ancestors: Vec::new(),
    }
}

#[test]
fn every_prompt_carries_identity_path_numbered_selection_and_context() {
    let prompt = build_prompt(&sample_context());

    assert!(prompt.contains("Project: my-project"), "{prompt}");
    assert!(
        prompt.contains("Branch / feature: learning-mode"),
        "{prompt}"
    );
    assert!(prompt.contains("File: src/app/learning.rs"), "{prompt}");
    assert!(
        prompt.contains("lines 3-4 of src/app/learning.rs"),
        "{prompt}"
    );
    // The selection is numbered from its real line, not from 1.
    assert!(prompt.contains("     3 | let x = 1;"), "{prompt}");
    assert!(prompt.contains("     4 | let y = 2;"), "{prompt}");
    assert!(prompt.contains("Surrounding context"), "{prompt}");
    assert!(prompt.contains("What does this do?"), "{prompt}");
}

#[test]
fn the_explain_template_never_asks_for_a_change() {
    let prompt = build_prompt(&sample_context());
    assert!(prompt.contains("Do not propose changes"), "{prompt}");
    assert!(
        !prompt.contains("smallest concrete change"),
        "explain must not carry the action instruction: {prompt}"
    );
    assert!(!prompt.contains("imperative summary"), "{prompt}");
}

#[test]
fn the_action_template_asks_for_a_usable_one_line_title() {
    let mut ctx = sample_context();
    ctx.intent = LearningQaIntent::Action;
    let prompt = build_prompt(&ctx);
    assert!(prompt.contains("smallest concrete change"), "{prompt}");
    assert!(prompt.contains("imperative summary"), "{prompt}");
    assert!(prompt.contains("title of a work item"), "{prompt}");
    assert!(!prompt.contains("Do not propose changes"), "{prompt}");
}

/// A deep dive is labelled "read the repo" on the row and in the answer
/// pane. Read-only tools only make that possible, so the prompt has to be
/// what makes it true.
#[test]
fn the_deep_dive_template_requires_the_repository_to_be_read() {
    let mut ctx = sample_context();
    ctx.run_mode = crate::app::LearningRunMode::DeepDive;
    let prompt = build_prompt(&ctx);

    assert!(
        prompt.contains("read-only access to this repository"),
        "{prompt}"
    );
    assert!(
        prompt.contains("Ground every claim in what you actually read"),
        "permission is not enough — the reading has to be required: {prompt}"
    );
    assert!(
        prompt.contains("name the files and symbols you checked"),
        "and be checkable from the answer itself: {prompt}"
    );
    assert!(
        !prompt.contains("no access to the rest of the repository"),
        "the no-tools disclaimer must not survive into a run that has access: {prompt}"
    );
}

#[test]
fn the_no_tools_template_says_it_cannot_see_the_rest_of_the_repository() {
    let prompt = build_prompt(&sample_context());
    assert!(
        prompt.contains("no access to the rest of the repository"),
        "{prompt}"
    );
    assert!(prompt.contains("Do not invent file paths"), "{prompt}");
    assert!(
        !prompt.contains("read-only access to this repository"),
        "{prompt}"
    );
}

#[test]
fn the_newcomer_overlay_is_present_by_default_and_absent_when_familiar() {
    let newcomer = build_prompt(&sample_context());
    assert!(
        newcomer.contains("Define every technical term"),
        "{newcomer}"
    );
    assert!(newcomer.contains("Where to look next"), "{newcomer}");
    assert!(newcomer.contains("No question is too basic"), "{newcomer}");

    let mut ctx = sample_context();
    ctx.level = LearningLevel::Familiar;
    let familiar = build_prompt(&ctx);
    assert!(
        !familiar.contains("Define every technical term"),
        "{familiar}"
    );
    assert!(
        !familiar.contains("Finish with a section headed"),
        "{familiar}"
    );
    assert!(
        familiar.contains("Be dense and skip the basics"),
        "{familiar}"
    );
}

#[test]
fn a_follow_up_carries_its_parent_exactly_once() {
    let mut ctx = sample_context();
    ctx.question = "What's a trait?".to_string();
    ctx.ancestors = vec![ParentTurn {
        question: "What does this do?".to_string(),
        answer: "It implements a trait.".to_string(),
    }];
    let prompt = build_prompt(&ctx);

    assert!(prompt.contains("Earlier in this conversation"), "{prompt}");
    assert_eq!(
        prompt.matches("It implements a trait.").count(),
        1,
        "parent answer should appear once: {prompt}"
    );
    assert_eq!(
        prompt.matches("They asked: What does this do?").count(),
        1,
        "{prompt}"
    );
    assert!(prompt.contains("What's a trait?"), "{prompt}");
}

#[test]
fn follow_up_context_is_capped_at_the_configured_depth() {
    let mut ctx = sample_context();
    ctx.ancestors = (1..=6)
        .map(|i| ParentTurn {
            question: format!("question {i}"),
            answer: format!("answer {i}"),
        })
        .collect();
    let prompt = build_prompt(&ctx);

    // The oldest ancestors are trimmed; the most recent survive.
    assert!(!prompt.contains("answer 1"), "{prompt}");
    assert!(!prompt.contains("answer 3"), "{prompt}");
    assert!(prompt.contains("answer 4"), "{prompt}");
    assert!(prompt.contains("answer 6"), "{prompt}");
    assert_eq!(prompt.matches("They asked:").count(), MAX_FOLLOW_UP_DEPTH);
}

#[test]
fn the_project_anchor_prompt_has_no_file_or_selection() {
    let mut ctx = sample_context();
    ctx.anchor = LearningAnchor::Project;
    ctx.file_path = None;
    ctx.selection_text = String::new();
    ctx.question = "Give me a tour of this project.".to_string();
    let prompt = build_prompt(&ctx);

    assert!(prompt.contains("this whole project"), "{prompt}");
    assert!(!prompt.contains("File: "), "{prompt}");
    assert!(!prompt.contains("Surrounding context"), "{prompt}");
    assert!(prompt.contains("about the project as a whole"), "{prompt}");
}

/// A whole-file anchor already carries the file, so repeating it as
/// "surrounding context" would double the prompt for no gain.
#[test]
fn a_whole_file_anchor_does_not_repeat_the_file_as_context() {
    let mut ctx = sample_context();
    ctx.anchor = LearningAnchor::File;
    assert!(!build_prompt(&ctx).contains("Surrounding context"));
}

#[test]
fn oversized_selections_are_truncated_with_a_marker() {
    let mut ctx = sample_context();
    let long: Vec<String> = (1..=(MAX_SELECTION_LINES + 50))
        .map(|i| format!("line {i}"))
        .collect();
    ctx.selection_text = long.join("\n");
    let prompt = build_prompt(&ctx);
    assert!(prompt.contains("50 more lines not shown"), "{prompt}");
}

#[test]
fn prompt_context_comes_from_the_live_anchor() {
    let repo = repo_with_branch_change();
    let mut app = app_at(repo.path(), true);
    app.open_learning_mode(0, 0).unwrap();
    while learning(&app).content_path.as_deref() != Some("src/main.rs") {
        app.learning_select_next_entry();
    }
    app.learning_cursor_move(0);

    let ctx = app
        .learning_prompt_context("What is this?", LearningQaIntent::Explain, Vec::new())
        .unwrap();
    assert_eq!(ctx.file_path.as_deref(), Some("src/main.rs"));
    assert_eq!(ctx.project_name, "my-project");
    assert_eq!(ctx.level, LearningLevel::Newcomer);
    assert!(!ctx.file_lines.is_empty());
}

// ── asking, level, harness ───────────────────────────────

/// Deliver an answer the way a finished thread would, without running a
/// real harness.
fn deliver(app: &mut App, qa_id: &str, result: Result<String, String>) {
    app.learning_answer_tx
        .send(LearningAnswer {
            qa_id: qa_id.to_string(),
            result,
        })
        .unwrap();
    assert!(app.poll_learning_answers_bg());
}

// ── starter questions ────────────────────────────────────

fn starters_for(anchor: LearningAnchor) -> Vec<&'static str> {
    starter_questions_for(anchor)
        .into_iter()
        .map(|i| STARTER_QUESTIONS[i].text)
        .collect()
}

#[test]
fn project_presets_are_offered_only_for_the_project_anchor() {
    let project_only: Vec<&str> = STARTER_QUESTIONS
        .iter()
        .filter(|q| q.scope == StarterScope::Project)
        .map(|q| q.text)
        .collect();
    assert!(!project_only.is_empty(), "the table has project presets");

    let offered = starters_for(LearningAnchor::Project);
    for text in &project_only {
        assert!(
            offered.contains(text),
            "{text} missing at the project anchor"
        );
    }
    for anchor in [
        LearningAnchor::File,
        LearningAnchor::Lines { start: 1, end: 3 },
    ] {
        let offered = starters_for(anchor);
        for text in &project_only {
            assert!(
                !offered.contains(text),
                "{text} should not be offered at {anchor:?}"
            );
        }
    }
}

#[test]
fn line_presets_need_a_line_or_hunk_range() {
    let line_only: Vec<&str> = STARTER_QUESTIONS
        .iter()
        .filter(|q| q.scope == StarterScope::Lines)
        .map(|q| q.text)
        .collect();
    assert!(!line_only.is_empty(), "the table has line presets");

    for anchor in [
        LearningAnchor::Lines { start: 4, end: 9 },
        LearningAnchor::Hunk { index: 0 },
    ] {
        let offered = starters_for(anchor);
        for text in &line_only {
            assert!(offered.contains(text), "{text} missing at {anchor:?}");
        }
    }
    for anchor in [LearningAnchor::Project, LearningAnchor::File] {
        let offered = starters_for(anchor);
        for text in &line_only {
            assert!(
                !offered.contains(text),
                "{text} should not be offered at {anchor:?}"
            );
        }
    }
}

/// A file-level question still applies once a range inside that file is
/// selected — the file is open either way.
#[test]
fn file_presets_apply_to_ranges_inside_that_file() {
    let file_only: Vec<&str> = STARTER_QUESTIONS
        .iter()
        .filter(|q| q.scope == StarterScope::File)
        .map(|q| q.text)
        .collect();
    for anchor in [
        LearningAnchor::File,
        LearningAnchor::Lines { start: 2, end: 2 },
        LearningAnchor::Hunk { index: 1 },
    ] {
        let offered = starters_for(anchor);
        for text in &file_only {
            assert!(offered.contains(text), "{text} missing at {anchor:?}");
        }
    }
    assert!(
        starters_for(LearningAnchor::Project)
            .iter()
            .all(|t| !file_only.contains(t)),
        "file presets need a file"
    );
}

/// Shared with `crate::handlers::learning`'s tests: a dashboard-mode app on
/// a real temp repo, for exercising the `K` entry key.
pub(crate) fn dashboard_app_for_handlers() -> (TempDir, App) {
    let repo = repo_with_branch_change();
    let app = app_at(repo.path(), true);
    (repo, app)
}

/// As above, but the project has no features — nothing for Learning Mode
/// to read.
pub(crate) fn featureless_app_for_handlers() -> (TempDir, App) {
    let repo = repo_with_branch_change();
    let mut app = app_at(repo.path(), true);
    app.store.projects[0].features.clear();
    (repo, app)
}

/// Shared with `crate::handlers::learning`'s tests: an overlay opened on a
/// real temp repo with a file loaded.
pub(crate) fn opened_app_for_handlers() -> (TempDir, App) {
    opened_app()
}

/// As above with a real database, for the handler tests that write
/// something — a TODO item has nowhere to live without one.
pub(crate) fn opened_app_with_db_for_handlers() -> (TempDir, TempDir, App) {
    opened_app_with_db()
}

fn opened_app() -> (TempDir, App) {
    let repo = repo_with_branch_change();
    let mut app = app_at(repo.path(), true);
    app.open_learning_mode(0, 0).unwrap();
    while learning(&app).content_path.as_deref() != Some("src/main.rs") {
        app.learning_select_next_entry();
    }
    (repo, app)
}

#[test]
fn asking_enqueues_a_row_and_returns_control_immediately() {
    let (_repo, mut app) = opened_app();

    let id = app
        .learning_ask("What does this do?", LearningQaIntent::Explain, None)
        .unwrap();

    let state = learning(&app);
    assert_eq!(state.qa.len(), 1);
    let row = &state.qa[0];
    assert_eq!(row.id, id);
    assert_eq!(row.question, "What does this do?");
    assert_eq!(row.intent, LearningQaIntent::Explain);
    assert_eq!(row.level, LearningLevel::Newcomer);
    assert_eq!(row.file_path.as_deref(), Some("src/main.rs"));
    assert_eq!(row.run_mode, crate::app::LearningRunMode::NoTools);
    // Queued or already running — either way the user is not blocked.
    assert!(row.status.is_in_flight());
    assert_eq!(state.in_flight_count(), 1);
    // The overlay is still fully interactive.
    assert!(!state.entries.is_empty());
}

#[test]
fn an_empty_question_is_not_enqueued() {
    let (_repo, mut app) = opened_app();
    assert!(
        app.learning_ask("   ", LearningQaIntent::Explain, None)
            .is_none()
    );
    assert!(learning(&app).qa.is_empty());
}

#[test]
fn answers_land_on_their_own_row_and_clear_the_in_flight_count() {
    let (_repo, mut app) = opened_app();
    let first = app
        .learning_ask("Question one", LearningQaIntent::Explain, None)
        .unwrap();
    let second = app
        .learning_ask("Question two", LearningQaIntent::Action, None)
        .unwrap();
    assert_eq!(learning(&app).in_flight_count(), 2);

    // Out of order, as real runs finish.
    deliver(&mut app, &second, Ok("Second answer".to_string()));
    let state = learning(&app);
    assert_eq!(state.in_flight_count(), 1);
    assert_eq!(
        state.qa.iter().find(|r| r.id == second).unwrap().answer,
        Some("Second answer".to_string())
    );
    assert!(
        state
            .qa
            .iter()
            .find(|r| r.id == first)
            .unwrap()
            .answer
            .is_none()
    );

    deliver(&mut app, &first, Ok("First answer".to_string()));
    assert_eq!(learning(&app).in_flight_count(), 0);
}

/// The pre-call notice is a *blocking* modal for user-initiated calls, but
/// Learning Mode answers run through the non-blocking toast path so a batch
/// of queued questions can't deadlock behind an un-dismissed modal.
#[test]
fn a_queued_learning_batch_drains_with_only_a_toast_no_modal() {
    let (_repo, mut app) = opened_app();

    let a = app
        .learning_ask("Question A", LearningQaIntent::Explain, None)
        .unwrap();
    let b = app
        .learning_ask("Question B", LearningQaIntent::Explain, None)
        .unwrap();

    // No blocking modal was raised — the overlay is still the active mode.
    assert!(
        matches!(app.mode, AppMode::Learning(_)),
        "asking must not open the pre-call modal"
    );
    assert!(
        app.toasts
            .iter()
            .any(|t| t.message.contains("Headless AI call")),
        "the run is announced with a toast instead"
    );
    assert_eq!(learning(&app).in_flight_count(), 2);

    // The queue drains normally with the notice path in place.
    deliver(&mut app, &b, Ok("Answer B".to_string()));
    deliver(&mut app, &a, Ok("Answer A".to_string()));
    assert_eq!(learning(&app).in_flight_count(), 0);
    let state = learning(&app);
    assert_eq!(
        state
            .qa
            .iter()
            .find(|r| r.id == a)
            .unwrap()
            .answer
            .as_deref(),
        Some("Answer A")
    );
    assert_eq!(
        state
            .qa
            .iter()
            .find(|r| r.id == b)
            .unwrap()
            .answer
            .as_deref(),
        Some("Answer B")
    );
}

#[test]
fn a_failed_run_keeps_the_row_and_says_what_to_do() {
    let (_repo, mut app) = opened_app();
    let id = app
        .learning_ask("What does this do?", LearningQaIntent::Explain, None)
        .unwrap();

    deliver(
        &mut app,
        &id,
        Err("Claude isn't installed or isn't on your PATH".to_string()),
    );

    let row = &learning(&app).qa[0];
    assert_eq!(row.status, crate::app::LearningQaStatus::Failed);
    assert!(row.error.as_deref().unwrap().contains("isn't installed"));
    assert!(row.answer.is_none(), "the question survives for a retry");
}

#[test]
fn a_missing_cli_failure_points_at_the_harness_wizard() {
    let msg =
        headless_failure_message(&AgentKind::Claude, &anyhow::anyhow!("claude CLI not found"));
    assert!(msg.contains("Press A"), "{msg}");
    assert!(msg.contains("Claude"), "{msg}");
}

#[test]
fn a_follow_up_inherits_the_anchor_and_carries_the_parent_forward() {
    let (_repo, mut app) = opened_app();
    let parent = app
        .learning_ask("What does this do?", LearningQaIntent::Explain, None)
        .unwrap();
    deliver(&mut app, &parent, Ok("It calls ok().".to_string()));

    let child = app
        .learning_ask(
            "What's a function?",
            LearningQaIntent::Explain,
            Some(parent.clone()),
        )
        .unwrap();

    let state = learning(&app);
    let row = state.qa.iter().find(|r| r.id == child).unwrap();
    assert_eq!(row.parent_qa_id.as_deref(), Some(parent.as_str()));
    assert_eq!(row.file_path.as_deref(), Some("src/main.rs"));

    // The prompt the follow-up would have been built with carries the
    // parent turn.
    let ancestors = app.learning_ancestor_turns(Some(&parent));
    assert_eq!(ancestors.len(), 1);
    assert_eq!(ancestors[0].answer, "It calls ok().");
}

/// An unanswered parent has nothing to contribute, so it isn't carried.
#[test]
fn unanswered_ancestors_are_skipped() {
    let (_repo, mut app) = opened_app();
    let parent = app
        .learning_ask("Pending question", LearningQaIntent::Explain, None)
        .unwrap();
    assert!(app.learning_ancestor_turns(Some(&parent)).is_empty());
}

#[test]
fn toggling_level_affects_later_questions_only() {
    let (_repo, mut app) = opened_app();
    let first = app
        .learning_ask("Question one", LearningQaIntent::Explain, None)
        .unwrap();
    deliver(&mut app, &first, Ok("An answer".to_string()));

    app.learning_toggle_level();
    assert_eq!(learning(&app).level, LearningLevel::Familiar);

    let second = app
        .learning_ask("Question two", LearningQaIntent::Explain, None)
        .unwrap();
    let state = learning(&app);
    // The earlier row keeps the level it was answered at, and its text is
    // untouched.
    let old = state.qa.iter().find(|r| r.id == first).unwrap();
    assert_eq!(old.level, LearningLevel::Newcomer);
    assert_eq!(old.answer.as_deref(), Some("An answer"));
    assert_eq!(
        state.qa.iter().find(|r| r.id == second).unwrap().level,
        LearningLevel::Familiar
    );

    app.learning_toggle_level();
    assert_eq!(learning(&app).level, LearningLevel::Newcomer);
}

#[test]
fn the_harness_picker_is_optional_and_pre_selected() {
    let repo = repo_with_branch_change();
    let mut store = store_at(repo.path(), true);
    store.available_harnesses = vec![AgentKind::Codex, AgentKind::Claude];
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.open_learning_mode(0, 0).unwrap();

    // Pre-selected from the first available harness — no picker needed.
    assert_eq!(learning(&app).harness, AgentKind::Codex);
    assert!(learning(&app).harness_picker.is_none());

    app.learning_open_harness_picker();
    let picker = learning(&app).harness_picker.as_ref().unwrap();
    assert_eq!(picker.harnesses.len(), 2);
    assert_eq!(picker.selected, 0, "opens on the harness in use");

    app.learning_harness_picker_move(1);
    app.learning_harness_picker_confirm();
    let state = learning(&app);
    assert_eq!(state.harness, AgentKind::Claude);
    assert!(state.harness_picker.is_none());
}

#[test]
fn cancelling_the_harness_picker_changes_nothing() {
    let (_repo, mut app) = opened_app();
    let before = learning(&app).harness.clone();
    app.learning_open_harness_picker();
    app.learning_harness_picker_move(1);
    app.learning_close_harness_picker();
    assert_eq!(learning(&app).harness, before);
    assert!(learning(&app).harness_picker.is_none());
}

#[test]
fn questions_record_the_harness_that_answers_them() {
    let (_repo, mut app) = opened_app();
    let id = app
        .learning_ask("What does this do?", LearningQaIntent::Explain, None)
        .unwrap();
    let expected = learning(&app).harness.clone();
    assert_eq!(
        learning(&app)
            .qa
            .iter()
            .find(|r| r.id == id)
            .unwrap()
            .harness,
        expected
    );
}

// ── overlay-level behaviour ──────────────────────────────

#[test]
fn opening_lists_repo_files_with_the_orientation_group_on_top() {
    let repo = repo_with_branch_change();
    let mut app = app_at(repo.path(), true);
    app.open_learning_mode(0, 0).unwrap();

    let state = learning(&app);
    assert_eq!(state.scope, BrowseScope::RepoTree);
    assert!(matches!(
        state.entries[0],
        LearningListEntry::StartHereHeader
    ));
    assert!(matches!(state.entries[1], LearningListEntry::ProjectTour));
    // The cursor opens on the tour question, not the group header.
    assert_eq!(state.selected_entry, 1);
    assert_eq!(state.anchor, LearningAnchor::Project);

    let paths: Vec<&str> = state.entries.iter().filter_map(|e| e.path()).collect();
    assert!(paths.contains(&"src/main.rs"), "{paths:?}");
    assert!(paths.contains(&"src/util.rs"), "{paths:?}");
    assert!(paths.contains(&"README.md"), "{paths:?}");
}

/// Whether this row is `path` *in the tree*. The `Start here` group pins
/// its own copies of a few files above the tree, and those are a reading
/// list rather than tree rows — they neither indent nor disappear when
/// their folder is collapsed. Tests about the tree have to say which copy
/// they mean, or collapsing `src/` looks like it kept `src/main.rs`.
fn is_tree_file(entry: &LearningListEntry, path: &str) -> bool {
    matches!(
        entry,
        LearningListEntry::File {
            path: p,
            group: LearningListGroup::Files,
            ..
        } if p == path
    )
}

/// Move the file-list cursor onto whichever row matches, or fail loudly:
/// a silent no-match would make the assertions below pass on nothing.
fn put_cursor_on(app: &mut App, pred: impl Fn(&LearningListEntry) -> bool) {
    let idx = learning(app)
        .entries
        .iter()
        .position(&pred)
        .unwrap_or_else(|| {
            let rows: Vec<String> = learning(app)
                .entries
                .iter()
                .map(|e| format!("{e:?}"))
                .collect();
            panic!("no matching row in {rows:#?}")
        });
    if let AppMode::Learning(state) = &mut app.mode {
        state.selected_entry = idx;
    }
    app.learning_load_selected_content();
}

/// A folder is navigation. Resting on one must not move the loaded file or
/// the anchor, or walking down to a file would silently drop the question
/// that was lined up two folders ago.
#[test]
fn a_folder_is_navigation_not_a_question_anchor() {
    let repo = repo_with_branch_change();
    let mut app = app_at(repo.path(), true);
    app.open_learning_mode(0, 0).unwrap();

    put_cursor_on(&mut app, |e| e.path() == Some("src/util.rs"));
    let before = learning(&app).anchor;
    assert_eq!(learning(&app).content_path.as_deref(), Some("src/util.rs"));

    put_cursor_on(&mut app, |e| e.dir_path() == Some("src"));
    assert_eq!(
        learning(&app).content_path.as_deref(),
        Some("src/util.rs"),
        "the folder row must not unload the file"
    );
    assert_eq!(learning(&app).anchor, before);
}

/// Collapsing leaves the cursor on the folder that was collapsed, the way
/// `collapsing_the_orientation_group_keeps_the_cursor_on_its_file` does for
/// the group — the row list is rebuilt from scratch, so an index alone
/// would land somewhere unrelated.
#[test]
fn collapsing_a_folder_keeps_the_cursor_on_it() {
    let repo = repo_with_branch_change();
    let mut app = app_at(repo.path(), true);
    app.open_learning_mode(0, 0).unwrap();

    put_cursor_on(&mut app, |e| e.dir_path() == Some("src"));
    app.learning_toggle_dir();

    let state = learning(&app);
    assert_eq!(
        state.selected_entry().and_then(|e| e.dir_path()),
        Some("src")
    );
    assert!(
        !state.entries.iter().any(|e| is_tree_file(e, "src/main.rs")),
        "collapsing should have taken the children with it"
    );

    app.learning_toggle_dir();
    assert_eq!(
        learning(&app).selected_entry().and_then(|e| e.dir_path()),
        Some("src"),
        "and re-expanding leaves it where it was"
    );
    assert!(
        learning(&app)
            .entries
            .iter()
            .any(|e| is_tree_file(e, "src/main.rs"))
    );
}

/// Closing a folder from *inside* it: the row under the cursor stops
/// existing, so the cursor moves to the folder that swallowed it rather
/// than to whatever the old index now points at.
#[test]
fn closing_the_folder_you_are_inside_moves_the_cursor_to_it() {
    let repo = repo_with_branch_change();
    let mut app = app_at(repo.path(), true);
    app.open_learning_mode(0, 0).unwrap();

    put_cursor_on(&mut app, |e| e.path() == Some("src/util.rs"));
    // `h` on a file steps out to its folder, and again closes it.
    app.learning_collapse_or_parent();
    assert_eq!(
        learning(&app).selected_entry().and_then(|e| e.dir_path()),
        Some("src")
    );
    app.learning_collapse_or_parent();
    let state = learning(&app);
    assert_eq!(
        state.selected_entry().and_then(|e| e.dir_path()),
        Some("src")
    );
    assert!(!state.expanded_dirs.contains("src"));
}

/// Tree keys belong to the file list. Pressing them while reading the
/// content pane must not move a cursor that isn't on screen — and must say
/// so, in both directions, rather than doing nothing.
#[test]
fn tree_keys_do_nothing_but_explain_themselves_off_the_file_list() {
    let repo = repo_with_branch_change();
    let mut app = app_at(repo.path(), true);
    app.open_learning_mode(0, 0).unwrap();

    put_cursor_on(&mut app, |e| e.path() == Some("src/util.rs"));
    let before = learning(&app).selected_entry;
    if let AppMode::Learning(state) = &mut app.mode {
        state.focus = LearningFocus::Content;
    }

    app.learning_collapse_or_parent();
    assert_eq!(
        learning(&app).selected_entry,
        before,
        "`h` off the file list must not move the hidden cursor"
    );
    let notice = learning(&app).notice.clone().unwrap_or_default();
    assert!(notice.contains("Tab"), "{notice:?}");

    app.learning_expand_or_open();
    assert_eq!(learning(&app).selected_entry, before);
    let notice = learning(&app).notice.clone().unwrap_or_default();
    assert!(notice.contains("Tab"), "{notice:?}");
    assert!(
        learning(&app).expanded_dirs.contains("src"),
        "and nothing about the tree changed either"
    );
}

/// A pinned `Start here` file keeps its row when the folder holding it has
/// none — the root truncated it away. Stepping out then has no target,
/// which is a refusal, and it says why.
#[test]
fn stepping_out_with_no_visible_parent_says_why() {
    let repo = repo_with_branch_change();
    let mut app = app_at(repo.path(), true);
    app.open_learning_mode(0, 0).unwrap();

    // Push `src` past the root's child cap: it sorts last, so it is the row
    // that gets dropped, while pinned `src/main.rs` is unaffected.
    if let AppMode::Learning(state) = &mut app.mode {
        state.repo_files = (0..MAX_DIR_CHILDREN)
            .map(|i| format!("aaa{i:06}/file.rs"))
            .collect();
        state.repo_files.push("src/main.rs".to_string());
    }
    app.learning_rebuild_tree();
    assert!(
        learning(&app)
            .entries
            .iter()
            .all(|e| e.dir_path() != Some("src"))
    );
    put_cursor_on(&mut app, |e| {
        matches!(
            e,
            LearningListEntry::File { path, group: LearningListGroup::StartHere, .. }
                if path == "src/main.rs"
        )
    });

    app.learning_collapse_or_parent();
    let notice = learning(&app).notice.clone().unwrap_or_default();
    assert!(notice.contains("src/"), "{notice:?}");
    assert!(notice.contains('Z'), "{notice:?}");
}

/// The branch-changes list has no folders at all, so stepping out of a
/// nested path there points at the scope key instead of going quiet.
#[test]
fn stepping_out_in_branch_changes_scope_points_at_the_tree() {
    let repo = repo_with_branch_change();
    let mut app = app_at(repo.path(), true);
    app.open_learning_mode(0, 0).unwrap();
    app.learning_toggle_scope();
    assert_eq!(learning(&app).scope, BrowseScope::BranchChanges);

    put_cursor_on(&mut app, |e| e.path() == Some("src/main.rs"));
    app.learning_collapse_or_parent();
    let notice = learning(&app).notice.clone().unwrap_or_default();
    assert!(notice.contains("flat"), "{notice:?}");
}

/// At the top level there is nothing to step out to, so `h` says so rather
/// than doing nothing — the swallowed keypress this mode exists to avoid.
#[test]
fn stepping_out_of_a_top_level_row_says_there_is_nowhere_to_go() {
    let repo = repo_with_branch_change();
    let mut app = app_at(repo.path(), true);
    app.open_learning_mode(0, 0).unwrap();

    put_cursor_on(&mut app, |e| e.dir_path() == Some("src"));
    // `src` is open, so the first press closes it; the second has no parent.
    app.learning_collapse_or_parent();
    app.learning_collapse_or_parent();
    let notice = learning(&app).notice.clone().unwrap_or_default();
    assert!(notice.contains("top level"), "{notice:?}");
}

/// Expand-all opens folders that had no row on screen, which is the whole
/// difference between it and pressing `l` repeatedly — and pressing it
/// again folds the tree back.
#[test]
fn expand_all_opens_every_folder_and_folds_them_again() {
    let repo = repo_with_branch_change();
    std::fs::create_dir_all(repo.path().join("docs/deep")).unwrap();
    std::fs::write(repo.path().join("docs/deep/notes.md"), "hi\n").unwrap();
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-m", "docs"]);

    let mut app = app_at(repo.path(), true);
    app.open_learning_mode(0, 0).unwrap();
    // `docs` was never opened, so its contents have no rows yet.
    assert!(
        learning(&app)
            .entries
            .iter()
            .all(|e| e.path() != Some("docs/deep/notes.md"))
    );

    app.learning_toggle_expand_all();
    assert!(
        learning(&app)
            .entries
            .iter()
            .any(|e| e.path() == Some("docs/deep/notes.md"))
    );

    app.learning_toggle_expand_all();
    let state = learning(&app);
    assert!(state.expanded_dirs.is_empty());
    assert!(!state.entries.iter().any(|e| is_tree_file(e, "src/main.rs")));
}

/// Expanding and collapsing must not re-read the repository: on a large
/// project that would make the tree slower than the flat list it replaced.
/// Proven by deleting a file behind the overlay's back — a listing that
/// re-ran `git ls-files` would notice.
#[test]
fn toggling_a_folder_does_not_re_read_the_repository() {
    let repo = repo_with_branch_change();
    let mut app = app_at(repo.path(), true);
    app.open_learning_mode(0, 0).unwrap();

    std::fs::remove_file(repo.path().join("src/util.rs")).unwrap();
    put_cursor_on(&mut app, |e| e.dir_path() == Some("src"));
    app.learning_toggle_dir();
    app.learning_toggle_dir();
    assert!(
        learning(&app)
            .entries
            .iter()
            .any(|e| e.path() == Some("src/util.rs")),
        "the cached listing should have been reused"
    );
}

#[test]
fn selecting_a_file_loads_its_content_and_a_line_anchor() {
    let repo = repo_with_branch_change();
    let mut app = app_at(repo.path(), true);
    app.open_learning_mode(0, 0).unwrap();

    // Walk to the first real file row.
    while learning(&app).content_path.is_none() {
        app.learning_select_next_entry();
    }
    let state = learning(&app);
    assert!(!state.content.is_empty());
    assert_eq!(state.anchor, LearningAnchor::File);

    app.learning_cursor_move(1);
    assert!(matches!(
        learning(&app).anchor,
        LearningAnchor::Lines { .. }
    ));
}

#[test]
fn toggling_scope_switches_to_the_branch_s_changed_files_and_back() {
    let repo = repo_with_branch_change();
    let mut app = app_at(repo.path(), true);
    app.open_learning_mode(0, 0).unwrap();

    app.learning_toggle_scope();
    let state = learning(&app);
    assert_eq!(state.scope, BrowseScope::BranchChanges);
    assert!(state.error.is_none(), "{:?}", state.error);
    let paths: Vec<&str> = state.entries.iter().filter_map(|e| e.path()).collect();
    assert_eq!(paths, vec!["src/main.rs"], "only the changed file");
    // No orientation group in this scope.
    assert!(
        !state
            .entries
            .iter()
            .any(|e| matches!(e, LearningListEntry::StartHereHeader))
    );

    app.learning_toggle_scope();
    assert_eq!(learning(&app).scope, BrowseScope::RepoTree);
}

#[test]
fn hunk_selection_works_only_once_a_changed_file_is_loaded() {
    let repo = repo_with_branch_change();
    let mut app = app_at(repo.path(), true);
    app.open_learning_mode(0, 0).unwrap();

    // Repo-tree scope: the key explains itself rather than doing nothing.
    app.learning_select_hunk();
    let err = learning(&app).error.clone().unwrap();
    assert!(err.contains("branch changes"), "{err}");

    app.learning_toggle_scope();
    assert!(learning(&app).hunk_selection_available());
    app.learning_select_hunk();
    let state = learning(&app);
    assert!(state.error.is_none(), "{:?}", state.error);
    assert!(matches!(state.anchor, LearningAnchor::Hunk { index: 0 }));
    assert!(!selection_text(state).is_empty());
}

/// Branch-changes scope needs git, so a plain directory says so instead of
/// showing an empty list.
#[test]
fn a_non_git_project_stays_in_repo_tree_scope_and_explains_why() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("main.py"), "print('hi')\n").unwrap();
    let mut app = app_at(dir.path(), false);
    app.open_learning_mode(0, 0).unwrap();

    let paths: Vec<&str> = learning(&app)
        .entries
        .iter()
        .filter_map(|e| e.path())
        .collect();
    assert_eq!(paths, vec!["main.py"], "falls back to a plain walk");

    app.learning_toggle_scope();
    let state = learning(&app);
    assert_eq!(state.scope, BrowseScope::RepoTree);
    let err = state.error.clone().unwrap();
    assert!(err.contains("git repository"), "{err}");
}

/// The scope key can't switch scope in a non-git project, so it rebuilds
/// the list instead — which is what the vanished-file message tells the
/// user to press, and that message is only reachable from this scope.
#[test]
fn the_scope_key_rebuilds_a_non_git_list_it_cannot_switch() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("main.py"), "print('hi')\n").unwrap();
    std::fs::write(dir.path().join("gone.py"), "print('bye')\n").unwrap();
    let mut app = app_at(dir.path(), false);
    app.open_learning_mode(0, 0).unwrap();

    let idx = learning(&app)
        .entries
        .iter()
        .position(|e| e.path() == Some("gone.py"))
        .expect("gone.py should be listed");
    if let AppMode::Learning(state) = &mut app.mode {
        state.selected_entry = idx;
    }
    app.learning_load_selected_content();
    std::fs::remove_file(dir.path().join("gone.py")).unwrap();
    app.learning_load_selected_content();
    let content_error = learning(&app).content_error.clone().unwrap();
    assert!(content_error.contains("press s"), "{content_error}");

    app.learning_toggle_scope();
    let state = learning(&app);
    assert_eq!(state.scope, BrowseScope::RepoTree, "still no other scope");
    let paths: Vec<&str> = state.entries.iter().filter_map(|e| e.path()).collect();
    assert_eq!(paths, vec!["main.py"], "the vanished file is gone from it");
    assert!(
        state.content_error.is_none(),
        "the surviving file loads: {:?}",
        state.content_error
    );
}

/// With no DB (as in tests) the overlay still opens and browses; history is
/// simply empty and nothing is persisted.
#[test]
fn the_overlay_works_without_a_database() {
    let repo = repo_with_branch_change();
    let mut app = app_at(repo.path(), true);
    assert!(app.db.is_none());
    app.open_learning_mode(0, 0).unwrap();
    assert!(learning(&app).qa.is_empty());
    assert!(learning(&app).session_id.is_empty());
    assert!(!learning(&app).entries.is_empty());
}

/// Without a database the overlay is still fully usable — questions are
/// asked and answered against the in-memory list — but that list is all
/// there is, so it goes when the overlay does. Asserted rather than assumed:
/// the alternative is refusing to answer at all, which would be worse.
#[test]
fn without_a_database_questions_still_work_but_do_not_outlive_the_overlay() {
    let (_repo, mut app) = opened_app();
    assert!(app.db.is_none());
    let id = app
        .learning_ask("What does this do?", LearningQaIntent::Explain, None)
        .unwrap();
    deliver(&mut app, &id, Ok("It runs the program.".to_string()));
    assert_eq!(
        learning(&app).qa[0].answer.as_deref(),
        Some("It runs the program.")
    );

    app.close_learning_mode();
    app.open_learning_mode(0, 0).unwrap();
    assert!(
        learning(&app).qa.is_empty(),
        "there was nowhere to keep it, and nothing pretends otherwise"
    );
}

#[test]
fn closing_returns_to_the_feature_it_was_opened_from() {
    let repo = repo_with_branch_change();
    let mut app = app_at(repo.path(), true);
    app.open_learning_mode(0, 0).unwrap();
    app.close_learning_mode();
    assert!(matches!(app.mode, AppMode::Normal));
    assert!(matches!(app.selection, Selection::Feature(0, 0)));
}

#[test]
fn collapsing_the_orientation_group_keeps_the_cursor_on_its_file() {
    let repo = repo_with_branch_change();
    let mut app = app_at(repo.path(), true);
    app.open_learning_mode(0, 0).unwrap();

    while learning(&app).selected_entry().and_then(|e| e.path()) != Some("src/util.rs") {
        app.learning_select_next_entry();
    }
    app.learning_toggle_start_here();

    let state = learning(&app);
    assert!(state.start_here_collapsed);
    assert_eq!(
        state.selected_entry().and_then(|e| e.path()),
        Some("src/util.rs")
    );
}

#[test]
fn fallback_walk_skips_noise_and_respects_caps() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("main.py"), "print()").unwrap();
    std::fs::create_dir_all(dir.path().join("node_modules/pkg")).unwrap();
    std::fs::write(dir.path().join("node_modules/pkg/index.js"), "x").unwrap();
    std::fs::create_dir_all(dir.path().join("lib/deep")).unwrap();
    std::fs::write(dir.path().join("lib/deep/util.py"), "y").unwrap();

    let walk = walk_files_capped(dir.path(), 100, 12);
    assert_eq!(walk.files, vec!["lib/deep/util.py", "main.py"]);
    assert!(walk.unreadable.is_empty());

    // The entry cap truncates rather than growing without bound.
    assert_eq!(walk_files_capped(dir.path(), 1, 12).files.len(), 1);
    // The depth cap keeps the walk shallow.
    assert_eq!(walk_files_capped(dir.path(), 100, 0).files, vec!["main.py"]);
}

/// A folder the walk can't open leaves no gap in the list, so the walk has
/// to report it — otherwise "no such directory" and "AMF couldn't read it"
/// are indistinguishable to someone who doesn't know the project.
#[test]
#[cfg(unix)]
fn the_fallback_walk_reports_folders_it_could_not_read() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("main.py"), "print()").unwrap();
    let locked = dir.path().join("locked");
    std::fs::create_dir_all(&locked).unwrap();
    std::fs::write(locked.join("secret.py"), "z").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

    let walk = walk_files_capped(dir.path(), 100, 12);
    // Running as root defeats the permission bits entirely; the point of
    // the test is the reporting path, so only assert it when it applies.
    if walk.files.iter().any(|f| f.contains("secret")) {
        return;
    }
    assert_eq!(walk.files, vec!["main.py"]);
    assert_eq!(walk.unreadable.len(), 1);
    assert!(
        walk.unreadable[0].starts_with("locked:"),
        "should name the folder it couldn't open, got {:?}",
        walk.unreadable
    );

    // Leave it readable so the temp dir can clean itself up.
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
}

// ── follow-ups ───────────────────────────────────────────

/// Ask, answer, and follow up — the loop a newcomer's second question
/// depends on.
fn ask_and_answer(app: &mut App, question: &str, answer: &str) -> String {
    let id = app
        .learning_ask(question, LearningQaIntent::Explain, None)
        .unwrap();
    deliver(app, &id, Ok(answer.to_string()));
    id
}

fn follow_up(app: &mut App, question: &str) -> String {
    app.learning_open_follow_up();
    assert!(
        learning(app).question.is_some(),
        "the follow-up prompt should be open"
    );
    for c in question.chars() {
        if let AppMode::Learning(state) = &mut app.mode
            && let Some(q) = &mut state.question
        {
            q.editor.insert_str(&c.to_string());
        }
    }
    app.learning_submit_question().unwrap()
}

#[test]
fn a_follow_up_carries_its_parents_question_and_answer_into_the_prompt() {
    let (_repo, mut app) = opened_app();
    let parent = ask_and_answer(&mut app, "What is this file for?", "It is the entry point.");

    let child = follow_up(&mut app, "What's an entry point?");

    let state = learning(&app);
    let row = state.qa.iter().find(|r| r.id == child).unwrap();
    assert_eq!(row.parent_qa_id.as_deref(), Some(parent.as_str()));
    assert_eq!(
        row.intent,
        LearningQaIntent::Explain,
        "a follow-up inherits its parent's intent"
    );

    // The prompt the agent would receive carries the earlier turn verbatim.
    let ancestors = app.learning_ancestor_turns(Some(&parent));
    let ctx = app
        .learning_prompt_context(
            "What's an entry point?",
            LearningQaIntent::Explain,
            ancestors,
        )
        .unwrap();
    let prompt = build_prompt(&ctx);
    assert!(prompt.contains("What is this file for?"), "{prompt}");
    assert!(prompt.contains("It is the entry point."), "{prompt}");
    assert_eq!(
        prompt.matches("It is the entry point.").count(),
        1,
        "the parent answer appears exactly once"
    );
}

#[test]
fn a_two_deep_follow_up_keeps_the_whole_conversation() {
    let (_repo, mut app) = opened_app();
    ask_and_answer(&mut app, "What is this file for?", "It is the entry point.");

    let second = follow_up(&mut app, "What's an entry point?");
    deliver(&mut app, &second, Ok("Where execution starts.".to_string()));
    let third = follow_up(&mut app, "What is execution?");

    let ancestors = app.learning_ancestor_turns(Some(&second));
    assert_eq!(ancestors.len(), 2, "both earlier turns come through");
    // Oldest first, so the agent reads the conversation in order.
    assert_eq!(ancestors[0].question, "What is this file for?");
    assert_eq!(ancestors[1].question, "What's an entry point?");

    let state = learning(&app);
    let row = state.qa.iter().find(|r| r.id == third).unwrap();
    assert_eq!(row.parent_qa_id.as_deref(), Some(second.as_str()));
}

#[test]
fn a_follow_up_asks_about_its_parents_code_not_wherever_browsing_ended_up() {
    let (_repo, mut app) = opened_app();
    app.learning_cursor_move(1);
    let parent = ask_and_answer(&mut app, "Explain this line.", "It prints.");
    let (parent_anchor, parent_path, parent_text) = {
        let row = learning(&app).qa.iter().find(|r| r.id == parent).unwrap();
        (
            row.anchor,
            row.file_path.clone(),
            row.selection_text.clone(),
        )
    };

    // Browse somewhere else entirely before following up.
    app.learning_select_next_entry();
    app.learning_select_project();
    assert_ne!(learning(&app).anchor, parent_anchor);

    let child = follow_up(&mut app, "What does printing mean here?");

    let state = learning(&app);
    let row = state.qa.iter().find(|r| r.id == child).unwrap();
    assert_eq!(row.anchor, parent_anchor, "same place as its parent");
    assert_eq!(row.file_path, parent_path);
    assert_eq!(row.selection_text, parent_text);
}

#[test]
fn a_follow_up_lands_under_the_thread_it_continues() {
    let (_repo, mut app) = opened_app();
    let first = ask_and_answer(&mut app, "First question?", "First answer.");
    // A second, unrelated question that would otherwise sit between the
    // parent and its follow-up.
    let unrelated = app
        .learning_ask("Unrelated question?", LearningQaIntent::Explain, None)
        .unwrap();

    // Follow up on the *first* row, not the newest.
    if let AppMode::Learning(state) = &mut app.mode {
        state.selected_qa = 0;
    }
    let child = follow_up(&mut app, "Follow-up?");

    let ids: Vec<&str> = learning(&app).qa.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![first.as_str(), child.as_str(), unrelated.as_str()],
        "the follow-up sits directly under its parent"
    );
    assert_eq!(
        learning(&app).selected_qa,
        1,
        "and the cursor follows the new question"
    );
}

// ── deep dive ────────────────────────────────────────────

#[test]
fn a_deep_dive_re_asks_the_same_question_with_the_repo_readable() {
    let (_repo, mut app) = opened_app();
    let origin = ask_and_answer(&mut app, "What does this do?", "It runs the thing.");

    let deeper = app.learning_deep_dive().unwrap();

    let state = learning(&app);
    assert_eq!(state.qa.len(), 2, "the first answer survives its rerun");
    let first = &state.qa[0];
    assert_eq!(first.id, origin);
    assert_eq!(
        first.answer.as_deref(),
        Some("It runs the thing."),
        "the shallow answer is left alone so the two can be compared"
    );

    let row = &state.qa[1];
    assert_eq!(row.id, deeper);
    assert_eq!(row.run_mode, crate::app::LearningRunMode::DeepDive);
    assert_eq!(row.question, first.question, "the same question, re-asked");
    assert_eq!(row.intent, first.intent);
    assert_eq!(row.anchor, first.anchor);
    assert_eq!(row.selection_text, first.selection_text);
    assert_eq!(
        row.parent_qa_id.as_deref(),
        Some(origin.as_str()),
        "it renders indented under the answer it is checking"
    );
    assert_eq!(state.selected_qa, 1, "and the cursor follows it");
}

/// The point of a deep dive is an independent re-derivation. Handing the
/// agent the answer it is checking would just get that answer back.
#[test]
fn a_deep_dive_does_not_feed_the_shallow_answer_back_to_the_agent() {
    let (_repo, mut app) = opened_app();
    ask_and_answer(
        &mut app,
        "What does this do?",
        "It calls into the widget pump.",
    );

    let origin = learning(&app).qa[0].clone();
    let prompt = build_prompt(&app.learning_deep_dive_context(&origin).unwrap());

    assert!(prompt.contains("What does this do?"), "{prompt}");
    assert!(
        !prompt.contains("widget pump"),
        "the answer under review must not be in the prompt reviewing it: {prompt}"
    );
    assert!(
        prompt.contains("Ground every claim in what you actually read"),
        "and the rerun is told to go and check, not merely allowed to: {prompt}"
    );
}

/// A deep dive of a follow-up still needs the turns that led to it — what
/// it drops is only the one answer it is re-deriving.
#[test]
fn a_deep_dive_of_a_follow_up_keeps_the_conversation_above_it() {
    let (_repo, mut app) = opened_app();
    ask_and_answer(&mut app, "What is this file for?", "It is the entry point.");
    let child = follow_up(&mut app, "What's an entry point?");
    deliver(&mut app, &child, Ok("Where execution begins.".to_string()));

    let origin = learning(&app)
        .qa
        .iter()
        .find(|r| r.id == child)
        .unwrap()
        .clone();
    let prompt = build_prompt(&app.learning_deep_dive_context(&origin).unwrap());

    assert!(
        prompt.contains("It is the entry point."),
        "the parent turn survives: {prompt}"
    );
    assert!(
        !prompt.contains("Where execution begins."),
        "but not the answer being re-derived: {prompt}"
    );
}

#[test]
fn a_deep_dive_keeps_the_level_its_original_was_answered_at() {
    let (_repo, mut app) = opened_app();
    let origin = ask_and_answer(&mut app, "What does this do?", "It runs the thing.");
    assert_eq!(learning(&app).level, LearningLevel::Newcomer);

    // The user moves on to denser answers, then sends the old one deeper.
    app.learning_toggle_level();
    assert_eq!(learning(&app).level, LearningLevel::Familiar);
    let deeper = app.learning_deep_dive().unwrap();

    let state = learning(&app);
    let row = state.qa.iter().find(|r| r.id == deeper).unwrap();
    assert_eq!(
        row.level,
        LearningLevel::Newcomer,
        "a rerun reads like the answer it reruns, not like the current setting"
    );
    assert_eq!(
        state.qa.iter().find(|r| r.id == origin).unwrap().level,
        row.level
    );
}

#[test]
fn a_deep_dive_asks_about_its_originals_code_not_wherever_browsing_ended_up() {
    let (_repo, mut app) = opened_app();
    app.learning_select_whole_file();
    ask_and_answer(&mut app, "What does this do?", "It runs the thing.");
    let asked_about = learning(&app).qa[0].file_path.clone();
    assert!(asked_about.is_some());

    // Browse away before sending it deeper.
    app.learning_select_next_entry();
    app.learning_load_selected_content();
    app.learning_deep_dive().unwrap();

    let state = learning(&app);
    assert_eq!(
        state.qa[1].file_path, asked_about,
        "the rerun follows the question, not the cursor"
    );
    assert_eq!(state.qa[1].selection_text, state.qa[0].selection_text);
}

#[test]
fn a_deep_dive_of_an_unanswered_question_says_to_wait() {
    let (_repo, mut app) = opened_app();
    app.learning_ask("What does this do?", LearningQaIntent::Explain, None)
        .unwrap();

    assert!(app.learning_deep_dive().is_none());

    let state = learning(&app);
    assert_eq!(state.qa.len(), 1, "nothing was started");
    assert!(
        state
            .error
            .as_deref()
            .is_some_and(|e| e.contains("still generating")),
        "got {:?}",
        state.error
    );
}

/// Which is also the Codex case: `effective_for` records those rows as deep
/// dives up front, because `codex exec` has no no-tools mode.
#[test]
fn a_deep_dive_of_a_deep_dive_says_it_already_read_the_repo() {
    let (_repo, mut app) = opened_app();
    let origin = ask_and_answer(&mut app, "What does this do?", "It runs the thing.");
    let deeper = app.learning_deep_dive().unwrap();
    deliver(
        &mut app,
        &deeper,
        Ok("It really runs the thing.".to_string()),
    );

    assert!(app.learning_deep_dive().is_none());

    let state = learning(&app);
    assert_eq!(state.qa.len(), 2, "no third row");
    assert!(
        state
            .error
            .as_deref()
            .is_some_and(|e| e.contains("already read the repository")),
        "got {:?}",
        state.error
    );
    assert_eq!(state.qa[0].id, origin);
}

#[test]
fn a_second_deep_dive_jumps_to_the_one_you_already_have() {
    let (_repo, mut app) = opened_app();
    ask_and_answer(&mut app, "What does this do?", "It runs the thing.");
    let deeper = app.learning_deep_dive().unwrap();
    deliver(
        &mut app,
        &deeper,
        Ok("It really runs the thing.".to_string()),
    );

    // Back to the original, and ask for a deep dive again.
    if let AppMode::Learning(state) = &mut app.mode {
        state.selected_qa = 0;
    }
    assert!(app.learning_deep_dive().is_none());

    let state = learning(&app);
    assert_eq!(state.qa.len(), 2, "the same run isn't paid for twice");
    assert_eq!(
        state.qa[state.selected_qa].id, deeper,
        "the cursor lands on the answer that already exists"
    );
}

/// A deep dive that failed is worth retrying — that is exactly when the
/// user wants it — so a failed row must not be mistaken for one that
/// already answered.
#[test]
fn a_failed_deep_dive_can_be_retried() {
    let (_repo, mut app) = opened_app();
    ask_and_answer(&mut app, "What does this do?", "It runs the thing.");
    let first = app.learning_deep_dive().unwrap();
    deliver(
        &mut app,
        &first,
        Err("codex: command not found".to_string()),
    );

    if let AppMode::Learning(state) = &mut app.mode {
        state.selected_qa = 0;
    }
    let retry = app.learning_deep_dive().unwrap();

    assert_ne!(retry, first);
    assert_eq!(learning(&app).qa.len(), 3);
}

/// The whole point of a deep dive is to replace an answer that may have
/// invented its evidence. If a follow-up on the verified answer walked the
/// thread back into the shallow one, the fabrication would be handed to the
/// agent as established fact one question later.
#[test]
fn a_follow_up_on_a_deep_dive_leaves_the_answer_it_replaced_behind() {
    let (_repo, mut app) = opened_app();
    ask_and_answer(
        &mut app,
        "What does this do?",
        "It calls into the widget pump.",
    );
    let deeper = app.learning_deep_dive().unwrap();
    deliver(
        &mut app,
        &deeper,
        Ok("It calls into the event loop.".to_string()),
    );

    let turns = app.learning_ancestor_turns(Some(&deeper));

    let answers: Vec<&str> = turns.iter().map(|t| t.answer.as_str()).collect();
    assert_eq!(
        answers,
        vec!["It calls into the event loop."],
        "only the verified answer continues the conversation"
    );
}

/// A deep dive of a follow-up steps over the turn it re-ran, not over the
/// conversation that led there.
#[test]
fn a_follow_up_on_a_deep_dive_still_carries_the_turns_above_it() {
    let (_repo, mut app) = opened_app();
    ask_and_answer(&mut app, "What is this file for?", "It is the entry point.");
    let child = follow_up(&mut app, "What's an entry point?");
    deliver(&mut app, &child, Ok("Where execution begins.".to_string()));

    // Send the follow-up deeper, then continue from the deep dive.
    if let AppMode::Learning(state) = &mut app.mode {
        state.selected_qa = state.qa.iter().position(|r| r.id == child).unwrap();
    }
    let deeper = app.learning_deep_dive().unwrap();
    deliver(
        &mut app,
        &deeper,
        Ok("Where the process starts running.".to_string()),
    );

    let answers: Vec<String> = app
        .learning_ancestor_turns(Some(&deeper))
        .into_iter()
        .map(|t| t.answer)
        .collect();
    assert_eq!(
        answers,
        vec![
            "It is the entry point.".to_string(),
            "Where the process starts running.".to_string(),
        ],
        "the grandparent turn survives; only the re-derived one is dropped"
    );
}

/// Under Codex every row is recorded as a deep dive (`effective_for`), so
/// "is this a rerun?" cannot be read off `run_mode` — doing so would strip
/// an ordinary Codex follow-up of the answer it is following up on.
#[test]
fn a_codex_follow_up_is_not_mistaken_for_a_rerun() {
    let (_repo, mut app) = opened_app();
    if let AppMode::Learning(state) = &mut app.mode {
        state.harness = AgentKind::Codex;
    }
    let parent = ask_and_answer(&mut app, "What is this file for?", "It is the entry point.");
    assert_eq!(
        learning(&app).qa[0].run_mode,
        crate::app::LearningRunMode::DeepDive,
        "codex has no no-tools mode"
    );

    let child = follow_up(&mut app, "What's an entry point?");
    let row = learning(&app)
        .qa
        .iter()
        .find(|r| r.id == child)
        .unwrap()
        .clone();
    assert_eq!(row.parent_qa_id.as_deref(), Some(parent.as_str()));
    assert!(
        row.deep_dive_of.is_none(),
        "a follow-up replaces nothing, whatever mode it runs in"
    );
    assert_eq!(
        app.learning_ancestor_turns(Some(&parent))
            .into_iter()
            .map(|t| t.answer)
            .collect::<Vec<_>>(),
        vec!["It is the entry point.".to_string()],
    );
}

/// `D` on a row that reads the repository is refused whether or not it has
/// landed, so the in-flight message must not promise it will work later.
#[test]
fn d_on_a_running_deep_dive_says_to_follow_up_not_to_wait() {
    let (_repo, mut app) = opened_app();
    ask_and_answer(&mut app, "What does this do?", "It runs the thing.");
    let deeper = app.learning_deep_dive().unwrap();
    assert!(
        learning(&app)
            .qa
            .iter()
            .find(|r| r.id == deeper)
            .unwrap()
            .status
            .is_in_flight()
    );

    assert!(app.learning_deep_dive().is_none(), "the cursor is on it");

    let error = learning(&app).error.clone().unwrap_or_default();
    assert!(error.contains("already reading the repository"), "{error}");
    assert!(
        error.contains("(F)"),
        "and points at what does work: {error}"
    );
    assert_eq!(learning(&app).qa.len(), 2, "nothing was started");
}

/// The second `D` jumps to the run that exists — which, while it is still
/// running, has not come back with anything to read.
#[test]
fn a_second_deep_dive_while_the_first_runs_says_it_is_still_going() {
    let (_repo, mut app) = opened_app();
    ask_and_answer(&mut app, "What does this do?", "It runs the thing.");
    let deeper = app.learning_deep_dive().unwrap();

    if let AppMode::Learning(state) = &mut app.mode {
        state.selected_qa = 0;
    }
    assert!(app.learning_deep_dive().is_none());

    let state = learning(&app);
    assert_eq!(state.qa.len(), 2, "the same run isn't paid for twice");
    assert_eq!(state.qa[state.selected_qa].id, deeper);
    let error = state.error.clone().unwrap_or_default();
    assert!(
        error.contains("still reading the repository"),
        "an unfinished run must not be described as one that came back: {error}"
    );
}

// ── re-filing an entry ───────────────────────────────────

/// The case the feature exists for: you asked what something did, the
/// answer told you it was broken, and the entry should now be filed as a
/// change without losing the explanation that got you there.
#[test]
fn re_filing_an_explanation_as_a_change_keeps_its_answer() {
    let (_repo, mut app) = opened_app();
    let id = ask_and_answer(
        &mut app,
        "What does this do?",
        "It retries forever, which is probably a bug.",
    );

    assert_eq!(
        app.learning_relabel_intent(),
        Some(LearningQaIntent::Action)
    );

    let state = learning(&app);
    let row = state.qa.iter().find(|r| r.id == id).unwrap();
    assert_eq!(row.intent, LearningQaIntent::Action);
    assert_eq!(
        row.answer.as_deref(),
        Some("It retries forever, which is probably a bug."),
        "the answer is a record of what was said, not something re-filing rewrites"
    );
    assert_eq!(
        row.question, "What does this do?",
        "and neither is the question"
    );
}

/// Re-filing is a two-way gesture: an answer that proposed no change at
/// all goes back to being a note.
#[test]
fn re_filing_goes_both_ways() {
    let (_repo, mut app) = opened_app();
    let id = app
        .learning_ask("Make this clearer", LearningQaIntent::Action, None)
        .unwrap();
    deliver(&mut app, &id, Ok("Nothing to change here.".to_string()));

    assert_eq!(
        app.learning_relabel_intent(),
        Some(LearningQaIntent::Explain)
    );
    assert_eq!(
        app.learning_relabel_intent(),
        Some(LearningQaIntent::Action),
        "and back again"
    );
}

/// The new marker must not be read as "the answer was rewritten to match",
/// which is exactly what a newcomer would assume from a label that changed
/// on its own.
#[test]
fn re_filing_says_the_answer_was_not_rewritten() {
    let (_repo, mut app) = opened_app();
    ask_and_answer(&mut app, "What does this do?", "It retries forever.");

    app.learning_relabel_intent();

    let state = learning(&app);
    assert!(
        state.error.is_none(),
        "nothing went wrong: {:?}",
        state.error
    );
    let notice = state.notice.clone().unwrap_or_default();
    assert!(
        notice.contains("The answer is unchanged"),
        "the banner has to say the text stayed put: {notice}"
    );
    assert!(
        notice.contains("(F)"),
        "and point at the key that does get an answer written the other way: {notice}"
    );
    // The banner is a single unwrapped line at the foot of the overlay, so
    // a sentence longer than a standard terminal loses exactly the tail
    // that carries the point.
    assert!(
        notice.chars().count() <= 130,
        "the banner has to fit a 140-column terminal, got {}: {notice}",
        notice.chars().count()
    );
}

/// The banner names one row, so it must not linger over another.
#[test]
fn the_re_filing_banner_clears_when_the_cursor_moves_on() {
    let (_repo, mut app) = opened_app();
    ask_and_answer(&mut app, "What does this do?", "It retries forever.");
    ask_and_answer(&mut app, "And this?", "It gives up.");

    app.learning_relabel_intent();
    assert!(learning(&app).notice.is_some());

    app.learning_select_qa(-1);
    assert!(
        learning(&app).notice.is_none(),
        "the confirmation described the row that was selected, not this one"
    );
}

/// The cursor also moves without the arrow keys: a follow-up selects the
/// row it just created, and the banner about the parent must not be left
/// standing over it.
#[test]
fn the_re_filing_banner_clears_when_a_follow_up_moves_the_cursor() {
    let (_repo, mut app) = opened_app();
    ask_and_answer(&mut app, "What does this do?", "It retries forever.");

    app.learning_relabel_intent();
    assert!(learning(&app).notice.is_some());

    follow_up(&mut app, "So what should change?");
    assert!(
        learning(&app).notice.is_none(),
        "the confirmation described the parent, not the follow-up now selected"
    );
}

/// Same for a deep dive, which selects its own new row.
#[test]
fn the_re_filing_banner_clears_when_a_deep_dive_moves_the_cursor() {
    let (_repo, mut app) = opened_app();
    ask_and_answer(&mut app, "What does this do?", "It retries forever.");

    app.learning_relabel_intent();
    assert!(learning(&app).notice.is_some());

    app.learning_deep_dive().unwrap();
    assert!(
        learning(&app).notice.is_none(),
        "the confirmation described the original, not the deep dive now selected"
    );
}

/// "The answer on its way was asked for as an explanation" is true until
/// the answer arrives, and nothing the user does marks that moment — so
/// the arrival has to take the banner down itself.
#[test]
fn the_re_filing_banner_clears_when_the_answer_lands() {
    let (_repo, mut app) = opened_app();
    let id = app
        .learning_ask("What does this do?", LearningQaIntent::Explain, None)
        .unwrap();

    app.learning_relabel_intent();
    let notice = learning(&app).notice.clone().unwrap_or_default();
    assert!(notice.contains("on its way"), "{notice}");

    deliver(&mut app, &id, Ok("It retries forever.".to_string()));

    assert!(
        learning(&app).notice.is_none(),
        "the answer is here, so a banner calling it on its way is now false"
    );
}

/// A label the next open of the overlay silently drops is worse than one
/// refused out loud, so a re-file that cannot be written through is undone
/// and reported rather than confirmed.
#[test]
fn a_re_file_that_cannot_be_saved_is_undone_and_says_so() {
    let (_repo, db_dir, mut app) = opened_app_with_db();
    let id = ask_and_answer(&mut app, "What does this do?", "It retries forever.");
    let before = learning(&app)
        .qa
        .iter()
        .find(|r| r.id == id)
        .unwrap()
        .updated_at
        .clone();
    // The same database, reopened read-only: the session and the row are
    // all still there, and only the write fails.
    app.db = Some(crate::db::AmfDb::open_read_only(&db_dir.path().join("amf.db")).unwrap());

    assert_eq!(
        app.learning_relabel_intent(),
        None,
        "nothing was re-filed, so no new intent is reported"
    );

    let state = learning(&app);
    let row = state.qa.iter().find(|r| r.id == id).unwrap();
    assert_eq!(
        row.intent,
        LearningQaIntent::Explain,
        "the on-screen label must match what a reopen would show"
    );
    assert_eq!(row.updated_at, before, "and so must the timestamp");
    assert!(
        state.notice.is_none(),
        "nothing to confirm: {:?}",
        state.notice
    );
    let error = state.error.clone().unwrap_or_default();
    assert!(
        error.contains("re-file") && error.contains("saved"),
        "the banner has to say the re-file did not stick: {error}"
    );
}

/// A follow-up inherits its parent's intent, so re-filing has to change
/// what the next question defaults to — otherwise the label is decoration.
#[test]
fn a_follow_up_after_re_filing_inherits_the_new_intent() {
    let (_repo, mut app) = opened_app();
    ask_and_answer(&mut app, "What does this do?", "It retries forever.");

    app.learning_relabel_intent();
    app.learning_open_follow_up();

    assert_eq!(
        learning(&app).question.as_ref().unwrap().intent,
        LearningQaIntent::Action
    );
}

#[test]
fn re_filing_with_nothing_asked_says_so() {
    let (_repo, mut app) = opened_app();

    assert_eq!(app.learning_relabel_intent(), None);

    let error = learning(&app).error.clone().unwrap_or_default();
    assert!(error.contains("Ask something first"), "{error}");
}

/// The prompt is already dispatched under the old framing whether the run
/// has landed or not, so refusing mid-flight would withhold the label for
/// no gain — but the banner must not claim there is an answer to keep.
#[test]
fn a_question_still_generating_can_be_re_filed() {
    let (_repo, mut app) = opened_app();
    app.learning_ask("What does this do?", LearningQaIntent::Explain, None)
        .unwrap();

    assert_eq!(
        app.learning_relabel_intent(),
        Some(LearningQaIntent::Action)
    );

    let state = learning(&app);
    assert_eq!(state.qa[0].intent, LearningQaIntent::Action);
    let notice = state.notice.clone().unwrap_or_default();
    assert!(
        notice.contains("on its way"),
        "an unfinished run has no answer to describe as kept: {notice}"
    );
}

/// A bare row, for the ordering helpers that only look at ids and parents.
fn qa_row(id: &str, parent: Option<&str>) -> LearningQa {
    LearningQa {
        id: id.to_string(),
        session_id: "s".to_string(),
        parent_qa_id: parent.map(str::to_string),
        deep_dive_of: None,
        file_path: None,
        anchor: LearningAnchor::Project,
        selection_text: String::new(),
        selection_is_diff: false,
        question: id.to_string(),
        intent: LearningQaIntent::Explain,
        level: LearningLevel::Newcomer,
        answer: None,
        harness: AgentKind::Claude,
        run_mode: crate::app::LearningRunMode::NoTools,
        status: crate::app::LearningQaStatus::Pending,
        error: None,
        todo_id: None,
        spawned_session_id: None,
        created_at: String::new(),
        updated_at: String::new(),
    }
}

#[test]
fn a_thread_insert_lands_past_every_descendant() {
    let rows = vec![
        qa_row("a", None),
        qa_row("b", Some("a")),
        qa_row("c", Some("b")),
        qa_row("d", None),
    ];
    // Past the whole a → b → c thread, not just past `a`.
    assert_eq!(thread_insert_index(&rows, "a"), Some(3));
    assert_eq!(thread_insert_index(&rows, "b"), Some(3));
    assert_eq!(thread_insert_index(&rows, "c"), Some(3));
    assert_eq!(thread_insert_index(&rows, "d"), Some(4));
    assert_eq!(
        thread_insert_index(&rows, "gone"),
        None,
        "a stale parent appends rather than vanishing"
    );
}

/// Stored order is the order things were asked; threaded order is the order
/// they are read in. Ids here are in ask order, so `b` and `e` were asked
/// long after the questions they continue.
#[test]
fn threading_a_stored_history_gathers_each_conversation() {
    let stored = vec![
        qa_row("a", None),
        qa_row("d", None),
        qa_row("b", Some("a")),
        qa_row("e", Some("d")),
        qa_row("c", Some("b")),
    ];
    let threaded = thread_rows(stored);
    let ids: Vec<&str> = threaded.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["a", "b", "c", "d", "e"],
        "each thread reads top to bottom, and roots keep the order they \
             were asked in"
    );
}

/// Orphans are not supposed to happen — the delete cascades — but a row
/// pointing at a parent that isn't there must still be readable rather than
/// dropped, since it is the only copy of a question someone asked.
#[test]
fn threading_keeps_a_row_whose_parent_is_gone() {
    let stored = vec![qa_row("a", None), qa_row("orphan", Some("vanished"))];
    let threaded = thread_rows(stored);
    let ids: Vec<&str> = threaded.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, vec!["a", "orphan"]);
}

#[test]
fn following_up_on_an_unanswered_question_says_to_wait() {
    let (_repo, mut app) = opened_app();
    app.learning_ask("Still thinking?", LearningQaIntent::Explain, None)
        .unwrap();

    app.learning_open_follow_up();
    let state = learning(&app);
    assert!(state.question.is_none(), "nothing to follow up on yet");
    let error = state.error.as_deref().unwrap_or_default();
    assert!(error.contains("still generating"), "{error}");
}

#[test]
fn a_huge_repo_listing_is_capped_and_says_so() {
    let mut small: Vec<String> = (0..5).map(|i| format!("f{i}.rs")).collect();
    assert_eq!(
        cap_repo_entries(&mut small, 10),
        None,
        "a list under the cap is left alone and reports nothing"
    );
    assert_eq!(small.len(), 5);

    let mut big: Vec<String> = (0..50).map(|i| format!("f{i}.rs")).collect();
    assert_eq!(
        cap_repo_entries(&mut big, 10),
        Some(50),
        "the original total comes back so the user can be told"
    );
    assert_eq!(big.len(), 10);
}

#[test]
fn a_codex_question_is_recorded_as_the_deep_dive_it_will_actually_be() {
    use crate::app::LearningRunMode;

    // Codex has no no-tools headless mode, so a row claiming "this file
    // only" would misdescribe the command that ran.
    assert_eq!(
        LearningRunMode::NoTools.effective_for(&AgentKind::Codex),
        LearningRunMode::DeepDive
    );
    assert_eq!(
        LearningRunMode::DeepDive.effective_for(&AgentKind::Codex),
        LearningRunMode::DeepDive
    );
    for harness in [AgentKind::Claude, AgentKind::Opencode, AgentKind::Pi] {
        assert_eq!(
            LearningRunMode::NoTools.effective_for(&harness),
            LearningRunMode::NoTools,
            "{harness:?} answers without tools when asked to"
        );
    }
}

#[test]
fn a_question_stranded_by_a_previous_run_reloads_as_failed_not_thinking() {
    let (_repo, mut app) = opened_app();
    app.learning_ask("What does this do?", LearningQaIntent::Explain, None)
        .unwrap();
    let stranded = learning(&app).qa[0].clone();
    assert!(stranded.status.is_in_flight());

    // A fresh process knows about no live runs, so the row is stranded.
    app.learning_runs_in_flight.clear();
    let rows = app.reconcile_interrupted_qa(vec![stranded.clone()]);
    assert_eq!(rows[0].status, crate::app::LearningQaStatus::Failed);
    assert_eq!(
        rows[0].question, stranded.question,
        "the question survives so it can be asked again"
    );
    let reason = rows[0].error.as_deref().unwrap_or_default();
    assert!(reason.contains("Ask it again"), "{reason}");

    // A run this process is genuinely still waiting on is left alone.
    app.learning_runs_in_flight.insert(stranded.id.clone());
    let rows = app.reconcile_interrupted_qa(vec![stranded.clone()]);
    assert_eq!(rows[0].status, stranded.status);
    assert!(rows[0].error.is_none());
}

// ── answers that outlive their overlay ───────────────────

/// An overlay backed by a real database, so history survives a close.
fn opened_app_with_db() -> (TempDir, TempDir, App) {
    let repo = repo_with_branch_change();
    let db_dir = TempDir::new().unwrap();
    let mut app = app_at(repo.path(), true);
    app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());
    app.open_learning_mode(0, 0).unwrap();
    while learning(&app).content_path.as_deref() != Some("src/main.rs") {
        app.learning_select_next_entry();
    }
    (repo, db_dir, app)
}

/// A run outlives the overlay that started it: closing the overlay while a
/// question is generating must not leave the stored row at "running", which
/// would reload as a question that never finishes.
#[test]
fn an_answer_arriving_after_the_overlay_closed_is_still_saved() {
    let (_repo, _db, mut app) = opened_app_with_db();
    let id = app
        .learning_ask("What does this do?", LearningQaIntent::Explain, None)
        .unwrap();
    assert!(!learning(&app).session_id.is_empty(), "persisted session");

    app.close_learning_mode();
    assert!(matches!(app.mode, AppMode::Normal));
    deliver(&mut app, &id, Ok("It is the entry point.".to_string()));

    app.open_learning_mode(0, 0).unwrap();
    let row = learning(&app)
        .qa
        .iter()
        .find(|r| r.id == id)
        .expect("the question is still in history")
        .clone();
    assert_eq!(row.status, crate::app::LearningQaStatus::Answered);
    assert_eq!(row.answer.as_deref(), Some("It is the entry point."));
    assert_eq!(
        learning(&app).in_flight_count(),
        0,
        "and it no longer counts as generating"
    );
}

/// The failure path takes the same route: a run that failed after its
/// overlay closed reloads as failed, not as still thinking.
#[test]
fn a_failure_arriving_after_the_overlay_closed_is_still_saved() {
    let (_repo, _db, mut app) = opened_app_with_db();
    let id = app
        .learning_ask("What does this do?", LearningQaIntent::Explain, None)
        .unwrap();
    app.close_learning_mode();
    deliver(&mut app, &id, Err("Claude couldn't answer".to_string()));

    app.open_learning_mode(0, 0).unwrap();
    let row = learning(&app)
        .qa
        .iter()
        .find(|r| r.id == id)
        .unwrap()
        .clone();
    assert_eq!(row.status, crate::app::LearningQaStatus::Failed);
    let reason = row.error.as_deref().unwrap_or_default();
    assert!(reason.contains("couldn't answer"), "{reason}");
    assert!(
        !reason.contains("AMF stopped"),
        "a real failure keeps its own reason rather than being reconciled: {reason}"
    );
}

// ── anchor drift ─────────────────────────────────────────

fn lines_of(text: &str) -> Vec<String> {
    text.lines().map(ToOwned::to_owned).collect()
}

/// An anchored row over `src/util.rs`, the file the overlay tests below
/// edit behind the overlay's back.
fn anchored_qa(start: usize, end: usize, selection: &str) -> LearningQa {
    let mut qa = qa_row("qa-1", None);
    qa.file_path = Some("src/util.rs".to_string());
    qa.anchor = LearningAnchor::Lines { start, end };
    qa.selection_text = selection.to_string();
    qa
}

/// The base case, and the one that must stay silent: nothing changed, so
/// nothing is reported. A check that cried drift on an untouched file would
/// be worse than no check at all.
#[test]
fn an_anchor_that_did_not_move_is_not_reported() {
    let file = lines_of("fn a() {}\nfn b() {}\nfn c() {}");
    let qa = anchored_qa(2, 2, "fn b() {}");
    assert_eq!(
        check_anchor_drift(&qa, &AnchorTarget::Lines(file)),
        None,
        "the code is exactly where it was left"
    );
}

/// Re-indenting a file moves every line's text but none of its meaning.
/// Comparison is trimmed so a `rustfmt` pass isn't reported as drift.
#[test]
fn re_indenting_the_code_is_not_movement() {
    let file = lines_of("fn a() {\n        let x = 1;\n}");
    let qa = anchored_qa(2, 2, "    let x = 1;");
    assert_eq!(check_anchor_drift(&qa, &AnchorTarget::Lines(file)), None);
}

/// The common case the plan names: lines were added above, so the code the
/// question was about is now further down. It is found again and the new
/// range is reported.
#[test]
fn code_that_moved_down_is_found_again() {
    let file = lines_of("use std::fmt;\n\nfn a() {}\nfn b() {}\nfn c() {}");
    let qa = anchored_qa(2, 2, "fn b() {}");
    assert_eq!(
        check_anchor_drift(&qa, &AnchorTarget::Lines(file)),
        Some(LearningAnchorDrift::Reanchored { start: 4, end: 4 })
    );
}

/// Both ends of a multi-line selection move together, and blank lines
/// inside the file don't shift the range's reported end.
#[test]
fn a_moved_range_reports_both_of_its_ends() {
    let file = lines_of("// header\n\nfn b() {\n    work();\n}\n");
    let qa = anchored_qa(1, 3, "fn b() {\n    work();\n}");
    assert_eq!(
        check_anchor_drift(&qa, &AnchorTarget::Lines(file)),
        Some(LearningAnchorDrift::Reanchored { start: 3, end: 5 })
    );
}

/// Code that is simply gone. The entry keeps its question and answer — they
/// are still the only record of what someone asked — but it stops claiming
/// the line numbers mean anything.
#[test]
fn code_that_was_rewritten_loses_its_anchor() {
    let file = lines_of("fn a() {}\nfn renamed() {}\nfn c() {}");
    let qa = anchored_qa(2, 2, "fn b() {}");
    assert_eq!(
        check_anchor_drift(&qa, &AnchorTarget::Lines(file)),
        Some(LearningAnchorDrift::Lost(LearningAnchorLoss::NotFound))
    );
}

/// The plan's open question, settled towards honesty: two candidates means
/// there is no way to say which one the question was about, so it is
/// reported lost rather than re-anchored to a guess.
#[test]
fn code_that_now_appears_twice_is_lost_rather_than_guessed_at() {
    let file = lines_of("fn a() {}\nfn dup() {}\nfn c() {}\nfn dup() {}");
    let qa = anchored_qa(3, 3, "fn dup() {}");
    assert_eq!(
        check_anchor_drift(&qa, &AnchorTarget::Lines(file)),
        Some(LearningAnchorDrift::Lost(LearningAnchorLoss::Ambiguous))
    );
}

/// …but a duplicate is only ambiguous if the original moved. Code that is
/// still where it was stored is anchored, however many copies of it have
/// since been made elsewhere — otherwise extracting a repeated idiom would
/// break every note about the original.
#[test]
fn a_copy_made_elsewhere_does_not_unanchor_the_original() {
    let file = lines_of("fn a() {}\nfn dup() {}\nfn c() {}\nfn dup() {}");
    let qa = anchored_qa(2, 2, "fn dup() {}");
    assert_eq!(check_anchor_drift(&qa, &AnchorTarget::Lines(file)), None);
}

/// A file that is no longer in the project is its own outcome, and reads
/// differently to a rewritten one.
#[test]
fn a_deleted_file_takes_every_anchor_in_it() {
    let qa = anchored_qa(2, 2, "fn b() {}");
    assert_eq!(
        check_anchor_drift(&qa, &AnchorTarget::Gone),
        Some(LearningAnchorDrift::Lost(LearningAnchorLoss::FileGone))
    );
}

/// A file that is there but can't be read claims nothing. "We didn't look"
/// is not one of the three outcomes, and a marker that meant it would be
/// worse than no marker.
#[test]
fn a_file_that_could_not_be_read_claims_nothing() {
    let qa = anchored_qa(2, 2, "fn b() {}");
    assert_eq!(check_anchor_drift(&qa, &AnchorTarget::Unreadable), None);
}

/// A whole-file anchor moves with its file: as long as the file is there,
/// the anchor is as good as it ever was, whatever the contents now say.
#[test]
fn a_whole_file_anchor_only_notices_the_file_going_away() {
    let mut qa = anchored_qa(1, 1, "anything at all");
    qa.anchor = LearningAnchor::File;
    assert_eq!(
        check_anchor_drift(&qa, &AnchorTarget::Lines(lines_of("something else"))),
        None
    );
    assert_eq!(
        check_anchor_drift(&qa, &AnchorTarget::Gone),
        Some(LearningAnchorDrift::Lost(LearningAnchorLoss::FileGone))
    );
}

/// A project anchor has no file and cannot drift.
#[test]
fn the_project_anchor_never_drifts() {
    let mut qa = anchored_qa(1, 1, "");
    qa.anchor = LearningAnchor::Project;
    qa.file_path = None;
    assert_eq!(check_anchor_drift(&qa, &AnchorTarget::Gone), None);
}

/// A diff excerpt is matched on the lines that are actually in the file —
/// its removals are dropped along with the markers.
#[test]
fn a_diff_selection_is_matched_on_what_survived_it() {
    let block = expected_block("-let x = 1;\n+let x = 2;\n let y = 3;", true);
    assert_eq!(block.lines, vec!["let x = 2;", "let y = 3;"]);
    assert_eq!(block.lead_offset, 1, "the removed row came off the front");
}

/// A selection that opens on a blank line is still where it was left: the
/// blank carries no evidence and is dropped, so the search starts one row
/// further down than the stored range does. Counting from the stored start
/// instead reports every such selection as having slid down by exactly the
/// number of leading blanks — drift invented out of the user's own
/// whitespace.
#[test]
fn a_selection_opening_on_a_blank_line_has_not_moved() {
    let qa = anchored_qa(2, 4, "\n  fn b() {}\n  let y = 3;");
    let file = lines_of("fn a() {}\n\nfn b() {}\nlet y = 3;\n");
    assert_eq!(check_anchor_drift(&qa, &AnchorTarget::Lines(file)), None);
}

/// And the offset is a shift, not a blanket amnesty: the same selection
/// genuinely pushed down the file is still reported as re-anchored.
#[test]
fn a_blank_leading_selection_that_really_moved_still_reports() {
    let qa = anchored_qa(2, 4, "\n  fn b() {}\n  let y = 3;");
    let file = lines_of("fn a() {}\n\n// inserted\n// inserted\nfn b() {}\nlet y = 3;\n");
    assert_eq!(
        check_anchor_drift(&qa, &AnchorTarget::Lines(file)),
        Some(LearningAnchorDrift::Reanchored { start: 5, end: 6 })
    );
}

/// A diff-sourced anchor can be reported lost but never re-anchored: its
/// stored range is numbered off `new_line.or(old_line)`, so a selection
/// opening on a removed line is already measured against the base side.
/// "That code is gone" survives that; "it moved to line 9" does not.
#[test]
fn a_diff_anchor_is_reported_lost_but_never_moved() {
    let mut qa = anchored_qa(2, 3, "+fn b() {}\n let y = 3;");
    qa.selection_is_diff = true;

    let moved = lines_of("// added\n// added\nfn b() {}\nlet y = 3;");
    assert_eq!(
        check_anchor_drift(&qa, &AnchorTarget::Lines(moved)),
        None,
        "still in the file, so nothing is claimed either way"
    );

    let gone = lines_of("fn renamed() {}\nlet y = 3;");
    assert_eq!(
        check_anchor_drift(&qa, &AnchorTarget::Lines(gone)),
        Some(LearningAnchorDrift::Lost(LearningAnchorLoss::NotFound))
    );
}

/// A row with nothing captured has nothing to check against, and says so by
/// staying quiet.
#[test]
fn a_row_with_no_captured_selection_is_left_alone() {
    let qa = anchored_qa(2, 2, "   \n\n");
    assert_eq!(
        check_anchor_drift(&qa, &AnchorTarget::Lines(lines_of("a\nb\nc"))),
        None
    );
}

/// An overlay with a real database, opened on `src/util.rs` — a file the
/// test can then edit behind the overlay's back.
fn app_on_util(contents: &str) -> (TempDir, TempDir, App) {
    let repo = repo_with_branch_change();
    std::fs::write(repo.path().join("src/util.rs"), contents).unwrap();
    let db_dir = TempDir::new().unwrap();
    let mut app = app_at(repo.path(), true);
    app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());
    app.open_learning_mode(0, 0).unwrap();
    for _ in 0..learning(&app).entries.len() {
        if learning(&app).content_path.as_deref() == Some("src/util.rs") {
            break;
        }
        app.learning_select_next_entry();
    }
    assert_eq!(learning(&app).content_path.as_deref(), Some("src/util.rs"));
    (repo, db_dir, app)
}

/// End to end: ask about a line, add code above it, reopen. The entry comes
/// back marked, pointing at where the code went — and the overlay says so
/// on the way in, because the marker is in a pane the user may not be
/// looking at.
#[test]
fn a_question_reloads_marked_when_its_code_moved() {
    let (repo, _db, mut app) = app_on_util("fn a() {}\nfn b() {}\nfn c() {}\n");
    app.learning_cursor_move(1);
    let id = ask_and_answer(&mut app, "What is this?", "It is b.");
    assert_eq!(
        learning(&app)
            .qa
            .iter()
            .find(|r| r.id == id)
            .unwrap()
            .anchor,
        LearningAnchor::Lines { start: 2, end: 2 }
    );
    app.close_learning_mode();

    std::fs::write(
        repo.path().join("src/util.rs"),
        "use std::fmt;\n\nfn a() {}\nfn b() {}\nfn c() {}\n",
    )
    .unwrap();
    app.open_learning_mode(0, 0).unwrap();

    let state = learning(&app);
    assert_eq!(
        state.drift_for(&id),
        Some(LearningAnchorDrift::Reanchored { start: 4, end: 4 })
    );
    let banner = state.notice.clone().unwrap_or_default();
    assert!(banner.contains("moved with the code"), "{banner}");
}

/// The stored range is the historical fact of where the question was asked,
/// and re-anchoring does not overwrite it — which is also what lets the
/// verdict be re-derived on every open instead of being believed once.
#[test]
fn re_anchoring_does_not_rewrite_the_range_it_was_asked_at() {
    let (repo, _db, mut app) = app_on_util("fn a() {}\nfn b() {}\nfn c() {}\n");
    app.learning_cursor_move(1);
    let id = ask_and_answer(&mut app, "What is this?", "It is b.");
    app.close_learning_mode();
    std::fs::write(
        repo.path().join("src/util.rs"),
        "use std::fmt;\n\nfn a() {}\nfn b() {}\nfn c() {}\n",
    )
    .unwrap();

    app.open_learning_mode(0, 0).unwrap();
    let row = learning(&app)
        .qa
        .iter()
        .find(|r| r.id == id)
        .unwrap()
        .clone();
    assert_eq!(
        row.anchor,
        LearningAnchor::Lines { start: 2, end: 2 },
        "the row still records where it was asked"
    );
    assert_eq!(row.answer.as_deref(), Some("It is b."));
}

/// A deleted file leaves its questions readable and honest about it.
#[test]
fn a_question_whose_file_was_deleted_reloads_marked_lost() {
    let (repo, _db, mut app) = app_on_util("fn a() {}\nfn b() {}\n");
    let id = ask_and_answer(&mut app, "What is this?", "It is a.");
    app.close_learning_mode();
    std::fs::remove_file(repo.path().join("src/util.rs")).unwrap();

    app.open_learning_mode(0, 0).unwrap();
    let state = learning(&app);
    assert_eq!(
        state.drift_for(&id),
        Some(LearningAnchorDrift::Lost(LearningAnchorLoss::FileGone))
    );
    let row = state.qa.iter().find(|r| r.id == id).unwrap();
    assert_eq!(
        row.answer.as_deref(),
        Some("It is a."),
        "the answer is still the only copy of what was said"
    );
    let banner = state.notice.clone().unwrap_or_default();
    assert!(
        banner.contains("1 no longer points at code"),
        "one lost anchor reads as one: {banner}"
    );
}

/// `Path::exists()` answers "no" to a file it cannot stat just as loudly as
/// to one that was deleted, and the two are not the same claim: a file
/// behind a directory this process can't read is almost certainly still
/// there. Unreadable means no verdict, here as everywhere else.
#[cfg(unix)]
#[test]
fn a_file_behind_an_unreadable_directory_is_not_reported_gone() {
    use std::os::unix::fs::PermissionsExt;

    let (repo, _db, mut app) = app_on_util("fn a() {}\nfn b() {}\n");
    let id = ask_and_answer(&mut app, "What is this?", "It is a.");
    app.close_learning_mode();

    let dir = repo.path().join("src");
    let restore = std::fs::metadata(&dir).unwrap().permissions();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000)).unwrap();
    // Root walks through mode bits, so there would be nothing to observe.
    let unreadable = std::fs::metadata(dir.join("util.rs")).is_err();
    let verdict = if unreadable {
        app.open_learning_mode(0, 0).unwrap();
        let verdict = learning(&app).drift_for(&id);
        app.close_learning_mode();
        Some(verdict)
    } else {
        None
    };
    std::fs::set_permissions(&dir, restore).unwrap();

    if let Some(verdict) = verdict {
        assert_eq!(
            verdict, None,
            "a file we could not look at is not a file that is gone"
        );
    }
}

/// Reopening a project nobody has touched says nothing at all.
#[test]
fn an_untouched_project_reloads_with_nothing_marked() {
    let (_repo, _db, mut app) = app_on_util("fn a() {}\nfn b() {}\n");
    let id = ask_and_answer(&mut app, "What is this?", "It is a.");
    app.close_learning_mode();

    app.open_learning_mode(0, 0).unwrap();
    let state = learning(&app);
    assert!(state.drift_for(&id).is_none());
    assert!(state.anchor_drift.is_empty());
    assert!(
        state.notice.is_none(),
        "nothing drifted, so nothing is announced: {:?}",
        state.notice
    );
}

/// Handing a drifted answer to a live agent has to say the code moved. The
/// excerpt in the seed still shows what the user remembers, so without this
/// the agent is sent to read a location that no longer holds it — the exact
/// route from a stale anchor to a confidently wrong answer.
#[test]
fn a_drifted_answer_is_handed_over_saying_where_the_code_went() {
    let mut qa = anchored_qa(2, 2, "fn b() {}");
    qa.answer = Some("It is b.".to_string());
    let seed = escalation_seed(
        &qa,
        Some(LearningAnchorDrift::Reanchored { start: 4, end: 4 }),
    );
    assert!(seed.contains("src/util.rs:2"), "{seed}");
    assert!(seed.contains("it is now line 4"), "{seed}");

    let lost = escalation_seed(
        &qa,
        Some(LearningAnchorDrift::Lost(LearningAnchorLoss::FileGone)),
    );
    assert!(lost.contains("no longer in the project"), "{lost}");

    let clean = escalation_seed(&qa, None);
    assert!(
        !clean.contains("moved"),
        "nothing to say when nothing moved"
    );
}

/// The same for a note kept on the TODO list: `todo_spawn_prompt` appends
/// this body verbatim, so a silent stale locator would travel just as far.
#[test]
fn a_drifted_answer_is_kept_saying_where_the_code_went() {
    let mut qa = anchored_qa(2, 2, "fn b() {}");
    qa.answer = Some("It is b.".to_string());
    let body = todo_body(
        &qa,
        Some(LearningAnchorDrift::Reanchored { start: 4, end: 4 }),
    );
    assert!(body.contains("src/util.rs:2"), "{body}");
    assert!(body.contains("it is now line 4"), "{body}");
    assert!(!todo_body(&qa, None).contains("moved"));
}

/// Re-filing is a durable decision about how an entry is kept, so it has
/// to be there on the next open rather than only until the overlay closes.
#[test]
fn a_re_filed_entry_reloads_the_way_it_was_filed() {
    let (_repo, _db, mut app) = opened_app_with_db();
    let id = app
        .learning_ask("What does this do?", LearningQaIntent::Explain, None)
        .unwrap();
    deliver(&mut app, &id, Ok("It retries forever.".to_string()));
    app.learning_relabel_intent();

    app.close_learning_mode();
    app.open_learning_mode(0, 0).unwrap();

    let row = learning(&app)
        .qa
        .iter()
        .find(|r| r.id == id)
        .expect("still in history")
        .clone();
    assert_eq!(row.intent, LearningQaIntent::Action);
    assert_eq!(
        row.answer.as_deref(),
        Some("It retries forever."),
        "and the answer came back with it"
    );
}

/// Stored history comes back oldest-first, but a follow-up is asked *after*
/// whatever else was asked in between — so replaying that order verbatim
/// would indent it under an unrelated question. This is the same defect
/// `a_follow_up_lands_under_the_thread_it_continues` fixed for the live
/// list, arriving by the other door.
#[test]
fn a_reloaded_thread_keeps_its_follow_ups_under_their_parents() {
    let (_repo, _db, mut app) = opened_app_with_db();
    let first = ask_and_answer(&mut app, "First question?", "First answer.");
    let unrelated = ask_and_answer(&mut app, "Unrelated question?", "Unrelated answer.");
    if let AppMode::Learning(state) = &mut app.mode {
        state.selected_qa = 0;
    }
    let child = follow_up(&mut app, "Follow-up?");
    deliver(&mut app, &child, Ok("Follow-up answer.".to_string()));

    app.close_learning_mode();
    app.open_learning_mode(0, 0).unwrap();

    let ids: Vec<&str> = learning(&app).qa.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![first.as_str(), child.as_str(), unrelated.as_str()],
        "a reopened history threads the same way the live one does"
    );
}

/// A rerun sits in its original's place for the same reason, and it is
/// threaded by `parent_qa_id` like everything else.
#[test]
fn a_reloaded_deep_dive_stays_with_the_question_it_re_asked() {
    let (_repo, _db, mut app) = opened_app_with_db();
    let origin = ask_and_answer(&mut app, "What does this do?", "A guess.");
    let unrelated = ask_and_answer(&mut app, "Unrelated question?", "Unrelated answer.");
    if let AppMode::Learning(state) = &mut app.mode {
        state.selected_qa = 0;
    }
    let deeper = app.learning_deep_dive().expect("the rerun starts");
    deliver(
        &mut app,
        &deeper,
        Ok("Checked against the repo.".to_string()),
    );

    app.close_learning_mode();
    app.open_learning_mode(0, 0).unwrap();

    let ids: Vec<&str> = learning(&app).qa.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![origin.as_str(), deeper.as_str(), unrelated.as_str()],
        "the rerun reloads under the answer it was checking"
    );
}

/// Every anchor kind has to survive a reload: the file and lines a question
/// was asked about are what make its answer readable a week later, and a
/// follow-up asked after the reload quotes them again.
#[test]
fn a_reloaded_question_still_knows_what_it_was_asked_about() {
    let (_repo, _db, mut app) = opened_app_with_db();
    app.learning_cursor_move(0);
    app.learning_start_range();
    app.learning_cursor_move(1);
    let anchor = learning(&app).anchor;
    assert!(
        matches!(anchor, LearningAnchor::Lines { .. }),
        "the test selected a range: {anchor:?}"
    );
    let id = app
        .learning_ask("Explain these lines.", LearningQaIntent::Explain, None)
        .unwrap();
    deliver(&mut app, &id, Ok("They parse the arguments.".to_string()));
    let selection = learning(&app).qa[0].selection_text.clone();
    assert!(!selection.is_empty(), "the question captured its lines");

    app.close_learning_mode();
    app.open_learning_mode(0, 0).unwrap();

    let row = learning(&app).qa[0].clone();
    assert_eq!(row.anchor, anchor);
    assert_eq!(row.file_path.as_deref(), Some("src/main.rs"));
    assert_eq!(row.selection_text, selection);
    assert_eq!(row.level, LearningLevel::Newcomer);
    assert_eq!(row.intent, LearningQaIntent::Explain);
}

/// The intro is the newcomer's discovery path, so it opens unasked — but
/// only once. A second visit that reopens it reads as the mode not
/// remembering the user was here.
#[test]
fn the_intro_opens_on_the_first_visit_only() {
    let repo = repo_with_branch_change();
    let db_dir = TempDir::new().unwrap();
    let mut app = app_at(repo.path(), true);
    app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());

    app.open_learning_mode(0, 0).unwrap();
    assert!(
        learning(&app).help_open,
        "the first visit explains what this is"
    );

    app.close_learning_mode();
    app.open_learning_mode(0, 0).unwrap();
    assert!(
        !learning(&app).help_open,
        "and the second one does not repeat itself"
    );
}

/// Without a database there is nowhere to record that the intro was shown,
/// so showing it would mean showing it on every single open.
#[test]
fn the_intro_stays_shut_when_there_is_nothing_to_remember_it_with() {
    let (_repo, app) = opened_app();
    assert!(app.db.is_none());
    assert!(!learning(&app).help_open);
}

// ── keeping an answer as a to-do ─────────────────────────

/// Ask, answer, and select some lines so the note has a real anchor.
fn app_with_an_answer() -> (TempDir, TempDir, App, String) {
    let (repo, db, mut app) = opened_app_with_db();
    app.learning_cursor_move(0);
    app.learning_start_range();
    app.learning_cursor_move(1);
    let id = app
        .learning_ask("What does this do?", LearningQaIntent::Explain, None)
        .unwrap();
    deliver(
        &mut app,
        &id,
        Ok("It runs the program.\n\nThe body is empty for now.".to_string()),
    );
    (repo, db, app, id)
}

/// Every item on the project's list, read back from the DB rather than from
/// whatever the overlay believes.
fn stored_todos(app: &App) -> Vec<crate::db::todos::Todo> {
    let db = app.db.as_ref().expect("db");
    match db
        .todo_list(&crate::db::todos::TodoScope::Project {
            project_id: "proj-1".to_string(),
        })
        .unwrap()
    {
        Some(list) => db.todos(&list.id).unwrap(),
        None => Vec::new(),
    }
}

fn action_editor(app: &App) -> &crate::app::LearningActionEditor {
    learning(app)
        .action_editor
        .as_ref()
        .expect("the confirmation is open")
}

/// The key that keeps an answer must not write anything by itself. This is
/// the one place in a mode that promises to change nothing where something
/// *is* written, so it happens on a second, explicit keypress or not at all.
#[test]
fn keeping_an_answer_writes_nothing_until_it_is_confirmed() {
    let (_repo, _db, mut app, id) = app_with_an_answer();

    app.learning_make_actionable();
    assert!(
        stored_todos(&app).is_empty(),
        "opening the confirmation wrote nothing"
    );
    assert!(action_editor(&app).qa_id == id);

    app.learning_cancel_action();
    assert!(
        stored_todos(&app).is_empty(),
        "and neither did walking away from it"
    );
    assert!(
        learning(&app).qa[0].todo_id.is_none(),
        "so the entry is not marked as kept"
    );
}

#[test]
fn a_kept_answer_lands_on_the_projects_todo_list() {
    let (_repo, _db, mut app, id) = app_with_an_answer();

    app.learning_make_actionable();
    let todo_id = app.learning_confirm_action().expect("wrote an item");

    let todos = stored_todos(&app);
    assert_eq!(todos.len(), 1);
    assert_eq!(todos[0].id, todo_id);
    assert_eq!(
        todos[0].title, "It runs the program.",
        "seeded from the answer's first line"
    );

    let body = todos[0].body.as_deref().unwrap_or_default();
    assert!(
        body.contains("src/main.rs:1-2"),
        "the note says where it came from: {body}"
    );
    assert!(
        body.contains("What does this do?"),
        "and what was asked: {body}"
    );
    assert!(
        body.contains("The body is empty for now."),
        "and enough of the answer to recognise it: {body}"
    );

    let row = learning(&app).qa.iter().find(|r| r.id == id).unwrap();
    assert_eq!(row.todo_id.as_deref(), Some(todo_id.as_str()));
    assert!(learning(&app).notice.is_some(), "and it says what it did");
}

/// A list with no TODOs session row is invisible from the dashboard, so a
/// note written into one would be a note the user can never find again.
#[test]
fn keeping_an_answer_makes_the_list_reachable_from_the_dashboard() {
    let (_repo, _db, mut app, _id) = app_with_an_answer();
    assert!(
        !app.store.projects[0]
            .features
            .iter()
            .any(|f| f.has_todos_session()),
        "no list to start with"
    );

    app.learning_make_actionable();
    app.learning_confirm_action().unwrap();

    assert!(
        app.store.projects[0]
            .features
            .iter()
            .any(|f| f.has_todos_session()),
        "the project now has a TODOs session to open the list from"
    );
}

/// The seeded title is a guess, so editing it is the expected path, not an
/// exception.
#[test]
fn the_title_you_type_is_the_one_that_is_written() {
    let (_repo, _db, mut app, _id) = app_with_an_answer();

    app.learning_make_actionable();
    if let AppMode::Learning(state) = &mut app.mode
        && let Some(editor) = &mut state.action_editor
    {
        editor.title = crate::editor::TextEditor::new("Work out why main is empty".to_string());
    }
    app.learning_confirm_action().unwrap();

    assert_eq!(stored_todos(&app)[0].title, "Work out why main is empty");
}

#[test]
fn a_note_with_no_title_says_so_instead_of_being_written() {
    let (_repo, _db, mut app, _id) = app_with_an_answer();

    app.learning_make_actionable();
    if let AppMode::Learning(state) = &mut app.mode
        && let Some(editor) = &mut state.action_editor
    {
        editor.title = crate::editor::TextEditor::new("   ".to_string());
    }
    assert!(app.learning_confirm_action().is_none());

    assert!(stored_todos(&app).is_empty());
    let editor = action_editor(&app);
    assert!(
        editor.error.as_deref().is_some_and(|e| e.contains("title")),
        "the refusal is raised inside the dialog, which covers the overlay's \
             banner line: {:?}",
        editor.error
    );
}

/// Pressing the key again on an entry that already produced an item opens
/// that item rather than paying for a duplicate.
#[test]
fn keeping_the_same_answer_twice_opens_the_item_you_already_have() {
    let (_repo, _db, mut app, _id) = app_with_an_answer();
    app.learning_make_actionable();
    let todo_id = app.learning_confirm_action().unwrap();

    app.learning_make_actionable();

    assert_eq!(stored_todos(&app).len(), 1, "no second item");
    let AppMode::Todos(state) = &app.mode else {
        panic!("expected the TODOs overlay, got another mode");
    };
    assert_eq!(
        state
            .focused()
            .and_then(|pane| pane.selected_todo())
            .map(|t| t.id.as_str()),
        Some(todo_id.as_str()),
        "with the cursor on the item this answer produced"
    );
}

/// The marker on a row is a promise the TODOs overlay can stop keeping: an
/// item can be deleted from over there. Jumping into an empty list would be
/// the swallowed keypress this mode is meant not to have.
#[test]
fn keeping_an_answer_whose_item_was_deleted_offers_a_new_one() {
    let (_repo, _db, mut app, id) = app_with_an_answer();
    app.learning_make_actionable();
    let todo_id = app.learning_confirm_action().unwrap();
    app.db.as_ref().unwrap().delete_todo(&todo_id).unwrap();

    app.learning_make_actionable();

    assert!(
        matches!(app.mode, AppMode::Learning(_)),
        "it stays put rather than opening a list the item has left"
    );
    let editor = action_editor(&app);
    assert_eq!(editor.qa_id, id);
    assert!(
        editor
            .error
            .as_deref()
            .is_some_and(|e| e.contains("deleted")),
        "and says why it is offering a new one: {:?}",
        editor.error
    );
    assert!(
        learning(&app).qa[0].todo_id.is_none(),
        "the dead link is dropped, so the row stops claiming an item"
    );
}

#[test]
fn a_kept_answer_is_still_marked_after_a_reopen() {
    let (_repo, _db, mut app, id) = app_with_an_answer();
    app.learning_make_actionable();
    let todo_id = app.learning_confirm_action().unwrap();

    app.close_learning_mode();
    app.open_learning_mode(0, 0).unwrap();

    let row = learning(&app).qa.iter().find(|r| r.id == id).unwrap();
    assert_eq!(row.todo_id.as_deref(), Some(todo_id.as_str()));
}

#[test]
fn an_answer_that_has_not_arrived_cannot_be_kept() {
    let (_repo, _db, mut app) = opened_app_with_db();
    app.learning_ask("What does this do?", LearningQaIntent::Explain, None)
        .unwrap();

    app.learning_make_actionable();

    assert!(learning(&app).action_editor.is_none());
    let error = learning(&app).error.clone().unwrap_or_default();
    assert!(error.contains("still generating"), "{error}");
    assert!(stored_todos(&app).is_empty());
}

#[test]
fn a_failed_question_says_to_ask_it_again_rather_than_keeping_nothing() {
    let (_repo, _db, mut app) = opened_app_with_db();
    let id = app
        .learning_ask("What does this do?", LearningQaIntent::Explain, None)
        .unwrap();
    deliver(&mut app, &id, Err("Claude couldn't answer".to_string()));

    app.learning_make_actionable();

    assert!(learning(&app).action_editor.is_none());
    let error = learning(&app).error.clone().unwrap_or_default();
    assert!(error.contains("Ask it again"), "{error}");
}

/// The Q&A history survives without a DB, but a TODO written into a list
/// nobody can open would not — so this one refuses out loud instead.
#[test]
fn nothing_is_kept_without_a_database() {
    let (_repo, mut app) = opened_app();
    let id = app
        .learning_ask("What does this do?", LearningQaIntent::Explain, None)
        .unwrap();
    deliver(&mut app, &id, Ok("It runs the program.".to_string()));

    app.learning_make_actionable();

    assert!(learning(&app).action_editor.is_none());
    let error = learning(&app).error.clone().unwrap_or_default();
    assert!(error.contains("database"), "{error}");
}

/// The confirmation banner is a single unwrapped line, so a sentence longer
/// than the pane loses its tail — and the tail here is the half that says
/// what was *not* written.
#[test]
fn the_confirmation_fits_on_one_line() {
    let (_repo, _db, mut app, _id) = app_with_an_answer();
    app.learning_make_actionable();
    app.learning_confirm_action().unwrap();

    let notice = learning(&app).notice.clone().unwrap();
    assert!(
        notice.contains("not a change"),
        "it has to say what it didn't do: {notice}"
    );
    assert!(
        notice.chars().count() <= 130,
        "banner is one unwrapped line at 140 columns, this is {} chars: {notice}",
        notice.chars().count()
    );
}

// ── the seeded note ──────────────────────────────────────

fn qa_with(answer: &str, intent: LearningQaIntent) -> LearningQa {
    LearningQa {
        id: "qa-1".to_string(),
        session_id: "sess-1".to_string(),
        parent_qa_id: None,
        deep_dive_of: None,
        file_path: Some("src/main.rs".to_string()),
        anchor: LearningAnchor::Lines { start: 4, end: 9 },
        selection_text: "fn main() {}".to_string(),
        selection_is_diff: false,
        question: "Why is this here?".to_string(),
        intent,
        level: LearningLevel::Newcomer,
        answer: Some(answer.to_string()),
        harness: AgentKind::Claude,
        run_mode: crate::app::LearningRunMode::NoTools,
        status: crate::app::LearningQaStatus::Answered,
        error: None,
        todo_id: None,
        spawned_session_id: None,
        created_at: String::new(),
        updated_at: String::new(),
    }
}

/// An action answer is written to lead with a one-line imperative summary,
/// so the title is already there — but it arrives wearing whatever markdown
/// the agent felt like, and a TODO title is rendered raw.
#[test]
fn a_change_proposals_lead_line_becomes_the_title_without_its_markup() {
    for lead in [
        "## Split `run_loop` into two functions",
        "**Split `run_loop` into two functions**",
        "- Split `run_loop` into two functions",
        "1. Split `run_loop` into two functions",
        "> Split `run_loop` into two functions",
    ] {
        let qa = qa_with(
            &format!("{lead}\n\nIt does two things at once."),
            LearningQaIntent::Action,
        );
        assert_eq!(
            todo_title_seed(&qa),
            "Split `run_loop` into two functions",
            "from {lead:?}"
        );
    }
}

/// A sentence that opens with a decimal number or a version string is not
/// an ordered-list item, and losing its first digits would rewrite what the
/// answer said.
#[test]
fn a_leading_number_is_only_a_list_marker_when_a_space_follows_it() {
    for lead in [
        "12.5 seconds is the default timeout.",
        "2.0 release adds the flag.",
        "3)x is the closing paren of a tuple index.",
    ] {
        let qa = qa_with(lead, LearningQaIntent::Explain);
        assert_eq!(todo_title_seed(&qa), lead, "from {lead:?}");
    }
}

#[test]
fn a_title_skips_blank_and_decoration_only_lines() {
    let qa = qa_with(
        "\n```\n\n# \n\nIt runs the program.",
        LearningQaIntent::Explain,
    );
    assert_eq!(todo_title_seed(&qa), "It runs the program.");
}

/// An explanation has no one-line summary in it, so the seed is a
/// truncation the user is expected to fix — it just has to be a legible
/// one, cut at a word rather than mid-word.
#[test]
fn an_explanations_title_is_a_truncation_you_can_edit() {
    let long = "This function is the entry point of the program, which means \
                    the operating system calls it first and everything else follows.";
    let qa = qa_with(long, LearningQaIntent::Explain);
    let title = todo_title_seed(&qa);

    assert!(title.ends_with('…'), "it says it was cut: {title}");
    assert!(title.chars().count() <= MAX_TODO_TITLE + 1, "{title}");
    assert!(
        !title.trim_end_matches('…').ends_with(' '),
        "cut at a word boundary, not mid-word or mid-space: {title}"
    );
    assert!(title.starts_with("This function is the entry point"));
}

/// An answer that opens with nothing usable still has to produce something
/// the user can recognise in a list.
#[test]
fn a_title_falls_back_to_the_question() {
    let mut qa = qa_with("", LearningQaIntent::Explain);
    qa.answer = Some("   \n\n".to_string());
    assert_eq!(todo_title_seed(&qa), "Why is this here?");
}

#[test]
fn the_note_says_where_in_the_project_it_came_from() {
    let mut qa = qa_with("It runs the program.", LearningQaIntent::Explain);
    assert_eq!(anchor_locator(&qa), "src/main.rs:4-9");

    qa.anchor = LearningAnchor::Lines { start: 4, end: 4 };
    assert_eq!(anchor_locator(&qa), "src/main.rs:4");

    qa.anchor = LearningAnchor::File;
    assert_eq!(anchor_locator(&qa), "src/main.rs");

    qa.anchor = LearningAnchor::Hunk { index: 1 };
    assert_eq!(anchor_locator(&qa), "src/main.rs (change #2)");

    qa.anchor = LearningAnchor::Project;
    qa.file_path = None;
    assert_eq!(anchor_locator(&qa), "the whole project");
}

/// The body is what a spawned agent receives verbatim
/// (`App::todo_spawn_prompt`), so it has to stand on its own — and it must
/// not bury the item it is attached to.
#[test]
fn a_long_answer_is_excerpted_into_the_note() {
    let answer: String = (0..40).map(|i| format!("line {i}\n")).collect();
    let qa = qa_with(&answer, LearningQaIntent::Explain);

    let body = todo_body(&qa, None);
    assert!(body.contains("src/main.rs:4-9"));
    assert!(body.contains("Why is this here?"));
    assert!(body.contains("line 0"));
    assert!(body.contains("line 11"));
    assert!(
        !body.contains("line 12"),
        "cut at the excerpt limit: {body}"
    );
    assert!(body.contains('…'), "and says it was cut: {body}");
    assert!(
        body.contains("answer began"),
        "phrased as an excerpt rather than the whole thing: {body}"
    );
}

// ── branch-changes context ───────────────────────────────

/// A repo whose branch changes one line deep inside a long file, so the
/// diff hunk and the file are very different sizes.
fn repo_with_a_small_change_in_a_big_file() -> TempDir {
    let repo = TempDir::new().unwrap();
    git(repo.path(), &["init", "--initial-branch=main"]);
    git(repo.path(), &["config", "user.name", "AMF Test"]);
    git(repo.path(), &["config", "user.email", "amf@example.com"]);
    std::fs::create_dir_all(repo.path().join("src")).unwrap();
    let base: String = (0..BIG_FILE_LINES)
        .map(|i| format!("fn line_{i}() {{}}\n"))
        .collect();
    std::fs::write(repo.path().join("src/big.rs"), &base).unwrap();
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-m", "initial"]);
    git(repo.path(), &["checkout", "-b", "my-feat"]);
    let changed = base.replace("fn line_30() {}", "fn line_30_renamed() {}");
    std::fs::write(repo.path().join("src/big.rs"), changed).unwrap();
    git(repo.path(), &["commit", "-am", "rename line 30"]);
    repo
}

const BIG_FILE_LINES: usize = 60;

/// Open the big-file repo in branch-changes scope with the changed file
/// loaded and every addressable diff line selected.
fn app_on_the_changed_file() -> (TempDir, App) {
    let repo = repo_with_a_small_change_in_a_big_file();
    let mut app = app_at(repo.path(), true);
    app.open_learning_mode(0, 0).unwrap();
    app.learning_toggle_scope();
    while learning(&app).content_path.as_deref() != Some("src/big.rs") {
        app.learning_select_next_entry();
    }
    (repo, app)
}

/// Browsing a diff must not narrow what the agent can see: the surrounding
/// file is hydrated from the snapshot, so a whole-file anchor really is the
/// whole file and a line anchor still has context around it.
#[test]
fn a_changed_file_carries_its_whole_file_not_just_the_hunks() {
    let (_repo, mut app) = app_on_the_changed_file();

    let state = learning(&app);
    assert_eq!(state.scope, BrowseScope::BranchChanges);
    assert_eq!(
        state.content.len(),
        BIG_FILE_LINES,
        "the snapshot's copy of the file, not only the diff rows"
    );
    assert!(
        state.selectable_line_count() < BIG_FILE_LINES,
        "the pane itself still addresses diff rows only"
    );

    // "Whole file" means the whole file, in this scope too.
    app.learning_select_whole_file();
    assert_eq!(
        app.learning_selection_text().lines().count(),
        BIG_FILE_LINES
    );

    // And a line selection gets surrounding context to sit in.
    app.learning_start_range();
    app.learning_cursor_move(1000);
    let ctx = app
        .learning_prompt_context("What changed here?", LearningQaIntent::Explain, Vec::new())
        .unwrap();
    assert_eq!(ctx.file_lines.len(), BIG_FILE_LINES);
    let prompt = build_prompt(&ctx);
    assert!(prompt.contains("Surrounding context"), "{prompt}");
    assert!(
        prompt.contains("fn line_0() {}"),
        "context reaches code the hunk never touched: {prompt}"
    );
}

/// A diff excerpt must stay readable *as* a diff: markers intact, and the
/// prompt saying what they mean, so an addition and the line it replaced
/// can't read as two adjacent source lines.
#[test]
fn a_diff_selection_keeps_its_markers_and_says_it_is_a_diff() {
    let (_repo, mut app) = app_on_the_changed_file();
    app.learning_start_range();
    app.learning_cursor_move(1000);

    let selection = app.learning_selection_text();
    assert!(
        selection
            .lines()
            .any(|l| l.starts_with("+fn line_30_renamed")),
        "the addition keeps its marker: {selection}"
    );
    assert!(
        selection.lines().any(|l| l.starts_with("-fn line_30()")),
        "and so does the line it replaced: {selection}"
    );

    let ctx = app
        .learning_prompt_context("What changed here?", LearningQaIntent::Explain, Vec::new())
        .unwrap();
    assert!(ctx.selection_is_diff);
    let prompt = build_prompt(&ctx);
    assert!(prompt.contains("unified diff"), "{prompt}");
    assert!(
        prompt.contains("+fn line_30_renamed() {}"),
        "quoted verbatim, with no line-number gutter to hide the marker: {prompt}"
    );
    assert!(
        !prompt.contains("--- The code they are asking about ---"),
        "a diff is never presented as plain source: {prompt}"
    );
}

/// Diff-ness is captured with the selection, not re-read at submit time:
/// following up after browsing back to the repo tree must still label the
/// parent's excerpt as a diff.
#[test]
fn a_follow_up_keeps_its_parents_diff_labelling_after_browsing_away() {
    let (_repo, mut app) = app_on_the_changed_file();
    app.learning_start_range();
    app.learning_cursor_move(1000);
    let parent = ask_and_answer(&mut app, "What changed here?", "A function was renamed.");
    assert!(
        learning(&app)
            .qa
            .iter()
            .find(|r| r.id == parent)
            .unwrap()
            .selection_is_diff
    );

    // Browse back to plain source before following up.
    app.learning_toggle_scope();
    assert_eq!(learning(&app).scope, BrowseScope::RepoTree);
    assert!(
        !learning(&app).selection_is_diff(),
        "the live cursor is on ordinary source now"
    );

    let child = follow_up(&mut app, "Why would you rename it?");
    let row = learning(&app).qa.iter().find(|r| r.id == child).unwrap();
    assert!(
        row.selection_is_diff,
        "the follow-up quotes its parent's diff, so it is still a diff"
    );
    assert!(row.selection_text.lines().any(|l| l.starts_with('+')));
}

// ── escalating to a live session ─────────────────────────

/// The seed is the whole point of escalation: a live agent that has to be
/// told everything again is no better than opening a session by hand.
#[test]
fn an_escalated_question_carries_where_what_and_the_answer() {
    let qa = qa_with(
        "It is the program's entry point.",
        LearningQaIntent::Explain,
    );

    let seed = escalation_seed(&qa, None);

    assert!(
        seed.contains("src/main.rs:4-9"),
        "where they were reading: {seed}"
    );
    assert!(seed.contains("fn main() {}"), "the code itself: {seed}");
    assert!(seed.contains("Why is this here?"), "the question: {seed}");
    assert!(
        seed.contains("It is the program's entry point."),
        "and what they were told: {seed}"
    );
}

/// A no-tools answer may name files that do not exist. Handing it over
/// without saying so would launder a guess into an established fact — the
/// live agent has tools, so it is told to check.
#[test]
fn a_shallow_answer_is_handed_over_with_its_limits_stated() {
    let shallow = qa_with("Look at src/nonexistent.rs.", LearningQaIntent::Explain);
    let seed = escalation_seed(&shallow, None);
    assert!(
        seed.contains("could only see the excerpt"),
        "the live agent has to know what this answer was worth: {seed}"
    );

    let mut deep = qa_with("Look at src/main.rs.", LearningQaIntent::Explain);
    deep.run_mode = crate::app::LearningRunMode::DeepDive;
    let seed = escalation_seed(&deep, None);
    assert!(
        !seed.contains("could only see the excerpt"),
        "this one did read the repository: {seed}"
    );
    assert!(seed.contains("read-only access"), "{seed}");
}

/// The two intents ask for different things: one continues a conversation,
/// the other requests work.
#[test]
fn the_seed_asks_for_what_the_entry_was_filed_as() {
    let explain = escalation_seed(&qa_with("It parses argv.", LearningQaIntent::Explain), None);
    assert!(explain.contains("carry on"), "{explain}");
    assert!(
        !explain.to_lowercase().contains("make that change"),
        "an explanation must not turn into a work order: {explain}"
    );

    let action = escalation_seed(
        &qa_with("Split this function.", LearningQaIntent::Action),
        None,
    );
    assert!(action.contains("make that change"), "{action}");

    // Whichever it is, the last thing on screen when the composer opens
    // says that this session is not bound by Learning Mode's promise. The
    // composer scrolls to the end, so the tail is the only part guaranteed
    // to be read before Enter.
    for seed in [&explain, &action] {
        assert!(
            seed.trim_end().ends_with("before you do it.")
                || seed.trim_end().ends_with("before you change anything."),
            "the boundary has to be the closing line: {seed}"
        );
        assert!(seed.contains("you can change files here"), "{seed}");
    }
}

/// The user escalating at newcomer level is the one least able to read a
/// silent diff, so the seed asks the live agent to narrate.
#[test]
fn a_newcomer_seed_asks_the_live_agent_to_explain_itself() {
    let mut qa = qa_with("It parses argv.", LearningQaIntent::Action);
    assert_eq!(qa.level, LearningLevel::Newcomer);
    assert!(escalation_seed(&qa, None).contains("new to this codebase"));

    qa.level = LearningLevel::Familiar;
    let seed = escalation_seed(&qa, None);
    assert!(
        !seed.contains("new to this codebase"),
        "someone who switched to familiar asked for the denser version: {seed}"
    );
}

/// A failed run is exactly when a live agent is worth reaching for, so the
/// row is escalatable — and the seed says there was no answer rather than
/// leaving a gap that reads as one.
#[test]
fn escalating_a_failed_question_says_there_was_no_answer() {
    let mut qa = qa_with("", LearningQaIntent::Explain);
    qa.answer = None;
    qa.status = crate::app::LearningQaStatus::Failed;
    qa.error = Some("claude: command not found".to_string());

    let seed = escalation_seed(&qa, None);

    assert!(seed.contains("Why is this here?"), "the question survives");
    assert!(seed.contains("never got an answer"), "{seed}");
}

#[test]
fn a_long_answer_is_excerpted_into_the_seed() {
    let long: String = (1..=200).map(|n| format!("line {n}\n")).collect::<String>();
    let qa = qa_with(&long, LearningQaIntent::Explain);

    let seed = escalation_seed(&qa, None);

    assert!(seed.contains("line 1\n"));
    assert!(!seed.contains("line 200"), "the tail is cut: {seed}");
    assert!(
        seed.contains("more lines not shown"),
        "and the cut is marked, so nothing reads as the whole answer: {seed}"
    );
}

#[test]
fn a_diff_selection_is_handed_over_as_a_diff() {
    let mut qa = qa_with("A function was renamed.", LearningQaIntent::Explain);
    qa.selection_is_diff = true;
    qa.selection_text = "-fn old() {}\n+fn new() {}".to_string();

    let seed = escalation_seed(&qa, None);

    assert!(seed.contains("unified diff"), "{seed}");
    assert!(seed.contains("+fn new() {}"), "markers survive: {seed}");
}

/// The label names the code, not the question: a session list full of
/// truncated questions is a session list you cannot scan.
#[test]
fn the_session_is_labelled_with_the_code_it_is_about() {
    let qa = qa_with("It is the entry point.", LearningQaIntent::Explain);
    assert_eq!(learning_session_label(&qa), "Learning: src/main.rs:4-9");

    let mut deep = qa_with("x", LearningQaIntent::Explain);
    deep.file_path = Some("src/app/some/deeply/nested/module.rs".to_string());
    deep.anchor = LearningAnchor::File;
    let label = learning_session_label(&deep);
    assert!(label.starts_with("Learning: …"), "{label}");
    assert!(
        label.ends_with("nested/module.rs"),
        "the tail is what identifies a path: {label}"
    );
}

/// Shared with `crate::handlers::learning`'s tests: an overlay that can
/// really launch a session, for the `S` key.
pub(crate) fn launchable_app_for_handlers() -> (TempDir, TempDir, App) {
    opened_app_that_can_launch()
}

/// An overlay whose tmux is mocked well enough to actually launch — the one
/// Learning Mode action that starts anything.
fn opened_app_that_can_launch() -> (TempDir, TempDir, App) {
    let repo = repo_with_branch_change();
    let db_dir = TempDir::new().unwrap();

    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().return_const(true);
    // A linked session is only reusable while its window is still there.
    tmux.expect_window_exists().return_const(true);
    tmux.expect_create_window().returning(|_, _, _| Ok(()));
    tmux.expect_launch_claude()
        .returning(|_, _, _, _, _| Ok(()));
    tmux.expect_resize_pane().returning(|_, _, _, _| Ok(()));
    tmux.expect_select_window().returning(|_, _| Ok(()));

    let mut app = App::new_for_test(
        store_at(repo.path(), true),
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );
    // Both gates off: what is under test is the escalation, not the
    // resource warning it would otherwise raise on a loaded machine.
    app.config.max_concurrent_agents = 0;
    app.config.low_memory_warn_mb = 0;
    app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());
    app.open_learning_mode(0, 0).unwrap();
    while learning(&app).content_path.as_deref() != Some("src/main.rs") {
        app.learning_select_next_entry();
    }
    (repo, db_dir, app)
}

fn launchable_with_an_answer() -> (TempDir, TempDir, App, String) {
    let (repo, db, mut app) = opened_app_that_can_launch();
    let id = app
        .learning_ask("What does this do?", LearningQaIntent::Explain, None)
        .unwrap();
    deliver(&mut app, &id, Ok("It is the entry point.".to_string()));
    (repo, db, app, id)
}

/// The whole contract of this key: a session exists, the prompt is in the
/// composer, and **nothing has been sent**. Learning Mode's promise is that
/// it changes nothing, and this is the one door out of that — so the door
/// has to open onto something the user reads before it acts.
#[test]
fn escalating_opens_a_session_with_the_prompt_filled_in_and_unsent() {
    let (_repo, _db, mut app, id) = launchable_with_an_answer();

    let session_id = app.learning_escalate().expect("a session was started");

    let sessions = &app.store.projects[0].features[0].sessions;
    assert_eq!(sessions.len(), 1, "exactly one session was created");
    assert_eq!(sessions[0].id, session_id);
    assert!(sessions[0].kind.is_agent_harness());
    assert!(
        sessions[0].label.starts_with("Learning:"),
        "got {}",
        sessions[0].label
    );

    // The prompt is sitting in the composer, unsent: the seed is the
    // editor's text, and only Enter would hand it over.
    match &app.mode {
        AppMode::Compose(state) => {
            let text = state.editor.text();
            assert!(text.contains("What does this do?"), "{text}");
            assert!(text.contains("It is the entry point."), "{text}");
            // The composer opens with the cursor after the last line, so
            // the tail is what is on screen — which is where the boundary
            // this key crosses has to be stated.
            assert!(
                text.trim_end().ends_with("before you change anything."),
                "the last thing they see says this session can change files: {text}"
            );
        }
        other => panic!(
            "expected the composer, got {:?}",
            std::mem::discriminant(other)
        ),
    }

    // Reopening finds the link, so the entry renders as `→ session`.
    app.open_learning_mode(0, 0).unwrap();
    let row = learning(&app)
        .qa
        .iter()
        .find(|r| r.id == id)
        .expect("still in history")
        .clone();
    assert_eq!(row.spawned_session_id.as_deref(), Some(session_id.as_str()));
}

/// A second press must not pay for a second agent: the conversation it
/// would start already exists.
#[test]
fn a_second_escalation_returns_to_the_session_you_already_have() {
    let (_repo, _db, mut app, _id) = launchable_with_an_answer();
    let first = app.learning_escalate().unwrap();

    // Back to the overlay, cursor on the same row.
    app.cancel_compose();
    app.open_learning_mode(0, 0).unwrap();
    let again = app.learning_escalate().unwrap();

    assert_eq!(again, first, "the same session");
    assert_eq!(
        app.store.projects[0].features[0].sessions.len(),
        1,
        "and no second one was created"
    );
    assert!(
        matches!(app.mode, AppMode::Viewing(_)),
        "it jumps into the session rather than re-seeding it"
    );
    assert!(
        app.toasts
            .iter()
            .any(|t| t.message.contains("already opened a session")),
        "the screen changed under a keypress that looked like it would start \
             something, so it has to say why it didn't"
    );
}

/// `→ session` is a promise the session list can stop keeping. Jumping into
/// a session that no longer exists is the swallowed keypress this mode is
/// built not to have.
#[test]
fn escalating_after_the_session_was_removed_starts_a_new_one() {
    let (_repo, _db, mut app, id) = launchable_with_an_answer();
    let first = app.learning_escalate().unwrap();

    app.cancel_compose();
    app.store.projects[0].features[0].sessions.clear();
    app.open_learning_mode(0, 0).unwrap();
    let second = app.learning_escalate().unwrap();

    assert_ne!(second, first, "a fresh session, not the dead link");
    assert_eq!(app.store.projects[0].features[0].sessions.len(), 1);
    // Through `message`, not a toast: the composer is the mode now, and it
    // draws no toasts — the pane promotes this the moment they step back.
    assert!(
        app.message
            .as_deref()
            .is_some_and(|m| m.contains("is gone — this is a new one")),
        "which of the two happened has to be said: {:?}",
        app.message
    );

    app.cancel_compose();
    app.open_learning_mode(0, 0).unwrap();
    let row = learning(&app).qa.iter().find(|r| r.id == id).unwrap();
    assert_eq!(row.spawned_session_id.as_deref(), Some(second.as_str()));
}

/// The record outliving the agent is the ordinary case — an agent that quit,
/// a window killed from tmux — and it looks exactly like a live link from
/// the store alone. Jumping into a dead pane is the same swallowed keypress
/// as jumping into a removed one.
#[test]
fn escalating_after_the_agent_exited_starts_a_new_one() {
    let (_repo, _db, mut app, id) = launchable_with_an_answer();
    let first = app.learning_escalate().unwrap();
    app.cancel_compose();

    // The feature is still running — other windows are alive — but the
    // window this answer opened is not.
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().return_const(true);
    tmux.expect_window_exists().return_const(false);
    tmux.expect_create_window().returning(|_, _, _| Ok(()));
    tmux.expect_launch_claude()
        .returning(|_, _, _, _, _| Ok(()));
    tmux.expect_resize_pane().returning(|_, _, _, _| Ok(()));
    tmux.expect_select_window().returning(|_, _| Ok(()));
    app.tmux = Box::new(tmux);

    app.open_learning_mode(0, 0).unwrap();
    let second = app.learning_escalate().unwrap();

    assert_ne!(second, first, "a fresh session, not the dead window");
    assert!(
        app.message
            .as_deref()
            .is_some_and(|m| m.contains("is gone — this is a new one")),
        "which of the two happened has to be said: {:?}",
        app.message
    );

    app.cancel_compose();
    app.open_learning_mode(0, 0).unwrap();
    let row = learning(&app).qa.iter().find(|r| r.id == id).unwrap();
    assert_eq!(row.spawned_session_id.as_deref(), Some(second.as_str()));
}

/// "Nothing was changed" has to be true. A launch that dies partway used to
/// leave the session record behind, so the tree showed a session with no
/// agent in it and the next press started yet another.
#[test]
fn a_failed_launch_leaves_no_session_behind() {
    let (_repo, _db, mut app, id) = launchable_with_an_answer();

    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().return_const(true);
    tmux.expect_window_exists().return_const(true);
    tmux.expect_create_window()
        .returning(|_, _, _| Ok(()))
        .times(1);
    tmux.expect_launch_claude()
        .returning(|_, _, _, _, _| anyhow::bail!("no claude here"));
    // The window got as far as existing, so the rollback takes it out.
    tmux.expect_kill_window().returning(|_, _| Ok(())).times(1);
    app.tmux = Box::new(tmux);

    assert!(app.learning_escalate().is_none());

    assert!(
        app.store.projects[0].features[0].sessions.is_empty(),
        "the session record is rolled back with the failed launch"
    );
    let row = learning(&app).qa.iter().find(|r| r.id == id).unwrap();
    assert_eq!(
        row.spawned_session_id, None,
        "and nothing is linked, so the next press starts one rather than \
             opening a session that was never created"
    );
    let error = learning(&app).error.clone().unwrap_or_default();
    assert!(error.contains("nothing was changed"), "{error}");
}

/// Two agents on the same question at once is worth refusing; the refusal
/// says when to come back.
#[test]
fn escalating_a_question_still_generating_says_to_wait() {
    let (_repo, _db, mut app) = opened_app_that_can_launch();
    app.learning_ask("What does this do?", LearningQaIntent::Explain, None)
        .unwrap();

    assert!(app.learning_escalate().is_none());

    assert!(app.store.projects[0].features[0].sessions.is_empty());
    let error = learning(&app).error.clone().unwrap_or_default();
    assert!(error.contains("still generating"), "{error}");
}

#[test]
fn escalating_with_nothing_asked_says_so() {
    let (_repo, _db, mut app) = opened_app_that_can_launch();

    assert!(app.learning_escalate().is_none());

    assert!(app.store.projects[0].features[0].sessions.is_empty());
    let error = learning(&app).error.clone().unwrap_or_default();
    assert!(error.contains("Ask something first"), "{error}");
}
