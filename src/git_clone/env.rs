//! The single builder through which every git/ssh child of the clone feature
//! is spawned.
//!
//! Goals, all enforced here and nowhere else:
//! - the child can never prompt: stdin is null, it runs in its own session
//!   (so ssh has no controlling TTY to open `/dev/tty` on), and every askpass
//!   / terminal-prompt / credential-manager path is switched off;
//! - the child leads its own process group (`setsid` makes pgid == pid), so a
//!   runner can kill `git` together with `ssh` / `git-remote-https`;
//! - only https and ssh transports are permitted, and `--` precedes the URL;
//! - the user's ssh configuration (`GIT_SSH_COMMAND`, `core.sshCommand`,
//!   `~/.ssh/config`) is deliberately left alone.

use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::container::ContainerSettings;
use crate::container::args as container_args;
use crate::container::args::host_uid_gid;

static CONTAINER_SEQ: AtomicU64 = AtomicU64::new(0);

/// Run git inside a throwaway container instead of on the host.
#[derive(Debug, Clone)]
struct ContainerRun {
    settings: ContainerSettings,
    uid_gid: String,
    /// The one host directory mounted (read-write, same path) into the
    /// container; `None` for commands that touch no checkout (`ls-remote`).
    mount: Option<PathBuf>,
}

/// What to do with one of the child's output streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Output {
    Null,
    Piped,
}

impl Output {
    fn stdio(self) -> Stdio {
        match self {
            Self::Null => Stdio::null(),
            Self::Piped => Stdio::piped(),
        }
    }
}

/// Environment variables that could make ssh or git launch an askpass helper.
const ASKPASS_VARS: &[&str] = &["SSH_ASKPASS", "GIT_ASKPASS", "ASKPASS"];

#[derive(Debug, Clone)]
pub struct GitEnv {
    git_binary: PathBuf,
    allow_file: bool,
    container: Option<ContainerRun>,
}

impl Default for GitEnv {
    fn default() -> Self {
        Self::new()
    }
}

impl GitEnv {
    /// Production builder: `git` from `PATH`, https and ssh only.
    pub fn new() -> Self {
        Self {
            git_binary: PathBuf::from("git"),
            allow_file: false,
            container: None,
        }
    }

    /// The same builder, but every git command runs in a container from
    /// `settings` (host uid, no credentials, no capabilities) with only
    /// `mount` visible. The host's git never touches the untrusted checkout.
    pub fn in_container(&self, settings: &ContainerSettings, mount: Option<PathBuf>) -> Self {
        Self {
            container: Some(ContainerRun {
                settings: settings.clone(),
                uid_gid: host_uid_gid(),
                mount,
            }),
            ..self.clone()
        }
    }

    pub fn is_containerized(&self) -> bool {
        self.container.is_some()
    }

    /// Use a different git executable (fake git scripts in tests).
    pub fn with_git_binary(mut self, path: impl Into<PathBuf>) -> Self {
        self.git_binary = path.into();
        self
    }

    /// Test-only: additionally permit the `file` transport so local bare
    /// repositories can serve as fixture remotes.
    #[cfg(test)]
    pub fn with_file_transport(mut self, allow: bool) -> Self {
        self.allow_file = allow;
        self
    }

    pub fn git_binary(&self) -> &Path {
        &self.git_binary
    }

    /// Global `-c` options placed before the subcommand.
    fn config_prefix(&self) -> Vec<OsString> {
        let mut cfg = vec![
            "credential.interactive=never",
            "protocol.allow=never",
            "protocol.https.allow=always",
            "protocol.ssh.allow=always",
        ];
        if self.allow_file {
            cfg.push("protocol.file.allow=always");
        }
        cfg.into_iter()
            .flat_map(|c| [OsString::from("-c"), OsString::from(c)])
            .collect()
    }

    /// Subcommand argv for `ls-remote --symref -- <url>`.
    pub fn ls_remote_args(url: &str) -> Vec<OsString> {
        ["ls-remote", "--symref", "--", url]
            .into_iter()
            .map(OsString::from)
            .collect()
    }

    /// Subcommand argv for `clone --progress --no-recurse-submodules -- <url> <dest>`.
    pub fn clone_args(url: &str, dest: &Path) -> Vec<OsString> {
        let mut args: Vec<OsString> = ["clone", "--progress", "--no-recurse-submodules", "--"]
            .into_iter()
            .map(OsString::from)
            .collect();
        args.push(OsString::from(url));
        args.push(dest.as_os_str().to_owned());
        args
    }

    /// Build a git command. `args` is the subcommand and its arguments (use
    /// the `*_args` helpers for anything involving a URL); the hardening
    /// `-c` options are prepended. stdin is always null.
    pub fn command<I, S>(&self, args: I, stdout: Output, stderr: Output) -> Command
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        let mut cmd = match &self.container {
            None => {
                let mut cmd = Command::new(&self.git_binary);
                cmd.args(self.config_prefix());
                cmd.args(args.into_iter().map(Into::into));
                cmd
            }
            Some(run) => {
                let mut git_args = self.config_prefix();
                git_args.extend(args.into_iter().map(Into::into));
                let name = format!(
                    "amf-git-{}-{}",
                    std::process::id(),
                    CONTAINER_SEQ.fetch_add(1, Ordering::Relaxed)
                );
                // A mount that cannot be expressed is a bug in path
                // validation upstream; fail the spawn rather than run
                // unmounted with a wrong view of the checkout.
                let argv = container_args::git_run(
                    &run.settings,
                    &run.uid_gid,
                    &name,
                    run.mount.as_deref(),
                    git_args,
                )
                .unwrap_or_else(|e| {
                    vec![
                        OsString::from("__amf_invalid_container_args__"),
                        OsString::from(e.to_string()),
                    ]
                });
                let mut cmd = Command::new("docker");
                cmd.args(argv);
                cmd
            }
        };
        self.harden(&mut cmd);
        cmd.stdin(Stdio::null())
            .stdout(stdout.stdio())
            .stderr(stderr.stdio());
        cmd
    }

    /// Like [`command`](Self::command) but for a non-git helper (e.g.
    /// `ssh-add -l`, `gh auth status`) that needs the same no-prompt, own
    /// session treatment. No git config prefix is added.
    pub fn helper_command<I, S>(
        program: impl Into<OsString>,
        args: I,
        stdout: Output,
        stderr: Output,
    ) -> Command
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        let mut cmd = Command::new(program.into());
        cmd.args(args.into_iter().map(Into::into));
        Self::new().harden(&mut cmd);
        cmd.stdin(Stdio::null())
            .stdout(stdout.stdio())
            .stderr(stderr.stdio());
        cmd
    }

    fn harden(&self, cmd: &mut Command) {
        for var in ASKPASS_VARS {
            cmd.env_remove(var);
        }
        cmd.env("GIT_TERMINAL_PROMPT", "0")
            .env("SSH_ASKPASS_REQUIRE", "never")
            .env("LC_ALL", "C");
        // SAFETY: the closure runs between fork and exec and only calls the
        // async-signal-safe `setsid`. A new session means no controlling
        // terminal (ssh cannot open /dev/tty) and pgid == pid, so the whole
        // tree can be signalled via `kill(-pid, ..)`. `setsid` fails with
        // EPERM for a process-group leader, so `process_group(0)` must NOT
        // also be used.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn containerized_command_runs_git_in_docker_with_only_the_mount() {
        let dest = PathBuf::from("/home/u/code/repo");
        let env = GitEnv::new().in_container(&ContainerSettings::default(), Some(dest.clone()));
        assert!(env.is_containerized());
        let cmd = env.command(
            GitEnv::clone_args("https://h/o/r", &dest),
            Output::Null,
            Output::Piped,
        );
        assert_eq!(cmd.get_program(), "docker");
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args[0], "run");
        assert!(args.iter().any(|a| a == "--cap-drop"));
        assert_eq!(args.iter().filter(|a| *a == "--mount").count(), 1);
        let git_at = args.iter().position(|a| a == "git").unwrap();
        // Hardening -c options still come first, then the clone itself.
        assert_eq!(args[git_at + 1], "-c");
        assert!(args.iter().any(|a| a == "protocol.allow=never"));
        assert!(args.iter().any(|a| a == "--no-recurse-submodules"));
    }

    #[test]
    fn host_env_is_not_containerized() {
        assert!(!GitEnv::new().is_containerized());
        let cmd = GitEnv::new().command(["--version"], Output::Piped, Output::Piped);
        assert_eq!(cmd.get_program(), "git");
    }
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    fn wait_bounded(child: &mut std::process::Child, limit: Duration) -> bool {
        let start = Instant::now();
        loop {
            if child.try_wait().unwrap().is_some() {
                return true;
            }
            if start.elapsed() > limit {
                unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
                let _ = child.wait();
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn argv(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn argv_has_hardening_prefix_and_double_dash_before_url() {
        let env = GitEnv::new();
        let cmd = env.command(
            GitEnv::ls_remote_args("https://h/o/r.git"),
            Output::Piped,
            Output::Piped,
        );
        let a = argv(&cmd);
        assert_eq!(
            a,
            [
                "-c",
                "credential.interactive=never",
                "-c",
                "protocol.allow=never",
                "-c",
                "protocol.https.allow=always",
                "-c",
                "protocol.ssh.allow=always",
                "ls-remote",
                "--symref",
                "--",
                "https://h/o/r.git"
            ]
        );
        assert!(!a.iter().any(|x| x.contains("file")));

        let a = argv(&env.command(
            GitEnv::clone_args("git@h:o/r.git", Path::new("/d/r")),
            Output::Null,
            Output::Piped,
        ));
        let dd = a.iter().position(|x| x == "--").unwrap();
        assert_eq!(&a[dd + 1..], ["git@h:o/r.git", "/d/r"]);
        assert!(a.contains(&"--no-recurse-submodules".to_string()));
        assert!(a[..dd].contains(&"clone".to_string()));
    }

    #[test]
    fn file_transport_only_when_explicitly_allowed() {
        let a = argv(&GitEnv::new().with_file_transport(true).command(
            ["--version"],
            Output::Null,
            Output::Null,
        ));
        assert!(a.contains(&"protocol.file.allow=always".to_string()));
    }

    #[test]
    fn env_is_hardened_and_ssh_config_untouched() {
        let cmd = GitEnv::new().command(["--version"], Output::Null, Output::Null);
        let envs: std::collections::HashMap<_, _> = cmd
            .get_envs()
            .map(|(k, v)| (k.to_string_lossy().into_owned(), v.map(|v| v.to_owned())))
            .collect();
        let val = |k: &str| {
            envs.get(k)
                .cloned()
                .flatten()
                .map(|v| v.to_string_lossy().into_owned())
        };
        assert_eq!(val("GIT_TERMINAL_PROMPT").as_deref(), Some("0"));
        assert_eq!(val("SSH_ASKPASS_REQUIRE").as_deref(), Some("never"));
        assert_eq!(val("LC_ALL").as_deref(), Some("C"));
        for v in ASKPASS_VARS {
            // Present in the map as an explicit removal (None).
            assert_eq!(envs.get(*v), Some(&None), "{v} must be removed");
        }
        assert!(!envs.contains_key("GIT_SSH_COMMAND"));
        assert!(!envs.contains_key("GIT_SSH"));
        assert!(!argv(&cmd).iter().any(|a| a.contains("sshCommand")));
    }

    #[test]
    fn child_sees_hardened_env_null_stdin_and_inherited_askpass_removed() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("out");
        let git = script(
            tmp.path(),
            "fakegit",
            &format!(
                "{{ env; echo \"STDIN_BYTES=$(cat | wc -c | tr -d ' ')\"; }} > '{}'",
                out.display()
            ),
        );
        let mut cmd =
            GitEnv::new()
                .with_git_binary(&git)
                .command(["x"], Output::Null, Output::Null);
        // Hostile inherited environment (askpass-only; harmless to other tests).
        for v in ASKPASS_VARS {
            unsafe { std::env::set_var(v, "/bin/false") };
        }
        let mut child = cmd.spawn().unwrap();
        for v in ASKPASS_VARS {
            unsafe { std::env::remove_var(v) };
        }
        assert!(wait_bounded(&mut child, Duration::from_secs(10)));
        let dump = fs::read_to_string(&out).unwrap();
        assert!(dump.contains("GIT_TERMINAL_PROMPT=0"));
        assert!(dump.contains("SSH_ASKPASS_REQUIRE=never"));
        assert!(dump.contains("LC_ALL=C"));
        assert!(dump.contains("STDIN_BYTES=0"));
        for v in ASKPASS_VARS {
            assert!(
                !dump.lines().any(|l| l.starts_with(&format!("{v}="))),
                "{v} leaked"
            );
        }
    }

    #[test]
    fn child_leads_its_own_session_and_process_group() {
        let tmp = tempfile::tempdir().unwrap();
        let git = script(tmp.path(), "fakegit", "sleep 30");
        let mut child = GitEnv::new()
            .with_git_binary(&git)
            .command(["x"], Output::Null, Output::Null)
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        let (sid, pgid, mine) = unsafe { (libc::getsid(pid), libc::getpgid(pid), libc::getsid(0)) };
        unsafe { libc::kill(-pid, libc::SIGKILL) };
        let _ = child.wait();
        assert_eq!(sid, pid, "child must be a session leader");
        assert_eq!(
            pgid, pid,
            "pgid must equal pid so kill(-pid) reaches the tree"
        );
        assert_ne!(sid, mine);
    }

    /// git really invokes the fake ssh for an ssh:// URL; the fake ssh tries to
    /// open /dev/tty, which must fail because the session has no controlling
    /// terminal. The whole thing must finish well inside the timeout.
    #[test]
    fn ssh_child_cannot_open_dev_tty_and_fails_fast() {
        if Command::new("git").arg("--version").output().is_err() {
            eprintln!("git not installed; skipping");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("marker");
        let ssh = script(
            tmp.path(),
            "ssh-fake",
            &format!(
                "if (exec 3</dev/tty) 2>/dev/null; then echo OPENED >> '{m}'; else echo DENIED >> '{m}'; fi\nexit 255",
                m = marker.display()
            ),
        );
        let started = Instant::now();
        let mut cmd = GitEnv::new().command(
            GitEnv::ls_remote_args("ssh://git@example.invalid/o/r.git"),
            Output::Null,
            Output::Null,
        );
        cmd.env("GIT_SSH", &ssh).env("GIT_SSH_VARIANT", "ssh");
        let mut child = cmd.spawn().unwrap();
        let finished = wait_bounded(&mut child, Duration::from_secs(20));
        assert!(finished, "clone probe hung instead of failing fast");
        assert!(started.elapsed() < Duration::from_secs(20));
        let m = fs::read_to_string(&marker).expect("git never invoked the fake ssh");
        assert!(m.contains("DENIED"), "marker: {m}");
        assert!(!m.contains("OPENED"), "ssh reached a controlling tty: {m}");
    }

    #[test]
    fn file_transport_is_refused_by_default_and_allowed_for_tests() {
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("r.git");
        assert!(
            Command::new("git")
                .args(["init", "--bare", "-q"])
                .arg(&repo)
                .status()
                .unwrap()
                .success()
        );
        let url = format!("file://{}", repo.display());
        let run = |env: GitEnv| {
            let mut c = env.command(GitEnv::ls_remote_args(&url), Output::Piped, Output::Piped);
            let mut child = c.spawn().unwrap();
            assert!(wait_bounded(&mut child, Duration::from_secs(20)));
            child.wait().unwrap().success()
        };
        assert!(!run(GitEnv::new()), "file transport must be refused");
        assert!(run(GitEnv::new().with_file_transport(true)));
    }
}
