//! Read the actual inputs consumed by worktree settings copying. The committed
//! destination is independent of the reviewed checkout's current working file.
use anyhow::{Context, Result, bail, ensure};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::Path;

const SETTINGS: &str = ".claude/settings.local.json";

fn git(repo: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()?;
    ensure!(
        output.status.success(),
        "Cannot read companion base settings from Git"
    );
    Ok(output.stdout)
}
fn entry(repo: &Path, base: &str, path: &str) -> Result<Option<(String, String)>> {
    let bytes = git(repo, &["ls-tree", "-z", base, "--", path])?;
    if bytes.is_empty() {
        return Ok(None);
    }
    let row = std::str::from_utf8(&bytes)?.trim_end_matches('\0');
    let (metadata, name) = row
        .split_once('\t')
        .context("Invalid companion base tree entry")?;
    ensure!(name == path, "Unexpected companion base settings path");
    let fields = metadata.split_whitespace().collect::<Vec<_>>();
    ensure!(
        fields.len() == 3,
        "Invalid companion base settings metadata"
    );
    Ok(Some((fields[0].to_string(), fields[2].to_string())))
}
fn object(bytes: &[u8], label: &str) -> Result<()> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .with_context(|| format!("Invalid JSON in {label}; companion creation was refused"))?;
    ensure!(
        value.is_object(),
        "{label} must contain a JSON object; companion creation was refused"
    );
    Ok(())
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// A revision binds the repository source's raw bytes/physical identity and the
/// immutable destination tree/blob. No configuration is written or defaulted.
/// The GUI checks again immediately before creation and after a delayed hook.
pub(crate) fn revision(repo: &Path, base: &str) -> Result<String> {
    let path = repo.join(SETTINGS);
    let source = match std::fs::read(&path) {
        Ok(bytes) => {
            object(&bytes, "repository .claude/settings.local.json")?;
            let physical = path.canonicalize()?;
            let metadata = physical.metadata()?;
            ensure!(
                metadata.is_file(),
                "Repository settings source is not a regular file"
            );
            #[cfg(unix)]
            let identity = {
                use std::os::unix::fs::MetadataExt;
                (metadata.dev(), metadata.ino())
            };
            #[cfg(not(unix))]
            let identity = metadata.created().ok();
            Some(json!([
                physical
                    .to_str()
                    .context("Repository settings source path is not valid UTF-8")?,
                identity,
                digest(&bytes)
            ]))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).context("Cannot read repository settings source"),
    };
    let parent = entry(repo, base, ".claude")?;
    if parent.as_ref().is_some_and(|(mode, _)| mode != "040000") {
        bail!(
            "The pinned companion base tracks .claude as a symlink or non-directory; refusing the settings-copy destination"
        );
    }
    let destination = entry(repo, base, SETTINGS)?;
    if let Some((mode, oid)) = &destination {
        ensure!(
            matches!(mode.as_str(), "100644" | "100755"),
            "The pinned companion base tracks .claude/settings.local.json as a symlink or unsupported file; refusing the settings-copy destination"
        );
        let bytes = git(repo, &["cat-file", "blob", oid])?;
        object(&bytes, "pinned companion base .claude/settings.local.json")?;
    }
    Ok(digest(&serde_json::to_vec(&json!([
        base,
        source,
        parent,
        destination
    ]))?))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn command(repo: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Fixture")
            .env("GIT_COMMITTER_NAME", "Fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.test")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.test")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed ({}): stdout={} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().into()
    }
    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        command(dir.path(), &["init", "-b", "main"]);
        std::fs::create_dir(dir.path().join(".claude")).unwrap();
        std::fs::write(dir.path().join(SETTINGS), "{\"unknown\":{\"kept\":true}}").unwrap();
        command(dir.path(), &["add", "-f", ".claude"]);
        command(dir.path(), &["commit", "-m", "settings"]);
        dir
    }
    #[test]
    fn binds_raw_repository_source_and_preserves_unknown_metadata_without_writes() {
        let dir = repo();
        let base = command(dir.path(), &["rev-parse", "HEAD"]);
        let path = dir.path().join(SETTINGS);
        let before = std::fs::read(&path).unwrap();
        let first = revision(dir.path(), &base).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before);
        std::fs::write(&path, b"{ \"unknown\":{\"kept\":true}}").unwrap();
        assert_ne!(first, revision(dir.path(), &base).unwrap());
        std::fs::write(&path, "[]").unwrap();
        assert!(
            revision(dir.path(), &base)
                .unwrap_err()
                .to_string()
                .contains("JSON object")
        );
    }
    #[test]
    fn malformed_committed_destination_is_refused_even_after_working_copy_repair() {
        let dir = repo();
        let path = dir.path().join(SETTINGS);
        std::fs::write(&path, "{malformed base").unwrap();
        command(dir.path(), &["add", "-f", ".claude"]);
        command(dir.path(), &["commit", "-m", "bad base"]);
        let base = command(dir.path(), &["rev-parse", "HEAD"]);
        std::fs::write(&path, "{\"fixed_locally\":true}").unwrap();
        let error = revision(dir.path(), &base).unwrap_err().to_string();
        assert!(error.contains("pinned companion base"), "{error}");
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "{\"fixed_locally\":true}"
        );
    }
    #[cfg(unix)]
    #[test]
    fn tracked_settings_and_parent_symlinks_are_refused_before_checkout() {
        for parent in [false, true] {
            let dir = repo();
            let path = dir.path().join(SETTINGS);
            if parent {
                std::fs::remove_dir_all(dir.path().join(".claude")).unwrap();
                std::os::unix::fs::symlink("elsewhere", dir.path().join(".claude")).unwrap();
            } else {
                std::fs::remove_file(&path).unwrap();
                std::os::unix::fs::symlink("elsewhere", &path).unwrap();
            }
            command(dir.path(), &["add", "-f", ".claude"]);
            command(dir.path(), &["commit", "-m", "symlink base"]);
            let base = command(dir.path(), &["rev-parse", "HEAD"]);
            // A valid local directory/file must not hide the committed link.
            if parent {
                std::fs::remove_file(dir.path().join(".claude")).unwrap();
                std::fs::create_dir(dir.path().join(".claude")).unwrap();
            } else {
                std::fs::remove_file(&path).unwrap();
            }
            std::fs::write(&path, "{}").unwrap();
            assert!(
                revision(dir.path(), &base)
                    .unwrap_err()
                    .to_string()
                    .contains("symlink")
            );
        }
    }
}
