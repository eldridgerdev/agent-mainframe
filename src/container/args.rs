//! Pure builders for `docker` argv. Nothing here spawns a process.
//!
//! Every container AMF starts for a sandboxed project shares one hardening
//! baseline ([`hardening`]): no capabilities, no privilege escalation, the
//! host uid (never root), bounded memory and pids, and a read-only root
//! filesystem. Only paths passed in as mounts are writable from the host's
//! point of view.

use std::ffi::OsString;
use std::path::Path;

use super::settings::ContainerSettings;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgsError {
    /// `--mount` fields are comma separated, so a comma in a path cannot be
    /// expressed safely.
    UnmountablePath(String),
    NotAbsolute(String),
}

impl std::fmt::Display for ArgsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnmountablePath(p) => write!(
                f,
                "{p} contains a comma and cannot be mounted into a container; choose a different location"
            ),
            Self::NotAbsolute(p) => write!(f, "{p} is not an absolute path"),
        }
    }
}

impl std::error::Error for ArgsError {}

/// `uid:gid` of the host user; containers run as them, never as root.
pub fn host_uid_gid() -> String {
    // SAFETY: getuid/getgid take no arguments and cannot fail.
    unsafe { format!("{}:{}", libc::getuid(), libc::getgid()) }
}

fn os(s: &str) -> OsString {
    OsString::from(s)
}

/// `--mount type=bind,...` for `src` at the same path inside the container.
/// Mounting at an identical path keeps absolute paths inside git metadata
/// (worktree `.git` files) valid on both sides.
pub fn bind_mount(src: &Path, read_only: bool) -> Result<[OsString; 2], ArgsError> {
    let text = src.to_string_lossy();
    if !src.is_absolute() {
        return Err(ArgsError::NotAbsolute(text.into_owned()));
    }
    if text.contains(',') {
        return Err(ArgsError::UnmountablePath(text.into_owned()));
    }
    let ro = if read_only { ",readonly" } else { "" };
    Ok([
        os("--mount"),
        OsString::from(format!("type=bind,src={text},dst={text}{ro}")),
    ])
}

/// Flags every sandbox container gets. `uid_gid` is the host user.
pub fn hardening(settings: &ContainerSettings, uid_gid: &str) -> Vec<OsString> {
    let mut a: Vec<OsString> = vec![
        os("--runtime"),
        OsString::from(&settings.runtime),
        os("--cap-drop"),
        os("ALL"),
        os("--security-opt"),
        os("no-new-privileges"),
        os("--read-only"),
        os("--tmpfs"),
        os("/tmp:rw,nosuid,nodev,size=512m,mode=1777"),
        os("--user"),
        OsString::from(uid_gid),
        os("--memory"),
        OsString::from(&settings.memory),
        os("--pids-limit"),
        OsString::from(settings.pids_limit.to_string()),
        os("--init"),
    ];
    // Nothing the host exposes by default should leak in.
    a.extend([os("-e"), os("HOME=/home/sandbox")]);
    a
}

/// `docker run` for one git command: a throwaway container running `git`
/// with `mount` (if any) mounted read-write at the same path and nothing else.
/// The network is the default bridge; there are no credentials, no SSH agent
/// and no proxy, so only public HTTPS remotes work.
pub fn git_run(
    settings: &ContainerSettings,
    uid_gid: &str,
    name: &str,
    mount: Option<&Path>,
    git_args: Vec<OsString>,
) -> Result<Vec<OsString>, ArgsError> {
    let mut a: Vec<OsString> = vec![os("run"), os("--rm"), os("--name"), OsString::from(name)];
    a.extend(hardening(settings, uid_gid));
    match mount {
        Some(dest) => {
            a.extend(bind_mount(dest, false)?);
            a.extend([os("-w"), dest.as_os_str().to_owned()]);
        }
        None => a.extend([os("-w"), os("/tmp")]),
    }
    a.extend([
        os("-e"),
        os("GIT_TERMINAL_PROMPT=0"),
        os("-e"),
        os("GIT_CONFIG_GLOBAL=/dev/null"),
        os("-e"),
        os("GIT_CONFIG_SYSTEM=/dev/null"),
        os("-e"),
        os("LC_ALL=C"),
    ]);
    a.push(OsString::from(&settings.image));
    a.push(os("git"));
    a.extend(git_args);
    Ok(a)
}

/// Docker object names for one feature's sandbox. Derived from the tmux
/// session name so a restarted AMF finds what an earlier one started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Names {
    pub network: String,
    pub proxy: String,
    pub sandbox: String,
}

impl Names {
    pub fn for_session(tmux_session: &str) -> Self {
        let clean: String = tmux_session
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        Self {
            network: format!("amf-net-{clean}"),
            proxy: format!("amf-proxy-{clean}"),
            sandbox: format!("amf-sbx-{clean}"),
        }
    }
}

/// Everything needed to start one feature's sandbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxSpec {
    pub tmux_session: String,
    /// Host directories mounted read-write at the same path: the project's
    /// repository and, if it lies outside it, the feature's worktree.
    pub mounts: Vec<std::path::PathBuf>,
    /// Paths inside `mounts` mounted read-only on top: the git config and
    /// hooks. Host git reads these constantly; if code in the sandbox could
    /// write `core.fsmonitor` or a hook there, the host would run it.
    pub protect: Vec<std::path::PathBuf>,
    pub workdir: std::path::PathBuf,
    /// Named volume that holds the sandbox's `$HOME` (agent logins). One per
    /// project, so one hostile project cannot read another's credentials.
    pub home_volume: String,
    pub settings: ContainerSettings,
}

/// `docker network create --internal`: no route out, so the proxy is the only
/// way to the internet.
pub fn network_create(names: &Names) -> Vec<OsString> {
    vec![
        os("network"),
        os("create"),
        os("--internal"),
        os("--label"),
        os("amf.sandbox=1"),
        OsString::from(&names.network),
    ]
}

/// The allowlisting proxy: on the default bridge (so it can reach out), with
/// its config directory mounted read-only. It is attached to the internal
/// network afterwards ([`proxy_connect`]).
pub fn proxy_run(
    spec: &SandboxSpec,
    uid_gid: &str,
    names: &Names,
    conf_dir: &Path,
) -> Result<Vec<OsString>, ArgsError> {
    let mut a: Vec<OsString> = vec![
        os("run"),
        os("-d"),
        os("--name"),
        OsString::from(&names.proxy),
        os("--label"),
        os("amf.sandbox=1"),
    ];
    a.extend(hardening(&spec.settings, uid_gid));
    a.extend(bind_mount_at(conf_dir, Path::new("/etc/amf-proxy"), true)?);
    a.push(OsString::from(&spec.settings.image));
    a.extend([
        os("tinyproxy"),
        os("-d"),
        os("-c"),
        os("/etc/amf-proxy/tinyproxy.conf"),
    ]);
    Ok(a)
}

pub fn proxy_connect(names: &Names) -> Vec<OsString> {
    vec![
        os("network"),
        os("connect"),
        os("--alias"),
        OsString::from(&names.proxy),
        OsString::from(&names.network),
        OsString::from(&names.proxy),
    ]
}

/// `--mount` for a host path at an explicit container path.
pub fn bind_mount_at(src: &Path, dst: &Path, read_only: bool) -> Result<[OsString; 2], ArgsError> {
    let (s, d) = (src.to_string_lossy(), dst.to_string_lossy());
    if !src.is_absolute() {
        return Err(ArgsError::NotAbsolute(s.into_owned()));
    }
    if s.contains(',') {
        return Err(ArgsError::UnmountablePath(s.into_owned()));
    }
    let ro = if read_only { ",readonly" } else { "" };
    Ok([
        os("--mount"),
        OsString::from(format!("type=bind,src={s},dst={d}{ro}")),
    ])
}

/// The long-lived container the feature's panes `docker exec` into. Only the
/// spec's mounts and its home volume are writable from the host's point of
/// view; with egress filtering on, its only network is the internal one.
pub fn sandbox_run(
    spec: &SandboxSpec,
    uid_gid: &str,
    names: &Names,
) -> Result<Vec<OsString>, ArgsError> {
    let mut a: Vec<OsString> = vec![
        os("run"),
        os("-d"),
        os("--name"),
        OsString::from(&names.sandbox),
        os("--label"),
        os("amf.sandbox=1"),
    ];
    a.extend(hardening(&spec.settings, uid_gid));
    if spec.settings.egress_allowlist_enabled {
        a.extend([os("--network"), OsString::from(&names.network)]);
        for (k, v) in super::egress::proxy_env(&names.proxy) {
            a.extend([os("-e"), OsString::from(format!("{k}={v}"))]);
        }
    }
    for m in &spec.mounts {
        a.extend(bind_mount(m, false)?);
    }
    for p in &spec.protect {
        a.extend(bind_mount(p, true)?);
    }
    a.extend([
        os("--mount"),
        OsString::from(format!(
            "type=volume,src={},dst=/home/sandbox",
            spec.home_volume
        )),
    ]);
    a.extend([os("-w"), spec.workdir.as_os_str().to_owned()]);
    a.push(OsString::from(&spec.settings.image));
    a.extend([os("sleep"), os("infinity")]);
    Ok(a)
}

/// Single-quote `value` for a POSIX shell.
pub fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// The command that turns a pane's shell into a shell inside the sandbox.
pub fn exec_shell_command(names: &Names, workdir: &Path) -> String {
    format!(
        "exec docker exec -it -e TERM=xterm-256color -w {} {} bash -l",
        sh_quote(&workdir.to_string_lossy()),
        sh_quote(&names.sandbox)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn strs(v: &[OsString]) -> Vec<String> {
        v.iter().map(|s| s.to_string_lossy().into_owned()).collect()
    }

    fn spec() -> SandboxSpec {
        SandboxSpec {
            tmux_session: "amf-proj-feat".into(),
            mounts: vec![PathBuf::from("/code/repo")],
            protect: vec![
                PathBuf::from("/code/repo/.git/config"),
                PathBuf::from("/code/repo/.git/hooks"),
            ],
            workdir: PathBuf::from("/code/repo/.worktrees/feat"),
            home_volume: "amf-sbx-home-abc".into(),
            settings: ContainerSettings::default(),
        }
    }

    #[test]
    fn sandbox_joins_only_the_internal_network_with_proxy_env() {
        let spec = spec();
        let names = Names::for_session(&spec.tmux_session);
        let a = strs(&sandbox_run(&spec, "1000:1000", &names).unwrap());
        let j = a.join(" ");
        assert!(j.contains("--network amf-net-amf-proj-feat"));
        assert!(j.contains("HTTPS_PROXY=http://amf-proxy-amf-proj-feat:8888"));
        assert!(j.contains("type=bind,src=/code/repo,dst=/code/repo "));
        assert!(j.contains("src=/code/repo/.git/config,dst=/code/repo/.git/config,readonly"));
        assert!(j.contains("src=/code/repo/.git/hooks,dst=/code/repo/.git/hooks,readonly"));
        assert!(j.contains("type=volume,src=amf-sbx-home-abc,dst=/home/sandbox"));
        assert!(j.ends_with("amf-sandbox:latest sleep infinity"));
        assert!(!j.contains("docker.sock") && !j.contains("--privileged"));
        assert!(!j.contains("--network host") && !j.contains("--network bridge"));
    }

    #[test]
    fn without_egress_filtering_the_sandbox_uses_the_default_network() {
        let mut spec = spec();
        spec.settings.egress_allowlist_enabled = false;
        let names = Names::for_session(&spec.tmux_session);
        let j = strs(&sandbox_run(&spec, "1000:1000", &names).unwrap()).join(" ");
        assert!(!j.contains("--network"));
        assert!(!j.contains("PROXY"));
    }

    #[test]
    fn proxy_is_config_readonly_and_never_gets_the_checkout() {
        let spec = spec();
        let names = Names::for_session(&spec.tmux_session);
        let a = strs(
            &proxy_run(
                &spec,
                "1000:1000",
                &names,
                Path::new("/state/sbx/amf-proj-feat"),
            )
            .unwrap(),
        );
        let j = a.join(" ");
        assert!(j.contains("src=/state/sbx/amf-proj-feat,dst=/etc/amf-proxy,readonly"));
        assert!(!j.contains("/code/repo"));
        assert!(j.contains("tinyproxy -d -c /etc/amf-proxy/tinyproxy.conf"));
        assert_eq!(
            strs(&proxy_connect(&names)).join(" "),
            "network connect --alias amf-proxy-amf-proj-feat amf-net-amf-proj-feat amf-proxy-amf-proj-feat"
        );
        assert!(strs(&network_create(&names)).contains(&"--internal".to_string()));
    }

    #[test]
    fn names_are_sanitized_and_exec_command_is_quoted() {
        let n = Names::for_session("amf-we ird/na;me");
        assert_eq!(n.sandbox, "amf-sbx-amf-we_ird_na_me");
        let c = exec_shell_command(&n, Path::new("/a b/it's"));
        assert!(c.starts_with("exec docker exec -it"));
        assert!(c.contains("'/a b/it'\\''s'"), "{c}");
    }

    #[test]
    fn mount_is_same_path_and_rejects_commas_and_relative() {
        let m = bind_mount(Path::new("/work/repo"), false).unwrap();
        assert_eq!(strs(&m)[1], "type=bind,src=/work/repo,dst=/work/repo");
        let ro = bind_mount(Path::new("/work/repo"), true).unwrap();
        assert!(strs(&ro)[1].ends_with(",readonly"));
        assert!(matches!(
            bind_mount(Path::new("/a,b"), false),
            Err(ArgsError::UnmountablePath(_))
        ));
        assert!(matches!(
            bind_mount(Path::new("rel"), false),
            Err(ArgsError::NotAbsolute(_))
        ));
    }

    #[test]
    fn hardening_drops_everything() {
        let s = ContainerSettings {
            runtime: "runsc".into(),
            ..Default::default()
        };
        let a = strs(&hardening(&s, "1000:1000"));
        let joined = a.join(" ");
        assert!(joined.contains("--runtime runsc"));
        assert!(joined.contains("--cap-drop ALL"));
        assert!(joined.contains("--security-opt no-new-privileges"));
        assert!(joined.contains("--read-only"));
        assert!(joined.contains("--user 1000:1000"));
        assert!(!joined.contains("--privileged"));
        assert!(!a.iter().any(|x| x == "root" || x.starts_with("0:")));
    }

    #[test]
    fn git_run_mounts_only_dest_and_runs_git() {
        let s = ContainerSettings::default();
        let dest = PathBuf::from("/home/u/code/repo");
        let a = strs(
            &git_run(
                &s,
                "1000:1000",
                "amf-clone-1",
                Some(&dest),
                vec![
                    os("clone"),
                    os("--"),
                    os("https://github.com/o/r"),
                    dest.clone().into(),
                ],
            )
            .unwrap(),
        );
        assert_eq!(a[0], "run");
        assert_eq!(a.iter().filter(|x| *x == "--mount").count(), 1);
        assert!(
            a.iter()
                .any(|x| x.starts_with("type=bind,src=/home/u/code/repo"))
        );
        let img = a.iter().position(|x| x == "amf-sandbox:latest").unwrap();
        assert_eq!(a[img + 1], "git");
        assert!(!a.iter().any(|x| x.contains("docker.sock")));
        assert!(!a.iter().any(|x| x.contains(".ssh")));
    }
}

#[cfg(test)]
mod no_mount_tests {
    use super::*;

    #[test]
    fn ls_remote_style_run_mounts_nothing() {
        let s = ContainerSettings::default();
        let a: Vec<String> = git_run(&s, "1:1", "n", None, vec![OsString::from("ls-remote")])
            .unwrap()
            .iter()
            .map(|x| x.to_string_lossy().into_owned())
            .collect();
        assert!(!a.iter().any(|x| x == "--mount"));
    }
}
