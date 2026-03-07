use anyhow::{Context, Result};
use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

pub fn upgrade() -> Result<()> {
    let platform = detect_platform()?;
    println!("Detected platform: {}", platform.pretty_name);

    let current_exe = env::current_exe().context("Failed to get current executable path")?;
    let exe_dir = current_exe
        .parent()
        .context("Failed to determine executable directory")?;

    println!("Current amf location: {}", current_exe.display());

    check_write_permission(&current_exe)?;

    let latest_version = fetch_latest_version()?;
    println!("Latest version: {}", latest_version);

    let download_url = format!(
        "https://github.com/eldridgerdev/agent-mainframe/releases/download/{}/{}",
        latest_version, platform.binary_name
    );

    println!("Downloading from: {}", download_url);

    let temp_path = exe_dir.join(format!("amf-{}", latest_version));
    download_binary(&download_url, &temp_path)?;

    fs::set_permissions(&temp_path, fs::Permissions::from_mode(0o755))?;

    fs::rename(&temp_path, &current_exe).context("Failed to replace binary")?;

    println!("Successfully upgraded to {}!", latest_version);
    Ok(())
}

fn detect_platform() -> Result<Platform> {
    let arch = env::consts::ARCH;
    let os = env::consts::OS;

    let platform = match (arch, os) {
        ("x86_64", "linux") => Platform {
            binary_name: "amf-x86_64-unknown-linux-musl".to_string(),
            pretty_name: "Linux x86_64 (musl)".to_string(),
        },
        ("x86_64", "macos") => Platform {
            binary_name: "amf-aarch64-apple-darwin".to_string(),
            pretty_name: "macOS (Apple Silicon)".to_string(),
        },
        ("aarch64", "linux") => Platform {
            binary_name: "amf-aarch64-unknown-linux-gnu".to_string(),
            pretty_name: "Linux aarch64".to_string(),
        },
        _ => anyhow::bail!(
            "Unsupported platform: {}-{}. Please upgrade manually from GitHub releases.",
            arch,
            os
        ),
    };

    Ok(platform)
}

struct Platform {
    binary_name: String,
    pretty_name: String,
}

fn check_write_permission(exe_path: &Path) -> Result<()> {
    let parent = exe_path
        .parent()
        .context("No parent directory for executable")?;

    if !parent.exists() {
        anyhow::bail!("Parent directory does not exist: {}", parent.display());
    }

    let metadata = fs::metadata(parent).context("Failed to read directory permissions")?;
    let permissions = metadata.permissions();
    let mode = permissions.mode();

    let can_write = mode & 0o200 != 0;

    if !can_write {
        anyhow::bail!(
            "Cannot write to {}. Please run with sudo: sudo amf upgrade",
            parent.display()
        );
    }

    Ok(())
}

fn fetch_latest_version() -> Result<String> {
    let mut response =
        ureq::get("https://api.github.com/repos/eldridgerdev/agent-mainframe/releases/latest")
            .call()
            .context("Failed to fetch latest release info")?;

    let body = response
        .body_mut()
        .read_to_string()
        .context("Failed to read response body")?;

    let json: serde_json::Value =
        serde_json::from_str(&body).context("Failed to parse release info")?;

    let tag_name = json
        .get("tag_name")
        .and_then(|v: &serde_json::Value| v.as_str())
        .context("Release info missing tag_name")?;

    Ok(tag_name.to_string())
}

fn download_binary(url: &str, dest: &Path) -> Result<()> {
    let mut response = ureq::get(url)
        .call()
        .with_context(|| format!("Failed to download from {}", url))?;

    let buffer = response
        .body_mut()
        .read_to_vec()
        .context("Failed to read binary data")?;

    fs::write(dest, buffer).with_context(|| format!("Failed to write file {}", dest.display()))?;

    Ok(())
}
