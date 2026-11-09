//! Last step of a clone: confirm the checked-out branch, register the project,
//! and settle ownership of the directory.
//!
//! UI-free and App-free: the caller supplies the registration step as a
//! closure (in production, a wrapper over `App::create_project_from_request`).
//!
//! Ownership outcome is all-or-nothing:
//! - every failure (branch unreadable, detached HEAD, mismatch with the
//!   preflight branch, registration error or panic) rolls the owned directory
//!   back, so a failed registration never leaves a clone behind;
//! - success releases the directory to the registered project.
//!
//! Panic safety uses a drop guard rather than `catch_unwind`. After
//! `begin_point_of_no_return` the handle's own `Drop` deliberately leaves the
//! directory alone ("registration owns the outcome"), so unwinding out of the
//! callback would leak the clone. The guard closes that gap by calling
//! [`CloneHandle::rollback`] on unwind, while the panic itself still
//! propagates to the caller (nothing is swallowed, and the handle's Drop
//! semantics are untouched).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

use super::env::{GitEnv, Output};
use super::runner::{CloneHandle, RemoveOutcome};

/// What a successful finalize reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finalized {
    pub branch: String,
    pub dest: PathBuf,
}

/// Rolls back on drop unless disarmed. Holds the handle for the whole
/// finalize so every exit path, including unwinding, is covered.
struct RollbackGuard<'a> {
    handle: &'a mut CloneHandle,
    armed: bool,
}

impl Drop for RollbackGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.handle.rollback();
        }
    }
}

/// Read the checked-out branch of `dest` through the shared git builder.
fn read_branch(env: &GitEnv, dest: &Path) -> Result<String> {
    let mut cmd = env.command(
        [
            std::ffi::OsString::from("-C"),
            dest.as_os_str().to_owned(),
            "symbolic-ref".into(),
            "--short".into(),
            "HEAD".into(),
        ],
        Output::Piped,
        Output::Piped,
    );
    let out = cmd
        .spawn()
        .context("could not run git to read the cloned branch")?
        .wait_with_output()
        .context("could not read the cloned branch")?;
    if !out.status.success() {
        bail!(
            "the clone has no checked-out branch (detached HEAD?): {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if branch.is_empty() {
        bail!("the clone reported an empty branch name");
    }
    Ok(branch)
}

fn check_branch(env: &GitEnv, dest: &Path, expected: Option<&str>) -> Result<String> {
    let branch = read_branch(env, dest)?;
    if let Some(want) = expected
        && branch != want
    {
        bail!("the clone checked out '{branch}' but the remote's default branch is '{want}'");
    }
    Ok(branch)
}

/// Finalize a successful clone. `register` receives the destination and the
/// project name and must create the project record; it runs at most once.
///
/// Cancel is refused from the moment this begins. On any failure the owned
/// directory is deleted and the cause returned.
pub fn finalize<F>(
    env: &GitEnv,
    handle: &mut CloneHandle,
    preflight_branch: Option<&str>,
    name: &str,
    register: F,
) -> Result<Finalized>
where
    F: FnOnce(&Path, &str) -> Result<()>,
{
    let mut guard = RollbackGuard {
        handle,
        armed: true,
    };
    if !guard.handle.begin_point_of_no_return() {
        return Err(rollback_error(
            &mut guard,
            anyhow!("the clone did not complete successfully; nothing was registered"),
        ));
    }
    let dest = guard.handle.path().to_path_buf();

    let branch = match check_branch(env, &dest, preflight_branch) {
        Ok(b) => b,
        Err(e) => return Err(rollback_error(&mut guard, e)),
    };
    if let Err(e) = register(&dest, name) {
        return Err(rollback_error(
            &mut guard,
            e.context("registering the project failed"),
        ));
    }

    guard.armed = false;
    guard.handle.release();
    Ok(Finalized { branch, dest })
}

/// Roll back now (disarming the guard) and fold any cleanup problem into the
/// returned error so the user learns the directory may still exist.
fn rollback_error(guard: &mut RollbackGuard<'_>, cause: anyhow::Error) -> anyhow::Error {
    guard.armed = false;
    match guard.handle.rollback() {
        RemoveOutcome::Removed | RemoveOutcome::Gone => cause,
        other => cause.context(format!(
            "the cloned directory could not be removed ({other:?}); delete it manually"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_clone::runner::{CloneOutcome, start};
    use std::cell::RefCell;
    use std::fs;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn trunk_remote(root: &Path) -> String {
        let bare = root.join("remote.git");
        let work = root.join("work");
        fs::create_dir_all(&work).unwrap();
        git(root, &["init", "--bare", "-b", "trunk", "remote.git"]);
        git(&work, &["init", "-b", "trunk"]);
        fs::write(work.join("README"), "hi").unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-m", "init"]);
        git(&work, &["push", bare.to_str().unwrap(), "trunk"]);
        format!("file://{}", bare.display())
    }

    fn env() -> GitEnv {
        GitEnv::new().with_file_transport(true)
    }

    fn cloned(tmp: &Path) -> (CloneHandle, PathBuf) {
        let url = trunk_remote(tmp);
        let dest = tmp.join("repo");
        let mut h = start(&env(), &url, &dest).unwrap();
        assert_eq!(h.wait(), CloneOutcome::Success);
        (h, dest)
    }

    #[test]
    fn success_registers_once_keeps_dir_and_releases() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut h, dest) = cloned(tmp.path());
        let calls = RefCell::new(Vec::new());
        let out = finalize(&env(), &mut h, Some("trunk"), "proj", |p, n| {
            calls.borrow_mut().push((p.to_path_buf(), n.to_string()));
            Ok(())
        })
        .unwrap();
        assert_eq!(
            out,
            Finalized {
                branch: "trunk".into(),
                dest: dest.clone()
            }
        );
        assert_eq!(*calls.borrow(), vec![(dest.clone(), "proj".to_string())]);
        assert!(!h.cancel(), "cancel refused after finalize");
        // Released: neither rollback nor drop may touch it.
        assert!(matches!(h.rollback(), RemoveOutcome::NotOwned));
        drop(h);
        assert!(dest.join("README").is_file());
    }

    #[test]
    fn none_preflight_branch_accepts_actual_head() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut h, dest) = cloned(tmp.path());
        let out = finalize(&env(), &mut h, None, "p", |_, _| Ok(())).unwrap();
        assert_eq!(out.branch, "trunk");
        drop(h);
        assert!(dest.is_dir());
    }

    #[test]
    fn registration_error_removes_dir_and_returns_cause() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut h, dest) = cloned(tmp.path());
        let records = RefCell::new(Vec::<String>::new());
        let err = finalize(&env(), &mut h, Some("trunk"), "proj", |_, n| {
            // A real registrar fails before recording; nothing is retained.
            let _ = &records;
            let _ = n;
            Err(anyhow!("duplicate project name"))
        })
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("duplicate project name"), "{msg}");
        assert!(msg.contains("registering the project failed"), "{msg}");
        assert!(records.borrow().is_empty());
        assert!(!dest.exists(), "owned dir must be removed");
        assert!(!h.cancel());
        assert!(tmp.path().is_dir(), "parent must survive");
    }

    #[test]
    fn branch_mismatch_rolls_back_without_registering() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut h, dest) = cloned(tmp.path());
        let called = RefCell::new(false);
        let err = finalize(&env(), &mut h, Some("main"), "p", |_, _| {
            *called.borrow_mut() = true;
            Ok(())
        })
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("'trunk'") && msg.contains("'main'"), "{msg}");
        assert!(!*called.borrow(), "must not register on mismatch");
        assert!(!dest.exists());
    }

    #[test]
    fn detached_head_rolls_back() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut h, dest) = cloned(tmp.path());
        git(&dest, &["checkout", "--detach"]);
        let called = RefCell::new(false);
        let err = finalize(&env(), &mut h, None, "p", |_, _| {
            *called.borrow_mut() = true;
            Ok(())
        })
        .unwrap_err();
        assert!(format!("{err:#}").contains("no checked-out branch"));
        assert!(!*called.borrow());
        assert!(!dest.exists());
    }

    #[test]
    fn cancel_is_refused_once_finalize_begins() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut h, dest) = cloned(tmp.path());
        // `finalize` holds `&mut CloneHandle`, so no cancel can interleave;
        // the flag it sets is what refuses cancel afterwards, on every path.
        assert!(h.begin_point_of_no_return());
        assert!(!h.cancel());
        let out = finalize(&env(), &mut h, None, "p", |_, _| Ok(()));
        assert!(out.is_ok());
        assert!(!h.cancel());
        drop(h);
        assert!(dest.is_dir());
    }

    #[test]
    fn unsuccessful_clone_is_not_registered() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("repo");
        let bogus = format!("file://{}/nope.git", tmp.path().display());
        let mut h = start(&env(), &bogus, &dest).unwrap();
        assert!(matches!(h.wait(), CloneOutcome::Failed { .. }));
        let called = RefCell::new(false);
        let err = finalize(&env(), &mut h, None, "p", |_, _| {
            *called.borrow_mut() = true;
            Ok(())
        });
        assert!(err.is_err());
        assert!(!*called.borrow());
        assert!(!dest.exists());
    }

    #[test]
    fn panic_in_callback_still_rolls_back_and_propagates() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut h, dest) = cloned(tmp.path());
        let r = catch_unwind(AssertUnwindSafe(|| {
            let _ = finalize(&env(), &mut h, Some("trunk"), "p", |_, _| {
                panic!("registrar blew up")
            });
        }));
        assert!(r.is_err(), "panic must propagate, not be swallowed");
        assert!(!dest.exists(), "guard must roll the dir back on unwind");
        drop(h);
        assert!(tmp.path().is_dir());
    }
}
