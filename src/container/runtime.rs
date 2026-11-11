//! Starting and stopping one feature's sandbox containers. Spawns `docker`.

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use super::args::{self, Names, SandboxSpec};
use super::{egress, image};

fn docker(argv: &[OsString]) -> Result<String> {
    let out = Command::new("docker")
        .args(argv)
        .stdin(Stdio::null())
        .output()
        .context("could not run docker")?;
    if !out.status.success() {
        bail!(
            "docker {} failed: {}",
            argv.first()
                .map(|a| a.to_string_lossy())
                .unwrap_or_default(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn docker_ok(argv: &[&str]) -> bool {
    Command::new("docker")
        .args(argv)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn is_running(name: &str) -> bool {
    Command::new("docker")
        .args(["inspect", "-f", "{{.State.Running}}", name])
        .stdin(Stdio::null())
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "true")
        .unwrap_or(false)
}

fn conf_dir(names: &Names) -> PathBuf {
    crate::project::amf_config_dir()
        .join("sandbox")
        .join(&names.sandbox)
}

/// Write the proxy's config where only this user can touch it.
fn write_proxy_conf(spec: &SandboxSpec, names: &Names) -> Result<PathBuf> {
    let dir = conf_dir(names);
    fs::create_dir_all(&dir).with_context(|| format!("could not create {}", dir.display()))?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    fs::write(
        dir.join("allow.txt"),
        egress::filter_file(&spec.settings.egress_allowlist),
    )?;
    fs::write(
        dir.join("tinyproxy.conf"),
        egress::proxy_conf("/etc/amf-proxy/allow.txt"),
    )?;
    // The container runs as the host uid; make the files readable to it.
    for f in ["allow.txt", "tinyproxy.conf"] {
        fs::set_permissions(dir.join(f), fs::Permissions::from_mode(0o644))?;
    }
    Ok(dir)
}

/// Make sure the sandbox (and its proxy) for `spec` is running. Idempotent: a
/// restarted AMF finds the containers an earlier one started. Never builds
/// the image (that can take minutes and this runs on the UI thread); the
/// clone pre-flight does.
pub fn ensure_running(spec: &SandboxSpec) -> Result<()> {
    spec.settings.validate()?;
    image::docker_available()?;
    let names = Names::for_session(&spec.tmux_session);
    let egress = spec.settings.egress_allowlist_enabled;
    if is_running(&names.sandbox) && (!egress || is_running(&names.proxy)) {
        return Ok(());
    }
    if !image::image_exists(&spec.settings.image) {
        bail!(
            "The sandbox image '{}' is missing. Clone a project in container mode once, or build it with `docker build -t {} docker/sandbox`.",
            spec.settings.image,
            spec.settings.image
        );
    }
    teardown_names(&names);

    let uid_gid = args::host_uid_gid();
    if egress {
        let dir = write_proxy_conf(spec, &names)?;
        if !docker_ok(&["network", "inspect", &names.network]) {
            docker(&args::network_create(&names))?;
        }
        docker(&args::proxy_run(spec, &uid_gid, &names, &dir)?)?;
        docker(&args::proxy_connect(&names))?;
    }
    docker(&args::sandbox_run(spec, &uid_gid, &names)?)?;
    Ok(())
}

fn teardown_names(names: &Names) {
    for c in [&names.sandbox, &names.proxy] {
        let _ = docker_ok(&["rm", "-f", c]);
    }
    let _ = docker_ok(&["network", "rm", &names.network]);
}

/// Remove a feature's containers and network (not its home volume, which
/// keeps the agent login). Best effort.
pub fn teardown(tmux_session: &str) {
    teardown_names(&Names::for_session(tmux_session));
}
