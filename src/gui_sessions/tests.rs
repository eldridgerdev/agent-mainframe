use super::*;
use crate::app::App;
use crate::app::session_ops::TEST_VSCODE_CLI;
use crate::db::editors::EditorKind;
use crate::extension::set_test_global_extension_config;
use crate::gui_contract::GuiErrorKind;
use crate::project::{AgentKind, Feature, Project, ProjectStore, VibeMode};
use crate::traits::{MockTmuxOps, MockWorktreeOps};
use std::path::Path;
use std::sync::{Arc, Mutex};

const PROJECT_ID: &str = "proj-1";
const FEATURE_ID: &str = "feat-1";

/// Points this test thread's VS Code CLI at `cli` (or at nothing) and the
/// global extension config at `global` (until `App::new_for_test` resets it),
/// restoring both afterwards: test threads can be reused.
struct Seams;

impl Seams {
    fn new(cli: Option<PathBuf>, global: Option<ExtensionConfig>) -> Self {
        TEST_VSCODE_CLI.with(|slot| *slot.borrow_mut() = cli);
        set_test_global_extension_config(Some(global.unwrap_or_default()));
        Seams
    }
}

impl Drop for Seams {
    fn drop(&mut self) {
        TEST_VSCODE_CLI.with(|slot| *slot.borrow_mut() = None);
        set_test_global_extension_config(None);
    }
}

fn target() -> FeatureTarget {
    FeatureTarget {
        project_id: PROJECT_ID.to_string(),
        feature_id: FEATURE_ID.to_string(),
    }
}

fn store(root: &Path, status: ProjectStatus) -> ProjectStore {
    let workdir = root.join("worktree");
    std::fs::create_dir_all(workdir.join("web")).unwrap();
    let mut feature = Feature::new_for_project(
        "demo",
        "feat".to_string(),
        "feat".to_string(),
        workdir,
        true,
        VibeMode::default(),
        false,
        false,
        AgentKind::default(),
        false,
        false,
    );
    feature.id = FEATURE_ID.to_string();
    feature.status = status;
    feature.add_session(SessionKind::Claude);
    let mut project = Project::new(
        "demo".to_string(),
        root.to_path_buf(),
        true,
        AgentKind::default(),
    );
    project.id = PROJECT_ID.to_string();
    project.features.push(feature);
    let mut store = ProjectStore::empty();
    store.projects.push(project);
    store
}

const AMF_JSON: &str = r#"{
  "custom_sessions": [
    {
      "name": "Dev server",
      "description": "Vite on :5173",
      "command": "npm run dev",
      "working_dir": "web",
      "icon": "web",
      "icon_nerd": "nf-md-web",
      "on_stop": "pkill -f vite",
      "autolaunch": true,
      "pre_check": "test -d ."
    },
    {
      "name": "Database",
      "command": "docker compose up db",
      "pre_check": "echo 'docker is not running' >&2; exit 3"
    }
  ]
}"#;

fn handle(root: &Path, status: ProjectStatus, tmux: MockTmuxOps) -> GuiHandle {
    std::fs::write(root.join("amf.json"), AMF_JSON).unwrap();
    let app = App::new_for_test(
        store(root, status),
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );
    GuiHandle::from_app(app)
}

fn attach_db(gui: &mut GuiHandle) -> tempfile::NamedTempFile {
    let file = tempfile::NamedTempFile::new().unwrap();
    let db = crate::db::AmfDb::open(file.path()).unwrap();
    let app = gui.app_for_workflow();
    db.save_store(&app.store).unwrap();
    app.db = Some(db);
    file
}

fn running_tmux() -> MockTmuxOps {
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().return_const(true);
    tmux
}

/// A `code` stand-in that answers `--version` and exits on a launch.
fn stub_cli(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("code");
    std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn request(gui: &mut GuiHandle, name: &str) -> AddCustomSessionRequest {
    let options = gui.new_session_options(&target()).unwrap();
    let option = options
        .custom
        .iter()
        .find(|option| option.name == name)
        .unwrap();
    AddCustomSessionRequest {
        target: target(),
        name: name.to_string(),
        revision: option.revision.clone(),
        label: None,
        approved: false,
    }
}

#[test]
fn options_follow_the_tui_picker_and_list_configured_sessions() {
    let root = tempfile::tempdir().unwrap();
    let global = ExtensionConfig {
        custom_sessions: vec![
            CustomSessionConfig {
                name: "Logs".into(),
                command: Some("tail -f log".into()),
                ..Default::default()
            },
            // The project's entry of the same name wins.
            CustomSessionConfig {
                name: "Database".into(),
                command: Some("global".into()),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let _seams = Seams::new(None, None);
    let mut gui = handle(root.path(), ProjectStatus::Idle, running_tmux());
    // `App::new_for_test` resets the global config seam.
    set_test_global_extension_config(Some(global));

    let options = gui.new_session_options(&target()).unwrap();

    let kinds: Vec<_> = options.builtin.iter().map(|o| o.kind.clone()).collect();
    assert_eq!(
        kinds[kinds.len() - 4..],
        [
            SessionKind::Terminal,
            SessionKind::Nvim,
            SessionKind::Vscode,
            SessionKind::Todos
        ]
    );
    let vscode = options
        .builtin
        .iter()
        .find(|o| o.kind == SessionKind::Vscode)
        .unwrap();
    assert_eq!(vscode.disabled.as_deref(), Some("code not found in PATH"));
    assert!(!options.feature_stopped);
    assert_eq!(options.config_warning, None);

    let names: Vec<_> = options.custom.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(names, ["Dev server", "Database", "Logs"]);
    let dev = &options.custom[0];
    assert_eq!(dev.icon.as_deref(), Some("web"));
    assert_eq!(dev.icon_nerd.as_deref(), Some("\u{f059f}"));
    assert_eq!(dev.working_dir.as_deref(), Some("web"));
    assert_eq!(dev.on_stop.as_deref(), Some("pkill -f vite"));
    assert!(dev.autolaunch);
    assert_eq!(dev.source, CustomSessionSource::Project);
    assert_eq!(
        options.custom[1].command.as_deref(),
        Some("docker compose up db")
    );
    assert_eq!(options.custom[2].source, CustomSessionSource::Global);
}

#[test]
fn vscode_is_offered_with_a_runnable_cli_and_todos_only_once() {
    let root = tempfile::tempdir().unwrap();
    let bin = tempfile::tempdir().unwrap();
    let _seams = Seams::new(Some(stub_cli(bin.path())), None);
    let mut gui = handle(root.path(), ProjectStatus::Idle, running_tmux());
    gui.app_for_workflow().store.projects[0].features[0].add_session(SessionKind::Todos);

    let options = gui.new_session_options(&target()).unwrap();

    let vscode = options
        .builtin
        .iter()
        .find(|o| o.kind == SessionKind::Vscode)
        .unwrap();
    assert_eq!(vscode.disabled, None);
    assert!(!options.builtin.iter().any(|o| o.kind == SessionKind::Todos));
}

#[test]
fn an_unreadable_project_config_is_reported_not_silently_dropped() {
    let root = tempfile::tempdir().unwrap();
    let _seams = Seams::new(None, None);
    let mut gui = handle(root.path(), ProjectStatus::Idle, running_tmux());
    std::fs::write(root.path().join("amf.json"), "{ not json").unwrap();

    let options = gui.new_session_options(&target()).unwrap();

    assert!(options.custom.is_empty());
    let warning = options.config_warning.unwrap();
    assert!(warning.contains("could not be parsed"), "{warning}");
}

#[test]
fn a_custom_session_opens_its_window_with_the_configured_command() {
    let root = tempfile::tempdir().unwrap();
    let _seams = Seams::new(None, None);
    let workdir = root.path().join("worktree");
    let mut tmux = running_tmux();
    let expected_dir = workdir.join("web");
    tmux.expect_create_window()
        .withf(move |session, window, dir| {
            session == "amf-demo-feat" && window == "dev-server" && dir == expected_dir
        })
        .times(1)
        .returning(|_, _, _| Ok(()));
    let commands = Arc::new(Mutex::new(Vec::new()));
    let seen = commands.clone();
    tmux.expect_run_shell_command()
        .times(1)
        .returning(move |_, _, command| {
            seen.lock().unwrap().push(command.to_string());
            Ok(())
        });
    let mut gui = handle(root.path(), ProjectStatus::Idle, tmux);
    gui.app_for_workflow().store.projects[0].features[0].tmux_session = "amf-demo-feat".to_string();
    let request = request(&mut gui, "Dev server");
    let gui = Mutex::new(gui);

    let response = add_custom_session(&gui, request).unwrap();

    let AddCustomSessionResponse::Added {
        target: added,
        label,
        autolaunch,
        ..
    } = response
    else {
        panic!("expected the session to be added: {response:?}");
    };
    assert_eq!(label, "Dev server");
    assert!(autolaunch);
    let mut gui = gui.into_inner().unwrap();
    let feature = &gui.snapshot().projects[0].features[0];
    let session = feature
        .sessions
        .iter()
        .find(|session| session.id == added.session_id)
        .expect("the new session is saved under its stable id");
    assert_eq!(session.kind, SessionKind::Custom);
    assert_eq!(session.on_stop.as_deref(), Some("pkill -f vite"));
    assert_eq!(session.pre_check.as_deref(), Some("test -d ."));
    let command = commands.lock().unwrap()[0].clone();
    assert!(command.contains("npm run dev"), "{command}");
    assert!(command.contains(&added.session_id), "{command}");
    assert!(command.contains("AMF_STATUS_DIR"), "{command}");
    assert!(gui.app_for_workflow().message.is_none());
}

#[test]
fn a_failing_pre_check_reports_its_output_and_creates_nothing() {
    let root = tempfile::tempdir().unwrap();
    let _seams = Seams::new(None, None);
    // No create_window/run_shell_command expectation: any launch panics.
    let mut gui = handle(root.path(), ProjectStatus::Idle, running_tmux());
    let request = request(&mut gui, "Database");
    let gui = Mutex::new(gui);

    let response = add_custom_session(&gui, request).unwrap();

    match response {
        AddCustomSessionResponse::PreCheckFailed {
            name,
            pre_check,
            output,
        } => {
            assert_eq!(name, "Database");
            assert!(pre_check.contains("exit 3"));
            assert_eq!(output, "docker is not running");
        }
        other => panic!("expected a pre_check failure: {other:?}"),
    }
    let gui = gui.into_inner().unwrap();
    assert_eq!(gui.snapshot().projects[0].features[0].sessions.len(), 1);
}

#[test]
fn a_changed_or_removed_configuration_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let _seams = Seams::new(None, None);
    let mut gui = handle(root.path(), ProjectStatus::Idle, running_tmux());
    let request = request(&mut gui, "Dev server");
    std::fs::write(
        root.path().join("amf.json"),
        AMF_JSON.replace("npm run dev", "npm run start"),
    )
    .unwrap();
    let gui = Mutex::new(gui);

    let changed = add_custom_session(&gui, request.clone()).unwrap_err();
    assert_eq!(changed.kind, GuiErrorKind::Conflict);
    assert!(changed.message.contains("changed"), "{}", changed.message);

    std::fs::write(root.path().join("amf.json"), r#"{"custom_sessions": []}"#).unwrap();
    let removed = add_custom_session(&gui, request).unwrap_err();
    assert_eq!(removed.kind, GuiErrorKind::NotFound);
}

#[test]
fn a_stale_feature_is_not_found() {
    let root = tempfile::tempdir().unwrap();
    let _seams = Seams::new(None, None);
    let mut gui = handle(root.path(), ProjectStatus::Idle, running_tmux());
    let mut request = request(&mut gui, "Dev server");
    request.target.feature_id = "deleted".to_string();

    let error = add_custom_session(&Mutex::new(gui), request).unwrap_err();

    assert_eq!(error.kind, GuiErrorKind::NotFound);
}

#[test]
fn a_custom_session_that_starts_the_feature_waits_for_approval() {
    let _lease_lock = crate::resources::limits::lock_lease_tests();
    assert_eq!(crate::resources::limits::wait_for_in_flight(0), 0);
    let _lease = crate::resources::limits::HeadlessLease::acquire();
    let root = tempfile::tempdir().unwrap();
    let _seams = Seams::new(None, None);
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().return_const(false);
    tmux.expect_list_panes().returning(Vec::new);
    let mut gui = handle(root.path(), ProjectStatus::Stopped, tmux);
    gui.app_for_workflow().config.max_concurrent_agents = 1;
    gui.app_for_workflow().config.low_memory_warn_mb = 0;
    let request = request(&mut gui, "Dev server");

    let error = add_custom_session(&Mutex::new(gui), request).unwrap_err();

    assert_eq!(error.kind, GuiErrorKind::NeedsApproval);
    assert!(
        error.message.contains("Adding 'Dev server'"),
        "{}",
        error.message
    );
}

#[test]
fn the_todos_session_is_added_once_per_feature() {
    let root = tempfile::tempdir().unwrap();
    let _seams = Seams::new(None, None);
    // No tmux use at all beyond the store refresh: the list is native.
    let mut gui = handle(root.path(), ProjectStatus::Stopped, MockTmuxOps::new());

    let added = gui
        .add_session(target(), SessionKind::Todos, None, false)
        .unwrap();

    assert_eq!(added.label, "TODOs");
    let feature = &gui.snapshot().projects[0].features[0];
    assert_eq!(feature.todos_session().unwrap().id, added.target.session_id);
    let again = gui
        .add_session(target(), SessionKind::Todos, None, false)
        .unwrap_err();
    assert_eq!(again.kind, GuiErrorKind::Conflict);
}

#[test]
fn vscode_without_its_cli_is_refused_before_anything_starts() {
    let root = tempfile::tempdir().unwrap();
    let _seams = Seams::new(None, None);
    // A stopped feature: starting it would need create_session_with_window.
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().return_const(false);
    let mut gui = handle(root.path(), ProjectStatus::Stopped, tmux);

    let error = gui.open_vscode(target(), true).unwrap_err();

    assert_eq!(error.kind, GuiErrorKind::Conflict);
    assert!(error.message.contains("`code`"), "{}", error.message);
}

#[test]
fn vscode_opens_a_tracked_window_and_refuses_a_second_while_it_resolves() {
    let root = tempfile::tempdir().unwrap();
    let bin = tempfile::tempdir().unwrap();
    let _seams = Seams::new(Some(stub_cli(bin.path())), None);
    let mut gui = handle(root.path(), ProjectStatus::Idle, running_tmux());
    let _db = attach_db(&mut gui);

    let opened = gui.open_vscode(target(), false).unwrap();

    assert!(!opened.started_feature);
    let editor_id = opened.editor_id.expect("the launch is recorded");
    let rows = gui
        .db()
        .unwrap()
        .launched_editors_for_feature(FEATURE_ID)
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, editor_id);
    assert!(!rows[0].dedicated, "owned only once a new window is found");
    assert!(rows[0].command.contains("--new-window"));
    let snapshot = gui.snapshot();
    let editors = &snapshot.sidebar.features[FEATURE_ID].editors;
    assert_eq!(editors.len(), 1);
    assert_eq!(editors[0].state, FeatureEditorState::Opening);
    assert!(editors[0].closes_with_feature);

    let again = gui.open_vscode(target(), false).unwrap_err();
    assert_eq!(again.kind, GuiErrorKind::Conflict);
    assert!(again.message.contains("still opening"), "{}", again.message);
}

fn record(gui: &mut GuiHandle, pid: i64, dedicated: bool, workdir: &Path) -> String {
    gui.db()
        .unwrap()
        .record_launched_editor(
            FEATURE_ID,
            None,
            EditorKind::Vscode,
            pid,
            workdir,
            dedicated,
            "code --new-window",
        )
        .unwrap()
        .id
}

#[test]
fn listed_editor_states_follow_ownership_and_liveness() {
    let root = tempfile::tempdir().unwrap();
    let _seams = Seams::new(None, None);
    let mut gui = handle(root.path(), ProjectStatus::Idle, running_tmux());
    let _db = attach_db(&mut gui);
    let workdir = root.path().join("worktree");
    let alive = std::process::id() as i64;
    let owned = record(&mut gui, alive, true, &workdir);
    // An owned window that has since closed is not listed.
    record(&mut gui, 999_999_999, true, &workdir);
    // Another process's launch, seconds old, still resolving.
    let fresh = record(&mut gui, 0, false, &workdir);

    let editors = gui.snapshot().sidebar.features[FEATURE_ID].editors.clone();

    let state = |id: &str| editors.iter().find(|e| e.id == id).map(|e| e.state);
    assert_eq!(editors.len(), 2);
    assert_eq!(state(&owned), Some(FeatureEditorState::Open));
    assert_eq!(state(&fresh), Some(FeatureEditorState::Opening));
}

/// A live process with VS Code's shape on the worktree, which this test
/// owns: AMF's identity check accepts it exactly as it would a real window.
fn lookalike(dir: &Path, workdir: &Path) -> crate::resources::test_support::TestChild {
    let fake = dir.join("code");
    std::os::unix::fs::symlink("/bin/bash", &fake).unwrap();
    std::process::Command::new(&fake)
        .args([
            "-c".as_ref(),
            "sleep 60 & wait".as_ref(),
            "--new-window".as_ref(),
            workdir.as_os_str(),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(crate::resources::test_support::TestChild::new)
        .unwrap()
}

#[test]
fn closing_editors_closes_only_owned_windows_and_refuses_an_unseen_one() {
    let root = tempfile::tempdir().unwrap();
    let bin = tempfile::tempdir().unwrap();
    let _seams = Seams::new(None, None);
    let mut gui = handle(root.path(), ProjectStatus::Idle, running_tmux());
    let db_file = attach_db(&mut gui);
    let workdir = root.path().join("worktree");
    let mut child = lookalike(bin.path(), &workdir);
    let pid = child.id() as i64;
    std::thread::sleep(std::time::Duration::from_millis(200));
    let owned = record(&mut gui, pid, true, &workdir);
    // Handed to someone else's instance: listed, never closed.
    let foreign = record(&mut gui, 0, false, &workdir);
    rusqlite::Connection::open(db_file.path())
        .unwrap()
        .execute(
            "UPDATE launched_editors SET started_at = '2020-01-01T00:00:00Z' WHERE id = ?1",
            [&foreign],
        )
        .unwrap();

    let stale = gui.close_editors(target(), vec![]).unwrap_err();
    assert_eq!(stale.kind, GuiErrorKind::Conflict);
    assert!(crate::resources::procs::pid_alive(pid));

    let closed = gui
        .close_editors(target(), vec![owned.clone(), foreign.clone()])
        .unwrap();

    assert!(!closed.already_closed);
    assert_eq!(closed.editors.killed.len(), 1);
    assert!(
        closed
            .editors
            .skipped
            .iter()
            .any(|skipped| skipped.deliberate),
        "the foreign window is reported as left running"
    );
    let _ = child.wait();
    let rows = gui
        .db()
        .unwrap()
        .launched_editors_for_feature(FEATURE_ID)
        .unwrap();
    assert_eq!(
        rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        [foreign.as_str()]
    );

    let again = gui.close_editors(target(), vec![owned]).unwrap();
    assert!(again.already_closed);
}
