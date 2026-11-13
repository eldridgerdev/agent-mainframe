//! The Decision Log outbox: `<checkout>/.amf/design-review/outbox.jsonl`.
//!
//! The agent's CLI appends one JSON line per record and exits; AMF reads new
//! complete lines from a stored byte offset. The agent never reads this file,
//! and nothing here opens `amf.db`.

use std::fs::OpenOptions;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::record::{DecisionRecord, OutboxLine};

/// The outbox, relative to the checkout root.
pub const OUTBOX_RELATIVE_PATH: &str = ".amf/design-review/outbox.jsonl";

pub fn outbox_path(checkout_root: &Path) -> PathBuf {
    checkout_root.join(OUTBOX_RELATIVE_PATH)
}

/// Append one line, creating the directory as needed. The line is written with
/// a single `write` on an `O_APPEND` handle, so concurrent appenders never
/// interleave within a line. The directory carries its own `*` `.gitignore`
/// so the outbox stays out of commits even where `.amf/` is not ignored.
pub fn append(path: &Path, line: &OutboxLine) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
        let ignore = dir.join(".gitignore");
        if !ignore.exists() {
            let _ = std::fs::write(&ignore, "*\n");
        }
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

/// Read every complete line after `offset`. Returns the parsed lines, the
/// offset to resume from, and how many lines were malformed (skipped, but
/// consumed). A trailing line without `\n` is still being written and is left
/// for next time. A file shorter than `offset` was truncated or replaced, so
/// reading restarts at 0; ingest is idempotent by uid. A missing file reads as
/// empty.
pub fn read_from(path: &Path, offset: u64) -> io::Result<(Vec<OutboxLine>, u64, usize)> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok((Vec::new(), 0, 0)),
        Err(err) => return Err(err),
    };
    let len = file.metadata()?.len();
    let start = if len < offset { 0 } else { offset };
    file.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;

    let Some(last_newline) = buf.iter().rposition(|b| *b == b'\n') else {
        return Ok((Vec::new(), start, 0));
    };
    let complete = &buf[..=last_newline];
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
    Ok((lines, start + complete.len() as u64, malformed))
}

/// Every decision already in the outbox, in file order.
pub fn decisions(path: &Path) -> io::Result<Vec<DecisionRecord>> {
    let (lines, _, _) = read_from(path, 0)?;
    Ok(lines
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
            head: None,
            session_id: None,
            harness: "claude".into(),
            ts: "2026-11-13T00:00:00Z".into(),
        })
    }

    #[test]
    fn append_creates_dirs_and_reads_back_from_offsets() {
        let dir = TempDir::new().unwrap();
        let path = outbox_path(dir.path());
        append(&path, &decision("a", 1)).unwrap();
        assert!(path.parent().unwrap().join(".gitignore").exists());

        let (lines, offset, bad) = read_from(&path, 0).unwrap();
        assert_eq!(lines, vec![decision("a", 1)]);
        assert_eq!(bad, 0);
        assert_eq!(offset, std::fs::metadata(&path).unwrap().len());

        append(&path, &decision("b", 2)).unwrap();
        let (lines, next, _) = read_from(&path, offset).unwrap();
        assert_eq!(lines, vec![decision("b", 2)]);
        assert!(next > offset);
        let (lines, same, _) = read_from(&path, next).unwrap();
        assert!(lines.is_empty());
        assert_eq!(same, next);
    }

    #[test]
    fn missing_file_reads_as_empty() {
        let dir = TempDir::new().unwrap();
        assert_eq!(
            read_from(&outbox_path(dir.path()), 42).unwrap(),
            (Vec::new(), 0, 0)
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

        let (lines, offset, bad) = read_from(&path, 0).unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(bad, 0);
        assert_eq!(offset, whole, "the partial line is not consumed");

        file.write_all(format!("{tail}\n").as_bytes()).unwrap();
        let (lines, _, bad) = read_from(&path, offset).unwrap();
        assert_eq!(lines, vec![decision("b", 2)]);
        assert_eq!(bad, 0);
    }

    #[test]
    fn a_truncated_file_restarts_at_zero() {
        let dir = TempDir::new().unwrap();
        let path = outbox_path(dir.path());
        append(&path, &decision("a", 1)).unwrap();
        append(&path, &decision("b", 2)).unwrap();
        let (_, offset, _) = read_from(&path, 0).unwrap();

        std::fs::remove_file(&path).unwrap();
        append(&path, &decision("c", 1)).unwrap();
        let (lines, next, _) = read_from(&path, offset).unwrap();
        assert_eq!(lines, vec![decision("c", 1)]);
        assert_eq!(next, std::fs::metadata(&path).unwrap().len());
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

        let (lines, offset, bad) = read_from(&path, 0).unwrap();
        assert_eq!(lines, vec![decision("a", 1), decision("b", 2)]);
        assert_eq!(bad, 2);
        assert_eq!(offset, std::fs::metadata(&path).unwrap().len());
    }
}
