//! `amf decision add`: record one design decision.
//!
//! Runs in the agent's process, outside the TUI. It never opens `amf.db` and
//! needs no running AMF: it appends one line to the checkout's outbox and
//! prints a one-line acknowledgement. Everything mechanical (sequence number,
//! uid, timestamp, session, `HEAD`, changed paths) is filled in here so the
//! agent spends no tokens on it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use super::outbox;
use super::record::{
    DecisionKind, DecisionRecord, DecisionText, OutboxLine, PATHS_CAP, apply_caps,
};

/// The only harness Design Review supports today
/// ([`super::support::design_review_supported`]).
const HARNESS: &str = "claude";

/// Env var AMF sets in every agent session; attributes a record to it.
pub const SESSION_ENV: &str = "AMF_FEATURE_SESSION_ID";

/// Record a decision for the checkout containing `cwd` and return the line to
/// print, e.g. `d3 recorded` or `d3 recorded (truncated)`.
pub fn decision_add(
    cwd: &Path,
    kind: DecisionKind,
    text: DecisionText,
    session_id: Option<String>,
) -> Result<String> {
    let root = git(cwd, &["rev-parse", "--show-toplevel"])
        .map(PathBuf::from)
        .context("not inside a git checkout")?;
    let path = outbox::outbox_path(&root);
    let existing =
        outbox::decisions(&path).with_context(|| format!("cannot read {}", path.display()))?;
    let seq = existing.len() as u32 + 1;
    let head = git(
        &root,
        &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"],
    );
    let previous_head = existing.last().and_then(|record| record.head.clone());
    let paths = changed_paths(&root, previous_head.as_deref());

    let (text, truncated) = apply_caps(text);
    if text.title.is_empty() || text.why.is_empty() {
        bail!("--title and --why must not be empty");
    }
    let record = DecisionRecord {
        uid: uuid::Uuid::new_v4().to_string(),
        seq,
        kind,
        title: text.title,
        why: text.why,
        rejected: text.rejected,
        ask: text.ask,
        supersedes: text.supersedes,
        paths,
        head,
        session_id: session_id.filter(|id| !id.trim().is_empty()),
        harness: HARNESS.to_string(),
        ts: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    };
    outbox::append(&path, &OutboxLine::Decision(record))?;
    Ok(ack(seq, truncated))
}

/// The one-line acknowledgement the agent sees.
pub fn ack(seq: u32, truncated: bool) -> String {
    if truncated {
        format!("d{seq} recorded (truncated)")
    } else {
        format!("d{seq} recorded")
    }
}

/// Files changed since `since` (the previous record's `HEAD`), or, for the
/// first record or when that commit is gone, since the merge-base with the
/// default branch; plus untracked files. AMF-managed paths are dropped and the
/// list is capped at [`PATHS_CAP`]. Best-effort: a git failure yields fewer
/// paths, never an error, since the record itself is what matters.
fn changed_paths(root: &Path, since: Option<&str>) -> Vec<String> {
    let base = since
        .filter(|commit| git(root, &["cat-file", "-e", &format!("{commit}^{{commit}}")]).is_some())
        .map(str::to_string)
        .or_else(|| {
            crate::diff::resolve_base_ref(root, None)
                .ok()
                .map(|resolved| resolved.base_commit)
        });
    let mut paths = BTreeSet::new();
    if let Some(base) = base
        && let Some(out) = git(root, &["diff", "--name-only", &base, "--"])
    {
        paths.extend(out.lines().map(str::to_string));
    }
    if let Some(out) = git(root, &["ls-files", "--others", "--exclude-standard"]) {
        paths.extend(out.lines().map(str::to_string));
    }
    paths
        .into_iter()
        .filter(|path| !path.is_empty() && !crate::amf_managed::is_amf_managed_path(path))
        .take(PATHS_CAP)
        .collect()
}

/// Run git in `dir`; trimmed stdout on success, `None` on any failure. Empty
/// output from a successful command is `Some("")`.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn run(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(status.status.success(), "git {args:?} failed");
    }

    fn repo() -> TempDir {
        let dir = TempDir::new().unwrap();
        run(dir.path(), &["init", "-q", "-b", "main"]);
        std::fs::write(dir.path().join("README.md"), "hi\n").unwrap();
        run(dir.path(), &["add", "."]);
        run(dir.path(), &["commit", "-q", "-m", "init"]);
        run(dir.path(), &["checkout", "-q", "-b", "feature"]);
        dir
    }

    fn text(title: &str) -> DecisionText {
        DecisionText {
            title: title.into(),
            why: "because".into(),
            ..DecisionText::default()
        }
    }

    #[test]
    fn records_are_numbered_and_carry_paths_since_the_previous_record() {
        let dir = repo();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), "a").unwrap();
        // AMF-managed paths never count as changes.
        std::fs::create_dir_all(root.join(".claude")).unwrap();
        std::fs::write(root.join(".claude/settings.local.json"), "{}").unwrap();

        let ack = decision_add(
            root,
            DecisionKind::Architecture,
            text("first"),
            Some("sess-1".into()),
        )
        .unwrap();
        assert_eq!(ack, "d1 recorded");

        run(root, &["add", "src/a.rs"]);
        run(root, &["commit", "-q", "-m", "a"]);
        std::fs::write(root.join("src/b.rs"), "b").unwrap();
        let ack = decision_add(
            root,
            DecisionKind::Risk,
            DecisionText {
                title: "t".repeat(200),
                ..text("")
            },
            None,
        )
        .unwrap();
        assert_eq!(ack, "d2 recorded (truncated)");

        let records = outbox::decisions(&outbox::outbox_path(root)).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].seq, 1);
        assert_eq!(records[0].paths, vec!["src/a.rs".to_string()]);
        assert_eq!(records[0].session_id.as_deref(), Some("sess-1"));
        assert_eq!(records[0].harness, "claude");
        assert!(records[0].head.is_some());
        assert_eq!(records[1].seq, 2);
        // Since d1's HEAD: the commit of a.rs is newer than d1's head, and
        // b.rs is untracked.
        assert_eq!(
            records[1].paths,
            vec!["src/a.rs".to_string(), "src/b.rs".to_string()]
        );
        assert_ne!(records[0].uid, records[1].uid);
    }

    #[test]
    fn paths_are_capped() {
        let dir = repo();
        for i in 0..(PATHS_CAP + 5) {
            std::fs::write(dir.path().join(format!("f{i:03}.txt")), "x").unwrap();
        }
        decision_add(dir.path(), DecisionKind::Data, text("many"), None).unwrap();
        let records = outbox::decisions(&outbox::outbox_path(dir.path())).unwrap();
        assert_eq!(records[0].paths.len(), PATHS_CAP);
    }

    #[test]
    fn outside_a_checkout_is_an_error() {
        let dir = TempDir::new().unwrap();
        assert!(decision_add(dir.path(), DecisionKind::Data, text("x"), None).is_err());
    }

    #[test]
    fn empty_title_is_an_error() {
        let dir = repo();
        assert!(decision_add(dir.path(), DecisionKind::Data, text("  "), None).is_err());
        assert!(!outbox::outbox_path(dir.path()).exists());
    }
}
