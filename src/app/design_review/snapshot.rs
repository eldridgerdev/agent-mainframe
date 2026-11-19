//! What the review room reads from git when it opens: the base and head
//! commits, a fingerprint of the diff, and the files the detectors and the
//! "possibly stale" marker compare against. Every part is optional, since a
//! checkout that is not on a named branch has no diff to read.

use std::path::Path;
use std::process::Command;

use sha2::{Digest, Sha256};

use crate::diff::DiffFile;

#[derive(Debug, Clone, Default)]
pub(crate) struct RoomSnapshot {
    pub base_commit: Option<String>,
    pub head_commit: Option<String>,
    pub fingerprint: Option<String>,
    /// The current diff against the base; `None` when it could not be loaded.
    pub files: Option<Vec<DiffFile>>,
    /// Directories that exist at the base commit, for the new-module detector.
    pub base_dirs: Vec<String>,
}

impl RoomSnapshot {
    pub(crate) fn changed_paths(&self) -> Option<Vec<String>> {
        self.files
            .as_ref()
            .map(|files| files.iter().map(|f| f.path.clone()).collect())
    }
}

/// A stable digest of what the diff changes: each file's path, status and
/// patch, in path order.
pub(crate) fn diff_fingerprint(files: &[DiffFile]) -> String {
    let mut sorted: Vec<&DiffFile> = files.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    let mut hasher = Sha256::new();
    for file in sorted {
        hasher.update(file.path.as_bytes());
        hasher.update([0]);
        hasher.update(format!("{:?}", file.status).as_bytes());
        hasher.update([0]);
        hasher.update(file.patch.as_bytes());
        hasher.update([0]);
    }
    format!("{:x}", hasher.finalize())
}

fn git(workdir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(workdir)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Read the snapshot from `workdir`. Never fails: anything git cannot answer
/// stays `None`.
pub(crate) fn load_room_snapshot(workdir: &Path) -> RoomSnapshot {
    let head_commit = git(workdir, &["rev-parse", "--verify", "--quiet", "HEAD"])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let Ok(snapshot) = crate::diff::load_snapshot(workdir, None, false) else {
        return RoomSnapshot {
            head_commit,
            ..Default::default()
        };
    };
    let base_commit = Some(snapshot.base_commit.clone()).filter(|c| !c.is_empty());
    let base_dirs = base_commit
        .as_deref()
        .and_then(|base| git(workdir, &["ls-tree", "-d", "-r", "--name-only", base]))
        .map(|out| out.lines().map(str::to_string).collect())
        .unwrap_or_default();
    RoomSnapshot {
        base_commit,
        head_commit,
        fingerprint: Some(diff_fingerprint(&snapshot.files)),
        files: Some(snapshot.files),
        base_dirs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::DiffFileStatus;

    fn file(path: &str, patch: &str) -> DiffFile {
        DiffFile {
            old_path: None,
            path: path.into(),
            status: DiffFileStatus::Modified,
            additions: 1,
            deletions: 0,
            is_binary: false,
            old_content: None,
            new_content: None,
            patch: patch.into(),
            hunks: vec![],
        }
    }

    #[test]
    fn fingerprint_ignores_order_and_tracks_content() {
        let a = vec![file("a.rs", "+1"), file("b.rs", "+2")];
        let b = vec![file("b.rs", "+2"), file("a.rs", "+1")];
        assert_eq!(diff_fingerprint(&a), diff_fingerprint(&b));
        let c = vec![file("a.rs", "+1"), file("b.rs", "+3")];
        assert_ne!(diff_fingerprint(&a), diff_fingerprint(&c));
    }
}
