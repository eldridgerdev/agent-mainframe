use super::support::*;
use crate::app::*;
use crate::db::todos::{TodoPriority, TodoStatus};
use crate::project::TodoSessionReference;
use crate::project::{AgentKind, FeatureSession, SessionKind};
use crate::traits::{MockTmuxOps, MockWorktreeOps};
use chrono::Utc;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tempfile::TempDir;

#[test]
fn session_picker_offers_todos_when_project_has_none() {
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Feature(0, 0);

    app.open_session_picker().unwrap();

    match &app.mode {
        AppMode::SessionPicker(state) => {
            assert!(
                state
                    .builtin_sessions
                    .iter()
                    .any(|session| session.kind == SessionKind::Todos),
                "TODOs should be offered when the project has none"
            );
        }
        _ => panic!("expected SessionPicker mode"),
    }
}

#[test]
fn session_picker_hides_todos_when_project_already_has_one() {
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Feature(0, 0);
    app.add_builtin_session(0, 0, SessionKind::Todos).unwrap();

    app.selection = Selection::Feature(0, 0);
    app.open_session_picker().unwrap();

    match &app.mode {
        AppMode::SessionPicker(state) => {
            assert!(
                !state
                    .builtin_sessions
                    .iter()
                    .any(|session| session.kind == SessionKind::Todos),
                "TODOs should be hidden once the project already has one"
            );
        }
        _ => panic!("expected SessionPicker mode"),
    }
}

#[test]
fn add_builtin_session_blocks_second_todos_per_project() {
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Feature(0, 0);

    app.add_builtin_session(0, 0, SessionKind::Todos).unwrap();
    assert!(
        app.store.projects[0]
            .features
            .iter()
            .any(|f| f.has_todos_session())
    );
    let todos_count = |app: &App| {
        app.store.projects[0]
            .features
            .iter()
            .flat_map(|f| &f.sessions)
            .filter(|s| s.kind == SessionKind::Todos)
            .count()
    };
    assert_eq!(todos_count(&app), 1);

    // A second attempt is rejected; still exactly one TODOs session.
    app.selection = Selection::Feature(0, 0);
    app.add_builtin_session(0, 0, SessionKind::Todos).unwrap();
    assert_eq!(
        todos_count(&app),
        1,
        "a second TODOs session must be blocked"
    );
    assert_eq!(
        app.message.as_deref(),
        Some("This feature already has a TODOs session")
    );
}

#[test]
fn todos_session_does_not_create_a_tmux_window() {
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Feature(0, 0);

    app.add_builtin_session(0, 0, SessionKind::Todos).unwrap();

    let session = app.store.projects[0].features[0]
        .sessions
        .iter()
        .find(|s| s.kind == SessionKind::Todos)
        .expect("TODOs session should exist");
    assert!(!session.kind.is_tmux_backed());
    assert_eq!(session.label, "TODOs");
}

/// Build a DB-backed app whose project `proj-1` has two features (`feat-1`,
/// `feat-2`) and a TODO list hosted by `feat-1` with one item. Returns the app
/// with `feat-1` already removed from the store (as after a delete).
fn app_with_todo_host_deleted(remove_all_features: bool) -> App {
    let mut store = store_with_feature(ProjectStatus::Active);
    if !remove_all_features {
        let mut second = store.projects[0].features[0].clone();
        second.id = "feat-2".to_string();
        second.name = "other-feat".to_string();
        store.projects[0].features.push(second);
    }

    let db_dir = TempDir::new().unwrap();
    // Keep the tempdir alive for the app's lifetime by leaking it; tests are
    // short-lived processes.
    let db_path = db_dir.keep().join("amf.db");
    let db = crate::db::AmfDb::open(&db_path).unwrap();
    db.save_store(&store).unwrap();
    let list = db
        .create_todo_list(&test_project_scope("proj-1"), Some("feat-1"))
        .unwrap();
    db.add_todo(
        &list.id,
        "do a thing",
        None,
        crate::db::todos::TodoPriority::Med,
    )
    .unwrap();

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.db = Some(db);
    // Simulate the host feature already having been removed from the store.
    app.store.projects[0].features.retain(|f| f.id != "feat-1");
    app
}

#[test]
fn host_feature_delete_prompts_rehome_onto_surviving_feature() {
    let mut app = app_with_todo_host_deleted(false);

    let opened = app.handle_todos_host_feature_deleted("my-project", "my-feat", Some("feat-1"));
    assert!(
        opened,
        "deleting the host feature should open the re-home prompt"
    );
    match &app.mode {
        AppMode::TodosHostReassign(state) => {
            assert_eq!(
                state.candidates,
                vec![("other-feat".to_string(), "feat-2".to_string())]
            );
            assert_eq!(state.todo_count, 1);
            assert_eq!(state.selected, 0);
        }
        _ => panic!("expected TodosHostReassign mode"),
    }

    // Confirm the (default) re-home onto the surviving feature.
    app.confirm_todos_host_reassign().unwrap();
    let reloaded = app
        .db
        .as_ref()
        .unwrap()
        .todo_list(&test_project_scope("proj-1"))
        .unwrap()
        .unwrap();
    assert_eq!(reloaded.feature_id.as_deref(), Some("feat-2"));
    assert!(matches!(app.mode, AppMode::Normal));
}

#[test]
fn host_feature_delete_can_delete_the_list() {
    let mut app = app_with_todo_host_deleted(false);
    app.handle_todos_host_feature_deleted("my-project", "my-feat", Some("feat-1"));

    // Move selection onto the trailing "Delete" option and confirm.
    if let AppMode::TodosHostReassign(state) = &mut app.mode {
        state.selected = state.candidates.len();
    }
    app.confirm_todos_host_reassign().unwrap();

    assert!(
        app.db
            .as_ref()
            .unwrap()
            .todo_list(&test_project_scope("proj-1"))
            .unwrap()
            .is_none()
    );
    assert!(matches!(app.mode, AppMode::Normal));
}

#[test]
fn host_feature_delete_drops_list_when_no_features_remain() {
    let mut app = app_with_todo_host_deleted(true);

    let opened = app.handle_todos_host_feature_deleted("my-project", "my-feat", Some("feat-1"));
    assert!(!opened, "no surviving features → no prompt");
    // The orphaned list is dropped, not left dangling.
    assert!(
        app.db
            .as_ref()
            .unwrap()
            .todo_list(&test_project_scope("proj-1"))
            .unwrap()
            .is_none()
    );
}

#[test]
fn deleting_non_host_feature_leaves_todo_list_untouched() {
    let mut app = app_with_todo_host_deleted(false);

    // A different (non-host) feature id is passed in.
    let opened = app.handle_todos_host_feature_deleted("my-project", "other-feat", Some("feat-2"));
    assert!(!opened);
    let list = app
        .db
        .as_ref()
        .unwrap()
        .todo_list(&test_project_scope("proj-1"))
        .unwrap()
        .unwrap();
    assert_eq!(
        list.feature_id.as_deref(),
        Some("feat-1"),
        "list host should be unchanged"
    );
}

fn test_project_scope(project_id: &str) -> crate::db::todos::TodoScope {
    crate::db::todos::TodoScope::Project {
        project_id: project_id.to_string(),
    }
}

fn sample_todo(title: &str, done: bool) -> crate::db::todos::Todo {
    use crate::db::todos::{TodoStatus, TodoWorkState};
    crate::db::todos::Todo {
        id: format!("todo-{title}"),
        list_id: "list-1".to_string(),
        title: title.to_string(),
        body: None,
        priority: crate::db::todos::TodoPriority::Med,
        sort_order: 0,
        work: TodoWorkState {
            status: if done {
                TodoStatus::Completed
            } else {
                TodoStatus::NotStarted
            },
            agent_session_id: None,
        },
        linked_feature_id: None,
        created_at: String::new(),
        updated_at: String::new(),
    }
}

#[test]
fn entering_todos_session_opens_native_overlay() {
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Feature(0, 0);
    app.add_builtin_session(0, 0, SessionKind::Todos).unwrap();

    // Select the TODOs session and open it.
    let si = app.store.projects[0].features[0]
        .sessions
        .iter()
        .position(|s| s.kind == SessionKind::Todos)
        .unwrap();
    app.selection = Selection::Session(0, 0, si);
    app.enter_view().unwrap();

    match &app.mode {
        AppMode::Todos(state) => {
            assert_eq!(state.project_name, "my-project");
            assert_eq!(state.feature_name, "my-feat");
            assert_eq!(state.pi, 0);
            assert_eq!(state.fi, 0);
        }
        _ => panic!("expected Todos overlay"),
    }
}

#[test]
fn closing_todos_overlay_returns_to_session_selection() {
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Feature(0, 0);
    app.add_builtin_session(0, 0, SessionKind::Todos).unwrap();
    let si = app.store.projects[0].features[0]
        .sessions
        .iter()
        .position(|s| s.kind == SessionKind::Todos)
        .unwrap();
    app.selection = Selection::Session(0, 0, si);
    app.enter_view().unwrap();

    app.close_todos_view();

    assert!(matches!(app.mode, AppMode::Normal));
    assert!(matches!(app.selection, Selection::Session(0, 0, s) if s == si));
}

#[test]
fn closing_todos_overlay_returns_to_the_paused_plan_interview() {
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Feature(0, 0);
    app.start_plan_interview_for_selected_feature();
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.editor = crate::editor::TextEditor::new("Inspect the TODO list".into());
    }
    app.pause_plan_interview();

    app.add_builtin_session(0, 0, SessionKind::Todos).unwrap();
    let si = app.store.projects[0].features[0]
        .sessions
        .iter()
        .position(|session| session.kind == SessionKind::Todos)
        .unwrap();
    app.selection = Selection::Session(0, 0, si);
    app.enter_view().unwrap();
    app.close_todos_view();

    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state) if state.editor.text() == "Inspect the TODO list"
    ));
    assert!(app.paused_plan_interview.is_none());
}

#[test]
fn todos_navigation_wraps_around() {
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.mode = AppMode::Todos(todo_view_with(vec![
        sample_todo("a", false),
        sample_todo("b", false),
        sample_todo("c", true),
    ]));

    let selected = |app: &App| match &app.mode {
        AppMode::Todos(state) => state.panes[0].selected,
        _ => panic!("expected Todos overlay"),
    };

    app.todos_select_next();
    assert_eq!(selected(&app), 1);
    app.todos_select_next();
    app.todos_select_next();
    assert_eq!(selected(&app), 0, "next wraps from last back to first");
    app.todos_select_prev();
    assert_eq!(selected(&app), 2, "prev wraps from first to last");
}

#[test]
fn todos_navigation_no_op_when_empty() {
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.mode = AppMode::Todos(empty_todo_view());

    app.todos_select_next();
    app.todos_select_prev();
    match &app.mode {
        AppMode::Todos(state) => assert_eq!(state.panes[0].selected, 0),
        _ => panic!("expected Todos overlay"),
    }
}

/// A one-pane overlay over the project's list — the shape a feature sitting on
/// the repo root opens with, and the simplest thing to assert against. Tests
/// that need the side panes build them explicitly.
fn empty_todo_view() -> TodoViewState {
    todo_view_with(vec![])
}

fn todo_view_with(todos: Vec<crate::db::todos::Todo>) -> TodoViewState {
    TodoViewState {
        pi: 0,
        fi: 0,
        project_name: "my-project".to_string(),
        feature_name: "my-feat".to_string(),
        panes: vec![crate::app::TodoPane {
            kind: crate::app::TodoPaneKind::Project,
            scope: crate::db::todos::TodoScope::Project {
                project_id: "proj-1".to_string(),
            },
            title: "my-project".to_string(),
            list: None,
            todos,
            selected: 0,
            scroll_offset: 0,
        }],
        focus: Some(0),
        editor: None,
        todo_vim_enabled: false,
        pending_delete: false,
        launch: None,
        scope_move: None,
    }
}

fn todos_app() -> App {
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.mode = AppMode::Todos(empty_todo_view());
    app
}

fn type_str(app: &mut App, s: &str) {
    for c in s.chars() {
        crate::handlers::handle_todos_key(app, ke(KeyCode::Char(c))).unwrap();
    }
}

fn todo_titles(app: &App) -> Vec<String> {
    match &app.mode {
        AppMode::Todos(state) => state.panes[0]
            .todos
            .iter()
            .map(|t| t.title.clone())
            .collect(),
        _ => panic!("expected Todos overlay"),
    }
}

// ----- TODO launch chooser -------------------------------------------------

/// Push one TODO into the open overlay and select it.
fn seed_selected_todo(app: &mut App, title: &str) -> String {
    let todo = sample_todo(title, false);
    let id = todo.id.clone();
    match &mut app.mode {
        AppMode::Todos(state) => {
            state.panes[0].todos.push(todo);
            state.panes[0].selected = state.panes[0].todos.len() - 1;
        }
        _ => panic!("expected Todos overlay"),
    }
    id
}

fn launch_step(app: &App) -> Option<&crate::app::TodoLaunchStep> {
    match &app.mode {
        AppMode::Todos(state) => state.launch.as_ref(),
        _ => None,
    }
}

/// `Enter` on a TODO with nothing linked has to ask rather than pick for the user:
/// spawning and planning are different amounts of work to commit to.
#[test]
fn enter_on_an_unlinked_todo_opens_the_chooser_instead_of_spawning() {
    let mut app = todos_app();
    seed_selected_todo(&mut app, "wire up the chooser");

    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Enter)).unwrap();

    assert!(
        matches!(
            launch_step(&app),
            Some(crate::app::TodoLaunchStep::Choice { .. })
        ),
        "expected the chooser, got {:?}",
        launch_step(&app).is_some()
    );
    // The list is still underneath: the chooser is a layer, not a new mode.
    assert_eq!(todo_titles(&app), vec!["wire up the chooser"]);
}

/// Esc unwinds one step at a time, so reaching the destination step by mistake
/// costs one keypress rather than dropping the user back to the dashboard.
#[test]
fn esc_walks_back_from_destination_to_chooser_to_the_list() {
    let mut app = todos_app();
    seed_selected_todo(&mut app, "plan me");

    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Enter)).unwrap();
    // Move to "Plan this TODO first" (the third option) and take it.
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('j'))).unwrap();
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('j'))).unwrap();
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Enter)).unwrap();
    assert!(matches!(
        launch_step(&app),
        Some(crate::app::TodoLaunchStep::Destination { .. })
    ));

    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Esc)).unwrap();
    assert!(
        matches!(
            launch_step(&app),
            Some(crate::app::TodoLaunchStep::Choice { selected: 2, .. })
        ),
        "Esc returns to the chooser with the cursor on the option that got here"
    );

    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Esc)).unwrap();
    assert!(
        launch_step(&app).is_none(),
        "a second Esc returns to the list"
    );
    assert!(
        matches!(app.mode, AppMode::Todos(_)),
        "and stays in the overlay"
    );
}

/// Choosing plan mode is the lifecycle boundary: the TODO changes before the
/// destination is chosen, and backing out of that workflow does not pretend it
/// was never begun. The layered picker must not disturb the list viewport.
#[test]
fn starting_then_cancelling_todo_planning_keeps_it_in_progress_and_preserves_the_pane() {
    let mut app = todos_app();
    seed_selected_todo(&mut app, "plan me");
    if let AppMode::Todos(state) = &mut app.mode {
        state.panes[0].scroll_offset = 7;
    }

    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Enter)).unwrap();
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('j'))).unwrap();
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('j'))).unwrap();
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Enter)).unwrap();

    match &app.mode {
        AppMode::Todos(state) => {
            assert!(matches!(
                state.launch,
                Some(crate::app::TodoLaunchStep::Destination { .. })
            ));
            assert!(state.panes[0].todos[0].work.status.is_in_progress());
            assert_eq!(state.focus, Some(0));
            assert_eq!(state.panes[0].selected, 0);
            assert_eq!(state.panes[0].scroll_offset, 7);
        }
        _ => panic!("expected the destination picker over the TODO list"),
    }

    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Esc)).unwrap();
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Esc)).unwrap();
    match &app.mode {
        AppMode::Todos(state) => {
            assert!(state.launch.is_none());
            assert!(state.panes[0].todos[0].work.status.is_in_progress());
            assert_eq!(state.focus, Some(0));
            assert_eq!(state.panes[0].selected, 0);
            assert_eq!(state.panes[0].scroll_offset, 7);
        }
        _ => panic!("expected to return to the TODO list"),
    }
}

/// The chooser's middle option starts an agent in a fresh worktree without a
/// plan interview — the answer to "I don't want to plan this, but I don't want
/// to reuse an existing checkout either".
#[test]
fn chooser_middle_option_is_spawn_in_a_new_feature() {
    assert_eq!(
        crate::app::TodoLaunchAction::ALL[1],
        crate::app::TodoLaunchAction::SpawnInNewFeature
    );

    let mut app = todos_app();
    seed_selected_todo(&mut app, "no plan, new worktree");

    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Enter)).unwrap();
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('j'))).unwrap();
    assert_eq!(launch_step(&app).unwrap().selected(), 1);

    // The project in `todos_app()` is not a git repo, so the route declines
    // rather than opening a wizard that could not create a worktree — and it
    // says why instead of doing nothing.
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Enter)).unwrap();
    assert!(
        matches!(app.mode, AppMode::Todos(_)),
        "stays in the overlay"
    );
    assert!(
        app.toasts
            .last()
            .is_some_and(|t| t.message.contains("git repository")),
        "the refusal names the reason, got {:?}",
        app.toasts.last().map(|t| t.message.clone())
    );
    // Nothing was committed: the TODO is untouched and no seed was stashed.
    match &app.mode {
        AppMode::Todos(state) => {
            assert!(!state.panes[0].todos[0].work.status.is_in_progress())
        }
        _ => unreachable!(),
    }
    assert!(app.pending_todo_spawn_prompt.is_none());
    assert!(app.pending_todo_plan_brief.is_none());
}

/// On a git-backed project the same option pre-seeds the create-feature wizard
/// with the TODO — plan mode off — and stashes the TODO as the composer seed
/// for the launch to pick up once the feature exists.
#[test]
fn spawn_in_new_feature_seeds_the_wizard_with_plan_mode_off() {
    let mut app = todos_app();
    if let Some(project) = app.store.projects.get_mut(0) {
        project.is_git = true;
    }
    app.store.available_harnesses = vec![AgentKind::Claude];
    seed_selected_todo(&mut app, "carve out the parser");

    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Enter)).unwrap();
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('j'))).unwrap();
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Enter)).unwrap();

    match &app.mode {
        AppMode::CreatingFeature(state) => {
            assert!(!state.plan_mode, "this route is explicitly not plan mode");
            assert!(state.use_worktree);
            assert_eq!(
                state.todo_origin.as_ref().map(|o| o.todo_title.as_str()),
                Some("carve out the parser")
            );
        }
        _ => panic!("expected the create-feature wizard"),
    }
    assert!(app.pending_todo_plan_brief.is_none(), "not a plan run");
    assert!(
        app.pending_todo_spawn_prompt
            .as_deref()
            .is_some_and(|p| p.contains("carve out the parser")),
        "the TODO is stashed as the composer seed"
    );
}

/// The cursor clamps rather than wraps: wrapping would make j and k at the ends
/// look like they did nothing.
#[test]
fn chooser_cursor_clamps_at_both_ends() {
    let mut app = todos_app();
    seed_selected_todo(&mut app, "plan me");
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Enter)).unwrap();

    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('k'))).unwrap();
    assert_eq!(
        launch_step(&app).unwrap().selected(),
        0,
        "k at the top stays"
    );

    for _ in 0..5 {
        crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('j'))).unwrap();
    }
    assert_eq!(
        launch_step(&app).unwrap().selected(),
        2,
        "j past the end stays"
    );
}

/// A TODO already planned into a feature jumps there instead of asking again —
/// the decision was made the first time.
#[test]
fn enter_on_a_todo_linked_to_a_live_feature_jumps_there_without_asking() {
    let mut tmux = MockTmuxOps::new();
    // The jump enters the feature's view, which reconciles against tmux.
    tmux.expect_session_exists().return_const(true);
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );
    app.mode = AppMode::Todos(empty_todo_view());
    let feature_id = app.store.projects[0].features[0].id.clone();
    seed_selected_todo(&mut app, "already planned");
    match &mut app.mode {
        AppMode::Todos(state) => {
            state.panes[0].todos[0].linked_feature_id = Some(feature_id);
        }
        _ => unreachable!(),
    }

    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Enter)).unwrap();

    assert!(
        !matches!(app.mode, AppMode::Todos(_)),
        "the jump leaves the overlay"
    );
}

/// A link whose feature was deleted must not be a dead end: it is dropped, the
/// user is told, and the next press offers the chooser.
#[test]
fn enter_on_a_todo_linked_to_a_deleted_feature_clears_the_link_and_offers_the_chooser() {
    let mut app = todos_app();
    seed_selected_todo(&mut app, "planned into a ghost");
    match &mut app.mode {
        AppMode::Todos(state) => {
            state.panes[0].todos[0].linked_feature_id = Some("feat-that-is-gone".to_string());
        }
        _ => unreachable!(),
    }

    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Enter)).unwrap();

    match &app.mode {
        AppMode::Todos(state) => {
            assert!(
                state.panes[0].todos[0].linked_feature_id.is_none(),
                "the dead link is dropped"
            );
            assert!(
                matches!(
                    state.launch,
                    Some(crate::app::TodoLaunchStep::Choice { .. })
                ),
                "and the chooser is offered in its place"
            );
        }
        _ => panic!("expected to stay in the Todos overlay"),
    }
}

/// Deleting a feature must not leave TODOs pointing at it, in memory or on
/// disk. The TODO itself survives: the work outlived the branch.
#[test]
fn deleting_a_feature_clears_todo_links_to_it_but_keeps_the_todos() {
    let mut app = todos_app();
    seed_selected_todo(&mut app, "planned here");
    seed_selected_todo(&mut app, "planned elsewhere");
    match &mut app.mode {
        AppMode::Todos(state) => {
            state.panes[0].todos[0].linked_feature_id = Some("feat-doomed".to_string());
            state.panes[0].todos[1].linked_feature_id = Some("feat-safe".to_string());
        }
        _ => unreachable!(),
    }

    app.clear_todo_links_to_deleted_feature(Some("feat-doomed"));

    match &app.mode {
        AppMode::Todos(state) => {
            assert_eq!(state.panes[0].todos.len(), 2, "both TODOs survive");
            assert!(state.panes[0].todos[0].linked_feature_id.is_none());
            assert_eq!(
                state.panes[0].todos[1].linked_feature_id.as_deref(),
                Some("feat-safe"),
                "an unrelated link is untouched"
            );
        }
        _ => unreachable!(),
    }
}

#[test]
fn todos_add_via_handler_appends_and_selects() {
    let mut app = todos_app();
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('a'))).unwrap();
    type_str(&mut app, "buy milk");
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Enter)).unwrap();

    assert_eq!(todo_titles(&app), vec!["buy milk"]);
    match &app.mode {
        AppMode::Todos(state) => {
            assert_eq!(state.panes[0].selected, 0);
            assert!(state.editor.is_none(), "editor closes after commit");
        }
        _ => panic!("expected Todos overlay"),
    }
}

#[test]
fn pasting_into_a_todo_title_inserts_text_and_saves() {
    let mut app = todos_app();
    let db_file = tempfile::NamedTempFile::new().unwrap();
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());
    app.todos_begin_add();

    crate::handlers::handle_paste(&mut app, "buy oat milk").unwrap();
    assert!(matches!(
        &app.mode,
        AppMode::Todos(state)
            if state.editor.as_ref().is_some_and(|editor| editor.editor.text() == "buy oat milk"
                && editor.editor.cursor() == "buy oat milk".len())
    ));
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Enter)).unwrap();

    assert_eq!(todo_titles(&app), vec!["buy oat milk"]);
    let list = app
        .db
        .as_ref()
        .unwrap()
        .todo_list(&test_project_scope("proj-1"))
        .unwrap()
        .unwrap();
    assert_eq!(
        app.db.as_ref().unwrap().todos(&list.id).unwrap()[0].title,
        "buy oat milk"
    );
}

#[test]
fn pasting_newlines_into_todo_editors_is_safe_and_preserves_notes() {
    let mut app = todos_app();
    app.todos_begin_add();
    crate::handlers::handle_paste(&mut app, "first\r\nsecond\nthird").unwrap();
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Enter)).unwrap();
    assert_eq!(todo_titles(&app), vec!["firstsecondthird"]);

    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('o'))).unwrap();
    crate::handlers::handle_paste(&mut app, "line one\r\nline two\rline three").unwrap();
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Enter)).unwrap();

    match &app.mode {
        AppMode::Todos(state) => assert_eq!(
            state.panes[0].todos[0].body.as_deref(),
            Some("line one\nline two\nline three")
        ),
        _ => panic!("expected Todos overlay"),
    }
}

#[test]
fn todos_empty_title_is_not_added() {
    let mut app = todos_app();
    app.todos_begin_add();
    type_str(&mut app, "   ");
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Enter)).unwrap();
    assert!(todo_titles(&app).is_empty());
}

#[test]
fn todos_edit_title_replaces_text() {
    let mut app = todos_app();
    app.todos_begin_add();
    type_str(&mut app, "old");
    app.todos_commit_edit().unwrap();

    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('e'))).unwrap();
    // Clear the seeded "old" then type "new".
    for _ in 0..3 {
        crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Backspace)).unwrap();
    }
    type_str(&mut app, "new");
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Enter)).unwrap();

    assert_eq!(todo_titles(&app), vec!["new"]);
}

/// A `KeyEvent` carrying Ctrl, for the vim-toggle / cancel chords.
fn ke_ctrl(code: KeyCode) -> crossterm::event::KeyEvent {
    crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::CONTROL)
}

fn ke_shift(code: KeyCode) -> crossterm::event::KeyEvent {
    crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::SHIFT)
}

#[test]
fn todos_shift_enter_inserts_a_newline_without_committing() {
    let mut app = todos_app();
    app.todos_begin_add();
    type_str(&mut app, "write docs");
    app.todos_commit_edit().unwrap();

    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('o'))).unwrap();
    type_str(&mut app, "first line");
    crate::handlers::handle_todos_key(&mut app, ke_shift(KeyCode::Enter)).unwrap();
    type_str(&mut app, "second line");

    match &app.mode {
        AppMode::Todos(state) => assert_eq!(
            state.editor.as_ref().map(|editor| editor.editor.text()),
            Some("first line\nsecond line")
        ),
        _ => panic!("expected Todos overlay"),
    }

    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Enter)).unwrap();
    match &app.mode {
        AppMode::Todos(state) => assert_eq!(
            state.panes[0].todos[0].body.as_deref(),
            Some("first line\nsecond line")
        ),
        _ => panic!("expected Todos overlay"),
    }
}

#[test]
fn todos_ctrl_t_toggles_vim_and_is_remembered_for_later_edits() {
    let mut app = todos_app();
    app.todos_begin_add();

    // Ctrl+T turns vim on mid-edit; like the compose box it lands in Insert so
    // typing continues uninterrupted.
    crate::handlers::handle_todos_key(&mut app, ke_ctrl(KeyCode::Char('t'))).unwrap();
    match &app.mode {
        AppMode::Todos(state) => {
            assert!(state.todo_vim_enabled);
            assert_eq!(
                state.editor.as_ref().unwrap().editor.vim_mode(),
                Some(crate::editor::VimMode::Insert)
            );
        }
        _ => panic!("expected Todos overlay"),
    }

    // Commit and reopen: the remembered choice seeds the next editor as vim,
    // and a freshly seeded vim editor opens in Normal mode.
    type_str(&mut app, "task one");
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Enter)).unwrap();
    assert_eq!(todo_titles(&app), vec!["task one"]);
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('e'))).unwrap();
    match &app.mode {
        AppMode::Todos(state) => assert_eq!(
            state.editor.as_ref().unwrap().editor.vim_mode(),
            Some(crate::editor::VimMode::Normal)
        ),
        _ => panic!("expected Todos overlay"),
    }
}

#[test]
fn todos_esc_in_vim_insert_returns_to_normal_without_cancelling() {
    let mut app = todos_app();
    app.todos_begin_add();
    crate::handlers::handle_todos_key(&mut app, ke_ctrl(KeyCode::Char('t'))).unwrap();
    type_str(&mut app, "x");
    // Esc should go back to Normal mode — not close the editor.
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Esc)).unwrap();
    match &app.mode {
        AppMode::Todos(state) => assert_eq!(
            state
                .editor
                .as_ref()
                .expect("editor still open")
                .editor
                .vim_mode(),
            Some(crate::editor::VimMode::Normal)
        ),
        _ => panic!("expected Todos overlay"),
    }
}

#[test]
fn todos_ctrl_q_cancels_the_edit_in_vim_mode() {
    let mut app = todos_app();
    app.todos_begin_add();
    crate::handlers::handle_todos_key(&mut app, ke_ctrl(KeyCode::Char('t'))).unwrap();
    crate::handlers::handle_todos_key(&mut app, ke_ctrl(KeyCode::Char('q'))).unwrap();
    match &app.mode {
        AppMode::Todos(state) => {
            assert!(state.editor.is_none(), "Ctrl+Q discards the edit");
            assert!(
                state.todo_vim_enabled,
                "the keymap choice outlives the edit"
            );
        }
        _ => panic!("expected Todos overlay"),
    }
    assert!(todo_titles(&app).is_empty());
}

#[test]
fn todos_vim_normal_mode_edit_commits() {
    let mut app = todos_app();
    app.todos_begin_add();
    type_str(&mut app, "chore");
    app.todos_commit_edit().unwrap();

    // Re-open, turn on vim (lands in Insert), Esc to Normal, then `0x` deletes
    // the leading "c" as a real normal-mode command rather than typed text.
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('e'))).unwrap();
    crate::handlers::handle_todos_key(&mut app, ke_ctrl(KeyCode::Char('t'))).unwrap();
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Esc)).unwrap();
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('0'))).unwrap();
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('x'))).unwrap();
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Enter)).unwrap();

    assert_eq!(todo_titles(&app), vec!["hore"]);
}

#[test]
fn todos_vim_line_open_newline_is_flattened_out_of_title() {
    let mut app = todos_app();
    app.todos_begin_add();
    type_str(&mut app, "foo");

    // Vim on (Insert), Esc to Normal, `o` opens a line below and drops into
    // Insert there — inserting a `\n` into this single-line field. Type on the
    // new line, then commit with plain Enter.
    crate::handlers::handle_todos_key(&mut app, ke_ctrl(KeyCode::Char('t'))).unwrap();
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Esc)).unwrap();
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('o'))).unwrap();
    type_str(&mut app, "bar");
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Enter)).unwrap();

    // The persisted title is a single line, not "foo\nbar".
    assert_eq!(todo_titles(&app), vec!["foo bar"]);
}

#[test]
fn todos_toggle_done_sinks_item_below_open() {
    let mut app = todos_app();
    app.todos_begin_add();
    type_str(&mut app, "a");
    app.todos_commit_edit().unwrap();
    app.todos_begin_add();
    type_str(&mut app, "b");
    app.todos_commit_edit().unwrap();

    // Select "a" (index 0), advance through in progress, and mark it done.
    if let AppMode::Todos(state) = &mut app.mode {
        state.panes[0].selected = 0;
    }
    app.todos_toggle_done().unwrap();
    app.todos_toggle_done().unwrap();

    // Open "b" now sorts before done "a"; cursor follows "a".
    assert_eq!(todo_titles(&app), vec!["b", "a"]);
    match &app.mode {
        AppMode::Todos(state) => {
            assert!(state.panes[0].todos[1].work.status.is_completed());
            assert_eq!(state.panes[0].selected, 1);
        }
        _ => panic!("expected Todos overlay"),
    }
}

#[test]
fn todos_cycle_priority_rotates() {
    use crate::db::todos::TodoPriority;
    let mut app = todos_app();
    app.todos_begin_add();
    type_str(&mut app, "a");
    app.todos_commit_edit().unwrap();

    let prio = |app: &App| match &app.mode {
        AppMode::Todos(state) => state.panes[0].todos[0].priority,
        _ => panic!("expected Todos overlay"),
    };
    assert_eq!(prio(&app), TodoPriority::Med);
    app.todos_cycle_priority().unwrap();
    assert_eq!(prio(&app), TodoPriority::Low);
    app.todos_cycle_priority().unwrap();
    assert_eq!(prio(&app), TodoPriority::High);
    app.todos_cycle_priority().unwrap();
    assert_eq!(prio(&app), TodoPriority::Med);
}

#[test]
fn todos_reorder_moves_item() {
    let mut app = todos_app();
    for t in ["a", "b", "c"] {
        app.todos_begin_add();
        type_str(&mut app, t);
        app.todos_commit_edit().unwrap();
    }
    // Select "a" (top) and move it down.
    if let AppMode::Todos(state) = &mut app.mode {
        state.panes[0].selected = 0;
    }
    app.todos_reorder(1).unwrap();

    assert_eq!(todo_titles(&app), vec!["b", "a", "c"]);
    match &app.mode {
        AppMode::Todos(state) => {
            assert_eq!(state.panes[0].selected, 1, "cursor follows moved item")
        }
        _ => panic!("expected Todos overlay"),
    }
}

#[test]
fn todos_delete_requires_confirmation() {
    let mut app = todos_app();
    app.todos_begin_add();
    type_str(&mut app, "doomed");
    app.todos_commit_edit().unwrap();

    // 'd' arms the confirm; 'n' cancels and keeps the item.
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('d'))).unwrap();
    assert!(matches!(&app.mode, AppMode::Todos(s) if s.pending_delete));
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('n'))).unwrap();
    assert_eq!(todo_titles(&app), vec!["doomed"]);

    // 'd' then 'y' deletes it.
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('d'))).unwrap();
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('y'))).unwrap();
    assert!(todo_titles(&app).is_empty());
}

#[test]
fn deleting_a_todo_clears_all_session_sidebar_references_to_it() {
    let mut app = todos_app();
    app.todos_begin_add();
    type_str(&mut app, "doomed");
    app.todos_commit_edit().unwrap();
    let todo_id = match &app.mode {
        AppMode::Todos(state) => state.panes[0].todos[0].id.clone(),
        _ => unreachable!(),
    };

    let session = app.store.projects[0].features[0]
        .add_session_named(SessionKind::Claude, "TODO agent".to_string());
    session.todo_reference = Some(crate::project::TodoSessionReference {
        todo_id,
        launched_from_todo_menu: true,
    });

    app.todos_request_delete();
    app.todos_confirm_delete().unwrap();

    assert!(
        app.store.projects[0].features[0].sessions[0]
            .todo_reference
            .is_none()
    );
}

#[test]
fn todos_edit_scratchpad_banner() {
    let mut app = todos_app();
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('b'))).unwrap();
    type_str(&mut app, "finishing the parser");
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Enter)).unwrap();

    match &app.mode {
        AppMode::Todos(state) => {
            assert_eq!(
                state.panes[0]
                    .list
                    .as_ref()
                    .and_then(|l| l.carry_over.as_deref()),
                Some("finishing the parser")
            );
        }
        _ => panic!("expected Todos overlay"),
    }
}

#[test]
fn todos_edit_cancel_discards_changes() {
    let mut app = todos_app();
    app.todos_begin_add();
    type_str(&mut app, "scratch");
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Esc)).unwrap();
    assert!(todo_titles(&app).is_empty());
    assert!(matches!(&app.mode, AppMode::Todos(s) if s.editor.is_none()));
}

#[test]
fn todo_spawn_prompt_includes_title_and_body() {
    let mut todo = sample_todo("Wire up the parser", false);
    let title_only = App::todo_spawn_prompt(&todo);
    assert_eq!(
        title_only,
        "Please address this TODO item for this feature:\n\nWire up the parser"
    );

    todo.body = Some("  handle nested groups\nand escapes  ".to_string());
    let with_body = App::todo_spawn_prompt(&todo);
    assert_eq!(
        with_body,
        "Please address this TODO item for this feature:\n\nWire up the parser\n\nhandle nested groups\nand escapes"
    );

    // A whitespace-only body is treated as absent.
    todo.body = Some("   ".to_string());
    assert_eq!(App::todo_spawn_prompt(&todo), title_only);
}

#[test]
fn todo_session_label_truncates_long_titles() {
    assert_eq!(App::todo_session_label("short"), "TODO: short");
    let long = "a really long todo title that keeps going and going";
    let label = App::todo_session_label(long);
    assert!(label.starts_with("TODO: "));
    assert!(label.ends_with('…'), "long titles are elided: {label}");
    assert!(label.chars().count() <= "TODO: ".len() + 24 + 1);
}

#[test]
fn resolve_todo_host_feature_prefers_list_feature_id() {
    let app = todos_app();
    // Known feature id resolves to its index (0), regardless of the fallback.
    assert_eq!(app.resolve_todo_host_feature(0, Some("feat-1"), 3), 0);
    // Unknown / missing id falls back.
    assert_eq!(app.resolve_todo_host_feature(0, Some("nope"), 7), 7);
    assert_eq!(app.resolve_todo_host_feature(0, None, 5), 5);
}

// ----- implement next --------------------------------------------------

/// A live agent session pushed straight into the store: `add_builtin_session`
/// talks to tmux, and these tests only need the row to exist so a TODO's link
/// resolves.
fn push_agent_session(app: &mut App, id: &str) -> String {
    app.store.projects[0].features[0]
        .sessions
        .push(FeatureSession {
            id: id.to_string(),
            kind: SessionKind::Claude,
            label: "Claude".to_string(),
            tmux_window: "claude".to_string(),
            claude_session_id: None,
            todo_reference: None,
            token_usage_source: None,
            token_usage_source_match: None,
            created_at: Utc::now(),
            command: None,
            on_stop: None,
            pre_check: None,
            status_text: None,
            token_usage: None,
        });
    id.to_string()
}

/// A TODO with an explicit priority and sort position, for the selector tests.
fn prio_todo(
    id: &str,
    priority: crate::db::todos::TodoPriority,
    order: i64,
) -> crate::db::todos::Todo {
    let mut todo = sample_todo(id, false);
    todo.id = id.to_string();
    todo.priority = priority;
    todo.sort_order = order;
    todo
}

#[test]
fn next_todo_index_takes_the_highest_priority_first() {
    use crate::db::todos::TodoPriority::{High, Low, Med};
    let todos = vec![
        prio_todo("a", Low, 0),
        prio_todo("b", High, 1),
        prio_todo("c", Med, 2),
    ];
    assert_eq!(
        App::next_todo_index(&todos, &[]),
        Some(crate::app::todos::NextTodo::Ready(1))
    );
}

#[test]
fn next_todo_index_breaks_ties_by_list_order() {
    use crate::db::todos::TodoPriority::High;
    // Same priority throughout: the order the user arranged decides, so the
    // sort has to be stable rather than merely correct about priority.
    let todos = vec![
        prio_todo("first", High, 0),
        prio_todo("second", High, 1),
        prio_todo("third", High, 2),
    ];
    assert_eq!(
        App::next_todo_index(&todos, &[]),
        Some(crate::app::todos::NextTodo::Ready(0))
    );
}

#[test]
fn next_todo_index_skips_each_exclusion() {
    use crate::app::todos::NextTodo;
    use crate::db::todos::TodoPriority::High;

    // Done.
    let mut todos = vec![prio_todo("a", High, 0), prio_todo("b", High, 1)];
    todos[0].work.status = crate::db::todos::TodoStatus::Completed;
    assert_eq!(App::next_todo_index(&todos, &[]), Some(NextTodo::Ready(1)));

    // In progress.
    let mut todos = vec![prio_todo("a", High, 0), prio_todo("b", High, 1)];
    todos[0].work.status = crate::db::todos::TodoStatus::InProgress;
    assert_eq!(App::next_todo_index(&todos, &[]), Some(NextTodo::Ready(1)));

    // Explicitly skipped by a previous "skip to next".
    let todos = vec![prio_todo("a", High, 0), prio_todo("b", High, 1)];
    assert_eq!(
        App::next_todo_index(&todos, &["a".to_string()]),
        Some(NextTodo::Ready(1))
    );

    // Already linked to a session: held in reserve, not chosen, while an
    // unstarted item remains — even a lower-priority one further down.
    let mut todos = vec![
        prio_todo("a", High, 0),
        prio_todo("b", crate::db::todos::TodoPriority::Low, 1),
    ];
    todos[0].work.agent_session_id = Some("sess-1".to_string());
    assert_eq!(App::next_todo_index(&todos, &[]), Some(NextTodo::Ready(1)));

    // Same for a TODO planned into its own feature: the work moved elsewhere.
    let mut todos = vec![
        prio_todo("a", High, 0),
        prio_todo("b", crate::db::todos::TodoPriority::Low, 1),
    ];
    todos[0].linked_feature_id = Some("feat-9".to_string());
    assert_eq!(App::next_todo_index(&todos, &[]), Some(NextTodo::Ready(1)));
}

#[test]
fn next_todo_index_falls_back_to_a_started_todo() {
    use crate::app::todos::NextTodo;
    use crate::db::todos::TodoPriority::{High, Med};
    // Nothing unstarted is left, so the highest-priority started item is
    // offered for the caller to ask about rather than silently reported as
    // "nothing to do".
    let mut todos = vec![prio_todo("a", Med, 0), prio_todo("b", High, 1)];
    todos[0].work.agent_session_id = Some("sess-1".to_string());
    todos[1].work.agent_session_id = Some("sess-2".to_string());
    assert_eq!(
        App::next_todo_index(&todos, &[]),
        Some(NextTodo::Started(1))
    );
}

#[test]
fn next_todo_index_returns_nothing_when_there_is_nothing() {
    use crate::db::todos::TodoPriority::High;
    assert_eq!(App::next_todo_index(&[], &[]), None);

    // Every item ineligible: done, in progress, and skipped in turn.
    let mut todos = vec![
        prio_todo("a", High, 0),
        prio_todo("b", High, 1),
        prio_todo("c", High, 2),
    ];
    todos[0].work.status = crate::db::todos::TodoStatus::Completed;
    todos[1].work.status = crate::db::todos::TodoStatus::InProgress;
    assert_eq!(App::next_todo_index(&todos, &["c".to_string()]), None);
}

#[test]
fn no_next_todo_message_names_the_reason() {
    use crate::db::todos::TodoPriority::High;
    assert_eq!(
        App::no_next_todo_message(&[], &[]),
        "No TODOs left to implement"
    );

    let mut done = vec![prio_todo("a", High, 0)];
    done[0].work.status = crate::db::todos::TodoStatus::Completed;
    assert_eq!(
        App::no_next_todo_message(&done, &[]),
        "No TODOs left to implement"
    );

    // Open items exist, they are just all underway — a different problem with a
    // different fix, so it gets different words.
    let mut busy = vec![prio_todo("a", High, 0)];
    busy[0].work.status = crate::db::todos::TodoStatus::InProgress;
    assert_eq!(
        App::no_next_todo_message(&busy, &[]),
        "All remaining TODOs are already in progress"
    );

    let skipped = vec![prio_todo("a", High, 0)];
    assert_eq!(
        App::no_next_todo_message(&skipped, &["a".to_string()]),
        "No other TODOs left to implement"
    );
}

#[test]
fn implement_next_toasts_when_nothing_is_eligible() {
    let mut app = todos_app();
    if let AppMode::Todos(state) = &mut app.mode {
        let mut todo = sample_todo("a", false);
        todo.work.status = crate::db::todos::TodoStatus::InProgress;
        state.panes[0].todos = vec![todo];
    }
    app.implement_next_todo_in_overlay().unwrap();
    // Still the list, and the refusal says why rather than doing nothing.
    assert!(matches!(app.mode, AppMode::Todos(_)));
    assert!(
        app.toasts
            .iter()
            .any(|t| t.message.contains("already in progress")),
        "expected an in-progress toast, got {:?}",
        app.toasts.iter().map(|t| &t.message).collect::<Vec<_>>()
    );
}

#[test]
fn implement_next_prompts_when_the_only_candidate_is_started() {
    let mut app = todos_app();
    // The linked session has to exist, or reconciliation clears the link and
    // the TODO comes back as an ordinary unstarted candidate.
    let session_id = push_agent_session(&mut app, "sess-1");
    if let AppMode::Todos(state) = &mut app.mode {
        let mut todo = sample_todo("a", false);
        todo.work.agent_session_id = Some(session_id);
        state.panes[0].todos = vec![todo];
    }

    app.implement_next_todo_in_overlay().unwrap();
    match &app.mode {
        AppMode::TodoImplementChoice(state) => {
            assert_eq!(state.todo_id, "todo-a");
            // The list is stashed, not thrown away.
            assert!(matches!(state.origin.as_ref(), AppMode::Todos(_)));
        }
        _ => panic!("expected the already-started prompt"),
    }

    // Esc restores exactly what the key was pressed in.
    app.cancel_todo_implement_choice();
    match &app.mode {
        AppMode::Todos(state) => assert_eq!(state.panes[0].todos.len(), 1),
        _ => panic!("expected the list back"),
    }
}

#[test]
fn implement_next_skip_moves_on_to_the_next_todo() {
    let mut app = todos_app();
    let session_id = push_agent_session(&mut app, "sess-1");
    if let AppMode::Todos(state) = &mut app.mode {
        let mut first = sample_todo("a", false);
        first.work.agent_session_id = Some(session_id.clone());
        let mut second = sample_todo("b", false);
        second.work.agent_session_id = Some(session_id);
        state.panes[0].todos = vec![first, second];
    }

    app.implement_next_todo_in_overlay().unwrap();
    match &app.mode {
        AppMode::TodoImplementChoice(state) => assert_eq!(state.todo_id, "todo-a"),
        _ => panic!("expected the already-started prompt"),
    }

    // Skip to next: the scan resumes past the item just passed over.
    if let AppMode::TodoImplementChoice(state) = &mut app.mode {
        state.selected = 2;
    }
    app.confirm_todo_implement_choice().unwrap();
    match &app.mode {
        AppMode::TodoImplementChoice(state) => {
            assert_eq!(state.todo_id, "todo-b");
            assert_eq!(state.skipped_ids, vec!["todo-a".to_string()]);
        }
        _ => panic!("expected the prompt on the second TODO"),
    }

    // Nothing else is left, so the next skip reports it instead of looping.
    if let AppMode::TodoImplementChoice(state) = &mut app.mode {
        state.selected = 2;
    }
    app.confirm_todo_implement_choice().unwrap();
    assert!(matches!(app.mode, AppMode::Todos(_)));
    assert!(
        app.toasts
            .iter()
            .any(|t| t.message.contains("No other TODOs left")),
        "expected a nothing-left toast"
    );
}

#[test]
fn implement_next_clears_only_the_missing_session_association() {
    let mut app = todos_app();
    if let AppMode::Todos(state) = &mut app.mode {
        // Marked underway by an earlier launch whose session has since been
        // removed. Also done, so the scan stops at the toast rather than going
        // on to spawn: what is under test is the reconciliation, which runs
        // over the whole list rather than just the candidate.
        let mut stale = sample_todo("a", true);
        stale.work.agent_session_id = Some("sess-gone".to_string());
        stale.work.status = crate::db::todos::TodoStatus::InProgress;
        // Marked underway by hand: no session to lose, so nothing to clear.
        let mut manual = sample_todo("b", false);
        manual.work.status = crate::db::todos::TodoStatus::InProgress;
        state.panes[0].todos = vec![stale, manual];
    }

    app.implement_next_todo_in_overlay().unwrap();

    match &app.mode {
        AppMode::Todos(state) => {
            assert!(state.panes[0].todos[0].work.agent_session_id.is_none());
            assert!(
                state.panes[0].todos[0].work.status.is_in_progress(),
                "a dead link does not make the work unstarted"
            );
            assert!(
                state.panes[0].todos[1].work.status.is_in_progress(),
                "a hand-marked TODO has no link to lose and is left alone"
            );
        }
        _ => panic!("expected Todos overlay"),
    }
}

/// A dead feature link is the one thing "implement next" cannot skip past on
/// its own: the item is held in reserve, so the prompt returns to it forever
/// unless the failed jump drops the link — which is exactly what `g` does.
#[test]
fn implement_next_jump_clears_a_dead_feature_link_and_frees_the_todo() {
    use crate::app::todos::NextTodo;

    let mut app = todos_app();
    if let AppMode::Todos(state) = &mut app.mode {
        let mut todo = sample_todo("a", false);
        // Planned into a feature that has since been deleted, and with no
        // session of its own to fall back to.
        todo.linked_feature_id = Some("feat-that-is-gone".to_string());
        state.panes[0].todos = vec![todo];
    }

    app.implement_next_todo_in_overlay().unwrap();
    match &app.mode {
        AppMode::TodoImplementChoice(state) => {
            assert_eq!(state.todo_id, "todo-a");
            assert_eq!(
                state.choice(),
                crate::app::TodoImplementChoice::Jump,
                "Jump is the default answer, so it is the one that must self-heal"
            );
        }
        _ => panic!("expected the already-started prompt"),
    }

    app.confirm_todo_implement_choice().unwrap();

    match &app.mode {
        AppMode::Todos(state) => {
            assert!(
                state.panes[0].todos[0].linked_feature_id.is_none(),
                "the dead link is dropped by the failed jump"
            );
            assert_eq!(
                App::next_todo_index(&state.panes[0].todos, &[]),
                Some(NextTodo::Ready(0)),
                "so the next scan starts the TODO instead of re-offering it"
            );
        }
        _ => panic!("expected the list back"),
    }
    assert!(
        app.toasts
            .iter()
            .any(|t| t.message.contains("the link was cleared")),
        "the user is told the link went away, got {:?}",
        app.toasts.iter().map(|t| &t.message).collect::<Vec<_>>()
    );
}

#[test]
fn implement_next_is_inert_off_a_todos_session_row() {
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    push_agent_session(&mut app, "sess-1");

    for selection in [
        Selection::Project(0),
        Selection::Feature(0, 0),
        Selection::Session(0, 0, 0),
    ] {
        app.selection = selection;
        app.toasts.clear();
        app.implement_next_todo_from_dashboard().unwrap();
        assert!(matches!(app.mode, AppMode::Normal), "mode must not change");
        assert!(app.toasts.is_empty(), "no toast on a row the key isn't for");
    }
}

#[test]
fn manual_toggle_advances_in_progress_to_completed() {
    let mut app = todos_app();
    if let AppMode::Todos(state) = &mut app.mode {
        let mut todo = sample_todo("a", false);
        todo.work.status = crate::db::todos::TodoStatus::InProgress;
        state.panes[0].todos = vec![todo];
    }
    app.todos_toggle_done().unwrap();
    match &app.mode {
        AppMode::Todos(state) => {
            assert!(state.panes[0].todos[0].work.status.is_completed());
        }
        _ => panic!("expected Todos overlay"),
    }
}

#[test]
fn manual_toggle_cycles_completed_to_not_started_to_in_progress() {
    let mut app = todos_app();
    if let AppMode::Todos(state) = &mut app.mode {
        state.panes[0].todos = vec![sample_todo("a", true)];
    }
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('i'))).unwrap();
    match &app.mode {
        AppMode::Todos(state) => {
            assert!(state.panes[0].todos[0].work.status.is_not_started());
        }
        _ => panic!("expected Todos overlay"),
    }

    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('i'))).unwrap();
    match &app.mode {
        AppMode::Todos(state) => {
            assert!(state.panes[0].todos[0].work.status.is_in_progress())
        }
        _ => panic!("expected Todos overlay"),
    }
}

#[test]
fn todos_mark_in_progress_updates_in_memory() {
    let mut app = todos_app();
    if let AppMode::Todos(state) = &mut app.mode {
        state.panes[0].todos = vec![sample_todo("a", false), sample_todo("b", false)];
    }
    app.todos_mark_in_progress("todo-b", Some("sess-42"))
        .unwrap();
    match &app.mode {
        AppMode::Todos(state) => {
            assert!(state.panes[0].todos[0].work.agent_session_id.is_none());
            assert!(state.panes[0].todos[0].work.status.is_not_started());
            assert_eq!(
                state.panes[0].todos[1].work.agent_session_id.as_deref(),
                Some("sess-42")
            );
            // The session link and the in-progress flag are written together:
            // the flag is what keeps "implement next" off this item.
            assert!(state.panes[0].todos[1].work.status.is_in_progress());
        }
        _ => panic!("expected Todos overlay"),
    }
}

#[test]
fn mark_in_progress_updates_every_in_memory_scope_without_moving_the_cursor() {
    let mut app = todos_app();
    let mut worktree = sample_todo("worktree", false);
    let mut project = sample_todo("project", false);
    let mut global = sample_todo("global", false);
    worktree.id = "todo-worktree".to_string();
    project.id = "todo-project".to_string();
    global.id = "todo-global".to_string();
    app.mode = AppMode::Todos(three_pane_view(vec![worktree], vec![project], vec![global]));
    if let AppMode::Todos(state) = &mut app.mode {
        state.focus = Some(2);
        state.panes[0].scroll_offset = 3;
        state.panes[1].scroll_offset = 4;
        state.panes[2].scroll_offset = 5;
    }

    for id in ["todo-worktree", "todo-project", "todo-global"] {
        app.todos_mark_in_progress(id, None).unwrap();
    }

    match &app.mode {
        AppMode::Todos(state) => {
            assert!(
                state
                    .panes
                    .iter()
                    .all(|pane| pane.todos[0].work.status.is_in_progress())
            );
            assert_eq!(state.focus, Some(2));
            assert_eq!(
                state
                    .panes
                    .iter()
                    .map(|pane| (pane.selected, pane.scroll_offset))
                    .collect::<Vec<_>>(),
                vec![(0, 3), (0, 4), (0, 5)]
            );
        }
        _ => panic!("expected Todos overlay"),
    }
}

#[test]
fn mark_in_progress_persists_all_scopes_and_is_idempotent() {
    use crate::db::todos::{TodoPriority, TodoScope, TodoStatus};

    let mut app = todos_app();
    let db_file = tempfile::NamedTempFile::new().unwrap();
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());

    let scopes = [
        worktree_scope("proj-1", "/tmp/test-workdir"),
        test_project_scope("proj-1"),
        TodoScope::Global,
    ];
    let mut originals = Vec::new();
    for (index, scope) in scopes.iter().enumerate() {
        let list = app
            .db
            .as_ref()
            .unwrap()
            .create_todo_list(scope, (index < 2).then_some("feat-1"))
            .unwrap();
        let mut todo = app
            .db
            .as_ref()
            .unwrap()
            .add_todo(
                &list.id,
                &format!("scope {index}"),
                Some("keep these notes"),
                TodoPriority::High,
            )
            .unwrap();
        todo.sort_order = 40 + index as i64;
        todo.work.agent_session_id = Some(format!("existing-session-{index}"));
        todo.linked_feature_id = Some(format!("linked-feature-{index}"));
        app.db.as_ref().unwrap().update_todo(&todo).unwrap();
        originals.push(todo);
    }
    app.mode = AppMode::Normal;

    for todo in &originals {
        app.todos_mark_in_progress(&todo.id, None).unwrap();
        app.todos_mark_in_progress(&todo.id, None).unwrap();
    }

    for original in originals {
        let loaded = app
            .db
            .as_ref()
            .unwrap()
            .find_todo_by_id(&original.id)
            .unwrap()
            .unwrap();
        assert_eq!(loaded.work.status, TodoStatus::InProgress);
        assert_eq!(loaded.work.agent_session_id, original.work.agent_session_id);
        assert_eq!(loaded.list_id, original.list_id);
        assert_eq!(loaded.title, original.title);
        assert_eq!(loaded.body, original.body);
        assert_eq!(loaded.priority, original.priority);
        assert_eq!(loaded.sort_order, original.sort_order);
        assert_eq!(loaded.linked_feature_id, original.linked_feature_id);
    }
}

#[test]
fn todo_launch_request_reserves_before_a_session_exists() {
    let mut app = todos_app();
    let todo = sample_todo("a", false);
    if let AppMode::Todos(state) = &mut app.mode {
        state.panes[0].todos = vec![todo.clone()];
    }

    assert!(app.todos_reserve_launch(&todo).unwrap());
    match &app.mode {
        AppMode::Todos(state) => {
            let work = &state.panes[0].todos[0].work;
            assert!(work.status.is_in_progress());
            assert!(work.agent_session_id.is_none());
        }
        _ => panic!("expected Todos overlay"),
    }
}

#[test]
fn accepted_plan_continues_from_the_in_progress_state_created_at_plan_start() {
    let mut app = todos_app();
    let mut todo = sample_todo("planned", false);
    todo.work.status = crate::db::todos::TodoStatus::InProgress;
    if let AppMode::Todos(state) = &mut app.mode {
        state.panes[0].todos = vec![todo.clone()];
    }

    assert_eq!(
        app.todos_prepare_planned_launch(&todo).unwrap(),
        Some(false)
    );
    match &app.mode {
        AppMode::Todos(state) => {
            assert!(state.panes[0].todos[0].work.status.is_in_progress());
        }
        _ => panic!("expected Todos overlay"),
    }
}

#[test]
fn accepted_older_plan_reserves_not_started_work_but_blocks_completed_work() {
    let mut app = todos_app();
    let not_started = sample_todo("not-started", false);
    if let AppMode::Todos(state) = &mut app.mode {
        state.panes[0].todos = vec![not_started.clone()];
    }
    assert_eq!(
        app.todos_prepare_planned_launch(&not_started).unwrap(),
        Some(true),
        "a legacy or externally reset plan needs a rollback-capable reservation"
    );

    let mut completed = sample_todo("completed", true);
    completed.work.status = crate::db::todos::TodoStatus::Completed;
    assert_eq!(app.todos_prepare_planned_launch(&completed).unwrap(), None);
}

#[test]
fn ordinary_duplicate_spawn_for_in_progress_todo_is_blocked_without_side_effects() {
    let mut app = todos_app();
    let mut todo = sample_todo("a", false);
    todo.work.status = crate::db::todos::TodoStatus::InProgress;
    let before = todo.work.clone();

    app.spawn_todo_agent(0, 0, &todo, false).unwrap();

    assert_eq!(todo.work, before);
    assert!(app.toasts.iter().any(|toast| {
        toast.message.contains("already in progress")
            && toast.message.contains("another agent was not launched")
    }));
    assert!(app.store.projects[0].features[0].sessions.is_empty());
}

#[test]
fn active_todo_completion_keybind_updates_db_and_retains_reference() {
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let db = crate::db::AmfDb::open(tmp.path()).unwrap();
    let list = db
        .create_todo_list(&test_project_scope("proj-1"), Some("feat-1"))
        .unwrap();
    let todo = db
        .add_todo(&list.id, "finish sidebar", None, TodoPriority::High)
        .unwrap();
    app.db = Some(db);
    app.store.projects[0].features[0]
        .sessions
        .push(FeatureSession {
            id: "session-todo".into(),
            kind: SessionKind::Claude,
            label: "Agent".into(),
            tmux_window: "claude".into(),
            claude_session_id: None,
            todo_reference: Some(TodoSessionReference {
                todo_id: todo.id.clone(),
                launched_from_todo_menu: true,
            }),
            token_usage_source: None,
            token_usage_source_match: None,
            created_at: Utc::now(),
            command: None,
            on_stop: None,
            pre_check: None,
            status_text: None,
            token_usage: None,
        });
    app.mode = AppMode::Viewing(ViewState::new(
        "my-project".into(),
        "my-feat".into(),
        "amf-my-feat".into(),
        "claude".into(),
        "Agent".into(),
        SessionKind::Claude,
        VibeMode::default(),
        false,
    ));
    crate::handlers::handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL),
        24,
    )
    .unwrap();
    crate::handlers::handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE),
        24,
    )
    .unwrap();
    assert!(matches!(
        app.mode,
        AppMode::ConfirmTodoReferenceCompletion(_)
    ));
    crate::handlers::handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
        24,
    )
    .unwrap();

    let loaded = app
        .db
        .as_ref()
        .unwrap()
        .find_todo_by_id(&todo.id)
        .unwrap()
        .unwrap();
    assert_eq!(loaded.work.status, TodoStatus::Completed);
    assert!(matches!(app.mode, AppMode::Viewing(_)));
    assert!(
        app.store.projects[0].features[0].sessions[0]
            .todo_reference
            .is_some()
    );
    // The sidebar text is served from a cache refreshed on completion, not
    // re-resolved from SQLite per frame.
    let cached = app
        .active_todos_sidebar_cache
        .get("session-todo")
        .expect("completion refreshes this session's active TODO cache");
    assert!(cached.contains("finish sidebar"));
    assert!(cached.ends_with("State: completed"));
}

/// Planning a TODO into a brand-new feature must leave the same sidebar
/// provenance the direct "start an agent in a new feature" route records:
/// `link_todo_to_new_feature` tags the feature's initial agent session, the
/// active-TODO cache picks it up, and the row's work state names that session.
#[test]
fn planning_a_todo_into_a_new_feature_attaches_the_active_todo_sidebar_reference() {
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let session_id = app.store.projects[0].features[0]
        .add_session_named(SessionKind::Claude, "Claude 1".to_string())
        .id
        .clone();

    let tmp = tempfile::NamedTempFile::new().unwrap();
    let db = crate::db::AmfDb::open(tmp.path()).unwrap();
    let list = db
        .create_todo_list(&test_project_scope("proj-1"), Some("feat-1"))
        .unwrap();
    let todo = db
        .add_todo(&list.id, "wire the sidebar", None, TodoPriority::High)
        .unwrap();
    app.db = Some(db);
    // Plan mode marks the row in progress when it begins, before the feature
    // exists, so it has no session association yet.
    app.todos_mark_in_progress(&todo.id, None).unwrap();

    let origin = TodoPlanOrigin {
        todo_id: todo.id.clone(),
        list_id: list.id.clone(),
        todo_title: "wire the sidebar".to_string(),
        host_feature_id: "feat-1".to_string(),
    };
    app.link_todo_to_new_feature(&origin, "my-project", "my-feat");

    let session = &app.store.projects[0].features[0].sessions[0];
    let reference = session
        .todo_reference
        .as_ref()
        .expect("the planned feature's agent session carries a TODO reference");
    assert_eq!(reference.todo_id, todo.id);
    assert!(reference.launched_from_todo_menu);

    let cached = app
        .active_todos_sidebar_cache
        .get(&session_id)
        .expect("linking refreshes the planned session's active TODO cache");
    assert!(cached.contains("wire the sidebar"));
    assert!(cached.ends_with("State: open"));

    let loaded = app
        .db
        .as_ref()
        .unwrap()
        .find_todo_by_id(&todo.id)
        .unwrap()
        .unwrap();
    assert_eq!(loaded.linked_feature_id.as_deref(), Some("feat-1"));
    assert_eq!(
        loaded.work.agent_session_id.as_deref(),
        Some(session_id.as_str())
    );
}

/// The host-feature plan route (`start_todo_plan_session`) spins up a fresh
/// agent session on the accepted plan; that session must carry the same
/// sidebar reference the non-plan routes record.
#[test]
fn starting_a_host_feature_plan_session_attaches_the_active_todo_sidebar_reference() {
    let mut tmux = MockTmuxOps::new();
    tmux.expect_list_sessions().returning(|| Ok(vec![]));
    tmux.expect_list_panes().returning(Vec::new);
    tmux.expect_session_exists().return_const(true);
    tmux.expect_create_window().returning(|_, _, _| Ok(()));
    tmux.expect_launch_claude()
        .returning(|_, _, _, _, _| Ok(()));
    tmux.expect_resize_pane().returning(|_, _, _, _| Ok(()));
    tmux.expect_select_window().returning(|_, _| Ok(()));

    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );
    app.config.max_concurrent_agents = 8;
    app.config.low_memory_warn_mb = 0;

    let tmp = tempfile::NamedTempFile::new().unwrap();
    let db = crate::db::AmfDb::open(tmp.path()).unwrap();
    let list = db
        .create_todo_list(&test_project_scope("proj-1"), Some("feat-1"))
        .unwrap();
    let todo = db
        .add_todo(&list.id, "plan then build", None, TodoPriority::High)
        .unwrap();
    app.db = Some(db);
    app.todos_mark_in_progress(&todo.id, None).unwrap();

    let origin = TodoPlanOrigin {
        todo_id: todo.id.clone(),
        list_id: list.id.clone(),
        todo_title: "plan then build".to_string(),
        host_feature_id: "feat-1".to_string(),
    };
    app.start_todo_plan_session(
        &origin,
        std::path::Path::new("/tmp/test-workdir"),
        "plan.md",
    )
    .unwrap();

    let session = app.store.projects[0].features[0]
        .sessions
        .iter()
        .find(|s| {
            s.todo_reference
                .as_ref()
                .is_some_and(|r| r.todo_id == todo.id)
        })
        .expect("the plan session is tagged with its TODO");
    assert!(
        session
            .todo_reference
            .as_ref()
            .unwrap()
            .launched_from_todo_menu
    );

    let cached = app
        .active_todos_sidebar_cache
        .get(&session.id)
        .expect("starting the plan session refreshes its active TODO cache");
    assert!(cached.contains("plan then build"));
}

/// `attach_launched_todo_reference` is the shared write behind both plan
/// routes: set the provenance, persist, refresh the cache — and no-op cleanly
/// if the session index has gone stale.
#[test]
fn attach_launched_todo_reference_records_provenance_and_survives_a_stale_index() {
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let session_id = app.store.projects[0].features[0]
        .add_session_named(SessionKind::Claude, "Claude 1".to_string())
        .id
        .clone();

    let tmp = tempfile::NamedTempFile::new().unwrap();
    let db = crate::db::AmfDb::open(tmp.path()).unwrap();
    let list = db
        .create_todo_list(&test_project_scope("proj-1"), Some("feat-1"))
        .unwrap();
    let todo = db
        .add_todo(&list.id, "attach me", None, TodoPriority::Med)
        .unwrap();
    app.db = Some(db);

    app.attach_launched_todo_reference(0, 0, 0, &todo.id);

    assert!(
        app.store.projects[0].features[0].sessions[0]
            .todo_reference
            .as_ref()
            .is_some_and(|r| r.todo_id == todo.id && r.launched_from_todo_menu)
    );
    assert!(
        app.active_todos_sidebar_cache
            .get(&session_id)
            .is_some_and(|c| c.contains("attach me"))
    );

    // An out-of-range session index is a no-op, not a panic.
    app.attach_launched_todo_reference(0, 0, 9, &todo.id);
}

#[test]
fn request_todo_reference_completion_without_db_warns_instead_of_opening_dialog() {
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.store.projects[0].features[0]
        .sessions
        .push(FeatureSession {
            id: "session-todo".into(),
            kind: SessionKind::Claude,
            label: "Agent".into(),
            tmux_window: "claude".into(),
            claude_session_id: None,
            todo_reference: Some(TodoSessionReference {
                todo_id: "todo-1".into(),
                launched_from_todo_menu: true,
            }),
            token_usage_source: None,
            token_usage_source_match: None,
            created_at: Utc::now(),
            command: None,
            on_stop: None,
            pre_check: None,
            status_text: None,
            token_usage: None,
        });
    app.mode = AppMode::Viewing(ViewState::new(
        "my-project".into(),
        "my-feat".into(),
        "amf-my-feat".into(),
        "claude".into(),
        "Agent".into(),
        SessionKind::Claude,
        VibeMode::default(),
        false,
    ));

    app.request_todo_reference_completion();

    assert!(matches!(app.mode, AppMode::Viewing(_)));
    assert!(
        app.toasts
            .iter()
            .any(|toast| toast.message.contains("persistence is unavailable"))
    );
}

#[test]
fn failed_agent_session_startup_and_prompt_setup_both_roll_back_todo_reservation() {
    let mut app = todos_app();
    let todo = sample_todo("a", false);
    if let AppMode::Todos(state) = &mut app.mode {
        state.panes[0].todos = vec![todo.clone()];
    }

    // Agent-creation failure path.
    assert!(app.todos_reserve_launch(&todo).unwrap());
    app.todos_rollback_launch(&todo.id).unwrap();
    match &app.mode {
        AppMode::Todos(state) => {
            assert_eq!(
                state.panes[0].todos[0].work,
                crate::db::todos::TodoWorkState::default()
            )
        }
        _ => panic!("expected Todos overlay"),
    }

    // Prompt-delivery failure path uses the same centralized rollback.
    let current = match &app.mode {
        AppMode::Todos(state) => state.panes[0].todos[0].clone(),
        _ => unreachable!(),
    };
    assert!(app.todos_reserve_launch(&current).unwrap());
    app.todos_mark_in_progress(&todo.id, Some("session-created"))
        .unwrap();
    app.todos_rollback_launch(&todo.id).unwrap();
    match &app.mode {
        AppMode::Todos(state) => {
            assert_eq!(
                state.panes[0].todos[0].work,
                crate::db::todos::TodoWorkState::default()
            )
        }
        _ => panic!("expected Todos overlay"),
    }
}

/// The scenario a plan interview hits between generating a plan and the user
/// accepting it: no `Todos` overlay is open (so there is no in-memory pane to
/// consult) and the TODO was moved to a different list in the meantime. The
/// lookup must still find it by id alone rather than the stale list it
/// started in — see `App::find_todo_by_id` and `start_todo_plan_session`.
#[test]
fn find_todo_by_id_resolves_after_a_move_when_no_overlay_is_open() {
    let mut app = todos_app();
    let tmp = tempfile::NamedTempFile::new().unwrap();
    app.db = Some(crate::db::AmfDb::open(tmp.path()).unwrap());

    let (dst_id, todo_id) = {
        let db = app.db.as_ref().unwrap();
        let src = db
            .create_todo_list(&test_project_scope("proj-1"), Some("feat-1"))
            .unwrap();
        let dst = db
            .create_todo_list(&crate::db::todos::TodoScope::Global, None)
            .unwrap();
        let todo = db
            .add_todo(
                &src.id,
                "port me",
                None,
                crate::db::todos::TodoPriority::Med,
            )
            .unwrap();
        db.move_todo(&todo.id, &dst.id).unwrap();
        (dst.id, todo.id)
    };
    // Whatever list the caller remembers (or none at all) must not matter.
    app.mode = AppMode::Normal;

    let found = app
        .find_todo_by_id(&todo_id)
        .expect("resolved via the db by id alone, without a stale list_id");
    assert_eq!(found.list_id, dst_id);
}

#[test]
fn reconciliation_clears_missing_session_but_keeps_in_progress_status() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let db = crate::db::AmfDb::open(tmp.path()).unwrap();
    let list = db
        .create_todo_list(&test_project_scope("proj-1"), Some("feat-1"))
        .unwrap();
    let mut todo = db
        .add_todo(&list.id, "a", None, crate::db::todos::TodoPriority::Med)
        .unwrap();
    todo.work.status = crate::db::todos::TodoStatus::InProgress;
    todo.work.agent_session_id = Some("missing-session".to_string());
    db.update_todo(&todo).unwrap();

    let mut app = todos_app();
    app.db = Some(db);
    app.reconcile_todo_agent_associations().unwrap();

    let loaded = app.db.as_ref().unwrap().todos(&list.id).unwrap();
    assert!(loaded[0].work.status.is_in_progress());
    assert!(loaded[0].work.agent_session_id.is_none());
}

fn todo_pane(
    kind: crate::app::TodoPaneKind,
    scope: crate::db::todos::TodoScope,
    todos: Vec<crate::db::todos::Todo>,
) -> crate::app::TodoPane {
    crate::app::TodoPane {
        kind,
        scope,
        title: kind.label().to_string(),
        list: None,
        todos,
        selected: 0,
        scroll_offset: 0,
    }
}

/// A three-pane overlay, which is the shape every cross-scope behaviour is
/// about.
fn three_pane_view(
    worktree: Vec<crate::db::todos::Todo>,
    project: Vec<crate::db::todos::Todo>,
    global: Vec<crate::db::todos::Todo>,
) -> TodoViewState {
    use crate::app::TodoPaneKind;
    let mut state = todo_view_with(vec![]);
    state.panes = vec![
        todo_pane(
            TodoPaneKind::Worktree,
            worktree_scope("proj-1", "/tmp/test-workdir"),
            worktree,
        ),
        todo_pane(TodoPaneKind::Project, test_project_scope("proj-1"), project),
        todo_pane(
            TodoPaneKind::Global,
            crate::db::todos::TodoScope::Global,
            global,
        ),
    ];
    state.focus = Some(0);
    state
}

fn prioritised(title: &str, priority: crate::db::todos::TodoPriority) -> crate::db::todos::Todo {
    let mut todo = sample_todo(title, false);
    todo.priority = priority;
    todo
}

/// Priority is the first question, scope only the tie-break: a low-priority
/// worktree item does not beat a high-priority global one.
#[test]
fn next_todo_across_puts_priority_before_scope() {
    use crate::app::todos::NextTodo;
    use crate::db::todos::TodoPriority;

    let worktree = vec![prioritised("wt-low", TodoPriority::Low)];
    let project = vec![prioritised("proj-med", TodoPriority::Med)];
    let global = vec![prioritised("global-high", TodoPriority::High)];

    assert_eq!(
        App::next_todo_across(&[&worktree, &project, &global], &[]),
        Some((2, NextTodo::Ready(0))),
        "the High item wins even though it is in the widest scope"
    );
}

/// At equal priority the narrower scope wins: worktree, then project, then
/// global.
#[test]
fn next_todo_across_breaks_equal_priority_ties_worktree_first() {
    use crate::app::todos::NextTodo;
    use crate::db::todos::TodoPriority;

    let worktree = vec![prioritised("wt", TodoPriority::Med)];
    let project = vec![prioritised("proj", TodoPriority::Med)];
    let global = vec![prioritised("global", TodoPriority::Med)];

    assert_eq!(
        App::next_todo_across(&[&worktree, &project, &global], &[]),
        Some((0, NextTodo::Ready(0)))
    );
    // Drop the worktree list and the project one inherits the tie.
    assert_eq!(
        App::next_todo_across(&[&[], &project, &global], &[]),
        Some((1, NextTodo::Ready(0)))
    );
    assert_eq!(
        App::next_todo_across(&[&[], &[], &global], &[]),
        Some((2, NextTodo::Ready(0)))
    );
}

/// Manual order still breaks ties *within* a list; scope only decides between
/// lists.
#[test]
fn next_todo_across_keeps_manual_order_within_a_list() {
    use crate::app::todos::NextTodo;
    let project = vec![sample_todo("first", false), sample_todo("second", false)];
    assert_eq!(
        App::next_todo_across(&[&[], &project, &[]], &[]),
        Some((1, NextTodo::Ready(0)))
    );
}

/// A started item in a *narrower* scope is still only held in reserve: an
/// unstarted item anywhere — even in the global list — outranks it.
#[test]
fn next_todo_across_holds_started_items_in_reserve_across_scopes() {
    use crate::app::todos::NextTodo;
    use crate::db::todos::TodoPriority;

    let mut started = prioritised("wt-high-started", TodoPriority::High);
    started.work.agent_session_id = Some("sess-1".to_string());
    let worktree = vec![started];
    let global = vec![prioritised("global-low", TodoPriority::Low)];

    assert_eq!(
        App::next_todo_across(&[&worktree, &[], &global], &[]),
        Some((2, NextTodo::Ready(0))),
        "an unstarted low-priority global item beats a started high-priority worktree one"
    );
}

/// …and it is returned, as `Started`, only once nothing unstarted remains in
/// any visible scope.
#[test]
fn next_todo_across_returns_started_only_when_nothing_unstarted_remains_anywhere() {
    use crate::app::todos::NextTodo;

    let mut started = sample_todo("wt-started", false);
    started.work.agent_session_id = Some("sess-1".to_string());
    let worktree = vec![started];
    let project = vec![sample_todo("proj-done", true)];
    let mut busy = sample_todo("global-busy", false);
    busy.work.status = crate::db::todos::TodoStatus::InProgress;
    let global = vec![busy];

    assert_eq!(
        App::next_todo_across(&[&worktree, &project, &global], &[]),
        Some((0, NextTodo::Started(0))),
        "done and in-progress items are passed over entirely, leaving the reserve"
    );
}

#[test]
fn next_todo_across_returns_nothing_when_every_list_is_empty() {
    assert_eq!(App::next_todo_across(&[&[], &[], &[]], &[]), None);
}

/// The refusal has to count every visible list, not just one: "nothing to do"
/// would be wrong when the other panes are full of in-progress work.
#[test]
fn no_next_todo_message_across_counts_every_list() {
    let mut busy = sample_todo("b", false);
    busy.work.status = crate::db::todos::TodoStatus::InProgress;
    let project = vec![busy];

    assert_eq!(
        App::no_next_todo_message_across(&[&[], &[], &[]], &[]),
        "No TODOs left to implement"
    );
    assert_eq!(
        App::no_next_todo_message_across(&[&[], &project, &[]], &[]),
        "All remaining TODOs are already in progress"
    );
    assert_eq!(
        App::no_next_todo_message_across(&[&[], &project, &[]], &["a".to_string()]),
        "No other TODOs left to implement"
    );
}

// ----- which scopes are visible -------------------------------------------

#[test]
fn todo_scope_visibility_is_independent_and_worktree_is_always_visible() {
    use crate::app::TodoPaneKind;
    let mut app = todos_app();
    make_feature_a_worktree(&mut app);

    let all = app.visible_todo_scopes(0, 0);
    assert_eq!(
        all.iter().map(|(k, _)| *k).collect::<Vec<_>>(),
        vec![
            TodoPaneKind::Worktree,
            TodoPaneKind::Project,
            TodoPaneKind::Global
        ]
    );
    app.todo_project_visible = false;
    assert_eq!(
        app.visible_todo_scopes(0, 0)
            .iter()
            .map(|(kind, _)| *kind)
            .collect::<Vec<_>>(),
        vec![TodoPaneKind::Worktree, TodoPaneKind::Global]
    );
    app.todo_global_visible = false;
    assert_eq!(
        app.visible_todo_scopes(0, 0)
            .iter()
            .map(|(kind, _)| *kind)
            .collect::<Vec<_>>(),
        vec![TodoPaneKind::Worktree]
    );

    let worktree = worktree_scope("proj-1", "/tmp/test-workdir");
    assert!(app.toggle_todo_scope_visibility(&worktree));
    assert!(app.todo_scope_visible(&worktree));
    assert!(!app.todo_project_visible);
    assert!(!app.todo_global_visible);
}

// ----- pane focus ----------------------------------------------------------

#[test]
fn tab_moves_focus_between_the_visible_panes() {
    let mut app = todos_app();
    app.mode = AppMode::Todos(three_pane_view(vec![], vec![], vec![]));

    let focus = |app: &App| match &app.mode {
        AppMode::Todos(state) => state.focus,
        _ => panic!("expected Todos overlay"),
    };

    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Tab)).unwrap();
    assert_eq!(focus(&app), Some(1));
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Tab)).unwrap();
    assert_eq!(focus(&app), Some(2));
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Tab)).unwrap();
    assert_eq!(focus(&app), Some(0), "focus wraps");
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::BackTab)).unwrap();
    assert_eq!(focus(&app), Some(2), "Shift+Tab wraps the other way");
}

#[test]
fn navigation_skips_hidden_scopes() {
    let mut app = todos_app();
    app.mode = AppMode::Todos(three_pane_view(vec![], vec![], vec![]));
    app.todo_project_visible = false;

    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Tab)).unwrap();
    assert!(matches!(&app.mode, AppMode::Todos(state) if state.focus == Some(2)));
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Tab)).unwrap();
    assert!(matches!(&app.mode, AppMode::Todos(state) if state.focus == Some(0)));
}

#[test]
fn hiding_the_focused_scope_advances_and_wraps() {
    let mut app = todos_app();
    app.mode = AppMode::Todos(three_pane_view(vec![], vec![], vec![]));
    if let AppMode::Todos(state) = &mut app.mode {
        state.focus = Some(1);
    }
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('p'))).unwrap();
    assert!(matches!(&app.mode, AppMode::Todos(state) if state.focus == Some(2)));

    // Restore project without moving focus, then hide global: the search wraps
    // from global back to worktree.
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('p'))).unwrap();
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('g'))).unwrap();
    assert!(matches!(&app.mode, AppMode::Todos(state) if state.focus == Some(0)));
}

#[test]
fn repo_root_view_supports_no_visible_actionable_pane_and_recovers() {
    let mut app = todos_app();
    let mut state = todo_view_with(vec![]);
    state.panes.push(todo_pane(
        crate::app::TodoPaneKind::Global,
        crate::db::todos::TodoScope::Global,
        vec![],
    ));
    app.mode = AppMode::Todos(state);

    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('p'))).unwrap();
    assert!(matches!(&app.mode, AppMode::Todos(state) if state.focus == Some(1)));
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('p'))).unwrap();
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('g'))).unwrap();
    assert!(matches!(&app.mode, AppMode::Todos(state) if state.focus == Some(0)));
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('p'))).unwrap();
    assert!(matches!(&app.mode, AppMode::Todos(state) if state.focus.is_none()));

    app.todos_select_next();
    app.todos_begin_add();
    assert!(matches!(&app.mode, AppMode::Todos(state) if state.editor.is_none()));
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('g'))).unwrap();
    assert!(matches!(&app.mode, AppMode::Todos(state) if state.focus == Some(1)));
}

#[test]
fn visibility_keys_replace_backslash_without_losing_priority_or_launch() {
    use crate::db::todos::TodoPriority;

    let mut app = todos_app();
    app.mode = AppMode::Todos(three_pane_view(
        vec![sample_todo("work", false)],
        vec![],
        vec![],
    ));

    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('\\'))).unwrap();
    assert!(app.todo_project_visible && app.todo_global_visible);
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('p'))).unwrap();
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('g'))).unwrap();
    assert!(!app.todo_project_visible && !app.todo_global_visible);

    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('P'))).unwrap();
    assert!(matches!(
        &app.mode,
        AppMode::Todos(state) if state.panes[0].todos[0].priority == TodoPriority::Low
    ));
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Enter)).unwrap();
    assert!(matches!(
        &app.mode,
        AppMode::Todos(state) if state.launch.is_some()
    ));
}

#[test]
fn visibility_is_shared_across_views_and_survives_reopening() {
    let mut app = todos_app();
    let mut second = app.store.projects[0].features[0].clone();
    second.id = "feat-2".to_string();
    second.name = "other-feat".to_string();
    second.workdir = PathBuf::from("/tmp/other-workdir");
    app.store.projects[0].features.push(second);

    app.open_todos_view(0, 0).unwrap();
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('p'))).unwrap();
    app.close_todos_view();
    app.open_todos_view(0, 0).unwrap();
    assert!(!app.todo_project_visible);
    assert!(app.todo_global_visible);

    app.close_todos_view();
    app.open_todos_view(0, 1).unwrap();
    assert!(
        !app.todo_project_visible,
        "the second TODO view shares state"
    );
    assert!(app.todo_global_visible);
}

#[test]
fn new_app_starts_with_both_optional_scopes_visible() {
    let mut first = todos_app();
    first.todo_project_visible = false;
    first.todo_global_visible = false;

    let restarted = todos_app();
    assert!(restarted.todo_project_visible);
    assert!(restarted.todo_global_visible);
}

#[test]
fn hidden_pane_state_is_restored_unchanged() {
    let mut app = todos_app();
    app.mode = AppMode::Todos(three_pane_view(
        vec![],
        vec![sample_todo("p1", false), sample_todo("p2", false)],
        vec![],
    ));
    if let AppMode::Todos(state) = &mut app.mode {
        state.focus = Some(1);
        state.panes[1].selected = 1;
        state.panes[1].scroll_offset = 7;
    }
    app.todos_toggle_project_visibility();
    app.todos_toggle_project_visibility();
    assert!(matches!(
        &app.mode,
        AppMode::Todos(state)
            if state.panes[1].selected == 1
                && state.panes[1].scroll_offset == 7
                && state.panes[1].todos.len() == 2
    ));
}

#[test]
fn hidden_scopes_are_excluded_from_implement_next_priority_resolution() {
    use crate::db::todos::TodoPriority;

    let mut app = todos_app();
    let session_id = push_agent_session(&mut app, "sess-visible");
    let mut project = prioritised("project-visible", TodoPriority::Med);
    project.work.agent_session_id = Some(session_id.clone());
    let mut global = prioritised("global-hidden", TodoPriority::High);
    global.work.agent_session_id = Some(session_id);
    app.mode = AppMode::Todos(three_pane_view(vec![], vec![project], vec![global]));
    app.todo_global_visible = false;

    app.implement_next_todo_in_overlay().unwrap();
    assert!(matches!(
        &app.mode,
        AppMode::TodoImplementChoice(state) if state.todo_id == "todo-project-visible"
    ));
}

#[test]
fn hidden_scopes_are_excluded_from_cross_pane_move_targets() {
    let mut app = todos_app();
    app.mode = AppMode::Todos(three_pane_view(
        vec![sample_todo("move-me", false)],
        vec![],
        vec![],
    ));
    app.todo_project_visible = false;

    app.todos_begin_scope_move(false);
    match &app.mode {
        AppMode::Todos(state) => assert_eq!(
            state
                .scope_move
                .as_ref()
                .unwrap()
                .targets
                .iter()
                .map(|(_, index)| *index)
                .collect::<Vec<_>>(),
            vec![2]
        ),
        _ => panic!("expected Todos overlay"),
    }
}

/// Each pane keeps its own cursor: moving focus away and back lands where it
/// was left.
#[test]
fn each_pane_keeps_its_own_cursor() {
    let mut app = todos_app();
    app.mode = AppMode::Todos(three_pane_view(
        vec![sample_todo("w1", false), sample_todo("w2", false)],
        vec![sample_todo("p1", false), sample_todo("p2", false)],
        vec![],
    ));

    app.todos_select_next(); // worktree pane → index 1
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Tab)).unwrap();
    assert!(matches!(&app.mode, AppMode::Todos(s) if s.panes[1].selected == 0));
    app.todos_select_next(); // project pane → index 1
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::BackTab)).unwrap();

    match &app.mode {
        AppMode::Todos(state) => {
            assert_eq!(state.focus, Some(0));
            assert_eq!(state.panes[0].selected, 1, "the worktree cursor was kept");
            assert_eq!(state.panes[1].selected, 1);
        }
        _ => panic!("expected Todos overlay"),
    }
}

// ----- move / copy between scopes -----------------------------------------

/// A move re-files the same work, so whatever was started for it comes along.
#[test]
fn moving_a_todo_to_another_scope_carries_its_links() {
    let mut app = todos_app();
    let mut todo = sample_todo("port me", false);
    todo.work.agent_session_id = Some("sess-1".to_string());
    todo.linked_feature_id = Some("feat-planned".to_string());
    todo.work.status = crate::db::todos::TodoStatus::InProgress;
    app.mode = AppMode::Todos(three_pane_view(vec![todo], vec![], vec![]));

    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('M'))).unwrap();
    // The chooser offers the two panes the item is *not* in.
    match &app.mode {
        AppMode::Todos(state) => {
            let step = state.scope_move.as_ref().expect("chooser is open");
            assert!(!step.copy);
            assert_eq!(
                step.targets.iter().map(|(_, i)| *i).collect::<Vec<_>>(),
                vec![1, 2],
                "the pane it already lives in is not a destination"
            );
        }
        _ => panic!("expected Todos overlay"),
    }
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Enter)).unwrap();

    match &app.mode {
        AppMode::Todos(state) => {
            assert!(state.scope_move.is_none(), "the chooser closes");
            assert!(state.panes[0].todos.is_empty(), "it left the worktree pane");
            let moved = &state.panes[1].todos[0];
            assert_eq!(moved.title, "port me");
            assert_eq!(moved.work.agent_session_id.as_deref(), Some("sess-1"));
            assert_eq!(moved.linked_feature_id.as_deref(), Some("feat-planned"));
            assert!(moved.work.status.is_in_progress());
        }
        _ => panic!("expected Todos overlay"),
    }
}

/// A copy is a second, unstarted item — two panes must never both claim the
/// same session.
#[test]
fn copying_a_todo_to_another_scope_leaves_it_unstarted() {
    let mut app = todos_app();
    let mut todo = sample_todo("share me", false);
    todo.work.agent_session_id = Some("sess-1".to_string());
    todo.linked_feature_id = Some("feat-planned".to_string());
    todo.work.status = crate::db::todos::TodoStatus::InProgress;
    app.mode = AppMode::Todos(three_pane_view(vec![todo], vec![], vec![]));

    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('C'))).unwrap();
    // Second target: the global pane.
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('j'))).unwrap();
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Enter)).unwrap();

    match &app.mode {
        AppMode::Todos(state) => {
            let original = &state.panes[0].todos[0];
            assert_eq!(original.work.agent_session_id.as_deref(), Some("sess-1"));
            assert!(
                original.work.status.is_in_progress(),
                "the original is untouched"
            );

            assert!(
                state.panes[1].todos.is_empty(),
                "the project pane is not a target here"
            );
            let copy = &state.panes[2].todos[0];
            assert_eq!(copy.title, "share me");
            assert_ne!(copy.id, original.id);
            assert!(copy.work.agent_session_id.is_none());
            assert!(copy.linked_feature_id.is_none());
            assert!(copy.work.status.is_not_started());
        }
        _ => panic!("expected Todos overlay"),
    }
}

/// The scope chooser is refused with a reason when there is nothing selected.
#[test]
fn move_with_nothing_selected_says_so() {
    let mut app = todos_app();
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('M'))).unwrap();
    assert!(matches!(&app.mode, AppMode::Todos(s) if s.scope_move.is_none()));
    assert!(
        app.toasts
            .iter()
            .any(|t| t.message.contains("No TODO selected")),
        "got {:?}",
        app.toasts.iter().map(|t| &t.message).collect::<Vec<_>>()
    );
}

// ----- quick-capture target ------------------------------------------------

/// Quick capture writes to the list of the checkout the session is in.
#[test]
fn quick_capture_targets_the_features_own_worktree_list() {
    let mut app = todos_app();
    make_feature_a_worktree(&mut app);

    let scope = app.default_todo_scope(0, 0).unwrap();
    assert_eq!(
        scope,
        worktree_scope("proj-1", "/tmp/test-workdir"),
        "the workdir is the key, not the feature id"
    );
    assert_eq!(app.todo_scope_label(&scope), "Worktree · my-feat");
}

/// A feature sitting on the repo root has no worktree list, so the note falls
/// back to the project's.
#[test]
fn quick_capture_falls_back_to_the_project_list_at_the_repo_root() {
    let app = todos_app();
    assert!(!app.store.projects[0].features[0].is_worktree);

    let scope = app.default_todo_scope(0, 0).unwrap();
    assert_eq!(scope, test_project_scope("proj-1"));
    assert_eq!(app.todo_scope_label(&scope), "Project · my-project");
}

/// A trailing separator is the same checkout, not a second one.
#[test]
fn worktree_keys_ignore_a_trailing_separator() {
    assert_eq!(
        App::todo_workdir_key(std::path::Path::new("/tmp/wt/")),
        App::todo_workdir_key(std::path::Path::new("/tmp/wt"))
    );
    // A root path is not trimmed away to nothing.
    assert_eq!(App::todo_workdir_key(std::path::Path::new("/")), "/");
}

/// The overlay names the list it will write to, so the target is never a guess.
#[test]
fn quick_capture_overlay_names_its_target_list() {
    let mut app = todos_app();
    make_feature_a_worktree(&mut app);
    app.mode = AppMode::Viewing(view_state_for("my-project", "my-feat"));

    app.open_todo_quick_capture();

    match &app.mode {
        AppMode::TodoQuickCapture(state) => {
            assert_eq!(state.list_label, "Worktree · my-feat");
        }
        _ => panic!("expected the quick-capture overlay"),
    }
}

// ----- feature deletion disposition ---------------------------------------

/// A worktree feature with unfinished TODOs, its list already in the DB.
fn app_with_worktree_todos(unfinished: usize, done: usize) -> (App, String) {
    let store = store_with_feature(ProjectStatus::Active);
    let db_dir = TempDir::new().unwrap();
    let db_path = db_dir.keep().join("amf.db");
    let db = crate::db::AmfDb::open(&db_path).unwrap();
    db.save_store(&store).unwrap();

    let scope = worktree_scope("proj-1", "/tmp/test-workdir");
    let list = db.create_todo_list(&scope, Some("feat-1")).unwrap();
    for i in 0..unfinished {
        db.add_todo(
            &list.id,
            &format!("open-{i}"),
            None,
            crate::db::todos::TodoPriority::Med,
        )
        .unwrap();
    }
    for i in 0..done {
        let mut todo = db
            .add_todo(
                &list.id,
                &format!("done-{i}"),
                None,
                crate::db::todos::TodoPriority::Med,
            )
            .unwrap();
        todo.work.status = crate::db::todos::TodoStatus::Completed;
        db.update_todo(&todo).unwrap();
    }

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.db = Some(db);
    make_feature_a_worktree(&mut app);
    (app, list.id)
}

/// Deleting the worktree deletes its list, so open work in it is asked about
/// rather than decided on the user's behalf.
#[test]
fn deleting_a_feature_with_unfinished_worktree_todos_asks_first() {
    let (mut app, _list_id) = app_with_worktree_todos(2, 1);

    app.mode = AppMode::DeletingFeature("my-project".to_string(), "my-feat".to_string());
    app.delete_feature().unwrap();

    match &app.mode {
        AppMode::TodoDeleteDisposition(state) => {
            assert_eq!(state.unfinished, 2, "completed items are not at stake");
            assert_eq!(state.feature_name, "my-feat");
            assert_eq!(state.workdir, "/tmp/test-workdir");
            assert_eq!(
                state.choice(),
                crate::app::TodoDeleteDisposition::MoveToProject,
                "the least destructive option is the default"
            );
        }
        _ => panic!("expected the disposition prompt"),
    }
    // Nothing has been touched yet.
    assert!(app.store.find_project("my-project").is_some());
    assert_eq!(app.store.projects[0].features.len(), 1);
}

/// A list with nothing open in it is not worth a prompt — there is no work to
/// lose — and neither is a feature on the repo root, which has no worktree
/// list at all.
#[test]
fn a_worktree_list_with_no_open_work_does_not_prompt() {
    let (app, _) = app_with_worktree_todos(0, 3);
    assert!(
        app.pending_todo_disposition("my-project", "my-feat")
            .is_none()
    );

    let (mut app, _) = app_with_worktree_todos(2, 0);
    app.store.projects[0].features[0].is_worktree = false;
    assert!(
        app.pending_todo_disposition("my-project", "my-feat")
            .is_none(),
        "a repo-root feature has no worktree list to disposition"
    );
}

#[test]
fn disposition_move_to_project_relocates_the_open_todos_and_drops_the_list() {
    use crate::app::TodoDeleteDisposition;
    let (mut app, list_id) = app_with_worktree_todos(2, 1);
    let state = app
        .pending_todo_disposition("my-project", "my-feat")
        .unwrap();

    app.apply_todo_disposition(&state, TodoDeleteDisposition::MoveToProject)
        .unwrap();

    let db = app.db.as_ref().unwrap();
    // The worktree list is gone, and with it the completed items.
    assert!(db.todo_list_by_id(&list_id).unwrap().is_none());
    assert!(
        db.todo_list(&worktree_scope("proj-1", "/tmp/test-workdir"))
            .unwrap()
            .is_none()
    );
    // The open ones landed in the project list, which was created for them.
    let project_list = db
        .todo_list(&test_project_scope("proj-1"))
        .unwrap()
        .unwrap();
    let titles: Vec<String> = db
        .todos(&project_list.id)
        .unwrap()
        .into_iter()
        .map(|t| t.title)
        .collect();
    assert_eq!(titles, vec!["open-0", "open-1"]);
}

/// The project list created by the move must not be hosted on the feature that
/// is about to be deleted: `handle_todos_host_feature_deleted` would then find
/// its host gone and — with no features left — delete the very items the user
/// just chose to keep.
#[test]
fn disposition_move_to_project_does_not_host_the_list_on_the_doomed_feature() {
    use crate::app::TodoDeleteDisposition;
    let (mut app, _) = app_with_worktree_todos(2, 0);
    let state = app
        .pending_todo_disposition("my-project", "my-feat")
        .unwrap();

    app.apply_todo_disposition(&state, TodoDeleteDisposition::MoveToProject)
        .unwrap();

    let project_list = {
        let db = app.db.as_ref().unwrap();
        let list = db
            .todo_list(&test_project_scope("proj-1"))
            .unwrap()
            .unwrap();
        assert_ne!(
            list.feature_id.as_deref(),
            Some("feat-1"),
            "the list would be orphaned the moment the deletion completes"
        );
        list
    };

    // Now finish the deletion the way the real flow does.
    app.store.projects[0].features.clear();
    app.handle_todos_host_feature_deleted("my-project", "my-feat", Some("feat-1"));

    let db = app.db.as_ref().unwrap();
    let titles: Vec<String> = db
        .todos(&project_list.id)
        .unwrap()
        .into_iter()
        .map(|t| t.title)
        .collect();
    assert_eq!(
        titles,
        vec!["open-0", "open-1"],
        "the moved items survive the deletion that prompted the move"
    );
}

/// When the project has another feature, that one hosts the new project list —
/// the host is a hint for later lookups, so a real one beats none.
#[test]
fn disposition_move_to_project_hosts_the_list_on_a_surviving_feature() {
    use crate::app::TodoDeleteDisposition;
    let (mut app, _) = app_with_worktree_todos(1, 0);
    let mut survivor = app.store.projects[0].features[0].clone();
    survivor.id = "feat-2".to_string();
    survivor.name = "other-feat".to_string();
    survivor.workdir = PathBuf::from("/tmp/other-workdir");
    app.store.projects[0].features.push(survivor);

    let state = app
        .pending_todo_disposition("my-project", "my-feat")
        .unwrap();
    app.apply_todo_disposition(&state, TodoDeleteDisposition::MoveToProject)
        .unwrap();

    let list_id = {
        let db = app.db.as_ref().unwrap();
        let list = db
            .todo_list(&test_project_scope("proj-1"))
            .unwrap()
            .unwrap();
        assert_eq!(list.feature_id.as_deref(), Some("feat-2"));
        list.id
    };

    // The host outlives the deletion, so there is no re-home prompt.
    app.store.projects[0].features.remove(0);
    assert!(!app.handle_todos_host_feature_deleted("my-project", "my-feat", Some("feat-1")));
    assert!(matches!(app.mode, AppMode::Normal));
    let db = app.db.as_ref().unwrap();
    assert_eq!(db.todos(&list_id).unwrap().len(), 1);
}

#[test]
fn disposition_move_to_global_takes_them_out_of_the_project() {
    use crate::app::TodoDeleteDisposition;
    let (mut app, _) = app_with_worktree_todos(1, 0);
    let state = app
        .pending_todo_disposition("my-project", "my-feat")
        .unwrap();

    app.apply_todo_disposition(&state, TodoDeleteDisposition::MoveToGlobal)
        .unwrap();

    let db = app.db.as_ref().unwrap();
    let global = db
        .todo_list(&crate::db::todos::TodoScope::Global)
        .unwrap()
        .unwrap();
    assert!(global.feature_id.is_none(), "the global list has no host");
    assert_eq!(db.todos(&global.id).unwrap().len(), 1);
    assert!(
        db.todo_list(&test_project_scope("proj-1"))
            .unwrap()
            .is_none(),
        "the project list is not created for a move that went past it"
    );
}

#[test]
fn disposition_delete_removes_the_list_and_its_items() {
    use crate::app::TodoDeleteDisposition;
    let (mut app, list_id) = app_with_worktree_todos(2, 1);
    let state = app
        .pending_todo_disposition("my-project", "my-feat")
        .unwrap();

    app.apply_todo_disposition(&state, TodoDeleteDisposition::Delete)
        .unwrap();

    let db = app.db.as_ref().unwrap();
    assert!(db.todo_list_by_id(&list_id).unwrap().is_none());
    assert!(db.todos(&list_id).unwrap().is_empty());
    assert!(
        db.todo_list(&test_project_scope("proj-1"))
            .unwrap()
            .is_none()
    );
    assert!(
        db.todo_list(&crate::db::todos::TodoScope::Global)
            .unwrap()
            .is_none()
    );
}

/// Cancel is the escape hatch on an irreversible action: nothing is deleted
/// and the feature stays.
#[test]
fn disposition_cancel_leaves_the_feature_and_its_todos_intact() {
    let (mut app, list_id) = app_with_worktree_todos(2, 0);
    app.mode = AppMode::DeletingFeature("my-project".to_string(), "my-feat".to_string());
    app.delete_feature().unwrap();
    assert!(matches!(app.mode, AppMode::TodoDeleteDisposition(_)));

    crate::handlers::handle_todo_delete_disposition_key(&mut app, KeyCode::Esc).unwrap();

    assert!(matches!(app.mode, AppMode::Normal));
    assert_eq!(
        app.store.projects[0].features.len(),
        1,
        "the feature is still there"
    );
    let db = app.db.as_ref().unwrap();
    assert!(db.todo_list_by_id(&list_id).unwrap().is_some());
    assert_eq!(db.todos(&list_id).unwrap().len(), 2);
}
