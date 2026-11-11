//! GUI adapter for "new project from a git URL".
//!
//! Thin wrappers over the shared `App` clone operations
//! (`app/clone_project.rs`), so the GUI gets exactly the validation, pre-flight
//! and clone outcomes the TUI gets. Nothing here decides anything: the
//! frontend renders [`CloneProjectSnapshot`] and the backend pushes it as a
//! `clone_project_changed` event whenever [`poll`] reports a change.
//!
//! Event model: the clone runs on background threads owned by the `App`; the
//! backend polls ([`poll`]) while [`is_active`] and emits each changed snapshot.
//! Every mutating call also returns the snapshot it produced, so a caller never
//! has to wait for the next tick to see its own transition.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::app::clone_project::CloneProjectError;
pub use crate::app::clone_project::{
    CloneCheck, CloneCheckStatus, CloneFix, ClonePhase, CloneProgress, CloneProjectSnapshot,
};
use crate::gui_contract::{GuiError, GuiErrorKind, GuiHandle, GuiResult};

/// Name and directory derived from a URL.
#[derive(Debug, Clone, Serialize)]
pub struct DerivedCloneTarget {
    pub name: String,
    pub dest: PathBuf,
}

fn input_error(err: CloneProjectError) -> GuiError {
    let kind = match err {
        CloneProjectError::Busy | CloneProjectError::NameTaken(_) => GuiErrorKind::Conflict,
        CloneProjectError::ProjectsRootMissing { .. } => GuiErrorKind::ProjectsRootMissing,
        CloneProjectError::Start(_) => GuiErrorKind::Internal,
        _ => GuiErrorKind::InvalidInput,
    };
    GuiError {
        kind,
        message: err.to_string(),
        github: None,
    }
}

/// Whether `dest`'s parent is exactly the (missing) projects root; `Some(root)`
/// means the UI may offer to create it or pick another.
pub fn projects_root_status(gui: &mut GuiHandle, dest: &Path) -> Option<PathBuf> {
    gui.app_for_workflow().projects_root_status(dest)
}

/// Create the effective projects root after the user confirmed. Takes no path.
pub fn create_projects_root(gui: &mut GuiHandle) -> GuiResult<PathBuf> {
    gui.app_for_workflow()
        .create_projects_root()
        .map_err(input_error)
}

/// Change and persist the projects root; returns the effective path.
pub fn set_projects_root(gui: &mut GuiHandle, path: &str) -> GuiResult<PathBuf> {
    gui.app_for_workflow()
        .set_projects_root(path)
        .map_err(input_error)
}

/// Run the clone, and every later session, in a container. Set before
/// deriving or validating: container mode refuses SSH URLs.
pub fn set_containerized(gui: &mut GuiHandle, containerized: bool) {
    gui.app_for_workflow()
        .set_clone_containerized(containerized);
}

/// Derive the project name and directory for `url`.
pub fn derive(gui: &mut GuiHandle, url: &str) -> GuiResult<DerivedCloneTarget> {
    let derived = gui
        .app_for_workflow()
        .derive_clone_target(url)
        .map_err(input_error)?;
    Ok(DerivedCloneTarget {
        name: derived.name,
        dest: derived.dest,
    })
}

/// Validate a (possibly edited) target without starting anything.
pub fn validate(gui: &mut GuiHandle, url: &str, dest: &Path, name: &str) -> GuiResult<()> {
    gui.app_for_workflow()
        .validate_clone_target(url, dest, name)
        .map(|_| ())
        .map_err(input_error)
}

/// Validate, then start the pre-flight checks (also the "Re-check" action).
pub fn start_preflight(
    gui: &mut GuiHandle,
    url: &str,
    dest: &Path,
    name: &str,
) -> GuiResult<CloneProjectSnapshot> {
    let app = gui.app_for_workflow();
    app.start_clone_preflight(url, dest, name)
        .map_err(input_error)?;
    Ok(app.clone_project_snapshot())
}

/// Start cloning after a passing or warning pre-flight.
pub fn start(
    gui: &mut GuiHandle,
    url: &str,
    dest: &Path,
    name: &str,
    agent: Option<&str>,
) -> GuiResult<CloneProjectSnapshot> {
    let app = gui.app_for_workflow();
    app.set_clone_preferred_agent(agent.and_then(crate::project::AgentKind::from_slug));
    app.start_clone(url, dest, name).map_err(input_error)?;
    Ok(app.clone_project_snapshot())
}

/// Cancel the running step. A refused cancel (too late) is not an error; the
/// returned snapshot says what state the flow is in.
pub fn cancel(gui: &mut GuiHandle) -> CloneProjectSnapshot {
    let app = gui.app_for_workflow();
    app.cancel_clone();
    app.clone_project_snapshot()
}

/// Return to idle (dialog dismissed, or back to the form after a result).
/// Refused while a clone is in flight; cancel it first.
pub fn reset(gui: &mut GuiHandle) -> GuiResult<CloneProjectSnapshot> {
    let app = gui.app_for_workflow();
    if !app.reset_clone_project() {
        return Err(input_error(CloneProjectError::Busy));
    }
    Ok(app.clone_project_snapshot())
}

pub fn snapshot(gui: &mut GuiHandle) -> CloneProjectSnapshot {
    gui.app_for_workflow().clone_project_snapshot()
}

/// Apply finished background work. `Some` carries the new snapshot when it
/// changed (the caller emits it as `clone_project_changed`).
pub fn poll(gui: &mut GuiHandle) -> Option<CloneProjectSnapshot> {
    let app = gui.app_for_workflow();
    let preflight = app.poll_clone_preflight();
    let progress = app.poll_clone_progress();
    (preflight || progress).then(|| app.clone_project_snapshot())
}

/// Whether background work is still running, i.e. whether polling should
/// continue.
pub fn is_active(snapshot: &CloneProjectSnapshot) -> bool {
    matches!(
        snapshot.phase,
        ClonePhase::PreflightRunning | ClonePhase::Cloning { .. } | ClonePhase::Registering
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::git_clone::env::GitEnv;
    use crate::project::ProjectStore;
    use crate::traits::{MockTmuxOps, MockWorktreeOps};
    use std::collections::HashMap;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use std::time::{Duration, Instant};

    fn gui_with_env(env: GitEnv, allow_file_urls: bool) -> GuiHandle {
        let mut worktree = MockWorktreeOps::new();
        worktree
            .expect_repo_root()
            .returning(|p| Ok(p.to_path_buf()));
        let mut app = App::new_for_test(
            ProjectStore {
                version: 5,
                projects: vec![],
                session_bookmarks: vec![],
                available_harnesses: vec![],
                prompt_templates: Vec::new(),
                extra: HashMap::new(),
            },
            Box::new(MockTmuxOps::new()),
            Box::new(worktree),
        );
        app.store_path = tempfile::NamedTempFile::new()
            .unwrap()
            .into_temp_path()
            .keep()
            .unwrap();
        app.set_clone_project_env_for_test(env, allow_file_urls);
        GuiHandle::from_app(app)
    }

    fn kind(snapshot: &CloneProjectSnapshot) -> &'static str {
        match snapshot.phase {
            ClonePhase::Idle => "idle",
            ClonePhase::Validating => "validating",
            ClonePhase::PreflightRunning => "preflight_running",
            ClonePhase::PreflightDone { .. } => "preflight_done",
            ClonePhase::Cloning { .. } => "cloning",
            ClonePhase::Registering => "registering",
            ClonePhase::Succeeded { .. } => "succeeded",
            ClonePhase::Failed { .. } => "failed",
            ClonePhase::Cancelled => "cancelled",
        }
    }

    /// Records the phase kinds an interface would be told about, in order,
    /// coalescing repeats (progress ticks) the way the event stream does.
    #[derive(Default)]
    struct Events(Vec<&'static str>);

    impl Events {
        fn see(&mut self, snapshot: &CloneProjectSnapshot) {
            let k = kind(snapshot);
            if self.0.last() != Some(&k) {
                self.0.push(k);
            }
        }

        fn until(
            &mut self,
            gui: &mut GuiHandle,
            what: &str,
            done: impl Fn(&CloneProjectSnapshot) -> bool,
        ) -> CloneProjectSnapshot {
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                if let Some(s) = poll(gui) {
                    self.see(&s);
                    if done(&s) {
                        return s;
                    }
                }
                assert!(Instant::now() < deadline, "timed out waiting for {what}");
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}");
    }

    fn local_remote(root: &Path) -> String {
        let bare = root.join("remote.git");
        let work = root.join("work");
        std::fs::create_dir_all(&work).unwrap();
        git(root, &["init", "--bare", "-b", "trunk", "remote.git"]);
        git(&work, &["init", "-b", "trunk"]);
        std::fs::write(work.join("README"), "hi").unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-m", "init"]);
        git(&work, &["push", bare.to_str().unwrap(), "trunk"]);
        format!("file://{}", bare.display())
    }

    /// Fake git answering the pre-flight, then running `clone_body`.
    fn fake_git(dir: &Path, clone_body: &str) -> GitEnv {
        let bin = dir.join("fakegit");
        std::fs::write(
            &bin,
            format!(
                "#!/bin/sh\ncase \"$*\" in\n*--version*) echo 'git version 9.9'; exit 0;;\n\
                 *ls-remote*) printf 'ref: refs/heads/main\\tHEAD\\nabc\\tHEAD\\n'; exit 0;;\n\
                 *clone*) {clone_body};;\nesac\nexit 0\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        GitEnv::new().with_git_binary(bin)
    }

    const URL: &str = "https://example.com/o/repo.git";

    fn run_preflight(gui: &mut GuiHandle, events: &mut Events, dest: &Path) {
        // The fake git is a freshly written script. Under heavy parallel load
        // another thread's fork can briefly hold its write fd open, so the
        // exec fails with ETXTBSY and a check reports Fail. That is a test
        // fixture race, not a product bug, so re-check a few times.
        let mut last = None;
        for _ in 0..5 {
            events.0.clear();
            let s = start_preflight(gui, URL, dest, "repo").unwrap();
            events.see(&s);
            let s = events.until(gui, "preflight", |s| {
                matches!(s.phase, ClonePhase::PreflightDone { .. })
            });
            if s.can_start {
                return;
            }
            last = Some(s);
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!(
            "preflight never became startable: {:?}",
            last.unwrap().phase
        );
    }

    #[test]
    fn success_emits_preflight_then_clone_then_succeeded() {
        let tmp = tempfile::tempdir().unwrap();
        let url = local_remote(tmp.path());
        let mut gui = gui_with_env(GitEnv::new().with_file_transport(true), true);
        let dest = tmp.path().join("repo");
        let mut events = Events::default();

        assert_eq!(kind(&snapshot(&mut gui)), "idle");
        events.see(&start_preflight(&mut gui, &url, &dest, "repo").unwrap());
        events.until(&mut gui, "preflight", |s| s.can_start);
        events.see(&start(&mut gui, &url, &dest, "repo", None).unwrap());
        let done = events.until(&mut gui, "clone", |s| {
            matches!(
                s.phase,
                ClonePhase::Succeeded { .. } | ClonePhase::Failed { .. }
            )
        });

        assert_eq!(
            events.0,
            [
                "preflight_running",
                "preflight_done",
                "cloning",
                "succeeded"
            ]
        );
        match &done.phase {
            ClonePhase::Succeeded { name, branch, .. } => {
                assert_eq!(name, "repo");
                assert_eq!(branch, "trunk");
            }
            other => panic!("{other:?}"),
        }
        assert!(!is_active(&done));
        assert_eq!(gui.app_for_workflow().store.projects.len(), 1);
        assert!(poll(&mut gui).is_none(), "nothing changes after the end");
    }

    #[test]
    fn failed_clone_emits_failed_and_returns_to_a_resettable_state() {
        let tmp = tempfile::tempdir().unwrap();
        let env = fake_git(
            tmp.path(),
            "echo \"fatal: Authentication failed for 'x'\" >&2; exit 128",
        );
        let mut gui = gui_with_env(env, false);
        let dest = tmp.path().join("repo");
        let mut events = Events::default();

        run_preflight(&mut gui, &mut events, &dest);
        events.see(&start(&mut gui, URL, &dest, "repo", None).unwrap());
        let done = events.until(&mut gui, "failure", |s| {
            matches!(s.phase, ClonePhase::Failed { .. })
        });

        assert_eq!(
            events.0,
            ["preflight_running", "preflight_done", "cloning", "failed"]
        );
        assert!(!dest.exists(), "owned directory is removed on failure");
        assert!(gui.app_for_workflow().store.projects.is_empty());
        assert!(!done.can_start);
        let idle = reset(&mut gui).unwrap();
        assert_eq!(kind(&idle), "idle");
    }

    #[test]
    fn cancel_emits_cancelled_and_removes_the_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let env = fake_git(
            tmp.path(),
            "printf 'Receiving objects:  10%% (1/10)\\r' >&2; sleep 300",
        );
        let mut gui = gui_with_env(env, false);
        let dest = tmp.path().join("repo");
        let mut events = Events::default();

        run_preflight(&mut gui, &mut events, &dest);
        events.see(&start(&mut gui, URL, &dest, "repo", None).unwrap());
        let s = events.until(&mut gui, "progress", |s| {
            matches!(s.phase, ClonePhase::Cloning { progress: Some(_) })
        });
        assert!(s.can_cancel);
        assert!(reset(&mut gui).is_err(), "no reset while cloning");
        cancel(&mut gui);
        let done = events.until(&mut gui, "cancel", |s| {
            matches!(s.phase, ClonePhase::Cancelled)
        });

        assert_eq!(
            events.0,
            [
                "preflight_running",
                "preflight_done",
                "cloning",
                "cancelled"
            ]
        );
        assert!(!done.can_cancel);
        assert!(!dest.exists());
        assert!(gui.app_for_workflow().store.projects.is_empty());
    }

    #[test]
    fn cancelling_preflight_is_immediate() {
        let tmp = tempfile::tempdir().unwrap();
        // A git that never answers keeps the pre-flight running.
        let bin = tmp.path().join("hang");
        std::fs::write(&bin, "#!/bin/sh\nsleep 300\n").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut gui = gui_with_env(GitEnv::new().with_git_binary(bin), false);
        let mut events = Events::default();
        let s = start_preflight(&mut gui, URL, &tmp.path().join("r"), "r").unwrap();
        events.see(&s);
        assert!(s.can_cancel);
        events.see(&cancel(&mut gui));
        assert_eq!(events.0, ["preflight_running", "cancelled"]);
    }

    #[test]
    fn invalid_input_is_a_structured_error_and_starts_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let mut gui = gui_with_env(GitEnv::new(), false);

        let err = derive(&mut gui, "file:///etc/passwd").unwrap_err();
        assert_eq!(err.kind, GuiErrorKind::InvalidInput);
        let err = start_preflight(&mut gui, URL, &tmp.path().join("r"), " ").unwrap_err();
        assert_eq!(err.kind, GuiErrorKind::InvalidInput);
        assert_eq!(kind(&snapshot(&mut gui)), "idle");
        let err = start(&mut gui, URL, &tmp.path().join("r"), "r", None).unwrap_err();
        assert_eq!(err.kind, GuiErrorKind::InvalidInput);

        gui.app_for_workflow().config.projects.projects_root =
            Some(tmp.path().to_string_lossy().into_owned());
        let derived = derive(&mut gui, URL).unwrap();
        assert_eq!(derived.name, "repo");
        assert_eq!(derived.dest, tmp.path().join("repo"));
        validate(&mut gui, URL, &derived.dest, &derived.name).unwrap();
    }

    #[test]
    fn missing_projects_root_has_its_own_error_kind() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("code");
        let mut gui = gui_with_env(GitEnv::new(), false);
        gui.app_for_workflow().config.projects.projects_root =
            Some(root.to_string_lossy().into_owned());
        let dest = root.join("repo");
        let err = validate(&mut gui, URL, &dest, "repo").unwrap_err();
        assert_eq!(err.kind, GuiErrorKind::ProjectsRootMissing);
        assert_eq!(
            serde_json::to_value(err.kind).unwrap(),
            "projects_root_missing"
        );
        let err = start_preflight(&mut gui, URL, &dest, "repo").unwrap_err();
        assert_eq!(err.kind, GuiErrorKind::ProjectsRootMissing);
        assert_eq!(projects_root_status(&mut gui, &dest), Some(root.clone()));
        // Another missing parent stays a plain invalid-input block.
        let other = tmp.path().join("nope").join("repo");
        let err = validate(&mut gui, URL, &other, "repo").unwrap_err();
        assert_eq!(err.kind, GuiErrorKind::InvalidInput);

        assert_eq!(create_projects_root(&mut gui).unwrap(), root);
        validate(&mut gui, URL, &dest, "repo").unwrap();
        let err = create_projects_root(&mut gui).unwrap_err();
        assert_eq!(err.kind, GuiErrorKind::InvalidInput);
        let err = set_projects_root(&mut gui, "").unwrap_err();
        assert_eq!(err.kind, GuiErrorKind::InvalidInput);
        let fresh = tmp.path().join("fresh");
        assert_eq!(
            set_projects_root(&mut gui, fresh.to_str().unwrap()).unwrap(),
            fresh
        );
        assert_eq!(derive(&mut gui, URL).unwrap().dest, fresh.join("repo"));
    }

    #[test]
    fn snapshot_serializes_for_the_frontend() {
        let mut gui = gui_with_env(GitEnv::new(), false);
        let json = serde_json::to_value(snapshot(&mut gui)).unwrap();
        assert_eq!(json["phase"]["kind"], "idle");
        assert_eq!(json["can_start"], false);
        assert_eq!(json["can_cancel"], false);
    }
}
