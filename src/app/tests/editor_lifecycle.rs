use super::support::*;
use crate::app::*;
use crate::traits::{MockTmuxOps, MockWorktreeOps};
use tempfile::NamedTempFile;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Editor reclamation on stop
// ---------------------------------------------------------------------------

/// Stand-in for a VS Code window AMF opened: a symlink to Bash named `code`,
/// launched with a VS Code-shaped argv, holding a child of its own the way a
/// real window holds a language server.
fn spawn_fake_editor(
    dir: &std::path::Path,
    workdir: &std::path::Path,
) -> crate::resources::test_support::TestChild {
    let fake = dir.join("code");
    std::os::unix::fs::symlink("/bin/bash", &fake).expect("link bash as a fake editor");
    spawn_stand_in(std::process::Command::new(&fake).args([
        "-c".as_ref(),
        "sleep 60 & wait".as_ref(),
        "--new-window".as_ref(),
        workdir.as_os_str(),
    ]))
}

/// Symlinking the stand-in avoids copying an executable while other tests
/// fork, which can inherit a writable descriptor and cause `ETXTBSY`.
fn spawn_stand_in(
    command: &mut std::process::Command,
) -> crate::resources::test_support::TestChild {
    command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(crate::resources::test_support::TestChild::new)
        .expect("stand-in should launch")
}

/// The same stand-in, holding `windows` renderer subprocesses — how a real VS
/// Code instance carries the windows open inside it.
fn spawn_fake_editor_hosting_windows(
    dir: &std::path::Path,
    workdir: &std::path::Path,
    windows: usize,
) -> crate::resources::test_support::TestChild {
    let fake = dir.join("code");
    std::os::unix::fs::symlink("/bin/bash", &fake).expect("link bash as a fake editor");
    // The renderers are spawned from a script file rather than an inline `-c`
    // string: an inline one would put `--type=renderer` in the *parent's* argv,
    // which is exactly what marks a process as a helper rather than a window.
    let mut script = String::new();
    for id in 1..=windows {
        script.push_str(&format!(
            "{} -c 'sleep 60 & wait' --type=renderer --window-id={id} &\n",
            fake.display()
        ));
    }
    script.push_str("wait\n");
    let script_path = dir.join("windows.sh");
    std::fs::write(&script_path, script).expect("write window script");

    let child = spawn_stand_in(std::process::Command::new(&fake).args([
        script_path.as_os_str(),
        "--new-window".as_ref(),
        workdir.as_os_str(),
    ]));
    // Observe the renderer processes rather than assuming a fixed sleep is
    // enough on a loaded CI runner. The guard cleans up if this times out.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while crate::resources::procs::vscode_window_count(
        &crate::resources::procs::list_processes(),
        child.id() as i64,
    ) != windows
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the stand-in never hosted {windows} renderer processes"
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    child
}

/// An app whose feature lives in `workdir`, with a temp DB attached.
fn app_with_db(workdir: &std::path::Path, db_file: &NamedTempFile) -> App {
    let mut store = store_with_feature(ProjectStatus::Idle);
    store.projects[0].features[0].workdir = workdir.to_path_buf();

    let mut tmux = MockTmuxOps::new();
    tmux.expect_kill_session().returning(|_| Ok(()));

    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());
    app.selection = Selection::Feature(0, 0);
    app
}

#[test]
fn stopping_a_feature_closes_the_editor_amf_opened() {
    let tmp = TempDir::new().unwrap();
    let workdir = tmp.path().join("worktree");
    std::fs::create_dir_all(&workdir).unwrap();
    let mut editor = spawn_fake_editor(tmp.path(), &workdir);
    // Let the shell fork the child that stands in for a language server.
    std::thread::sleep(std::time::Duration::from_millis(300));
    let editor_pid = editor.id() as i64;
    let tree = crate::resources::procs::process_tree(
        &crate::resources::procs::list_processes(),
        editor_pid,
    );
    assert!(tree.len() > 1, "expected a child process, got {tree:?}");

    let db_file = NamedTempFile::new().unwrap();
    let mut app = app_with_db(&workdir, &db_file);
    app.db
        .as_ref()
        .unwrap()
        .record_launched_editor(
            "feat-1",
            None,
            crate::db::editors::EditorKind::Vscode,
            editor_pid,
            &workdir,
            true,
            "code --new-window",
        )
        .unwrap();

    app.stop_feature().unwrap();
    let _ = editor.wait();

    for pid in &tree {
        assert!(
            !crate::resources::procs::pid_alive(*pid),
            "pid {pid} survived the stop"
        );
    }
    // The record is dropped along with the process it described.
    assert!(
        app.db
            .as_ref()
            .unwrap()
            .launched_editors_for_feature("feat-1")
            .unwrap()
            .is_empty()
    );
    assert!(
        app.message
            .as_deref()
            .unwrap_or_default()
            .contains("closed 1 editor"),
        "got {:?}",
        app.message
    );
}

#[test]
fn kill_editor_on_stop_opt_out_leaves_the_editor_running() {
    let tmp = TempDir::new().unwrap();
    let workdir = tmp.path().join("worktree");
    std::fs::create_dir_all(&workdir).unwrap();
    let mut editor = spawn_fake_editor(tmp.path(), &workdir);
    let editor_pid = editor.id() as i64;

    let db_file = NamedTempFile::new().unwrap();
    let mut app = app_with_db(&workdir, &db_file);
    app.config.kill_editor_on_stop = false;
    app.db
        .as_ref()
        .unwrap()
        .record_launched_editor(
            "feat-1",
            None,
            crate::db::editors::EditorKind::Vscode,
            editor_pid,
            &workdir,
            true,
            "code --new-window",
        )
        .unwrap();

    app.stop_feature().unwrap();

    assert!(
        crate::resources::procs::pid_alive(editor_pid),
        "the opt-out must bypass the kill entirely"
    );
    // And the record survives, since nothing was resolved.
    assert_eq!(
        app.db
            .as_ref()
            .unwrap()
            .launched_editors_for_feature("feat-1")
            .unwrap()
            .len(),
        1
    );

    let _ = editor.kill();
    let _ = editor.wait();
}

#[test]
fn an_editor_amf_did_not_open_is_reported_not_killed() {
    let tmp = TempDir::new().unwrap();
    let workdir = tmp.path().join("worktree");
    std::fs::create_dir_all(&workdir).unwrap();
    let mut editor = spawn_fake_editor(tmp.path(), &workdir);
    let editor_pid = editor.id() as i64;

    let db_file = NamedTempFile::new().unwrap();
    let mut app = app_with_db(&workdir, &db_file);
    // `dedicated: false` — the folder was handed to a window the user opened.
    app.db
        .as_ref()
        .unwrap()
        .record_launched_editor(
            "feat-1",
            None,
            crate::db::editors::EditorKind::Vscode,
            editor_pid,
            &workdir,
            false,
            "code",
        )
        .unwrap();

    let report = app.kill_tracked_editors("feat-1");

    assert!(report.killed.is_empty());
    assert!(crate::resources::procs::pid_alive(editor_pid));
    assert_eq!(
        report.summary().as_deref(),
        Some("left VS Code running (AMF did not open this window)")
    );

    let _ = editor.kill();
    let _ = editor.wait();
}

#[test]
fn a_recycled_pid_is_never_signalled() {
    let tmp = TempDir::new().unwrap();
    let workdir = tmp.path().join("worktree");
    std::fs::create_dir_all(&workdir).unwrap();

    // A live process that is emphatically not the recorded editor: if identity
    // revalidation were skipped, this would be killed.
    let mut bystander = std::process::Command::new("sleep")
        .arg("60")
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let bystander_pid = bystander.id() as i64;

    let db_file = NamedTempFile::new().unwrap();
    let mut app = app_with_db(&workdir, &db_file);
    app.db
        .as_ref()
        .unwrap()
        .record_launched_editor(
            "feat-1",
            None,
            crate::db::editors::EditorKind::Vscode,
            bystander_pid,
            &workdir,
            true,
            "code --new-window",
        )
        .unwrap();

    let report = app.kill_tracked_editors("feat-1");

    assert!(report.killed.is_empty());
    assert_eq!(
        report.skipped,
        vec![(
            "VS Code".to_string(),
            crate::app::editor_ops::SkipReason::PidRecycled
        )]
    );
    assert!(
        crate::resources::procs::pid_alive(bystander_pid),
        "an unrelated process must never be signalled"
    );

    let _ = bystander.kill();
    let _ = bystander.wait();
}

#[test]
fn a_window_sharing_its_instance_with_others_is_left_alone() {
    // AMF's launch started VS Code, so it owns the process — but the user has
    // since opened other windows in it. Killing the process would close their
    // work, which no amount of memory is worth.
    let tmp = TempDir::new().unwrap();
    let workdir = tmp.path().join("worktree");
    std::fs::create_dir_all(&workdir).unwrap();
    let mut editor = spawn_fake_editor_hosting_windows(tmp.path(), &workdir, 2);
    let editor_pid = editor.id() as i64;

    let db_file = NamedTempFile::new().unwrap();
    let mut app = app_with_db(&workdir, &db_file);
    app.db
        .as_ref()
        .unwrap()
        .record_launched_editor(
            "feat-1",
            None,
            crate::db::editors::EditorKind::Vscode,
            editor_pid,
            &workdir,
            true,
            "code --new-window",
        )
        .unwrap();

    let report = app.kill_tracked_editors("feat-1");

    assert!(report.killed.is_empty());
    assert_eq!(
        report.skipped,
        vec![(
            "VS Code".to_string(),
            crate::app::editor_ops::SkipReason::SharedInstance
        )]
    );
    assert!(
        crate::resources::procs::pid_alive(editor_pid),
        "a shared instance must survive the stop"
    );

    let _ = editor.kill();
    let _ = editor.wait();
}

#[test]
fn a_window_of_its_own_is_still_closed() {
    // The counterpart to the test above: one window in the instance is exactly
    // the case ownership was resolved for, and it must not be skipped.
    let tmp = TempDir::new().unwrap();
    let workdir = tmp.path().join("worktree");
    std::fs::create_dir_all(&workdir).unwrap();
    let mut editor = spawn_fake_editor_hosting_windows(tmp.path(), &workdir, 1);
    let editor_pid = editor.id() as i64;

    let db_file = NamedTempFile::new().unwrap();
    let mut app = app_with_db(&workdir, &db_file);
    app.db
        .as_ref()
        .unwrap()
        .record_launched_editor(
            "feat-1",
            None,
            crate::db::editors::EditorKind::Vscode,
            editor_pid,
            &workdir,
            true,
            "code --new-window",
        )
        .unwrap();

    let report = app.kill_tracked_editors("feat-1");
    let _ = editor.wait();

    assert_eq!(report.killed.len(), 1);
    assert!(!crate::resources::procs::pid_alive(editor_pid));
}

#[test]
fn stopping_while_the_window_is_still_opening_hands_it_to_the_resolver() {
    use crate::app::editor_ops::{PendingEditorLaunch, PendingLaunchState, lock_state};

    let tmp = TempDir::new().unwrap();
    let workdir = tmp.path().join("worktree");
    std::fs::create_dir_all(&workdir).unwrap();

    let db_file = NamedTempFile::new().unwrap();
    let mut app = app_with_db(&workdir, &db_file);
    // The state a launch is in for the first seconds: recorded, not yet owned.
    let record = app
        .db
        .as_ref()
        .unwrap()
        .record_launched_editor(
            "feat-1",
            None,
            crate::db::editors::EditorKind::Vscode,
            0,
            &workdir,
            false,
            "code --new-window",
        )
        .unwrap();
    let state = std::sync::Arc::new(std::sync::Mutex::new(PendingLaunchState::Resolving));
    app.pending_editor_launches.push(PendingEditorLaunch {
        feature_id: "feat-1".to_string(),
        record_id: record.id.clone(),
        kind: crate::db::editors::EditorKind::Vscode,
        state: state.clone(),
    });

    let report = app.kill_tracked_editors("feat-1");

    assert_eq!(report.pending, vec!["VS Code".to_string()]);
    assert!(
        report.skipped.is_empty(),
        "a window still opening is not a window AMF does not own, got {:?}",
        report.skipped
    );
    assert_eq!(
        *lock_state(&state),
        PendingLaunchState::Reclaim,
        "the resolver must be told to close the window it finds"
    );
    assert_eq!(
        report.summary().as_deref(),
        Some("VS Code will close once its window opens")
    );
}

#[test]
fn a_launch_that_resolved_before_the_stop_is_closed_by_the_stop() {
    // The other side of the race: the resolver won, so the row is owned and the
    // stop kills it directly. The pending entry must not swallow it.
    use crate::app::editor_ops::{PendingEditorLaunch, PendingLaunchState};

    let tmp = TempDir::new().unwrap();
    let workdir = tmp.path().join("worktree");
    std::fs::create_dir_all(&workdir).unwrap();
    let mut editor = spawn_fake_editor(tmp.path(), &workdir);
    std::thread::sleep(std::time::Duration::from_millis(300));
    let editor_pid = editor.id() as i64;

    let db_file = NamedTempFile::new().unwrap();
    let mut app = app_with_db(&workdir, &db_file);
    let record = app
        .db
        .as_ref()
        .unwrap()
        .record_launched_editor(
            "feat-1",
            None,
            crate::db::editors::EditorKind::Vscode,
            editor_pid,
            &workdir,
            true,
            "code --new-window",
        )
        .unwrap();
    app.pending_editor_launches.push(PendingEditorLaunch {
        feature_id: "feat-1".to_string(),
        record_id: record.id,
        kind: crate::db::editors::EditorKind::Vscode,
        state: std::sync::Arc::new(std::sync::Mutex::new(PendingLaunchState::Done)),
    });

    let report = app.kill_tracked_editors("feat-1");
    let _ = editor.wait();

    assert!(report.pending.is_empty());
    assert_eq!(report.killed.len(), 1);
    assert!(!crate::resources::procs::pid_alive(editor_pid));
    assert!(
        app.pending_editor_launches.is_empty(),
        "a resolved launch should be pruned"
    );
}
