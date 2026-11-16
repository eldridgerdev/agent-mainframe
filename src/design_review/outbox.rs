//! The Decision Log outbox: `<checkout>/.amf/design-review/outbox.jsonl`.
//!
//! The agent's CLI appends one JSON line per record and exits; AMF reads new
//! complete lines from a stored byte offset. The agent never reads this file,
//! and nothing here opens `amf.db`.

use std::fs::OpenOptions;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use super::record::{DecisionRecord, OutboxLine};

/// The outbox, relative to the checkout root.
pub const OUTBOX_RELATIVE_PATH: &str = ".amf/design-review/outbox.jsonl";

/// The id of the feature the checkout's outbox belongs to, beside the outbox.
pub const OWNER_RELATIVE_PATH: &str = ".amf/design-review/owner";

/// How many bytes before the stored offset [`OutboxCursor::tail_sha256`]
/// covers. Several lines' worth, so it includes at least one uid in practice.
pub const FINGERPRINT_WINDOW: u64 = 4096;

pub fn outbox_path(checkout_root: &Path) -> PathBuf {
    checkout_root.join(OUTBOX_RELATIVE_PATH)
}

fn owner_path(checkout_root: &Path) -> PathBuf {
    checkout_root.join(OWNER_RELATIVE_PATH)
}

/// Create the outbox directory with its own `*` `.gitignore`, so the outbox
/// stays out of commits even where `.amf/` is not ignored.
fn ensure_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let ignore = dir.join(".gitignore");
    if !ignore.exists() {
        let _ = std::fs::write(&ignore, "*\n");
    }
    Ok(())
}

/// The feature the checkout's outbox belongs to, if any feature bound it.
pub fn owner(checkout_root: &Path) -> Option<String> {
    let owner = std::fs::read_to_string(owner_path(checkout_root)).ok()?;
    let owner = owner.trim();
    (!owner.is_empty()).then(|| owner.to_string())
}

/// Make `feature_id` the owner of the checkout's outbox. The outbox is per
/// checkout but decisions belong to one feature, so when another feature (or
/// nobody) owned it, the old outbox is removed first: the new feature then
/// never ingests an earlier feature's decisions, and the CLI numbers from
/// `d1` again. Returns whether the outbox was reset. On error the owner is
/// left unchanged, so ingest keeps skipping a foreign outbox.
pub fn bind(checkout_root: &Path, feature_id: &str) -> Result<bool> {
    if owner(checkout_root).as_deref() == Some(feature_id) {
        return Ok(false);
    }
    let path = owner_path(checkout_root);
    if let Some(dir) = path.parent() {
        ensure_dir(dir)?;
    }
    let outbox = outbox_path(checkout_root);
    match std::fs::remove_file(&outbox) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err).with_context(|| format!("cannot remove {}", outbox.display())),
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, format!("{feature_id}\n"))
        .and_then(|()| std::fs::rename(&tmp, &path))
        .with_context(|| format!("cannot write {}", path.display()))?;
    Ok(true)
}

/// Append one line, creating the directory as needed. The line is written with
/// a single `write` on an `O_APPEND` handle, so concurrent appenders never
/// interleave within a line.
pub fn append(path: &Path, line: &OutboxLine) -> Result<()> {
    if let Some(dir) = path.parent() {
        ensure_dir(dir)?;
    }
    let mut bytes = serde_json::to_vec(line)?;
    bytes.push(b'\n');
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    file.write_all(&bytes)
        .with_context(|| format!("cannot write {}", path.display()))?;
    Ok(())
}

/// Where ingest stopped: a byte offset plus the SHA-256 (hex) of up to
/// [`FINGERPRINT_WINDOW`] bytes before it, so a replaced outbox is noticed
/// even when it is already longer than the offset.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutboxCursor {
    pub offset: u64,
    pub tail_sha256: Option<String>,
}

/// The result of [`read_from`].
#[derive(Debug, Clone, PartialEq)]
pub struct OutboxRead {
    pub lines: Vec<OutboxLine>,
    /// Where to resume next time.
    pub cursor: OutboxCursor,
    /// Lines that did not parse (skipped, but consumed).
    pub malformed: usize,
}

/// Read every complete line after `cursor`. A trailing line without `\n` is
/// still being written and is left for next time. When the bytes before the
/// offset no longer match the cursor's fingerprint (or the file is shorter
/// than the offset), the file was truncated or replaced, so reading restarts
/// at 0; ingest is idempotent by uid. A missing file reads as empty.
pub fn read_from(path: &Path, cursor: &OutboxCursor) -> io::Result<OutboxRead> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Ok(OutboxRead {
                lines: Vec::new(),
                cursor: OutboxCursor::default(),
                malformed: 0,
            });
        }
        Err(err) => return Err(err),
    };
    let len = file.metadata()?.len();
    let resumable = cursor.offset > 0
        && cursor.offset <= len
        && cursor.tail_sha256.is_some()
        && fingerprint(&mut file, cursor.offset)? == cursor.tail_sha256;
    let start = if resumable { cursor.offset } else { 0 };
    file.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;

    let complete = match buf.iter().rposition(|b| *b == b'\n') {
        Some(last_newline) => &buf[..=last_newline],
        None => &buf[..0],
    };
    let mut lines = Vec::new();
    let mut malformed = 0;
    for raw in complete.split(|b| *b == b'\n') {
        if raw.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match serde_json::from_slice::<OutboxLine>(raw) {
            Ok(line) => lines.push(line),
            Err(_) => malformed += 1,
        }
    }
    let offset = start + complete.len() as u64;
    let tail_sha256 = fingerprint(&mut file, offset)?;
    Ok(OutboxRead {
        lines,
        cursor: OutboxCursor {
            offset,
            tail_sha256,
        },
        malformed,
    })
}

/// SHA-256 of up to [`FINGERPRINT_WINDOW`] bytes before `offset`; `None` at
/// offset 0 or when the file is shorter than `offset`.
fn fingerprint(file: &mut std::fs::File, offset: u64) -> io::Result<Option<String>> {
    if offset == 0 {
        return Ok(None);
    }
    let window = offset.min(FINGERPRINT_WINDOW);
    file.seek(SeekFrom::Start(offset - window))?;
    let mut bytes = vec![0; window as usize];
    match file.read_exact(&mut bytes) {
        Ok(()) => Ok(Some(format!("{:x}", Sha256::digest(&bytes)))),
        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => Ok(None),
        Err(err) => Err(err),
    }
}

/// Every decision already in the outbox, in file order.
pub fn decisions(path: &Path) -> io::Result<Vec<DecisionRecord>> {
    Ok(read_from(path, &OutboxCursor::default())?
        .lines
        .into_iter()
        .map(|line| match line {
            OutboxLine::Decision(record) => record,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design_review::record::DecisionKind;
    use tempfile::TempDir;

    fn decision(uid: &str, seq: u32) -> OutboxLine {
        OutboxLine::Decision(DecisionRecord {
            uid: uid.into(),
            seq,
            kind: DecisionKind::Architecture,
            title: format!("title {seq}"),
            why: "why".into(),
            rejected: vec![],
            ask: None,
            supersedes: None,
            paths: vec![],
            dirty: Default::default(),
            head: None,
            session_id: None,
            harness: "claude".into(),
            ts: "2026-11-13T00:00:00Z".into(),
        })
    }

    fn read(path: &Path, cursor: &OutboxCursor) -> (Vec<OutboxLine>, OutboxCursor, usize) {
        let read = read_from(path, cursor).unwrap();
        (read.lines, read.cursor, read.malformed)
    }

    fn start() -> OutboxCursor {
        OutboxCursor::default()
    }

    #[test]
    fn append_creates_dirs_and_reads_back_from_offsets() {
        let dir = TempDir::new().unwrap();
        let path = outbox_path(dir.path());
        append(&path, &decision("a", 1)).unwrap();
        assert!(path.parent().unwrap().join(".gitignore").exists());

        let (lines, cursor, bad) = read(&path, &start());
        assert_eq!(lines, vec![decision("a", 1)]);
        assert_eq!(bad, 0);
        assert_eq!(cursor.offset, std::fs::metadata(&path).unwrap().len());
        assert!(cursor.tail_sha256.is_some());

        append(&path, &decision("b", 2)).unwrap();
        let (lines, next, _) = read(&path, &cursor);
        assert_eq!(lines, vec![decision("b", 2)]);
        assert!(next.offset > cursor.offset);
        let (lines, same, _) = read(&path, &next);
        assert!(lines.is_empty());
        assert_eq!(same, next);
    }

    #[test]
    fn missing_file_reads_as_empty() {
        let dir = TempDir::new().unwrap();
        let cursor = OutboxCursor {
            offset: 42,
            tail_sha256: Some("x".into()),
        };
        assert_eq!(
            read(&outbox_path(dir.path()), &cursor),
            (Vec::new(), start(), 0)
        );
    }

    #[test]
    fn a_partial_trailing_line_is_left_for_next_time() {
        let dir = TempDir::new().unwrap();
        let path = outbox_path(dir.path());
        append(&path, &decision("a", 1)).unwrap();
        let whole = std::fs::metadata(&path).unwrap().len();
        let full = serde_json::to_string(&decision("b", 2)).unwrap();
        let (head, tail) = full.split_at(10);
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(head.as_bytes()).unwrap();

        let (lines, cursor, bad) = read(&path, &start());
        assert_eq!(lines.len(), 1);
        assert_eq!(bad, 0);
        assert_eq!(cursor.offset, whole, "the partial line is not consumed");

        file.write_all(format!("{tail}\n").as_bytes()).unwrap();
        let (lines, _, bad) = read(&path, &cursor);
        assert_eq!(lines, vec![decision("b", 2)]);
        assert_eq!(bad, 0);
    }

    #[test]
    fn a_truncated_file_restarts_at_zero() {
        let dir = TempDir::new().unwrap();
        let path = outbox_path(dir.path());
        append(&path, &decision("a", 1)).unwrap();
        append(&path, &decision("b", 2)).unwrap();
        let (_, cursor, _) = read(&path, &start());

        std::fs::remove_file(&path).unwrap();
        append(&path, &decision("c", 1)).unwrap();
        let (lines, next, _) = read(&path, &cursor);
        assert_eq!(lines, vec![decision("c", 1)]);
        assert_eq!(next.offset, std::fs::metadata(&path).unwrap().len());
    }

    #[test]
    fn a_replaced_file_longer_than_the_offset_restarts_at_zero() {
        let dir = TempDir::new().unwrap();
        let path = outbox_path(dir.path());
        append(&path, &decision("a", 1)).unwrap();
        let (_, cursor, _) = read(&path, &start());

        std::fs::remove_file(&path).unwrap();
        append(&path, &decision("long-replacement-uid-c", 1)).unwrap();
        append(&path, &decision("d", 2)).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > cursor.offset);
        let (lines, _, bad) = read(&path, &cursor);
        assert_eq!(
            lines,
            vec![decision("long-replacement-uid-c", 1), decision("d", 2)]
        );
        assert_eq!(bad, 0, "no line is cut at the stale offset");
    }

    #[test]
    fn a_cursor_without_a_fingerprint_rereads_from_zero() {
        let dir = TempDir::new().unwrap();
        let path = outbox_path(dir.path());
        append(&path, &decision("a", 1)).unwrap();
        append(&path, &decision("b", 2)).unwrap();
        let (_, cursor, _) = read(&path, &start());
        let legacy = OutboxCursor {
            offset: cursor.offset,
            tail_sha256: None,
        };
        assert_eq!(read(&path, &legacy).0.len(), 2);
    }

    #[test]
    fn binding_a_new_feature_resets_the_outbox_and_rebinding_keeps_it() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let path = outbox_path(root);
        append(&path, &decision("stale", 1)).unwrap();
        assert_eq!(owner(root), None);

        assert!(bind(root, "feat-a").unwrap());
        assert_eq!(owner(root).as_deref(), Some("feat-a"));
        assert!(!path.exists(), "an unowned outbox is not inherited");

        append(&path, &decision("a", 1)).unwrap();
        assert!(!bind(root, "feat-a").unwrap());
        assert_eq!(decisions(&path).unwrap().len(), 1);

        assert!(bind(root, "feat-b").unwrap());
        assert_eq!(owner(root).as_deref(), Some("feat-b"));
        assert!(decisions(&path).unwrap().is_empty());
    }

    #[test]
    fn malformed_lines_are_counted_and_skipped() {
        let dir = TempDir::new().unwrap();
        let path = outbox_path(dir.path());
        append(&path, &decision("a", 1)).unwrap();
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"not json\n{\"type\":\"unknown\"}\n\n")
            .unwrap();
        append(&path, &decision("b", 2)).unwrap();

        let (lines, cursor, bad) = read(&path, &start());
        assert_eq!(lines, vec![decision("a", 1), decision("b", 2)]);
        assert_eq!(bad, 2);
        assert_eq!(cursor.offset, std::fs::metadata(&path).unwrap().len());
    }
}
