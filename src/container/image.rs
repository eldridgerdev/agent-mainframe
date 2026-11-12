//! Making sure the sandbox image exists. The only module here that spawns.

use std::io::Write;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use super::settings::ContainerSettings;

/// The Dockerfile AMF builds the default image from.
pub const DOCKERFILE: &str = include_str!("../../docker/sandbox/Dockerfile");

pub fn docker_available() -> Result<()> {
    let out = Command::new("docker")
        .args(["version", "--format", "{{.Server.Version}}"])
        .stdin(Stdio::null())
        .output()
        .context("Docker is not installed or not on PATH")?;
    if !out.status.success() {
        bail!(
            "Docker is installed but the daemon is not reachable: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// `true` when the daemon lists `runtime` as registered.
pub fn runtime_registered(runtime: &str) -> Result<bool> {
    let out = Command::new("docker")
        .args(["info", "--format", "{{json .Runtimes}}"])
        .stdin(Stdio::null())
        .output()
        .context("could not ask Docker for its runtimes")?;
    if !out.status.success() {
        bail!(
            "docker info failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(runtime_in_info(
        &String::from_utf8_lossy(&out.stdout),
        runtime,
    ))
}

fn runtime_in_info(json: &str, runtime: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|v| v.as_object().map(|o| o.contains_key(runtime)))
        .unwrap_or(false)
}

pub fn image_exists(image: &str) -> bool {
    Command::new("docker")
        .args(["image", "inspect", image])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Pre-flight for any container operation: Docker works, the chosen runtime is
/// registered, and the image exists (built from [`DOCKERFILE`] if it is the
/// default and missing; a custom image is never built or pulled implicitly).
pub fn ensure_ready(settings: &ContainerSettings) -> Result<()> {
    settings.validate()?;
    docker_available()?;
    if !runtime_registered(&settings.runtime)? {
        bail!(
            "Docker has no runtime named '{}'. Install and register it (for gVisor: `runsc install`) or choose another runtime in settings.",
            settings.runtime
        );
    }
    if image_exists(&settings.image) {
        return Ok(());
    }
    if !settings.uses_default_image() {
        bail!(
            "Container image '{}' does not exist locally. Build or pull it first.",
            settings.image
        );
    }
    build_default_image(&settings.image)
}

fn build_default_image(tag: &str) -> Result<()> {
    let mut child = Command::new("docker")
        .args(["build", "-t", tag, "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("could not start docker build")?;
    child
        .stdin
        .take()
        .context("no stdin for docker build")?
        .write_all(DOCKERFILE.as_bytes())?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = err.lines().rev().take(8).collect();
        bail!(
            "building the sandbox image failed:\n{}",
            tail.into_iter().rev().collect::<Vec<_>>().join("\n")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_runtime_in_docker_info_json() {
        let json = r#"{"runc":{"path":"runc"},"runsc":{"path":"/usr/bin/runsc"}}"#;
        assert!(runtime_in_info(json, "runsc"));
        assert!(!runtime_in_info(json, "kata"));
        assert!(!runtime_in_info("garbage", "runc"));
    }

    #[test]
    fn dockerfile_is_embedded() {
        assert!(DOCKERFILE.contains("tinyproxy"));
        assert!(DOCKERFILE.contains("FROM "));
    }
}
