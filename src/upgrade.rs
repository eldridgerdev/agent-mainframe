use crate::http_client;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

pub fn upgrade() -> Result<()> {
    let platform = detect_platform()?;
    println!("Detected platform: {}", platform.pretty_name);

    let current_exe = env::current_exe().context("Failed to get current executable path")?;
    let exe_dir = current_exe
        .parent()
        .context("Failed to determine executable directory")?;

    println!("Current amf location: {}", current_exe.display());

    check_write_permission(&current_exe)?;

    let release = fetch_latest_release()?;
    println!("Latest version: {}", release.tag_name);

    let asset = select_release_asset(&release, &platform)?;
    println!("Using release asset: {}", asset.name);
    println!("Downloading from: {}", asset.browser_download_url);

    if asset.name.ends_with(".tar.gz") {
        install_bundle_asset(asset, exe_dir)?;
    } else {
        install_binary_asset(asset, &current_exe, exe_dir)?;
    }

    println!("Successfully upgraded to {}!", release.tag_name);
    Ok(())
}

fn detect_platform() -> Result<Platform> {
    let apple_silicon_host = if env::consts::ARCH == "x86_64" && env::consts::OS == "macos" {
        Some(macos_host_is_apple_silicon()?)
    } else {
        None
    };

    platform_for(env::consts::ARCH, env::consts::OS, apple_silicon_host)
}

fn platform_for(arch: &str, os: &str, apple_silicon_host: Option<bool>) -> Result<Platform> {
    let platform = match (arch, os) {
        ("x86_64", "linux") => Platform {
            asset_stem: "amf-x86_64-unknown-linux-musl".to_string(),
            pretty_name: "Linux x86_64 (musl)".to_string(),
        },
        ("aarch64", "macos") => Platform {
            asset_stem: "amf-aarch64-apple-darwin".to_string(),
            pretty_name: "macOS Apple Silicon".to_string(),
        },
        ("aarch64", "linux") => Platform {
            asset_stem: "amf-aarch64-unknown-linux-gnu".to_string(),
            pretty_name: "Linux aarch64".to_string(),
        },
        ("x86_64", "macos") if apple_silicon_host == Some(true) => Platform {
            asset_stem: "amf-aarch64-apple-darwin".to_string(),
            pretty_name: "macOS Apple Silicon (running x86_64 AMF under Rosetta 2)".to_string(),
        },
        ("x86_64", "macos") => anyhow::bail!(
            "Unsupported platform: x86_64-macos. GitHub releases currently publish only Apple Silicon macOS bundles."
        ),
        _ => anyhow::bail!(
            "Unsupported platform: {arch}-{os}. Please upgrade manually from GitHub releases."
        ),
    };

    Ok(platform)
}

#[derive(Debug)]
struct Platform {
    asset_stem: String,
    pretty_name: String,
}

impl Platform {
    fn candidate_asset_names(&self) -> [String; 2] {
        [
            format!("{}.tar.gz", self.asset_stem),
            self.asset_stem.clone(),
        ]
    }
}

#[derive(Debug, Deserialize)]
struct ReleaseInfo {
    tag_name: String,
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
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

fn fetch_latest_release() -> Result<ReleaseInfo> {
    let mut response = http_client::https_agent()
        .get("https://api.github.com/repos/eldridgerdev/agent-mainframe/releases/latest")
        .call()
        .context("Failed to fetch latest release info")?;

    let body = response
        .body_mut()
        .read_to_string()
        .context("Failed to read response body")?;

    serde_json::from_str(&body).context("Failed to parse release info")
}

fn select_release_asset<'a>(
    release: &'a ReleaseInfo,
    platform: &Platform,
) -> Result<&'a ReleaseAsset> {
    let candidates = platform.candidate_asset_names();

    if let Some(asset) = candidates
        .iter()
        .find_map(|name| release.assets.iter().find(|asset| asset.name == *name))
    {
        return Ok(asset);
    }

    let available = release
        .assets
        .iter()
        .map(|asset| asset.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");

    anyhow::bail!(
        "Latest release {} does not contain a compatible asset for {}. Expected one of: {}. Available assets: {}",
        release.tag_name,
        platform.pretty_name,
        candidates.join(", "),
        available
    )
}

fn install_bundle_asset(asset: &ReleaseAsset, exe_dir: &Path) -> Result<()> {
    let bundle_dir_name = asset
        .name
        .strip_suffix(".tar.gz")
        .context("Bundle asset name must end with .tar.gz")?;

    let temp_dir = tempfile::Builder::new()
        .prefix("amf-upgrade-")
        .tempdir()
        .context("Failed to create temp directory")?;

    let archive_path = temp_dir.path().join(&asset.name);
    download_asset(&asset.browser_download_url, &archive_path)?;

    let status = Command::new("tar")
        .args([
            "-xzf",
            archive_path.to_str().unwrap(),
            "-C",
            temp_dir.path().to_str().unwrap(),
        ])
        .status()
        .context("Failed to run tar")?;
    anyhow::ensure!(status.success(), "tar extraction failed");

    let extracted_dir = temp_dir.path().join(bundle_dir_name);
    anyhow::ensure!(
        extracted_dir.exists(),
        "Expected extracted directory not found: {}",
        extracted_dir.display()
    );

    for entry in fs::read_dir(&extracted_dir).context("Failed to read extracted bundle")? {
        let entry = entry?;
        let dest = exe_dir.join(entry.file_name());
        fs::copy(entry.path(), &dest).with_context(|| {
            format!(
                "Failed to copy {} to {}",
                entry.path().display(),
                dest.display()
            )
        })?;
        let mut perms = fs::metadata(&dest)?.permissions();
        perms.set_mode(perms.mode() | 0o111);
        fs::set_permissions(&dest, perms)?;
    }

    Ok(())
}

fn install_binary_asset(asset: &ReleaseAsset, current_exe: &Path, exe_dir: &Path) -> Result<()> {
    let temp_path = exe_dir.join(format!("{}-download", asset.name));
    download_asset(&asset.browser_download_url, &temp_path)?;

    fs::set_permissions(&temp_path, fs::Permissions::from_mode(0o755))?;
    fs::rename(&temp_path, current_exe).context("Failed to replace binary")?;

    Ok(())
}

fn download_asset(url: &str, dest: &Path) -> Result<()> {
    let mut response = http_client::https_agent()
        .get(url)
        .call()
        .with_context(|| format!("Failed to download from {}", url))?;

    let buffer = response
        .body_mut()
        .read_to_vec()
        .context("Failed to read binary data")?;

    fs::write(dest, buffer).with_context(|| format!("Failed to write file {}", dest.display()))?;

    Ok(())
}

fn macos_host_is_apple_silicon() -> Result<bool> {
    let output = Command::new("sysctl")
        .args(["-in", "hw.optional.arm64"])
        .output()
        .context("Failed to run sysctl to detect macOS host architecture")?;

    anyhow::ensure!(
        output.status.success(),
        "sysctl failed while detecting macOS host architecture"
    );

    let stdout = String::from_utf8(output.stdout).context("sysctl returned non-UTF-8 output")?;
    match stdout.trim() {
        "1" => Ok(true),
        "0" => Ok(false),
        value => anyhow::bail!("Unexpected sysctl output for hw.optional.arm64: {value}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_release(assets: &[&str]) -> ReleaseInfo {
        ReleaseInfo {
            tag_name: "v0.12.0".to_string(),
            assets: assets
                .iter()
                .map(|name| ReleaseAsset {
                    name: (*name).to_string(),
                    browser_download_url: format!("https://example.com/{name}"),
                })
                .collect(),
        }
    }

    #[test]
    fn prefers_bundle_asset_when_available() {
        let release = sample_release(&[
            "amf-aarch64-apple-darwin",
            "amf-aarch64-apple-darwin.tar.gz",
        ]);
        let platform = Platform {
            asset_stem: "amf-aarch64-apple-darwin".to_string(),
            pretty_name: "macOS Apple Silicon".to_string(),
        };

        let asset = select_release_asset(&release, &platform).unwrap();

        assert_eq!(asset.name, "amf-aarch64-apple-darwin.tar.gz");
    }

    #[test]
    fn falls_back_to_legacy_binary_asset() {
        let release = sample_release(&["amf-aarch64-apple-darwin"]);
        let platform = Platform {
            asset_stem: "amf-aarch64-apple-darwin".to_string(),
            pretty_name: "macOS Apple Silicon".to_string(),
        };

        let asset = select_release_asset(&release, &platform).unwrap();

        assert_eq!(asset.name, "amf-aarch64-apple-darwin");
    }

    #[test]
    fn x86_64_macos_requires_apple_silicon_host() {
        let err = platform_for("x86_64", "macos", Some(false)).unwrap_err();

        assert!(
            err.to_string()
                .contains("publish only Apple Silicon macOS bundles")
        );
    }

    #[test]
    fn x86_64_macos_uses_arm_bundle_when_running_under_rosetta() {
        let platform = platform_for("x86_64", "macos", Some(true)).unwrap();

        assert_eq!(platform.asset_stem, "amf-aarch64-apple-darwin");
        assert!(platform.pretty_name.contains("Rosetta 2"));
    }
}
