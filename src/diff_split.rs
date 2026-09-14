//! Text-level splitting of a unified diff into per-file sections and each
//! section into hunk groups, preserving every byte so the pieces reassemble
//! into the exact original.
//!
//! This is deliberately separate from [`crate::diff`]'s semantic parser: the
//! batched-review orchestrator packs *diff text* into bounded prompts and, for
//! an oversized file, splits that file *by hunk*. Both operations must be
//! lossless — a reviewed prompt has to contain the same diff the harness would
//! have seen unsplit — so this module keeps the raw section/hunk strings
//! rather than a structured line model.

// `allow(dead_code)`: consumed by `crate::review_batch` and the review call
// sites, but a few helpers on the public types (e.g. `byte_len`) have no
// caller yet and are kept for the module's completeness.
#![allow(dead_code)]

use crate::headless::estimate_prompt_tokens;

/// A unified diff broken into per-file sections. Reassembling
/// [`SplitDiff::preamble`] followed by every [`FileSection::reassemble`] yields
/// the input byte-for-byte.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SplitDiff {
    /// Any text before the first `diff --git ` line — normally empty, but a
    /// `git format-patch` payload or a stray note would land here. Preserved
    /// only so the round trip is exact.
    pub preamble: String,
    /// File sections in source order.
    pub files: Vec<FileSection>,
}

/// One file's slice of the diff: its `diff --git` line plus every extended
/// header line (mode changes, `index`, `--- `/`+++ `, `Binary files …`) and
/// then its hunk groups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSection {
    /// Best-effort path for batch labelling and per-file finding grouping.
    /// Taken from `+++ b/…`, then `--- a/…`, then the `diff --git` header.
    pub path: String,
    /// The `diff --git` line through the last line before the first `@@`
    /// hunk header, verbatim (each line keeps its original terminator). For a
    /// pure rename, mode change, or binary file this is the whole section and
    /// [`FileSection::hunks`] is empty.
    pub header: String,
    /// Hunk groups in order. The `\ No newline at end of file` marker and any
    /// other trailing text stay attached to the hunk they follow.
    pub hunks: Vec<HunkGroup>,
}

/// A single `@@ … @@` hunk: its header line and the body lines up to the next
/// hunk header, the next file, or end of input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HunkGroup {
    /// The `@@ -a,b +c,d @@ …` line, including its trailing newline.
    pub header: String,
    /// Every line after the header until the next `@@`/`diff --git`/EOF,
    /// verbatim. May be empty.
    pub body: String,
}

impl HunkGroup {
    /// The hunk's exact original text (`header` then `body`).
    pub fn reassemble(&self) -> String {
        let mut out = String::with_capacity(self.header.len() + self.body.len());
        out.push_str(&self.header);
        out.push_str(&self.body);
        out
    }

    /// Byte length of the hunk's rendered text — used by the orchestrator to
    /// decide whether a file must be split hunk-by-hunk.
    pub fn byte_len(&self) -> usize {
        self.header.len() + self.body.len()
    }
}

impl FileSection {
    /// The file's exact original text (`header` then every hunk).
    pub fn reassemble(&self) -> String {
        let mut out = String::with_capacity(self.byte_len());
        out.push_str(&self.header);
        for hunk in &self.hunks {
            out.push_str(&hunk.header);
            out.push_str(&hunk.body);
        }
        out
    }

    /// Byte length of the file's rendered diff text.
    pub fn byte_len(&self) -> usize {
        self.header.len() + self.hunks.iter().map(HunkGroup::byte_len).sum::<usize>()
    }
}

impl SplitDiff {
    /// Split `patch` into sections. Infallible: any line that does not fit the
    /// unified-diff shape is kept in place (in the preamble or the current
    /// file/hunk) so [`SplitDiff::reassemble`] always round-trips.
    pub fn parse(patch: &str) -> SplitDiff {
        let mut split = SplitDiff::default();
        let mut current: Option<FileSection> = None;
        // Once a file section has produced a hunk, later non-`@@` lines belong
        // to that hunk's body rather than the section header.
        let mut in_hunks = false;

        for line in patch.split_inclusive('\n') {
            if line.starts_with("diff --git ") {
                if let Some(file) = current.take() {
                    split.files.push(file);
                }
                current = Some(FileSection {
                    path: path_from_git_header(line),
                    header: line.to_string(),
                    hunks: Vec::new(),
                });
                in_hunks = false;
                continue;
            }

            let Some(file) = current.as_mut() else {
                split.preamble.push_str(line);
                continue;
            };

            if is_hunk_header(line) {
                file.hunks.push(HunkGroup {
                    header: line.to_string(),
                    body: String::new(),
                });
                in_hunks = true;
                continue;
            }

            if in_hunks {
                // Safe to unwrap: `in_hunks` is only set right after a push.
                file.hunks
                    .last_mut()
                    .expect("in_hunks implies a hunk exists")
                    .body
                    .push_str(line);
            } else {
                refine_path_from_header_line(file, line);
                file.header.push_str(line);
            }
        }

        if let Some(file) = current.take() {
            split.files.push(file);
        }

        split
    }

    /// Reassemble the exact original diff text.
    pub fn reassemble(&self) -> String {
        let mut out = String::with_capacity(self.byte_len());
        out.push_str(&self.preamble);
        for file in &self.files {
            out.push_str(&file.reassemble());
        }
        out
    }

    /// Total byte length of the reassembled diff.
    pub fn byte_len(&self) -> usize {
        self.preamble.len() + self.files.iter().map(FileSection::byte_len).sum::<usize>()
    }
}

/// A unified-diff hunk header is `@@ ` at column zero. Context lines are always
/// prefixed with a space in a `git diff`, so a source line beginning `@@` never
/// reaches column zero in the body. Combined (merge) diffs use `@@@` and are
/// out of scope for review, but they still match here and split cleanly.
fn is_hunk_header(line: &str) -> bool {
    line.starts_with("@@ ") || line.starts_with("@@@ ")
}

/// Path from a `diff --git a/<old> b/<new>` line. Falls back to the raw
/// remainder if the `a/`…` b/` shape is not present (e.g. quoted paths with
/// spaces, which we do not try to unquote here).
fn path_from_git_header(line: &str) -> String {
    let rest = line
        .trim_end_matches(['\r', '\n'])
        .strip_prefix("diff --git ")
        .unwrap_or("")
        .trim();
    if let Some((old, new)) = rest.split_once(" b/") {
        let new = new.trim();
        if !new.is_empty() {
            return new.to_string();
        }
        return old.strip_prefix("a/").unwrap_or(old).to_string();
    }
    rest.to_string()
}

/// Prefer the `+++ b/…` path, then `--- a/…`, over whatever the `diff --git`
/// line gave us. `/dev/null` (add/delete) is ignored so the real side wins.
fn refine_path_from_header_line(file: &mut FileSection, line: &str) {
    let trimmed = line.trim_end_matches(['\r', '\n']);
    if let Some(rest) = trimmed.strip_prefix("+++ ") {
        if let Some(path) = strip_diff_path(rest) {
            file.path = path;
        }
    } else if let Some(rest) = trimmed.strip_prefix("--- ") {
        // Only take the `---` side if we have not already learned a `+++`
        // path; parse order guarantees `---` is seen first, so this just
        // means "don't clobber a deletion's real path with /dev/null".
        if let Some(path) = strip_diff_path(rest) {
            file.path = path;
        }
    }
}

/// `a/src/foo.rs` / `b/src/foo.rs` → `src/foo.rs`; `/dev/null` → `None`.
/// A trailing tab-delimited timestamp (non-git unified diffs) is dropped.
fn strip_diff_path(raw: &str) -> Option<String> {
    let raw = raw.split('\t').next().unwrap_or(raw).trim();
    if raw == "/dev/null" || raw.is_empty() {
        return None;
    }
    let path = raw
        .strip_prefix("a/")
        .or_else(|| raw.strip_prefix("b/"))
        .unwrap_or(raw);
    Some(path.to_string())
}

// ---------------------------------------------------------------------------
// Batch packing
//
// Greedily group whole file sections into review units that each stay under
// the token budget. A file whose own diff exceeds the budget can't be packed
// with anything — it is set aside as `OversizedFile` for `split_file_by_hunk`.
// Packing is order-preserving and lossless:
// concatenating every batch's sections, in batch order, reproduces the input
// file list exactly, so no diff is ever silently dropped.
// ---------------------------------------------------------------------------

/// One unit of batched-review work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewBatch {
    /// One or more complete file sections that together fit the budget. The
    /// common case; a lone small file is also `Files(vec![section])`.
    Files(Vec<FileSection>),
    /// A single file whose section alone exceeds the budget. The orchestrator
    /// emits one sub-prompt per hunk and merges the sub-findings; a file with
    /// no hunks (huge binary blob, giant rename) still lands here and is
    /// reported as un-splittable.
    OversizedFile(FileSection),
}

impl ReviewBatch {
    /// The batch's diff text — a valid unified diff on its own, since every
    /// [`FileSection::reassemble`] starts at a `diff --git` line.
    pub fn diff_text(&self) -> String {
        match self {
            ReviewBatch::Files(files) => files.iter().map(FileSection::reassemble).collect(),
            ReviewBatch::OversizedFile(file) => file.reassemble(),
        }
    }

    /// Estimated prompt-token cost of [`ReviewBatch::diff_text`] alone (no
    /// instruction-template overhead).
    pub fn token_estimate(&self) -> usize {
        estimate_prompt_tokens(&self.diff_text())
    }

    /// Paths covered by this batch, in order — for progress reporting and
    /// per-file finding attribution.
    pub fn paths(&self) -> Vec<&str> {
        match self {
            ReviewBatch::Files(files) => files.iter().map(|f| f.path.as_str()).collect(),
            ReviewBatch::OversizedFile(file) => vec![file.path.as_str()],
        }
    }
}

/// Greedily pack `files` (in order) into [`ReviewBatch`]es whose diff text
/// stays within `budget_tokens`.
///
/// - First-fit by source order: a file is added to the open batch while the
///   running estimate stays `<= budget_tokens`, otherwise the batch is
///   flushed and the file opens a new one.
/// - A file whose own estimate exceeds `budget_tokens` flushes the open batch
///   and becomes an [`ReviewBatch::OversizedFile`] of its own.
/// - `budget_tokens == 0` means "no limit": every file goes into a single
///   [`ReviewBatch::Files`] and nothing is flagged oversized. (Matches
///   `headless::will_overflow_with_budget`, where 0 disables the gate.)
pub fn pack_file_sections(files: Vec<FileSection>, budget_tokens: usize) -> Vec<ReviewBatch> {
    if files.is_empty() {
        return Vec::new();
    }
    if budget_tokens == 0 {
        return vec![ReviewBatch::Files(files)];
    }

    let mut batches = Vec::new();
    let mut open: Vec<FileSection> = Vec::new();
    let mut open_tokens = 0usize;

    for file in files {
        let file_tokens = estimate_prompt_tokens(&file.reassemble());

        if file_tokens > budget_tokens {
            if !open.is_empty() {
                batches.push(ReviewBatch::Files(std::mem::take(&mut open)));
                open_tokens = 0;
            }
            batches.push(ReviewBatch::OversizedFile(file));
            continue;
        }

        if !open.is_empty() && open_tokens + file_tokens > budget_tokens {
            batches.push(ReviewBatch::Files(std::mem::take(&mut open)));
            open_tokens = 0;
        }

        open_tokens += file_tokens;
        open.push(file);
    }

    if !open.is_empty() {
        batches.push(ReviewBatch::Files(open));
    }

    batches
}

// ---------------------------------------------------------------------------
// Hunk-level splitting
//
// A file flagged `OversizedFile` by `pack_file_sections` is divided into
// sub-diffs of one or more consecutive hunks, each `file header + hunks` and
// small enough to review on its own. A single hunk that still exceeds the
// budget cannot be divided further: it is emitted as an `oversized` subunit
// so the orchestrator reviews everything else and records that slice as "not
// reviewed" — coverage stays honest, nothing is dropped.
//
// Splitting is lossless at the hunk level: concatenating every subunit's
// hunks, in order, reproduces the file's hunk list, and every subunit carries
// the file header verbatim so a sub-prompt never loses file context.
// ---------------------------------------------------------------------------

/// An over-budget file divided into reviewable per-hunk sub-diffs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HunkSplit {
    pub path: String,
    /// Sub-diffs in file order.
    pub subunits: Vec<HunkSubunit>,
}

impl HunkSplit {
    /// Any slice that could not be made to fit even as a single hunk.
    pub fn has_unreviewable(&self) -> bool {
        self.subunits.iter().any(|s| s.oversized)
    }
}

/// One slice of a hunk-split file: the file header plus a run of consecutive
/// hunks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HunkSubunit {
    /// `path` + the verbatim file header + this slice's hunks;
    /// `section.reassemble()` is a valid unified diff.
    pub section: FileSection,
    /// Inclusive 0-based hunk indices within the parent file. `None` when the
    /// file had no hunks at all (a binary/rename blob flagged oversized by its
    /// header alone).
    pub hunk_span: Option<(usize, usize)>,
    /// A single hunk (or a headerless blob) still over budget — un-splittable.
    /// The orchestrator records it as not reviewed rather than dropping it.
    pub oversized: bool,
}

impl HunkSubunit {
    /// The sub-diff text for this slice.
    pub fn diff_text(&self) -> String {
        self.section.reassemble()
    }

    /// Human label for progress and per-file finding attribution, e.g.
    /// `"hunk 3"`, `"hunks 4–6"`, or `"file header"`.
    pub fn label(&self) -> String {
        match self.hunk_span {
            Some((a, b)) if a == b => format!("hunk {}", a + 1),
            Some((a, b)) => format!("hunks {}\u{2013}{}", a + 1, b + 1),
            None => "file header".to_string(),
        }
    }
}

/// Split an over-budget `file` into per-hunk sub-diffs that each stay within
/// `budget_tokens` (file header included, since every subunit repeats it).
///
/// - Consecutive hunks are greedily grouped; a group is flushed before it
///   would exceed the budget.
/// - A hunk that does not fit even alongside just the header is emitted as its
///   own `oversized` subunit (un-splittable).
/// - A file with no hunks yields one subunit of the header alone, `oversized`
///   iff the header itself is over budget.
/// - `budget_tokens == 0` (gate disabled) yields a single subunit of the
///   whole file.
pub fn split_file_by_hunk(file: &FileSection, budget_tokens: usize) -> HunkSplit {
    let make_section = |hunks: Vec<HunkGroup>| FileSection {
        path: file.path.clone(),
        header: file.header.clone(),
        hunks,
    };

    if file.hunks.is_empty() {
        let oversized = budget_tokens != 0 && estimate_prompt_tokens(&file.header) > budget_tokens;
        return HunkSplit {
            path: file.path.clone(),
            subunits: vec![HunkSubunit {
                section: make_section(Vec::new()),
                hunk_span: None,
                oversized,
            }],
        };
    }

    if budget_tokens == 0 {
        return HunkSplit {
            path: file.path.clone(),
            subunits: vec![HunkSubunit {
                section: make_section(file.hunks.clone()),
                hunk_span: Some((0, file.hunks.len() - 1)),
                oversized: false,
            }],
        };
    }

    let header_tokens = estimate_prompt_tokens(&file.header);
    let mut subunits = Vec::new();
    let mut group: Vec<HunkGroup> = Vec::new();
    let mut group_start = 0usize;
    let mut group_tokens = header_tokens;

    for (idx, hunk) in file.hunks.iter().enumerate() {
        let hunk_tokens = estimate_prompt_tokens(&hunk.reassemble());

        // Cannot fit even with just the header in front of it.
        if header_tokens + hunk_tokens > budget_tokens {
            if !group.is_empty() {
                subunits.push(HunkSubunit {
                    section: make_section(std::mem::take(&mut group)),
                    hunk_span: Some((group_start, idx - 1)),
                    oversized: false,
                });
            }
            subunits.push(HunkSubunit {
                section: make_section(vec![hunk.clone()]),
                hunk_span: Some((idx, idx)),
                oversized: true,
            });
            group_start = idx + 1;
            group_tokens = header_tokens;
            continue;
        }

        if !group.is_empty() && group_tokens + hunk_tokens > budget_tokens {
            subunits.push(HunkSubunit {
                section: make_section(std::mem::take(&mut group)),
                hunk_span: Some((group_start, idx - 1)),
                oversized: false,
            });
            group_start = idx;
            group_tokens = header_tokens;
        }

        group_tokens += hunk_tokens;
        group.push(hunk.clone());
    }

    if !group.is_empty() {
        subunits.push(HunkSubunit {
            section: make_section(group),
            hunk_span: Some((group_start, file.hunks.len() - 1)),
            oversized: false,
        });
    }

    HunkSplit {
        path: file.path.clone(),
        subunits,
    }
}

/// The outcome of reviewing one [`HunkSubunit`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HunkOutcome {
    /// The sub-run's answer text.
    Reviewed(String),
    /// The slice was not reviewed; the string says why (over budget even as a
    /// lone hunk, or the sub-run failed).
    NotReviewed(String),
}

/// One reviewed-or-not slice of a hunk-split file, ready for [`merge_hunk_findings`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HunkFindingPart {
    pub label: String,
    pub outcome: HunkOutcome,
}

/// Deterministically combine a hunk-split file's per-slice results into one
/// per-file block — no model call. This is the guaranteed per-file grouping
/// and the concatenation fallback behind the LLM synthesis pass (a later
/// task). Un-reviewed slices are listed explicitly with their reason so
/// partial coverage is never silent.
pub fn merge_hunk_findings(path: &str, parts: &[HunkFindingPart]) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    let _ = writeln!(out, "## {path}");
    if parts.len() > 1 {
        let _ = writeln!(out, "_reviewed in {} hunk groups_", parts.len());
    }
    for part in parts {
        let _ = writeln!(out);
        match &part.outcome {
            HunkOutcome::Reviewed(text) => {
                let _ = writeln!(out, "### {}", part.label);
                let trimmed = text.trim();
                let _ = writeln!(
                    out,
                    "{}",
                    if trimmed.is_empty() {
                        "_(no findings)_"
                    } else {
                        trimmed
                    }
                );
            }
            HunkOutcome::NotReviewed(reason) => {
                let _ = writeln!(out, "### {} \u{2014} NOT REVIEWED: {reason}", part.label);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const MULTI_FILE: &str = "\
diff --git a/src/alpha.rs b/src/alpha.rs
index 1111111..2222222 100644
--- a/src/alpha.rs
+++ b/src/alpha.rs
@@ -1,3 +1,4 @@
 fn alpha() {
-    old();
+    fresh();
+    extra();
 }
@@ -20,2 +21,2 @@ mod later
-    was;
+    now;
diff --git a/src/beta.rs b/src/beta.rs
new file mode 100644
index 0000000..3333333
--- /dev/null
+++ b/src/beta.rs
@@ -0,0 +1,2 @@
+fn beta() {}
+// tail
";

    const RENAME_HEAVY: &str = "\
diff --git a/old/name.rs b/new/name.rs
similarity index 100%
rename from old/name.rs
rename to new/name.rs
diff --git a/mod/perms.sh b/mod/perms.sh
old mode 100644
new mode 100755
diff --git a/assets/logo.png b/assets/logo.png
index 4444444..5555555 100644
Binary files a/assets/logo.png and b/assets/logo.png differ
diff --git a/src/moved.rs b/src/relocated.rs
similarity index 87%
rename from src/moved.rs
rename to src/relocated.rs
index 6666666..7777777 100644
--- a/src/moved.rs
+++ b/src/relocated.rs
@@ -1,2 +1,2 @@
-const NAME: &str = \"moved\";
+const NAME: &str = \"relocated\";
";

    #[test]
    fn reassembles_multi_file_diff_byte_for_byte() {
        let split = SplitDiff::parse(MULTI_FILE);
        assert_eq!(split.reassemble(), MULTI_FILE);
        assert_eq!(split.preamble, "");
        assert_eq!(split.files.len(), 2);
        assert_eq!(split.files[0].path, "src/alpha.rs");
        assert_eq!(split.files[0].hunks.len(), 2);
        assert_eq!(split.files[1].path, "src/beta.rs");
        assert_eq!(split.files[1].hunks.len(), 1);
    }

    #[test]
    fn reassembles_rename_and_mode_heavy_diff_byte_for_byte() {
        let split = SplitDiff::parse(RENAME_HEAVY);
        assert_eq!(split.reassemble(), RENAME_HEAVY);
        assert_eq!(split.files.len(), 4);

        // Pure rename: header only, no hunks.
        assert_eq!(split.files[0].path, "new/name.rs");
        assert!(split.files[0].hunks.is_empty());

        // Mode change: header only.
        assert!(split.files[1].hunks.is_empty());
        assert_eq!(split.files[1].path, "mod/perms.sh");

        // Binary: header only, path from the git line.
        assert!(split.files[2].hunks.is_empty());
        assert_eq!(split.files[2].path, "assets/logo.png");

        // Rename with edits: path follows the `+++` side, one hunk.
        assert_eq!(split.files[3].path, "src/relocated.rs");
        assert_eq!(split.files[3].hunks.len(), 1);
    }

    #[test]
    fn per_file_and_per_hunk_pieces_reassemble_to_their_section() {
        let split = SplitDiff::parse(MULTI_FILE);
        let alpha = &split.files[0];

        let mut from_hunks = alpha.header.clone();
        for hunk in &alpha.hunks {
            from_hunks.push_str(&hunk.reassemble());
        }
        assert_eq!(from_hunks, alpha.reassemble());

        // Concatenating every file section (no preamble here) is the original.
        let joined: String = split.files.iter().map(FileSection::reassemble).collect();
        assert_eq!(joined, MULTI_FILE);
    }

    #[test]
    fn hunk_header_context_suffix_is_preserved() {
        let split = SplitDiff::parse(MULTI_FILE);
        assert_eq!(
            split.files[0].hunks[1].header,
            "@@ -20,2 +21,2 @@ mod later\n"
        );
    }

    #[test]
    fn preserves_missing_trailing_newline() {
        let no_newline = "\
diff --git a/a.txt b/a.txt
index 1111111..2222222 100644
--- a/a.txt
+++ b/a.txt
@@ -1 +1 @@
-a
+b
\\ No newline at end of file";
        let split = SplitDiff::parse(no_newline);
        assert_eq!(split.reassemble(), no_newline);
        assert_eq!(split.files.len(), 1);
        assert_eq!(split.files[0].hunks.len(), 1);
        assert!(
            split.files[0].hunks[0]
                .body
                .ends_with("\\ No newline at end of file")
        );
    }

    #[test]
    fn preserves_crlf_line_endings() {
        let crlf = "diff --git a/a.txt b/a.txt\r\nindex 1..2 100644\r\n--- a/a.txt\r\n+++ b/a.txt\r\n@@ -1 +1 @@\r\n-a\r\n+b\r\n";
        let split = SplitDiff::parse(crlf);
        assert_eq!(split.reassemble(), crlf);
        assert_eq!(split.files[0].path, "a.txt");
    }

    #[test]
    fn keeps_leading_preamble_with_no_file_sections() {
        let preamble_only = "From 0000 Mon Sep 17 00:00:00 2001\nSubject: [PATCH] x\n\n";
        let split = SplitDiff::parse(preamble_only);
        assert_eq!(split.reassemble(), preamble_only);
        assert!(split.files.is_empty());
        assert_eq!(split.preamble, preamble_only);
    }

    #[test]
    fn empty_input_round_trips() {
        let split = SplitDiff::parse("");
        assert_eq!(split.reassemble(), "");
        assert!(split.files.is_empty());
        assert_eq!(split.byte_len(), 0);
    }

    #[test]
    fn byte_len_matches_reassembled_length() {
        for sample in [MULTI_FILE, RENAME_HEAVY] {
            let split = SplitDiff::parse(sample);
            assert_eq!(split.byte_len(), split.reassemble().len());
        }
    }

    // --- batch packing ---------------------------------------------------

    /// A file section whose reassembled text is roughly `body_bytes` long, so
    /// tests can dial a section above or below a token budget.
    fn sized_section(path: &str, body_bytes: usize) -> FileSection {
        let header = format!("diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n");
        FileSection {
            path: path.to_string(),
            header,
            hunks: vec![HunkGroup {
                header: "@@ -0,0 +1 @@\n".to_string(),
                body: format!("+{}\n", "x".repeat(body_bytes.max(1))),
            }],
        }
    }

    fn concat_paths(batches: &[ReviewBatch]) -> Vec<String> {
        batches
            .iter()
            .flat_map(|b| b.paths().into_iter().map(str::to_string))
            .collect()
    }

    #[test]
    fn small_files_pack_into_a_single_batch() {
        let files = vec![
            sized_section("a.rs", 40),
            sized_section("b.rs", 40),
            sized_section("c.rs", 40),
        ];
        let batches = pack_file_sections(files, 1_000);
        assert_eq!(batches.len(), 1);
        assert!(matches!(&batches[0], ReviewBatch::Files(f) if f.len() == 3));
    }

    #[test]
    fn packing_flushes_when_the_budget_would_be_exceeded() {
        let files = vec![
            sized_section("a.rs", 120),
            sized_section("b.rs", 120),
            sized_section("c.rs", 120),
        ];
        // Each section is ~46 tokens; pick a budget that fits one but not two.
        let one = estimate_prompt_tokens(&files[0].reassemble());
        let budget = one + 5;
        let batches = pack_file_sections(files, budget);
        assert_eq!(batches.len(), 3);
        for batch in &batches {
            assert!(matches!(batch, ReviewBatch::Files(_)));
            assert!(
                batch.token_estimate() <= budget,
                "every packed batch stays within budget"
            );
        }
        assert_eq!(concat_paths(&batches), ["a.rs", "b.rs", "c.rs"]);
    }

    #[test]
    fn an_oversized_file_becomes_its_own_batch_without_disturbing_order() {
        let files = vec![
            sized_section("small_before.rs", 40),
            sized_section("huge.rs", 4_000),
            sized_section("small_after.rs", 40),
        ];
        let batches = pack_file_sections(files, 100);
        assert_eq!(batches.len(), 3);
        assert!(matches!(&batches[0], ReviewBatch::Files(f) if f[0].path == "small_before.rs"));
        assert!(matches!(&batches[1], ReviewBatch::OversizedFile(f) if f.path == "huge.rs"));
        assert!(matches!(&batches[2], ReviewBatch::Files(f) if f[0].path == "small_after.rs"));
        assert_eq!(
            concat_paths(&batches),
            ["small_before.rs", "huge.rs", "small_after.rs"]
        );
    }

    #[test]
    fn packing_is_lossless_and_order_preserving() {
        let original = vec![
            sized_section("one.rs", 60),
            sized_section("two.rs", 5_000),
            sized_section("three.rs", 60),
            sized_section("four.rs", 60),
        ];
        let expected: String = original.iter().map(FileSection::reassemble).collect();

        let batches = pack_file_sections(original, 50);
        let rebuilt: String = batches.iter().map(ReviewBatch::diff_text).collect();
        assert_eq!(rebuilt, expected, "no diff text is lost or reordered");
    }

    #[test]
    fn a_zero_budget_packs_everything_into_one_batch() {
        let files = vec![sized_section("a.rs", 10_000), sized_section("b.rs", 10_000)];
        let batches = pack_file_sections(files, 0);
        assert_eq!(batches.len(), 1);
        assert!(matches!(&batches[0], ReviewBatch::Files(f) if f.len() == 2));
    }

    #[test]
    fn packing_an_empty_file_list_yields_no_batches() {
        assert!(pack_file_sections(Vec::new(), 1_000).is_empty());
    }

    #[test]
    fn a_file_exactly_at_budget_is_not_flagged_oversized() {
        let section = sized_section("edge.rs", 400);
        let budget = estimate_prompt_tokens(&section.reassemble());
        let batches = pack_file_sections(vec![section], budget);
        assert_eq!(batches.len(), 1);
        assert!(matches!(&batches[0], ReviewBatch::Files(_)));
    }

    // --- hunk-level splitting ------------------------------------------------

    /// A file section with one hunk per entry in `hunk_body_bytes`, each hunk
    /// body roughly that many bytes.
    fn multi_hunk_section(path: &str, hunk_body_bytes: &[usize]) -> FileSection {
        let header = format!("diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n");
        let hunks = hunk_body_bytes
            .iter()
            .enumerate()
            .map(|(i, &bytes)| HunkGroup {
                header: format!("@@ -{0},1 +{0},1 @@\n", i * 10 + 1),
                body: format!("+{}\n", "x".repeat(bytes.max(1))),
            })
            .collect();
        FileSection {
            path: path.to_string(),
            header,
            hunks,
        }
    }

    /// Every subunit's hunks, concatenated in order.
    fn all_split_hunks(split: &HunkSplit) -> Vec<HunkGroup> {
        split
            .subunits
            .iter()
            .flat_map(|s| s.section.hunks.clone())
            .collect()
    }

    #[test]
    fn hunk_split_is_lossless_and_repeats_the_header() {
        let file = multi_hunk_section("big.rs", &[300, 300, 300, 300, 300]);
        let split = split_file_by_hunk(&file, 120);

        assert!(split.subunits.len() > 1, "should actually divide");
        assert_eq!(
            all_split_hunks(&split),
            file.hunks,
            "no hunk lost or reordered"
        );
        for sub in &split.subunits {
            assert_eq!(sub.section.header, file.header);
            assert_eq!(sub.section.path, "big.rs");
            assert!(sub.diff_text().starts_with("diff --git a/big.rs"));
        }
        // Spans are contiguous and cover 0..=last with no gap or overlap.
        let mut expected_next = 0usize;
        for sub in &split.subunits {
            let (a, b) = sub.hunk_span.expect("hunk file has spans");
            assert_eq!(a, expected_next);
            expected_next = b + 1;
        }
        assert_eq!(expected_next, file.hunks.len());
    }

    #[test]
    fn hunk_split_groups_stay_within_budget() {
        let file = multi_hunk_section("big.rs", &[200, 200, 200, 200, 200, 200]);
        let split = split_file_by_hunk(&file, 130);
        for sub in &split.subunits {
            if !sub.oversized {
                assert!(
                    estimate_prompt_tokens(&sub.diff_text()) <= 130,
                    "group {} within budget",
                    sub.label()
                );
            }
        }
    }

    #[test]
    fn a_hunk_over_budget_even_alone_is_marked_unreviewable_but_still_emitted() {
        // Hunk 1 is tiny, hunk 2 is enormous, hunk 3 is tiny.
        let file = multi_hunk_section("mixed.rs", &[40, 20_000, 40]);
        let split = split_file_by_hunk(&file, 100);

        assert!(split.has_unreviewable());
        assert_eq!(
            all_split_hunks(&split),
            file.hunks,
            "the giant hunk is still present"
        );
        let oversized: Vec<_> = split.subunits.iter().filter(|s| s.oversized).collect();
        assert_eq!(oversized.len(), 1);
        assert_eq!(oversized[0].hunk_span, Some((1, 1)));
        // The tiny hunks around it are reviewed normally.
        assert!(
            split
                .subunits
                .iter()
                .any(|s| s.hunk_span == Some((0, 0)) && !s.oversized)
        );
        assert!(
            split
                .subunits
                .iter()
                .any(|s| s.hunk_span == Some((2, 2)) && !s.oversized)
        );
    }

    #[test]
    fn a_hunkless_oversized_blob_yields_one_header_only_subunit() {
        let mut file = sized_section("blob.bin", 10);
        file.hunks.clear();
        file.header = format!("diff --git a/blob.bin b/blob.bin\n{}\n", "A".repeat(5_000));

        let split = split_file_by_hunk(&file, 100);
        assert_eq!(split.subunits.len(), 1);
        assert_eq!(split.subunits[0].hunk_span, None);
        assert!(split.subunits[0].oversized);
        assert!(split.subunits[0].section.hunks.is_empty());
        assert_eq!(split.subunits[0].label(), "file header");
    }

    #[test]
    fn a_zero_budget_hunk_split_keeps_the_file_whole() {
        let file = multi_hunk_section("big.rs", &[9_000, 9_000]);
        let split = split_file_by_hunk(&file, 0);
        assert_eq!(split.subunits.len(), 1);
        assert!(!split.subunits[0].oversized);
        assert_eq!(split.subunits[0].hunk_span, Some((0, 1)));
        assert_eq!(split.subunits[0].section.hunks, file.hunks);
    }

    #[test]
    fn subunit_labels_read_naturally() {
        let whole = split_file_by_hunk(&multi_hunk_section("f.rs", &[10, 10]), 0);
        assert_eq!(whole.subunits[0].label(), "hunks 1\u{2013}2");

        let each = split_file_by_hunk(&multi_hunk_section("f.rs", &[400, 400]), 130);
        assert!(each.subunits.len() >= 2);
        assert_eq!(each.subunits[0].label(), "hunk 1");
        assert!(!each.subunits[0].oversized);
    }

    #[test]
    fn merge_hunk_findings_groups_by_file_and_names_gaps() {
        let parts = vec![
            HunkFindingPart {
                label: "hunk 1".to_string(),
                outcome: HunkOutcome::Reviewed("  Looks fine.  ".to_string()),
            },
            HunkFindingPart {
                label: "hunk 2".to_string(),
                outcome: HunkOutcome::NotReviewed(
                    "exceeds the size budget even as a single hunk".to_string(),
                ),
            },
            HunkFindingPart {
                label: "hunk 3".to_string(),
                outcome: HunkOutcome::Reviewed(String::new()),
            },
        ];
        let merged = merge_hunk_findings("src/big.rs", &parts);

        assert!(merged.starts_with("## src/big.rs\n"));
        assert!(merged.contains("_reviewed in 3 hunk groups_"));
        assert!(merged.contains("### hunk 1\nLooks fine."));
        assert!(merged.contains(
            "### hunk 2 \u{2014} NOT REVIEWED: exceeds the size budget even as a single hunk"
        ));
        assert!(merged.contains("### hunk 3\n_(no findings)_"));
    }

    #[test]
    fn merge_hunk_findings_omits_the_group_count_for_a_single_part() {
        let parts = vec![HunkFindingPart {
            label: "hunks 1\u{2013}4".to_string(),
            outcome: HunkOutcome::Reviewed("All clear.".to_string()),
        }];
        let merged = merge_hunk_findings("a.rs", &parts);
        assert!(merged.starts_with("## a.rs\n"));
        assert!(!merged.contains("hunk groups"));
        assert!(merged.contains("### hunks 1\u{2013}4\nAll clear."));
    }
}
