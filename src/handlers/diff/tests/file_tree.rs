use super::*;

#[test]
fn i_opens_syntax_picker_for_selected_diff_file() {
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
            VibeMode::Vibe,
            false,
        ),
        PathBuf::from("/tmp/project"),
    );
    state.layout = DiffViewerLayout::Unified;
    state.files = vec![DiffFile {
        old_path: None,
        path: "src/main.rs".into(),
        status: DiffFileStatus::Modified,
        additions: 1,
        deletions: 1,
        is_binary: false,
        old_content: None,
        new_content: None,
        patch: String::new(),
        hunks: vec![],
    }];
    app.mode = AppMode::DiffViewer(state);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('i'))).unwrap();

    match &app.mode {
        AppMode::SyntaxLanguagePicker(state) => {
            assert_eq!(
                state.languages[state.selected].language,
                crate::highlight::HighlightLanguage::Rust
            );
        }
        other => panic!(
            "expected syntax picker, got {:?}",
            std::mem::discriminant(other)
        ),
    }
}
#[test]
fn file_list_j_k_walk_tree_rows_and_z_folds_a_directory() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["README.md", "src/a.rs", "src/b.rs"]);

    // Rows: README.md, dir src, a.rs, b.rs. j from the first file lands on
    // the directory header without changing the diffed file.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap();
    let state = match &app.mode {
        AppMode::DiffViewer(state) => state,
        _ => panic!("left the diff viewer"),
    };
    assert_eq!(state.tree_cursor_dir.as_deref(), Some("src"));
    assert_eq!(state.selected_file, 0);

    // z folds it; the two files beneath lose their rows.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('z'))).unwrap();
    let state = match &app.mode {
        AppMode::DiffViewer(state) => state,
        _ => panic!("left the diff viewer"),
    };
    assert!(state.collapsed_dirs.contains("src"));
    assert_eq!(state.file_tree_rows().len(), 2);

    // n keeps its file meaning and re-expands what it lands inside.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('n'))).unwrap();
    let state = match &app.mode {
        AppMode::DiffViewer(state) => state,
        _ => panic!("left the diff viewer"),
    };
    assert_eq!(state.selected_file, 1);
    assert!(state.collapsed_dirs.is_empty());
}

#[test]
fn tree_fold_keys_are_ignored_while_the_patch_is_focused() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["src/a.rs"]);
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
    assert_eq!(app.diff_viewer_focus(), Some(DiffViewerFocus::Patch));

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('z'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('Z'))).unwrap();

    let state = match &app.mode {
        AppMode::DiffViewer(state) => state,
        _ => panic!("left the diff viewer"),
    };
    assert!(
        state.collapsed_dirs.is_empty(),
        "folding belongs to the file list, not the patch panel"
    );
}
/// The tree is not a review-mode feature: the plain diff viewer (leader d)
/// shares `draw_file_list` and the same fold bindings, so it groups and
/// folds identically.
#[test]
fn plain_diff_viewer_gets_the_tree_and_fold_keys_too() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["README.md", "src/a.rs", "src/ui/b.rs"]);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.review = false;
    }

    let rows = match &app.mode {
        AppMode::DiffViewer(state) => state.file_tree_rows(),
        _ => panic!("left the diff viewer"),
    };
    assert_eq!(
        rows.len(),
        5,
        "expected README + src/ + a.rs + ui/ + b.rs outside review mode: {rows:?}"
    );

    // j walks onto the `src` directory row, z folds it.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('z'))).unwrap();
    let state = match &app.mode {
        AppMode::DiffViewer(state) => state,
        _ => panic!("left the diff viewer"),
    };
    assert!(state.collapsed_dirs.contains("src"));
    assert_eq!(state.file_tree_rows().len(), 2);
    assert_eq!(
        state.selected_file, 0,
        "folding must not change the previewed file outside review mode either"
    );
}

/// A review over `TWO_FILE_PATCH` with one kept comment on the added line
/// (addressable index 2) of each file.

fn amf_entry(path: &str, status: DiffFileStatus, new: &str) -> DiffFile {
    DiffFile {
        old_path: None,
        path: path.to_string(),
        status,
        additions: new.lines().count(),
        deletions: 0,
        is_binary: false,
        old_content: None,
        new_content: Some(new.to_string()),
        patch: String::new(),
        hunks: vec![],
    }
}

/// The sweep's fresh-repo diff, in `load_snapshot`'s path order: four of
/// AMF's files sorted ahead of the user's README edit, plus a committed
/// `.claude/settings.json` the user changed, which must stay theirs.
fn sweep_files() -> Vec<DiffFile> {
    let mut readme = amf_entry("README.md", DiffFileStatus::Modified, "edited\n");
    readme.old_content = Some("# Demo\n".into());
    let mut settings = amf_entry(".claude/settings.json", DiffFileStatus::Modified, "{}\n");
    settings.old_content = Some("{\"a\":1}\n".into());
    vec![
        amf_entry(
            ".claude/.gitignore",
            DiffFileStatus::Untracked,
            "notifications/\nlatest-prompt.txt\n",
        ),
        amf_entry(
            ".claude/amf-hook-state.json",
            DiffFileStatus::Untracked,
            "{}\n",
        ),
        settings,
        amf_entry(
            ".claude/settings.local.json",
            DiffFileStatus::Untracked,
            &"{}\n".repeat(107),
        ),
        amf_entry(".gitignore", DiffFileStatus::Untracked, "CLAUDE.local.md\n"),
        readme,
    ]
}

fn plain_viewer_with(files: Vec<DiffFile>) -> App {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &[]);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.review = false;
        state.files = files;
        state.group_amf_files();
        state.selected_file = 0;
    }
    app
}

fn viewer(app: &App) -> &DiffViewerState {
    match &app.mode {
        AppMode::DiffViewer(state) => state,
        _ => panic!("left the diff viewer"),
    }
}

fn row_labels(state: &DiffViewerState) -> Vec<String> {
    state
        .file_tree_rows()
        .into_iter()
        .map(|row| match row {
            crate::app::FileTreeRow::Dir {
                label, collapsed, ..
            } => format!("{}{label}", if collapsed { "▸" } else { "▾" }),
            crate::app::FileTreeRow::File { name, .. } => name,
        })
        .collect()
}

#[test]
fn amf_files_sort_after_the_users_under_one_folded_row() {
    let app = plain_viewer_with(sweep_files());
    let state = viewer(&app);
    let order: Vec<&str> = state.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        order,
        vec![
            ".claude/settings.json",
            "README.md",
            ".claude/.gitignore",
            ".claude/amf-hook-state.json",
            ".claude/settings.local.json",
            ".gitignore",
        ],
        "the reviewer's files first, AMF's after, each in path order"
    );
    assert_eq!(state.amf_file_count, 4);
    assert_eq!(state.amf_files_start(), 2);
    assert_eq!(
        row_labels(state),
        vec!["▾.claude", "settings.json", "README.md", "▸AMF files"],
        "a tracked .claude/settings.json is an ordinary change"
    );
    assert_eq!(state.selected_file, 0, "the first file shown is the user's");
}

#[test]
fn dot_shows_and_folds_the_amf_files_from_either_panel() {
    let mut app = plain_viewer_with(sweep_files());
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('.'))).unwrap();
    assert_eq!(
        row_labels(viewer(&app)),
        vec![
            "▾.claude",
            "settings.json",
            "README.md",
            "▾AMF files",
            ".claude/.gitignore",
            ".claude/amf-hook-state.json",
            ".claude/settings.local.json",
            ".gitignore",
        ]
    );
    assert!(
        app.message
            .as_deref()
            .unwrap()
            .contains("Showing 4 AMF files")
    );

    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
    assert_eq!(app.diff_viewer_focus(), Some(DiffViewerFocus::Patch));
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('.'))).unwrap();
    assert_eq!(row_labels(viewer(&app)).last().unwrap(), "▸AMF files");
    assert!(
        app.message
            .as_deref()
            .unwrap()
            .contains("Folded 4 AMF files")
    );
}

#[test]
fn the_amf_row_folds_like_a_directory_and_navigation_reveals_it() {
    let mut app = plain_viewer_with(sweep_files());
    // j walks the rows down onto the folded group; l opens it.
    for _ in 0..3 {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap();
    }
    assert_eq!(
        viewer(&app).tree_cursor_dir.as_deref(),
        Some(crate::app::AMF_GROUP_KEY)
    );
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('l'))).unwrap();
    assert!(
        !viewer(&app)
            .collapsed_dirs
            .contains(crate::app::AMF_GROUP_KEY)
    );
    // h on one of its files steps out to the group row, z folds it again.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap();
    assert_eq!(viewer(&app).selected_file, 2);
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('h'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('z'))).unwrap();
    let state = viewer(&app);
    assert!(state.collapsed_dirs.contains(crate::app::AMF_GROUP_KEY));
    assert_eq!(state.selected_file, 2, "folding never changes the patch");
    let rows = state.file_tree_rows();
    assert!(matches!(
        &rows[state.tree_cursor_row(&rows).unwrap()],
        crate::app::FileTreeRow::Dir { path, .. } if path == crate::app::AMF_GROUP_KEY
    ));

    // n from the user's last file lands in the group and opens it.
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.selected_file = 1;
        state.on_file_changed();
    }
    app.diff_viewer_select_next_file();
    let state = viewer(&app);
    assert_eq!(state.files[state.selected_file].path, ".claude/.gitignore");
    assert!(!state.collapsed_dirs.contains(crate::app::AMF_GROUP_KEY));
}

/// `Z` keeps its meaning — the first press folds the tree — even though
/// the AMF row starts folded, and it never changes that row: `.` does.
#[test]
fn shift_z_folds_the_tree_first_and_leaves_the_amf_group_alone() {
    let mut app = plain_viewer_with(sweep_files());
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('Z'))).unwrap();
    assert_eq!(
        row_labels(viewer(&app)),
        vec!["▸.claude", "README.md", "▸AMF files"],
        "the first Z folds, as it does without AMF files"
    );
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('Z'))).unwrap();
    assert_eq!(
        row_labels(viewer(&app)),
        vec!["▾.claude", "settings.json", "README.md", "▸AMF files"],
        "unfolding the tree keeps the AMF group folded"
    );

    // Shown with `.`, the group stays shown through both presses.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('.'))).unwrap();
    for _ in 0..2 {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('Z'))).unwrap();
        assert!(
            !viewer(&app)
                .collapsed_dirs
                .contains(crate::app::AMF_GROUP_KEY)
        );
    }
}

/// Final Review keeps asking for a verdict on AMF's files (a deliberate
/// choice), so the finish prompt says how many of the undecided are
/// AMF's, and `u` reaches them through the folded row.
#[test]
fn finishing_final_review_names_the_undecided_amf_files_and_u_reveals_them() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &[]);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files = sweep_files();
        state.group_amf_files();
        state.selected_file = 0;
    }
    // Verdicts on both of the reviewer's files.
    app.diff_review_approve_current();
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.selected_file = 1;
    }
    app.diff_review_approve_current();
    app.confirm_or_finish_review().unwrap();
    let message = app.message.clone().unwrap_or_default();
    assert!(
        message.starts_with("4 file(s) (4 generated by AMF) have no verdict"),
        "{message}"
    );
    assert!(viewer(&app).finish_confirm);

    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.selected_file = 1;
    }
    app.diff_review_jump_next_undecided();
    let state = viewer(&app);
    assert!(state.is_amf_file(state.selected_file));
    assert!(
        !state.collapsed_dirs.contains(crate::app::AMF_GROUP_KEY),
        "jumping to an undecided AMF file unfolds its row"
    );
}

#[test]
fn shift_z_with_only_amf_files_has_no_tree_to_fold() {
    let only_amf: Vec<DiffFile> = sweep_files()
        .into_iter()
        .filter(|f| f.status == DiffFileStatus::Untracked)
        .collect();
    let mut app = plain_viewer_with(only_amf);
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('Z'))).unwrap();
    assert!(viewer(&app).collapsed_dirs.is_empty());
    assert_eq!(app.message.as_deref(), Some("No directories to collapse"));
}

/// A failed reload lists nothing, so it must not keep the previous
/// load's AMF count (the `.` hint, the split header).
#[test]
fn a_failed_reload_forgets_the_amf_count() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = plain_viewer_with(sweep_files());
    if let AppMode::DiffViewer(state) = &mut app.mode {
        assert_eq!(state.amf_file_count, 4);
        state.workdir = dir.path().join("not-a-repo");
    }
    app.refresh_diff_viewer();
    app.complete_diff_viewer_loading();
    let state = viewer(&app);
    assert!(state.error.is_some(), "the load should fail outside a repo");
    assert!(state.files.is_empty());
    assert_eq!(state.amf_file_count, 0);
}

#[test]
fn a_refresh_keeps_the_reviewers_choice_for_the_amf_group() {
    let mut app = plain_viewer_with(sweep_files());
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('.'))).unwrap();
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files = sweep_files();
        state.group_amf_files();
    }
    assert!(
        !viewer(&app)
            .collapsed_dirs
            .contains(crate::app::AMF_GROUP_KEY)
    );
}

#[test]
fn with_only_amf_files_the_group_starts_open() {
    let only_amf: Vec<DiffFile> = sweep_files()
        .into_iter()
        .filter(|f| f.status == DiffFileStatus::Untracked)
        .collect();
    let app = plain_viewer_with(only_amf);
    let state = viewer(&app);
    assert_eq!(state.amf_file_count, 4);
    assert_eq!(row_labels(state).first().unwrap(), "▾AMF files");
}

#[test]
fn dot_says_so_when_there_are_no_amf_files() {
    let mut app = plain_viewer_with(vec![amf_entry(
        "README.md",
        DiffFileStatus::Untracked,
        "x\n",
    )]);
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('.'))).unwrap();
    assert_eq!(viewer(&app).amf_file_count, 0);
    assert!(
        app.message
            .as_deref()
            .unwrap()
            .contains("No AMF-generated files")
    );
}

/// The real load path, on a real repository: what `r` and opening the
/// viewer run.
#[test]
fn loading_a_fresh_repo_groups_what_amf_wrote() {
    let repo = tempfile::TempDir::new().unwrap();
    let dir = repo.path();
    run_git(dir, &["init", "-q", "--initial-branch=main"]);
    run_git(dir, &["config", "user.name", "AMF Test"]);
    run_git(dir, &["config", "user.email", "amf@example.com"]);
    // A developer's global excludes often ignore `settings.local.json`.
    run_git(dir, &["config", "core.excludesFile", "/dev/null"]);
    std::fs::write(dir.join("README.md"), "# Demo\n").unwrap();
    run_git(dir, &["add", "README.md"]);
    run_git(dir, &["commit", "-q", "-m", "base"]);
    std::fs::create_dir_all(dir.join(".claude")).unwrap();
    std::fs::write(dir.join(".claude/settings.local.json"), "{}\n").unwrap();
    std::fs::write(dir.join(".claude/amf-hook-state.json"), "{}\n").unwrap();
    std::fs::write(dir.join(".gitignore"), "CLAUDE.local.md\n").unwrap();
    std::fs::write(dir.join("README.md"), "# Demo\nEdited\n").unwrap();

    let mut app = make_review_app(dir, &[]);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.review = false;
    }
    app.refresh_diff_viewer();
    app.complete_diff_viewer_loading();
    let state = viewer(&app);
    assert_eq!(state.error, None);
    assert_eq!(state.files[0].path, "README.md");
    assert_eq!(state.amf_file_count, 3);
    assert_eq!(row_labels(state), vec!["README.md", "▸AMF files"]);
}
