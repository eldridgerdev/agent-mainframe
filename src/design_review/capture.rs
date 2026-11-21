//! `amf decision add`: record one design decision.
//!
//! Runs in the agent's process, outside the TUI. It never opens `amf.db` and
//! needs no running AMF: it appends one line to the checkout's outbox and
//! prints a one-line acknowledgement. Everything mechanical (sequence number,
//! uid, timestamp, session, `HEAD`, changed paths) is filled in here so the
//! agent spends no tokens on it.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use super::brief::{RequestKind, parse_label};
use super::outbox;
use super::record::{
    DIRTY_CAP, DecisionKind, DecisionRecord, DecisionText, OutboxLine, PATHS_CAP, REPLY_CAP,
    ReplyKind, ReplyRecord, apply_caps, truncate_chars,
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
    let (paths, dirty) = changed_paths(&root, existing.last());

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
        dirty,
        head,
        session_id: session_id.filter(|id| !id.trim().is_empty()),
        harness: HARNESS.to_string(),
        ts: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    };
    outbox::append(&path, &OutboxLine::Decision(record))?;
    Ok(ack(seq, truncated))
}

/// Record the developer's reply to one brief item for the checkout containing
/// `cwd` and return the line to print, e.g. `C1 reply recorded` or
/// `C1 reply recorded (truncated)`. Like [`decision_add`] it only appends to
/// the outbox. The round is not recorded: ingest matches the newest round
/// that has the label.
pub fn reply_add(
    cwd: &Path,
    label: &str,
    kind: ReplyKind,
    text: &str,
    session_id: Option<String>,
) -> Result<String> {
    let Some((label, label_kind)) = parse_label(label) else {
        bail!("unknown label {label:?}: expected C<n> or Q<n>, as in the brief");
    };
    match (label_kind, kind) {
        (RequestKind::Change, ReplyKind::Done | ReplyKind::Pushback)
        | (RequestKind::Question, ReplyKind::Answer) => {}
        (RequestKind::Change, ReplyKind::Answer) => {
            bail!("{label} is a change: use --done or --pushback")
        }
        (RequestKind::Question, _) => bail!("{label} is a question: use --answer"),
    }
    let root = git(cwd, &["rev-parse", "--show-toplevel"])
        .map(PathBuf::from)
        .context("not inside a git checkout")?;
    let (text, truncated) = truncate_chars(text, REPLY_CAP);
    if text.is_empty() {
        bail!("the reply must not be empty");
    }
    let record = ReplyRecord {
        uid: uuid::Uuid::new_v4().to_string(),
        label: label.clone(),
        kind,
        text,
        session_id: session_id.filter(|id| !id.trim().is_empty()),
        ts: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    };
    outbox::append(&outbox::outbox_path(&root), &OutboxLine::Reply(record))?;
    Ok(if truncated {
        format!("{label} reply recorded (truncated)")
    } else {
        format!("{label} reply recorded")
    })
}

/// The one-line acknowledgement the agent sees.
pub fn ack(seq: u32, truncated: bool) -> String {
    if truncated {
        format!("d{seq} recorded (truncated)")
    } else {
        format!("d{seq} recorded")
    }
}

/// Marker for a tracked file that no longer exists in the working tree.
const DELETED: &str = "deleted";

/// The paths a new record should list, and the dirty snapshot to store on it.
///
/// First record (or the previous `HEAD` is gone): everything changed since the
/// merge-base with the default branch, plus untracked files. Later records
/// list only what is new since the previous one: files changed by commits in
/// `prev_head..HEAD` (unless they were dirty then with the very content now
/// committed) and dirty files whose content differs from the previous
/// record's snapshot. AMF-managed paths are dropped and the list is capped at
/// [`PATHS_CAP`]. Best-effort: a git failure yields fewer paths, never an
/// error, since the record itself is what matters. Git runs a fixed number of
/// times (plus a per-file fallback only when batched hashing fails), however
/// many files changed.
fn changed_paths(
    root: &Path,
    previous: Option<&DecisionRecord>,
) -> (Vec<String>, BTreeMap<String, String>) {
    let dirty = dirty_snapshot(root);
    let since = previous
        .and_then(|record| record.head.as_deref())
        .filter(|commit| git(root, &["cat-file", "-e", &format!("{commit}^{{commit}}")]).is_some());
    let mut paths = BTreeSet::new();
    match (previous, since) {
        (Some(previous), Some(since)) => {
            let committed = git_paths(root, &["diff", "--name-only", "-z", since, "HEAD", "--"]);
            let blobs = head_blobs(root, &committed);
            for path in committed {
                let blob = blobs.get(&path).map_or(DELETED, String::as_str);
                if previous.dirty.get(&path).map(String::as_str) != Some(blob) {
                    paths.insert(path);
                }
            }
            for (path, blob) in &dirty {
                if previous.dirty.get(path) != Some(blob) {
                    paths.insert(path.clone());
                }
            }
        }
        _ => {
            let base = crate::diff::resolve_base_ref(root, None)
                .ok()
                .map(|resolved| resolved.base_commit);
            if let Some(base) = base {
                paths.extend(git_paths(root, &["diff", "--name-only", "-z", &base, "--"]));
            }
            paths.extend(dirty.keys().cloned());
        }
    }
    let paths = paths
        .into_iter()
        .filter(|path| !crate::amf_managed::is_amf_managed_path(path))
        .take(PATHS_CAP)
        .collect();
    (paths, dirty)
}

/// Blob ids at `HEAD` for `paths`, from one `git ls-tree`. A path missing from
/// the result is not in `HEAD` (deleted there).
fn head_blobs(root: &Path, paths: &[String]) -> BTreeMap<String, String> {
    if paths.is_empty() {
        return BTreeMap::new();
    }
    let mut args = vec![
        "--literal-pathspecs",
        "ls-tree",
        "-r",
        "-z",
        "--full-tree",
        "HEAD",
        "--",
    ];
    args.extend(paths.iter().map(String::as_str));
    let Some(out) = git_raw(root, &args, None) else {
        return BTreeMap::new();
    };
    String::from_utf8_lossy(&out)
        .split('\0')
        .filter_map(|entry| {
            // `<mode> SP <type> SP <object> TAB <path>`
            let (meta, path) = entry.split_once('\t')?;
            let mut fields = meta.split(' ');
            let (_mode, kind, object) = (fields.next()?, fields.next()?, fields.next()?);
            (kind == "blob").then(|| (path.to_string(), object.to_string()))
        })
        .collect()
}

/// Uncommitted files (tracked changes against `HEAD` plus untracked), each
/// with its `git hash-object` blob id or [`DELETED`]. AMF-managed paths are
/// excluded and the map is capped at [`DIRTY_CAP`].
fn dirty_snapshot(root: &Path) -> BTreeMap<String, String> {
    let mut names = BTreeSet::new();
    for args in [
        &["diff", "--name-only", "-z", "HEAD", "--"][..],
        &["ls-files", "-z", "--others", "--exclude-standard"][..],
    ] {
        names.extend(git_paths(root, args));
    }
    let names: Vec<String> = names
        .into_iter()
        .filter(|path| !crate::amf_managed::is_amf_managed_path(path))
        .take(DIRTY_CAP)
        .collect();
    // `--stdin-paths` is newline-separated, so a path containing a newline
    // cannot be batched; it is recorded as deleted.
    let present: Vec<&str> = names
        .iter()
        .filter(|path| !path.contains('\n') && root.join(path).is_file())
        .map(String::as_str)
        .collect();
    let blobs = hash_objects(root, &present);
    names
        .into_iter()
        .map(|path| {
            let blob = blobs
                .get(&path)
                .cloned()
                .unwrap_or_else(|| DELETED.to_string());
            (path, blob)
        })
        .collect()
}

/// `git hash-object` for every path in one process. If the batch fails (a
/// file vanished between listing and hashing aborts the whole batch), hash
/// each path on its own instead.
fn hash_objects(root: &Path, paths: &[&str]) -> BTreeMap<String, String> {
    if paths.is_empty() {
        return BTreeMap::new();
    }
    let mut input = paths.join("\n");
    input.push('\n');
    let batched = git_raw(
        root,
        &["hash-object", "--stdin-paths"],
        Some(input.as_bytes()),
    )
    .map(|out| {
        String::from_utf8_lossy(&out)
            .lines()
            .map(str::to_string)
            .collect::<Vec<_>>()
    })
    .filter(|blobs| blobs.len() == paths.len());
    match batched {
        Some(blobs) => paths
            .iter()
            .map(|path| path.to_string())
            .zip(blobs)
            .collect(),
        None => paths
            .iter()
            .filter_map(|path| {
                let blob = git(root, &["hash-object", "--", path])?;
                Some((path.to_string(), blob))
            })
            .collect(),
    }
}

/// Run a git command whose output is a `-z` (NUL-separated) path list. Paths
/// come back unquoted, so non-ASCII names survive. Empty on failure.
fn git_paths(dir: &Path, args: &[&str]) -> Vec<String> {
    git_raw(dir, args, None)
        .map(|out| {
            String::from_utf8_lossy(&out)
                .split('\0')
                .filter(|path| !path.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Run git in `dir`; trimmed stdout on success, `None` on any failure. Empty
/// output from a successful command is `Some("")`.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    git_raw(dir, args, None).map(|out| String::from_utf8_lossy(&out).trim().to_string())
}

/// Run git in `dir`, feeding `stdin` when given; raw stdout on success.
fn git_raw(dir: &Path, args: &[&str], stdin: Option<&[u8]>) -> Option<Vec<u8>> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().ok()?;
    if let Some(input) = stdin {
        // Write from a thread so a large input cannot deadlock against a
        // full stdout pipe.
        let mut pipe = child.stdin.take()?;
        let input = input.to_vec();
        let writer = std::thread::spawn(move || pipe.write_all(&input));
        let output = child.wait_with_output().ok()?;
        writer.join().ok()?.ok()?;
        return output.status.success().then_some(output.stdout);
    }
    let output = child.wait_with_output().ok()?;
    output.status.success().then_some(output.stdout)
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
        // a.rs was already attributed to d1 and was committed unchanged;
        // only the untracked b.rs is new.
        assert_eq!(records[1].paths, vec!["src/b.rs".to_string()]);
        assert_ne!(records[0].uid, records[1].uid);
    }

    fn paths_of(root: &Path) -> Vec<Vec<String>> {
        outbox::decisions(&outbox::outbox_path(root))
            .unwrap()
            .into_iter()
            .map(|record| record.paths)
            .collect()
    }

    fn add(root: &Path) {
        decision_add(root, DecisionKind::Data, text("d"), None).unwrap();
    }

    #[test]
    fn an_unchanged_untracked_file_is_attributed_only_once() {
        let dir = repo();
        let root = dir.path();
        std::fs::write(root.join("new.rs"), "one").unwrap();
        add(root);
        add(root);
        std::fs::write(root.join("new.rs"), "two").unwrap();
        add(root);
        let paths = paths_of(root);
        assert_eq!(paths[0], vec!["new.rs".to_string()]);
        assert!(paths[1].is_empty(), "{:?}", paths[1]);
        assert_eq!(paths[2], vec!["new.rs".to_string()]);
    }

    #[test]
    fn a_dirty_file_committed_unchanged_is_not_reattributed() {
        let dir = repo();
        let root = dir.path();
        std::fs::write(root.join("a.rs"), "a").unwrap();
        add(root);
        run(root, &["add", "a.rs"]);
        run(root, &["commit", "-q", "-m", "a"]);
        add(root);
        assert!(paths_of(root)[1].is_empty());
    }

    #[test]
    fn a_newly_committed_or_edited_file_is_attributed() {
        let dir = repo();
        let root = dir.path();
        std::fs::write(root.join("a.rs"), "a").unwrap();
        add(root);
        // a.rs is edited before its commit, so the committed content differs.
        std::fs::write(root.join("a.rs"), "a2").unwrap();
        std::fs::write(root.join("b.rs"), "b").unwrap();
        run(root, &["add", "b.rs"]);
        run(root, &["commit", "-q", "-m", "b"]);
        add(root);
        let paths = paths_of(root);
        assert_eq!(paths[1], vec!["a.rs".to_string(), "b.rs".to_string()]);
    }

    #[test]
    fn dirty_snapshot_is_stored_capped_and_old_lines_still_parse() {
        let dir = repo();
        let root = dir.path();
        for i in 0..(DIRTY_CAP + 5) {
            std::fs::write(root.join(format!("f{i:03}.txt")), "x").unwrap();
        }
        add(root);
        let records = outbox::decisions(&outbox::outbox_path(root)).unwrap();
        assert_eq!(records[0].dirty.len(), DIRTY_CAP);
        let old = r#"{"type":"decision","uid":"u","seq":1,"kind":"data","title":"t","why":"w","harness":"claude","ts":"x"}"#;
        assert!(serde_json::from_str::<OutboxLine>(old).is_ok());
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
    fn non_ascii_and_glob_like_paths_keep_their_names() {
        let dir = repo();
        let root = dir.path();
        std::fs::write(root.join("é.rs"), "one").unwrap();
        std::fs::write(root.join("a*b.rs"), "one").unwrap();
        std::fs::write(root.join("axb.rs"), "one").unwrap();
        add(root);
        run(root, &["add", "."]);
        run(root, &["commit", "-q", "-m", "files"]);
        // Committed unchanged: not reattributed, which needs the HEAD blob of
        // each name, including the glob-like one, to resolve exactly.
        add(root);
        std::fs::write(root.join("é.rs"), "two").unwrap();
        add(root);
        let records = outbox::decisions(&outbox::outbox_path(root)).unwrap();
        assert_eq!(
            records[0].paths,
            vec![
                "a*b.rs".to_string(),
                "axb.rs".to_string(),
                "é.rs".to_string()
            ]
        );
        assert!(records[0].dirty.values().all(|blob| blob != DELETED));
        assert!(records[1].paths.is_empty(), "{:?}", records[1].paths);
        assert_eq!(records[2].paths, vec!["é.rs".to_string()]);
    }

    #[test]
    fn hashing_falls_back_per_file_when_a_batched_path_is_missing() {
        let dir = repo();
        let root = dir.path();
        std::fs::write(root.join("a.rs"), "a").unwrap();
        let blobs = hash_objects(root, &["a.rs", "gone.rs"]);
        assert_eq!(blobs.len(), 1);
        assert_eq!(
            blobs.get("a.rs"),
            git(root, &["hash-object", "--", "a.rs"]).as_ref()
        );
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

    fn replies(root: &Path) -> Vec<ReplyRecord> {
        outbox::read_from(&outbox::outbox_path(root), &outbox::OutboxCursor::default())
            .unwrap()
            .lines
            .into_iter()
            .filter_map(|line| match line {
                OutboxLine::Reply(reply) => Some(reply),
                OutboxLine::Decision(_) => None,
            })
            .collect()
    }

    #[test]
    fn reply_appends_a_line_and_acknowledges_in_one_line() {
        let dir = repo();
        let root = dir.path();
        let ack = reply_add(
            root,
            "c1",
            ReplyKind::Done,
            " timer added ",
            Some("s1".into()),
        )
        .unwrap();
        assert_eq!(ack, "C1 reply recorded");
        let ack = reply_add(root, "Q2", ReplyKind::Answer, "because", None).unwrap();
        assert_eq!(ack, "Q2 reply recorded");
        let got = replies(root);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].label, "C1");
        assert_eq!(got[0].kind, ReplyKind::Done);
        assert_eq!(got[0].text, "timer added");
        assert_eq!(got[0].session_id.as_deref(), Some("s1"));
        assert_eq!(got[1].kind, ReplyKind::Answer);
    }

    #[test]
    fn long_replies_are_truncated_not_rejected() {
        let dir = repo();
        let ack = reply_add(
            dir.path(),
            "C3",
            ReplyKind::Pushback,
            &"w".repeat(REPLY_CAP + 50),
            None,
        )
        .unwrap();
        assert_eq!(ack, "C3 reply recorded (truncated)");
        assert_eq!(replies(dir.path())[0].text.chars().count(), REPLY_CAP);
    }

    #[test]
    fn bad_labels_and_mismatched_kinds_are_errors_that_write_nothing() {
        let dir = repo();
        let root = dir.path();
        for label in ["d4", "C", "X1", "C0", ""] {
            let err = reply_add(root, label, ReplyKind::Done, "x", None).unwrap_err();
            assert!(!format!("{err:#}").contains('\n'));
            assert!(format!("{err:#}").contains("C<n> or Q<n>"), "{err:#}");
        }
        assert!(reply_add(root, "C1", ReplyKind::Answer, "x", None).is_err());
        assert!(reply_add(root, "Q1", ReplyKind::Done, "x", None).is_err());
        assert!(reply_add(root, "C1", ReplyKind::Done, "  ", None).is_err());
        assert!(!outbox::outbox_path(root).exists());
    }

    #[test]
    fn replies_do_not_count_toward_decision_numbers() {
        let dir = repo();
        let root = dir.path();
        decision_add(root, DecisionKind::Data, text("one"), None).unwrap();
        reply_add(root, "C1", ReplyKind::Done, "x", None).unwrap();
        let ack = decision_add(root, DecisionKind::Data, text("two"), None).unwrap();
        assert_eq!(ack, "d2 recorded");
    }
}
