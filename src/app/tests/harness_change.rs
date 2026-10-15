use super::support::*;
use crate::app::*;
use crate::gui_contract::{FeatureTarget, GuiHandle};
use crate::gui_harness_change::{self, HarnessChangeRequest};
use crate::project::{AgentKind, FeatureSession, SessionKind};
use crate::traits::{MockTmuxOps, MockWorktreeOps};
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn git(path: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn row(id: &str, kind: &str, stopped: bool) -> FeatureSession {
    serde_json::from_value(serde_json::json!({
        "id": id, "kind": kind, "label": id, "tmux_window": id,
        "claude_session_id": format!("conversation-{id}"), "stopped": stopped,
        "created_at": chrono::Utc::now(),
    }))
    .unwrap()
}
struct Fixture {
    root: TempDir,
    app: App,
}
impl Fixture {
    fn new(tmux: MockTmuxOps, status: ProjectStatus) -> Self {
        let root = TempDir::new().unwrap();
        let repo = root.path().join("repo");
        let workdir = root.path().join("checkout");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.test",
                "commit",
                "--allow-empty",
                "-m",
                "fixture",
            ],
        );
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "my-feat",
                workdir.to_str().unwrap(),
            ],
        );
        let mut store = store_with_worktree_agent(
            &repo,
            &workdir,
            AgentKind::Claude,
            status,
            vec![
                row("agent-one", "claude", false),
                row("agent-two", "codex", true),
                row("terminal", "terminal", false),
            ],
        );
        store.projects[0].features[0].mode = VibeMode::SuperVibe;
        let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
        app.store_path = root.path().join("store.json");
        app.save().unwrap();
        Self { root, app }
    }
    fn target(&self) -> FeatureTarget {
        FeatureTarget {
            project_id: "proj-1".into(),
            feature_id: "feat-1".into(),
        }
    }
}
fn stopped_tmux() -> MockTmuxOps {
    let mut tmux = MockTmuxOps::new();
    tmux.expect_check_harness_available().returning(|_| Ok(()));
    tmux.expect_session_exists().return_const(false);
    tmux.expect_list_panes().returning(Vec::new);
    tmux
}

#[test]
fn harness_change_converts_all_agent_rows_preserving_explicit_stops_and_nonagents() {
    let mut fixture = Fixture::new(stopped_tmux(), ProjectStatus::Stopped);
    let before = fixture.app.store.projects[0].features[0].clone();
    fixture
        .app
        .change_feature_harness(0, 0, AgentKind::Opencode)
        .unwrap();
    let after = &fixture.app.store.projects[0].features[0];
    assert_eq!(after.agent, AgentKind::Opencode);
    assert_eq!(after.status, ProjectStatus::Stopped);
    for (index, original) in before.sessions[..2].iter().enumerate() {
        let changed = &after.sessions[index];
        assert_eq!(changed.id, original.id);
        assert_eq!(changed.kind, SessionKind::Opencode);
        assert_eq!(changed.stopped, original.stopped);
        assert!(changed.claude_session_id.is_none());
        assert!(changed.token_usage_source.is_none());
        assert_eq!(changed.todo_reference, original.todo_reference);
    }
    assert_ne!(after.sessions[0].tmux_window, after.sessions[1].tmux_window);
    assert_eq!(
        serde_json::to_value(&after.sessions[2]).unwrap(),
        serde_json::to_value(&before.sessions[2]).unwrap()
    );
    assert!(
        after
            .workdir
            .join(".opencode/plugins/sidebar-state.js")
            .is_file()
    );
    assert_eq!(
        ProjectStore::load(&fixture.app.store_path)
            .unwrap()
            .projects[0]
            .features[0]
            .agent,
        AgentKind::Opencode
    );
}

#[test]
fn harness_change_unavailable_preflight_never_stops_or_writes() {
    let mut tmux = MockTmuxOps::new();
    tmux.expect_check_harness_available()
        .returning(|_| anyhow::bail!("fixture unavailable"));
    let mut fixture = Fixture::new(tmux, ProjectStatus::Active);
    let original = serde_json::to_value(&fixture.app.store).unwrap();
    let error = fixture
        .app
        .change_feature_harness(0, 0, AgentKind::Opencode)
        .unwrap_err();
    assert!(format!("{error:#}").contains("nothing was stopped"));
    assert_eq!(serde_json::to_value(&fixture.app.store).unwrap(), original);
    assert!(
        !fixture.app.store.projects[0].features[0]
            .workdir
            .join(".opencode")
            .exists()
    );
}

#[test]
fn harness_change_malformed_json_and_toml_are_preserved_before_shutdown() {
    for (path, content) in [
        (".claude/settings.local.json", "{broken"),
        (".codex/config.toml", "notify = ["),
        (
            ".claude/settings.local.json",
            "{\"hooks\":{\"Stop\":false}}",
        ),
        (".claude/settings.local.json", "{\"permissions\":false}"),
    ] {
        let mut tmux = MockTmuxOps::new();
        tmux.expect_check_harness_available().returning(|_| Ok(()));
        let mut fixture = Fixture::new(tmux, ProjectStatus::Active);
        let path = fixture.app.store.projects[0].features[0].workdir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        let error = fixture
            .app
            .change_feature_harness(0, 0, AgentKind::Opencode)
            .unwrap_err();
        assert!(format!("{error:#}").contains("nothing was stopped"));
        assert_eq!(std::fs::read_to_string(path).unwrap(), content);
        assert_eq!(
            fixture.app.store.projects[0].features[0].agent,
            AgentKind::Claude
        );
    }
}

#[test]
fn harness_change_hook_write_failure_keeps_original_bindings_and_no_launch() {
    let mut fixture = Fixture::new(stopped_tmux(), ProjectStatus::Stopped);
    let workdir = &fixture.app.store.projects[0].features[0].workdir;
    std::fs::write(workdir.join(".opencode"), "not a directory").unwrap();
    let error = fixture
        .app
        .change_feature_harness(0, 0, AgentKind::Opencode)
        .unwrap_err();
    assert!(format!("{error:#}").contains("could not finish hook setup"));
    assert_eq!(
        fixture.app.store.projects[0].features[0].agent,
        AgentKind::Claude
    );
    assert_eq!(
        fixture.app.store.projects[0].features[0].status,
        ProjectStatus::Stopped
    );
    assert_eq!(
        ProjectStore::load(&fixture.app.store_path)
            .unwrap()
            .projects[0]
            .features[0]
            .agent,
        AgentKind::Claude
    );
}

#[test]
fn harness_change_save_failure_does_not_launch_unsaved_replacement() {
    let mut fixture = Fixture::new(stopped_tmux(), ProjectStatus::Stopped);
    fixture.app.store_path = fixture.root.path().to_path_buf();
    let error = fixture
        .app
        .change_feature_harness(0, 0, AgentKind::Opencode)
        .unwrap_err();
    assert!(format!("{error:#}").contains("no replacement was launched"));
    assert_eq!(
        fixture.app.store.projects[0].features[0].agent,
        AgentKind::Claude
    );
    assert_eq!(
        fixture.app.store.projects[0].features[0].status,
        ProjectStatus::Stopped
    );
}

#[test]
fn harness_change_shutdown_failure_retains_old_bindings() {
    let mut tmux = MockTmuxOps::new();
    tmux.expect_check_harness_available().returning(|_| Ok(()));
    tmux.expect_kill_session()
        .returning(|_| anyhow::bail!("fixture kill failed"));
    let mut fixture = Fixture::new(tmux, ProjectStatus::Active);
    let error = fixture
        .app
        .change_feature_harness(0, 0, AgentKind::Opencode)
        .unwrap_err();
    assert!(format!("{error:#}").contains("original harness bindings remain"));
    assert_eq!(
        fixture.app.store.projects[0].features[0].agent,
        AgentKind::Claude
    );
    assert!(
        !fixture.app.store.projects[0].features[0]
            .workdir
            .join(".opencode")
            .exists()
    );
}

#[test]
fn harness_change_partial_restart_retains_saved_replacement_bindings() {
    let mut tmux = stopped_tmux();
    tmux.expect_kill_session().returning(|_| Ok(()));
    tmux.expect_create_session_with_window()
        .returning(|_, _, _| anyhow::bail!("fixture start failed"));
    let mut fixture = Fixture::new(tmux, ProjectStatus::Active);
    let error = fixture
        .app
        .change_feature_harness(0, 0, AgentKind::Opencode)
        .unwrap_err();
    assert!(format!("{error:#}").contains("bindings were saved, but restart failed"));
    let saved = ProjectStore::load(&fixture.app.store_path).unwrap();
    assert_eq!(saved.projects[0].features[0].agent, AgentKind::Opencode);
    assert_eq!(saved.projects[0].features[0].status, ProjectStatus::Stopped);
    assert!(saved.projects[0].features[0].sessions[1].stopped);
}

#[test]
fn harness_change_gui_refuses_saved_row_change_and_git_head_change() {
    for change in [
        "conversation",
        "stopped",
        "reorder",
        "head",
        "branch",
        "allowed",
    ] {
        let fixture = Fixture::new(stopped_tmux(), ProjectStatus::Stopped);
        let target = fixture.target();
        let mut gui = GuiHandle::from_app(fixture.app);
        let view = gui_harness_change::load(&mut gui, target.clone()).unwrap();
        let app = gui.app_for_workflow();
        let feature = &mut app.store.projects[0].features[0];
        match change {
            "conversation" => feature.sessions[0].claude_session_id = Some("different".into()),
            "stopped" => feature.sessions[0].stopped = true,
            "reorder" => feature.sessions.swap(0, 1),
            "head" => git(
                &feature.workdir,
                &[
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=fixture@example.test",
                    "commit",
                    "--allow-empty",
                    "-m",
                    "external",
                ],
            ),
            "branch" => git(&feature.workdir, &["switch", "-c", "external"]),
            "allowed" => std::fs::write(
                app.store.projects[0].repo.join("amf.json"),
                "{\"allowed_agents\":[\"claude\"]}",
            )
            .unwrap(),
            _ => unreachable!(),
        }
        let error = gui_harness_change::apply(
            &mut gui,
            HarnessChangeRequest {
                target,
                revision: view.revision,
                agent: AgentKind::Opencode,
            },
        )
        .unwrap_err();
        assert!(
            error.message.contains("changed"),
            "{change}: {}",
            error.message
        );
        assert_eq!(
            gui.app_for_workflow().store.projects[0].features[0].agent,
            AgentKind::Claude
        );
    }
}

#[test]
fn harness_change_gui_rechecks_live_pane_identity() {
    use std::sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    };
    let pid = Arc::new(AtomicI64::new(101));
    let observed = pid.clone();
    let mut tmux = MockTmuxOps::new();
    tmux.expect_check_harness_available().returning(|_| Ok(()));
    tmux.expect_session_exists().return_const(true);
    tmux.expect_list_panes().returning(move || {
        vec![(
            "amf-my-feat".into(),
            "agent-one".into(),
            observed.load(Ordering::SeqCst),
        )]
    });
    let fixture = Fixture::new(tmux, ProjectStatus::Active);
    let target = fixture.target();
    let mut gui = GuiHandle::from_app(fixture.app);
    let view = gui_harness_change::load(&mut gui, target.clone()).unwrap();
    pid.store(202, Ordering::SeqCst);
    assert!(
        gui_harness_change::apply(
            &mut gui,
            HarnessChangeRequest {
                target,
                revision: view.revision,
                agent: AgentKind::Opencode
            }
        )
        .unwrap_err()
        .message
        .contains("changed")
    );
}

#[test]
fn harness_change_running_feature_with_every_row_stopped_preserves_every_stop() {
    let mut tmux = stopped_tmux();
    tmux.expect_kill_session().returning(|_| Ok(()));
    let mut fixture = Fixture::new(tmux, ProjectStatus::Active);
    for session in &mut fixture.app.store.projects[0].features[0].sessions {
        session.stopped = true;
    }
    fixture
        .app
        .change_feature_harness(0, 0, AgentKind::Opencode)
        .unwrap();
    let feature = &fixture.app.store.projects[0].features[0];
    assert!(feature.sessions.iter().all(|session| session.stopped));
    assert_eq!(feature.status, ProjectStatus::Stopped);
    assert_eq!(feature.agent, AgentKind::Opencode);
}

#[test]
fn harness_change_gui_refuses_replaced_checkout_and_pending_targets() {
    for change in ["replaced", "deleted", "moved", "pending", "ambiguous"] {
        let fixture = Fixture::new(stopped_tmux(), ProjectStatus::Stopped);
        let target = fixture.target();
        let mut gui = GuiHandle::from_app(fixture.app);
        let view = gui_harness_change::load(&mut gui, target.clone()).unwrap();
        let app = gui.app_for_workflow();
        match change {
            "replaced" => {
                let workdir = app.store.projects[0].features[0].workdir.clone();
                let git_file = std::fs::read(workdir.join(".git")).unwrap();
                std::fs::rename(&workdir, fixture.root.path().join("old-checkout")).unwrap();
                std::fs::create_dir(&workdir).unwrap();
                std::fs::write(workdir.join(".git"), git_file).unwrap();
            }
            "deleted" => app.store.projects[0].features.clear(),
            "moved" => app.store.projects[0].id = "other-project".into(),
            "pending" => app.store.projects[0].features[0].pending_worktree_script = true,
            "ambiguous" => {
                let mut duplicate = app.store.projects[0].features[0].clone();
                duplicate.id = "other-feature".into();
                app.store.projects[0].features.push(duplicate);
            }
            _ => unreachable!(),
        }
        assert!(
            gui_harness_change::apply(
                &mut gui,
                HarnessChangeRequest {
                    target,
                    revision: view.revision,
                    agent: AgentKind::Opencode
                }
            )
            .is_err(),
            "{change}"
        );
        assert!(
            gui.app_for_workflow().store.projects[0]
                .features
                .iter()
                .all(|feature| feature.agent == AgentKind::Claude)
        );
    }
}

fn attach_db(app: &mut App, path: &Path) {
    let db = crate::db::AmfDb::open(path).unwrap();
    app.store_version = Some(db.save_store(&app.store).unwrap());
    app.db = Some(db);
}
fn external_nickname(path: &Path, nickname: &str) {
    let db = crate::db::AmfDb::open(path).unwrap();
    let mut store = db.load_store().unwrap();
    store.projects[0].features[0].nickname = Some(nickname.into());
    db.save_store(&store).unwrap();
}

#[test]
fn harness_change_cas_before_binding_save_adopts_external_edit_without_launch() {
    let database = tempfile::NamedTempFile::new().unwrap();
    let path = database.path().to_path_buf();
    let writer = path.clone();
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().return_const(false);
    tmux.expect_check_harness_available()
        .times(1)
        .returning(move |_| {
            external_nickname(&writer, "External edit");
            Ok(())
        });
    let mut fixture = Fixture::new(tmux, ProjectStatus::Stopped);
    attach_db(&mut fixture.app, &path);
    let error = fixture
        .app
        .change_feature_harness(0, 0, AgentKind::Opencode)
        .unwrap_err();
    assert!(format!("{error:#}").contains("no replacement was launched"));
    let feature = &fixture.app.store.projects[0].features[0];
    assert_eq!(feature.agent, AgentKind::Claude);
    assert_eq!(feature.nickname.as_deref(), Some("External edit"));
    let persisted = fixture.app.db.as_ref().unwrap().load_store().unwrap();
    assert_eq!(persisted.projects[0].features[0].agent, AgentKind::Claude);
    assert_eq!(
        persisted.projects[0].features[0].nickname.as_deref(),
        Some("External edit")
    );
}

#[test]
fn harness_change_cas_after_launch_keeps_replacement_and_reports_live_partial_state() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let database = tempfile::NamedTempFile::new().unwrap();
    let path = database.path().to_path_buf();
    let writer = path.clone();
    let exists = Arc::new(AtomicBool::new(false));
    let observed = exists.clone();
    let created = exists.clone();
    let mut tmux = MockTmuxOps::new();
    tmux.expect_check_harness_available().returning(|_| Ok(()));
    tmux.expect_kill_session().returning(|_| Ok(()));
    tmux.expect_session_exists()
        .returning(move |_| observed.load(Ordering::SeqCst));
    tmux.expect_create_session_with_window()
        .times(1)
        .returning(move |_, _, _| {
            created.store(true, Ordering::SeqCst);
            Ok(())
        });
    tmux.expect_set_session_env().returning(|_, _, _| Ok(()));
    tmux.expect_create_window().returning(|_, _, _| Ok(()));
    tmux.expect_select_window().returning(|_, _| Ok(()));
    tmux.expect_launch_opencode()
        .times(1)
        .returning(move |_, _, _| {
            external_nickname(&writer, "After launch");
            Ok(())
        });
    let mut fixture = Fixture::new(tmux, ProjectStatus::Active);
    attach_db(&mut fixture.app, &path);
    let error = fixture
        .app
        .change_feature_harness(0, 0, AgentKind::Opencode)
        .unwrap_err();
    assert!(format!("{error:#}").contains("final process state could not be saved"));
    assert!(exists.load(Ordering::SeqCst));
    assert!(format!("{error:#}").contains("Replacement tmux session exists"));
    let feature = &fixture.app.store.projects[0].features[0];
    assert_eq!(feature.agent, AgentKind::Opencode);
    assert_eq!(feature.nickname.as_deref(), Some("After launch"));
    assert!(feature.sessions[1].stopped);
    assert!(feature.sessions[0].claude_session_id.is_none());
    let persisted = fixture.app.db.as_ref().unwrap().load_store().unwrap();
    assert_eq!(persisted.projects[0].features[0].agent, AgentKind::Opencode);
    assert_eq!(
        persisted.projects[0].features[0].nickname.as_deref(),
        Some("After launch")
    );
}

#[test]
fn harness_change_cas_after_launch_handles_external_target_move_or_deletion() {
    for deletion in [false, true] {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let database = tempfile::NamedTempFile::new().unwrap();
        let path = database.path().to_path_buf();
        let writer = path.clone();
        let exists = Arc::new(AtomicBool::new(false));
        let observed = exists.clone();
        let created = exists.clone();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_check_harness_available().returning(|_| Ok(()));
        tmux.expect_kill_session().returning(|_| Ok(()));
        tmux.expect_session_exists()
            .returning(move |_| observed.load(Ordering::SeqCst));
        tmux.expect_create_session_with_window()
            .times(1)
            .returning(move |_, _, _| {
                created.store(true, Ordering::SeqCst);
                Ok(())
            });
        tmux.expect_set_session_env().returning(|_, _, _| Ok(()));
        tmux.expect_create_window().returning(|_, _, _| Ok(()));
        tmux.expect_select_window().returning(|_, _| Ok(()));
        tmux.expect_launch_opencode()
            .times(1)
            .returning(move |_, _, _| {
                let db = crate::db::AmfDb::open(&writer).unwrap();
                let mut store = db.load_store().unwrap();
                let feature = store.projects[0].features.remove(0);
                if !deletion {
                    let mut project = store.projects[0].clone();
                    project.id = "moved-project".into();
                    project.name = "Moved project".into();
                    project.features = vec![feature];
                    store.projects.push(project);
                }
                db.save_store(&store).unwrap();
                Ok(())
            });
        let mut fixture = Fixture::new(tmux, ProjectStatus::Active);
        attach_db(&mut fixture.app, &path);
        let error = fixture
            .app
            .change_feature_harness(0, 0, AgentKind::Opencode)
            .unwrap_err();
        assert!(format!("{error:#}").contains("final process state could not be saved"));
        assert!(exists.load(Ordering::SeqCst));
        assert!(format!("{error:#}").contains("Replacement tmux session exists"));
        assert!(format!("{error:#}").contains("Replacement tmux session exists"));
        assert!(format!("{error:#}").contains("no workspace row was reconciled or overwritten"));
        assert!(fixture.app.store.projects[0].features.is_empty());
        let persisted = fixture.app.db.as_ref().unwrap().load_store().unwrap();
        assert!(persisted.projects[0].features.is_empty());
        if !deletion {
            assert_eq!(
                fixture.app.store.projects[1].features[0].agent,
                AgentKind::Opencode
            );
            assert_eq!(persisted.projects[1].features[0].agent, AgentKind::Opencode);
        }
    }
}

#[test]
fn harness_change_gui_refreshes_real_external_sqlite_writer_before_confirming() {
    let database = tempfile::NamedTempFile::new().unwrap();
    let mut fixture = Fixture::new(stopped_tmux(), ProjectStatus::Stopped);
    attach_db(&mut fixture.app, database.path());
    let target = fixture.target();
    let mut gui = GuiHandle::from_app(fixture.app);
    let view = gui_harness_change::load(&mut gui, target.clone()).unwrap();
    external_nickname(database.path(), "Other AMF window");
    let error = gui_harness_change::apply(
        &mut gui,
        HarnessChangeRequest {
            target,
            revision: view.revision,
            agent: AgentKind::Opencode,
        },
    )
    .unwrap_err();
    assert!(error.message.contains("changed"));
    let feature = &gui.app_for_workflow().store.projects[0].features[0];
    assert_eq!(feature.nickname.as_deref(), Some("Other AMF window"));
    assert_eq!(feature.agent, AgentKind::Claude);
}

#[test]
fn harness_change_nonagent_only_feature_adds_unique_agent_window() {
    let mut fixture = Fixture::new(stopped_tmux(), ProjectStatus::Stopped);
    let feature = &mut fixture.app.store.projects[0].features[0];
    feature
        .sessions
        .retain(|session| !session.kind.is_agent_harness());
    feature.sessions[0].tmux_window = "opencode".into();
    let nonagent = serde_json::to_value(&feature.sessions[0]).unwrap();
    fixture
        .app
        .change_feature_harness(0, 0, AgentKind::Opencode)
        .unwrap();
    let sessions = &fixture.app.store.projects[0].features[0].sessions;
    assert_eq!(sessions[0].kind, SessionKind::Opencode);
    assert_eq!(sessions[0].tmux_window, "opencode-2");
    assert_eq!(serde_json::to_value(&sessions[1]).unwrap(), nonagent);
}

#[test]
fn harness_change_rejects_invalid_modes_nonworktrees_and_pending_scripts_before_side_effects() {
    for reason in ["codex-vibeless", "nonworktree", "pending"] {
        let mut fixture = Fixture::new(MockTmuxOps::new(), ProjectStatus::Active);
        let feature = &mut fixture.app.store.projects[0].features[0];
        match reason {
            "codex-vibeless" => feature.mode = VibeMode::Vibeless,
            "nonworktree" => feature.is_worktree = false,
            "pending" => feature.pending_worktree_script = true,
            _ => unreachable!(),
        }
        assert!(
            fixture
                .app
                .change_feature_harness(0, 0, AgentKind::Codex)
                .is_err()
        );
        assert_eq!(
            fixture.app.store.projects[0].features[0].agent,
            AgentKind::Claude
        );
    }
}
