//! Deterministic, origin-bound evidence capture for Expert Assist.
//!
//! This module deliberately performs no model calls. It turns explicitly
//! selected files into a bounded packet and keeps omitted byte ranges
//! addressable for a later, user-approved evidence request.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::ConsultationOrigin;

const MAX_PACKET_BYTES: usize = 48 * 1024;
const MAX_EXCERPT_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    Source,
    Diff,
    Failure,
    SessionExcerpt,
    UserNote,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvidenceRange {
    pub start: usize,
    pub end: usize,
}

impl EvidenceRange {
    fn validate(&self, len: usize) -> Result<()> {
        anyhow::ensure!(
            self.start <= self.end && self.end <= len,
            "invalid evidence range"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvidenceSelection {
    pub id: String,
    pub kind: EvidenceKind,
    pub relative_path: PathBuf,
    pub ranges: Vec<EvidenceRange>,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvidenceItem {
    pub id: String,
    pub kind: EvidenceKind,
    pub relative_path: PathBuf,
    pub source_hash: String,
    pub excerpt_hash: String,
    pub original_bytes: usize,
    pub included_bytes: usize,
    pub included_ranges: Vec<EvidenceRange>,
    pub omitted_ranges: Vec<EvidenceRange>,
    pub omission_reason: Option<String>,
    pub retrievable: bool,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceFingerprint {
    pub relative_path: PathBuf,
    pub kind: String,
    pub content_hash: Option<String>,
    pub mode: u32,
    pub symlink_target: Option<PathBuf>,
    pub absent: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceSnapshot {
    pub id: String,
    pub repository_identity: String,
    pub canonical_workdir: PathBuf,
    pub files: Vec<SourceFingerprint>,
    pub scope_hash: String,
    pub stable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvidencePacket {
    pub origin: EvidenceOrigin,
    pub snapshot: SourceSnapshot,
    pub items: Vec<EvidenceItem>,
    pub omitted_bytes: usize,
    pub packet_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvidenceOrigin {
    pub project_id: String,
    pub feature_id: String,
    pub session_id: String,
    pub launch_generation: String,
}

impl From<&ConsultationOrigin> for EvidenceOrigin {
    fn from(origin: &ConsultationOrigin) -> Self {
        Self {
            project_id: origin.project_id.clone(),
            feature_id: origin.feature_id.clone(),
            session_id: origin.session_id.clone(),
            launch_generation: origin.launch_generation.clone(),
        }
    }
}

impl EvidenceOrigin {
    pub fn matches(&self, origin: &ConsultationOrigin) -> bool {
        self == &Self::from(origin)
    }
}

#[derive(Debug, Clone)]
pub struct EvidenceCollector {
    root: PathBuf,
    packet_limit: usize,
    excerpt_limit: usize,
}

impl EvidenceCollector {
    pub fn new(origin: &ConsultationOrigin) -> Result<Self> {
        let root = fs::canonicalize(&origin.workdir)
            .with_context(|| format!("could not resolve {}", origin.workdir.display()))?;
        anyhow::ensure!(root.is_dir(), "evidence workdir is not a directory");
        Ok(Self {
            root,
            packet_limit: MAX_PACKET_BYTES,
            excerpt_limit: MAX_EXCERPT_BYTES,
        })
    }

    pub fn with_limits(mut self, packet_limit: usize, excerpt_limit: usize) -> Result<Self> {
        anyhow::ensure!(
            packet_limit > 0 && excerpt_limit > 0,
            "evidence limits must be positive"
        );
        self.packet_limit = packet_limit;
        self.excerpt_limit = excerpt_limit;
        Ok(self)
    }

    /// Capture exactly the selected paths. Every path is resolved beneath the
    /// origin workdir, and a symlink escaping it is rejected.
    pub fn capture(
        &self,
        origin: &ConsultationOrigin,
        selections: &[EvidenceSelection],
    ) -> Result<EvidencePacket> {
        anyhow::ensure!(
            self.root == fs::canonicalize(&origin.workdir)?,
            "collector origin changed"
        );
        let mut ids = std::collections::BTreeSet::new();
        let mut items = Vec::new();
        let mut fingerprints = Vec::new();
        let mut packet_bytes = 0;
        let mut omitted_bytes = 0;
        let mut ordered = selections.to_vec();
        // Required context is allocated first, so optional excerpts cannot
        // consume the packet budget and force silent loss of mandatory data.
        ordered.sort_by_key(|selection| !selection.required);
        for selection in &ordered {
            anyhow::ensure!(
                !selection.id.trim().is_empty() && ids.insert(&selection.id),
                "evidence IDs must be unique"
            );
            let path = self.safe_path(&selection.relative_path)?;
            let metadata = fs::symlink_metadata(&path).with_context(|| {
                format!(
                    "could not read evidence {}",
                    selection.relative_path.display()
                )
            })?;
            let resolved = fs::canonicalize(&path)?;
            anyhow::ensure!(
                resolved.starts_with(&self.root),
                "evidence symlink escapes origin"
            );
            anyhow::ensure!(metadata.is_file(), "evidence source must be a regular file");
            let bytes = fs::read(&resolved)?;
            let source_hash = hash(&bytes);
            let mode = metadata.mode();
            fingerprints.push(SourceFingerprint {
                relative_path: selection.relative_path.clone(),
                kind: "file".into(),
                content_hash: Some(source_hash.clone()),
                mode,
                symlink_target: metadata
                    .file_type()
                    .is_symlink()
                    .then(|| fs::read_link(&path).ok())
                    .flatten(),
                absent: false,
            });
            let mut ranges = normalize_ranges(&selection.ranges, bytes.len())?;
            if ranges.is_empty() {
                ranges.push(EvidenceRange {
                    start: 0,
                    end: bytes.len(),
                });
            }
            let requested: usize = ranges.iter().map(|range| range.end - range.start).sum();
            let available = self.packet_limit.saturating_sub(packet_bytes);
            let take = requested.min(self.excerpt_limit).min(available);
            let included_ranges = trim_ranges(&ranges, take);
            let included_bytes: usize = included_ranges
                .iter()
                .map(|range| range.end - range.start)
                .sum();
            let text = included_ranges
                .iter()
                .flat_map(|range| &bytes[range.start..range.end])
                .copied()
                .collect::<Vec<_>>();
            let omitted_ranges = subtract_ranges(&ranges, &included_ranges);
            let omitted = omitted_ranges
                .iter()
                .map(|range| range.end - range.start)
                .sum::<usize>();
            anyhow::ensure!(
                !selection.required || omitted == 0,
                "required evidence exceeds packet limit"
            );
            packet_bytes += included_bytes;
            omitted_bytes += omitted;
            items.push(EvidenceItem {
                id: selection.id.clone(),
                kind: selection.kind.clone(),
                relative_path: selection.relative_path.clone(),
                source_hash: source_hash.clone(),
                excerpt_hash: hash(&text),
                original_bytes: bytes.len(),
                included_bytes,
                included_ranges,
                omitted_ranges: omitted_ranges.clone(),
                omission_reason: (omitted > 0).then(|| "packet_limit".into()),
                retrievable: omitted > 0,
                text: String::from_utf8_lossy(&text).into_owned(),
            });
            // A file changing while it is being captured makes the packet
            // freshness unknown; callers must collect a new packet.
            let after = fs::read(&resolved)?;
            anyhow::ensure!(
                hash(&after) == source_hash,
                "evidence source changed during capture"
            );
        }
        let scope_hash = hash(&serde_json::to_vec(&fingerprints)?);
        let snapshot = SourceSnapshot {
            id: hash(format!("{}:{}", now_nanos(), scope_hash).as_bytes()),
            repository_identity: origin.repository_identity.clone(),
            canonical_workdir: self.root.clone(),
            files: fingerprints,
            scope_hash,
            stable: true,
        };
        Ok(EvidencePacket {
            origin: origin.into(),
            snapshot,
            items,
            omitted_bytes,
            packet_bytes,
        })
    }

    /// Retrieve only the omitted range named by its stable evidence ID.
    pub fn retrieve_omission(
        &self,
        origin: &ConsultationOrigin,
        packet: &EvidencePacket,
        id: &str,
        range: EvidenceRange,
    ) -> Result<EvidenceItem> {
        anyhow::ensure!(
            packet.origin.matches(origin),
            "evidence origin does not match consultation"
        );
        let item = packet
            .items
            .iter()
            .find(|item| item.id == id)
            .context("unknown evidence ID")?;
        anyhow::ensure!(
            item.retrievable && item.omitted_ranges.contains(&range),
            "evidence range is not an omitted retrievable range"
        );
        let path = self.safe_path(&item.relative_path)?;
        let bytes = fs::read(fs::canonicalize(path)?)?;
        anyhow::ensure!(
            hash(&bytes) == item.source_hash,
            "evidence source changed; refresh required"
        );
        range.validate(bytes.len())?;
        let end = range.end.min(range.start + self.excerpt_limit);
        let text = bytes[range.start..end].to_vec();
        Ok(EvidenceItem {
            id: format!("{}:{}-{}", id, range.start, end),
            kind: item.kind.clone(),
            relative_path: item.relative_path.clone(),
            source_hash: item.source_hash.clone(),
            excerpt_hash: hash(&text),
            original_bytes: bytes.len(),
            included_bytes: text.len(),
            included_ranges: vec![EvidenceRange {
                start: range.start,
                end,
            }],
            omitted_ranges: vec![],
            omission_reason: None,
            retrievable: false,
            text: String::from_utf8_lossy(&text).into_owned(),
        })
    }

    fn safe_path(&self, relative: &Path) -> Result<PathBuf> {
        anyhow::ensure!(!relative.is_absolute(), "evidence path must be relative");
        anyhow::ensure!(
            !relative
                .components()
                .any(|component| matches!(component, Component::ParentDir)),
            "evidence path traversal is not allowed"
        );
        let path = self.root.join(relative);
        anyhow::ensure!(path.starts_with(&self.root), "evidence path escapes origin");
        Ok(path)
    }
}

fn normalize_ranges(ranges: &[EvidenceRange], len: usize) -> Result<Vec<EvidenceRange>> {
    let mut sorted = ranges.to_vec();
    for range in &sorted {
        range.validate(len)?;
    }
    sorted.sort_by_key(|range| range.start);
    let mut merged: Vec<EvidenceRange> = Vec::new();
    for range in sorted {
        if let Some(last) = merged.last_mut()
            && range.start <= last.end
        {
            last.end = last.end.max(range.end);
            continue;
        }
        merged.push(range);
    }
    Ok(merged)
}

fn trim_ranges(ranges: &[EvidenceRange], budget: usize) -> Vec<EvidenceRange> {
    let mut remaining = budget;
    ranges
        .iter()
        .filter_map(|range| {
            if remaining == 0 {
                return None;
            }
            let length = (range.end - range.start).min(remaining);
            remaining -= length;
            Some(EvidenceRange {
                start: range.start,
                end: range.start + length,
            })
        })
        .collect()
}

fn subtract_ranges(all: &[EvidenceRange], included: &[EvidenceRange]) -> Vec<EvidenceRange> {
    let mut result = Vec::new();
    for source in all {
        let mut cursor = source.start;
        for kept in included
            .iter()
            .filter(|range| range.end > source.start && range.start < source.end)
        {
            if kept.start > cursor {
                result.push(EvidenceRange {
                    start: cursor,
                    end: kept.start.min(source.end),
                });
            }
            cursor = cursor.max(kept.end);
        }
        if cursor < source.end {
            result.push(EvidenceRange {
                start: cursor,
                end: source.end,
            });
        }
    }
    result
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

#[cfg(test)]
mod tests;
