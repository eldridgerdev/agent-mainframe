use super::support::*;
use crate::app::util::{shorten_path, slugify, slugify_shortened};
use crate::app::*;
use crate::project::{AgentKind, Feature, Project, SessionKind};
use crate::traits::{MockTmuxOps, MockWorktreeOps};
use chrono::{Duration, Utc};
use crossterm::event::KeyCode;
use std::collections::HashMap;

#[test]
fn startup_harness_setup_allows_quit_with_q() {
    let store = ProjectStore {
        version: 5,
        projects: vec![],
        session_bookmarks: vec![],
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
        extra: HashMap::new(),
    };
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    app.open_harness_setup(true);
    crate::handlers::handle_harness_setup_key(&mut app, KeyCode::Char('q')).unwrap();

    assert!(app.should_quit);
}

#[test]
fn startup_mask_expires_without_snapshot_updates() {
    let mut view = ViewState::new(
        "my-project".to_string(),
        "my-feat".to_string(),
        "amf-my-feat".to_string(),
        "codex-2".to_string(),
        "Codex 2".to_string(),
        SessionKind::Codex,
        VibeMode::default(),
        false,
    );
    view.show_startup_mask();
    assert!(view.startup_mask_active());

    view.startup_mask_started_at = Some(Instant::now() - STARTUP_MASK_MAX_DURATION);
    assert!(!view.startup_mask_active());
}

// ── slugify ───────────────────────────────────────────────

#[test]
fn slugify_spaces_become_hyphens() {
    assert_eq!(slugify("hello world"), "hello-world");
}

#[test]
fn slugify_special_chars_become_hyphens() {
    assert_eq!(slugify("foo/bar.baz"), "foo-bar-baz");
}

#[test]
fn slugify_consecutive_hyphens_collapsed() {
    assert_eq!(slugify("foo--bar"), "foo-bar");
}

#[test]
fn slugify_empty_input() {
    assert_eq!(slugify(""), "");
}

#[test]
fn slugify_all_specials() {
    assert_eq!(slugify("!@#$%"), "");
}

#[test]
fn slugify_shortened_cuts_on_a_word_boundary() {
    assert_eq!(
        slugify_shortened("add a way to shorten the feature branch title", 20),
        "add-a-way-to-shorten"
    );
}

#[test]
fn slugify_shortened_leaves_a_slug_that_already_fits() {
    assert_eq!(slugify_shortened("hello world", 20), "hello-world");
    assert_eq!(slugify_shortened("hello world", 11), "hello-world");
}

/// No word boundary to cut on: a hard cut beats a name that is still too long.
#[test]
fn slugify_shortened_hard_cuts_a_single_oversized_word() {
    assert_eq!(slugify_shortened("supercalifragilistic", 5), "super");
}

/// A cut must never leave a trailing or doubled separator behind.
#[test]
fn slugify_shortened_never_leaves_a_dangling_separator() {
    for max in 1..40 {
        let slug = slugify_shortened("alpha beta gamma delta epsilon zeta", max);
        assert!(slug.chars().count() <= max, "max={max} got {slug}");
        assert!(
            !slug.starts_with('-') && !slug.ends_with('-'),
            "max={max} got {slug}"
        );
        assert!(!slug.contains("--"), "max={max} got {slug}");
    }
}

/// Nothing sluggable in, nothing out — the callers that need a fallback name
/// check for the empty string.
#[test]
fn slugify_shortened_of_an_unsluggable_title_is_empty() {
    assert_eq!(slugify_shortened("!!! ???", 20), "");
}

#[test]
fn slugify_preserves_hyphens() {
    assert_eq!(slugify("my-feature"), "my-feature");
}

// ── shorten_path ──────────────────────────────────────────

#[test]
fn shorten_path_inside_home() {
    if let Some(home) = dirs::home_dir() {
        let path = home.join("projects").join("my-app");
        let result = shorten_path(&path);
        assert_eq!(result, "~/projects/my-app");
    }
}

#[test]
fn shorten_path_outside_home() {
    let path = std::path::Path::new("/tmp/some/path");
    let result = shorten_path(path);
    assert_eq!(result, "/tmp/some/path");
}

#[test]
fn visible_items_prioritizes_non_worktree_features() {
    let now = Utc::now();
    let project = Project {
        id: "proj-1".to_string(),
        name: "my-project".to_string(),
        repo: PathBuf::from("/tmp/test-repo"),
        collapsed: false,
        features: vec![
            Feature {
                id: "feat-worktree".to_string(),
                name: "worktree-newer".to_string(),
                branch: "worktree-newer".to_string(),
                workdir: PathBuf::from("/tmp/test-repo/.worktrees/worktree-newer"),
                is_worktree: true,
                tmux_session: "amf-worktree-newer".to_string(),
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
                created_at: now + Duration::minutes(1),
                last_accessed: now + Duration::minutes(1),
                summary: None,
                summary_updated_at: None,
                nickname: None,
                selected_plan_path: Some(PathBuf::from(
                    "/tmp/test-repo/.worktrees/worktree-newer/docs/accepted.md",
                )),
                triage_source: None,
                review_source: None,
            },
            Feature {
                id: "feat-repo".to_string(),
                name: "repo-older".to_string(),
                branch: "repo-older".to_string(),
                workdir: PathBuf::from("/tmp/test-repo"),
                is_worktree: false,
                tmux_session: "amf-repo-older".to_string(),
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
            },
        ],
        created_at: now,
        preferred_agent: AgentKind::default(),
        is_git: true,
    };
    let store = ProjectStore {
        version: 2,
        projects: vec![project],
        session_bookmarks: vec![],
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
        extra: HashMap::new(),
    };

    let app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let visible = app.visible_items();

    assert!(matches!(visible[0], VisibleItem::Project(0)));
    assert!(matches!(visible[1], VisibleItem::Feature(0, 1)));
    assert!(matches!(visible[2], VisibleItem::Feature(0, 0)));
    assert_eq!(
        visible.len(),
        3,
        "a persisted current plan must not become a dashboard tree item"
    );
}

#[test]
fn ensure_selection_visible_accounts_for_multi_line_sessions() {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    store.projects[0].features[0].sessions = vec![
        make_session("claude-1", Some("running")),
        make_session("claude-2", Some("running")),
        make_session("claude-3", None),
    ];

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Session(0, 0, 1);

    app.ensure_selection_visible(4);

    assert_eq!(app.scroll_offset, 2);
}

#[test]
fn item_index_at_visible_row_maps_status_line_to_same_session() {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    store.projects[0].features[0].sessions = vec![
        make_session("claude-1", Some("running")),
        make_session("claude-2", None),
    ];

    let app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    assert!(matches!(app.item_index_at_visible_row(0, 4), Some(0)));
    assert!(matches!(app.item_index_at_visible_row(1, 4), Some(1)));
    assert!(matches!(app.item_index_at_visible_row(2, 4), Some(2)));
    assert!(matches!(app.item_index_at_visible_row(3, 4), Some(2)));
    assert_eq!(app.item_index_at_visible_row(4, 4), None);
}
