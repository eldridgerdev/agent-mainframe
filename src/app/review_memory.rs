//! Review-findings memory doc: a version-controlled Markdown file that
//! accumulates a team's recurring code-review findings, grouped by category.
//!
//! This is the shared substrate for PR-review Epic E: the AI reviewer reads
//! it as context before each review, the lookback bootstrap seeds it from
//! history, and the "add to memory" pane key appends to it incrementally. See
//! `docs/backlog/pr-comment-review-plan.md` (Epic E).
//!
//! There are **two** such docs, distinguished by [`MemoryScope`]: each repo's
//! own committed doc (the default target for every write) and one
//! cross-project doc in the AMF config dir for habits that outlive a single
//! repo. The AI reviewer reads both, merged by [`merge_memory_context`]; every
//! other flow writes to exactly one, chosen by the user.
//!
//! AMF only ever *appends* here (dedup-aware, grouped by category) — it never
//! rewrites existing prose, so hand-edits are safe across runs. The one
//! exception is the explicit, user-triggered "compact" pass ([`compact_prompt`]),
//! which proposes a wholesale rewrite (merging near-duplicates, pruning stale
//! rules) but never writes it without the user reviewing and confirming the
//! result first — see `App::pr_review_compact_write`.

use std::path::{Path, PathBuf};

/// Default location of the review-memory doc, relative to the repo root.
pub const DEFAULT_REVIEW_MEMORY_PATH: &str = ".amf/review-memory.md";

/// Default filename of the cross-project review-memory doc, relative to the
/// AMF config dir — i.e. `~/.config/amf/review-memory.md`.
pub const DEFAULT_GLOBAL_REVIEW_MEMORY_FILE: &str = "review-memory.md";

/// Which of the two review-memory docs a flow reads or writes: the repo's own
/// committed doc, or the user's cross-project one. Writes always target
/// exactly one (the user picks); only the AI reviewer's context reads both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MemoryScope {
    /// `{repo}/.amf/review-memory.md` — committed, shared with the team.
    #[default]
    Project,
    /// `~/.config/amf/review-memory.md` — this user's, across every repo.
    Global,
}

impl MemoryScope {
    /// Lowercase word for dialog titles and toasts ("project" / "global").
    pub fn label(self) -> &'static str {
        match self {
            MemoryScope::Project => "project",
            MemoryScope::Global => "global",
        }
    }

    /// The other scope — what a `g` keypress switches a dialog to.
    pub fn toggled(self) -> Self {
        match self {
            MemoryScope::Project => MemoryScope::Global,
            MemoryScope::Global => MemoryScope::Project,
        }
    }
}

/// Both resolved review-memory doc paths for one project, so a caller that
/// needs either (or both, as the AI reviewer does) resolves them once instead
/// of repeating the config lookup per scope.
#[derive(Debug, Clone)]
pub struct ReviewMemoryPaths {
    pub project: PathBuf,
    pub global: PathBuf,
}

impl ReviewMemoryPaths {
    pub fn for_scope(&self, scope: MemoryScope) -> &Path {
        match scope {
            MemoryScope::Project => &self.project,
            MemoryScope::Global => &self.global,
        }
    }
}

const HEADER_TEMPLATE: &str = "\
# Review memory

Recurring findings from code review, grouped by category. AMF appends new
findings here (from PR triage and AI review) and reads this file as context
before each AI review, so the same issue doesn't need rediscovering every
time. Edit freely — AMF only ever appends, it never rewrites what's already
here.
";

const GLOBAL_HEADER_TEMPLATE: &str = "\
# Review memory (cross-project)

Recurring code-review findings that apply across every repo you work in, not
just one. AMF reads this file *in addition to* each repo's own
`.amf/review-memory.md` before an AI review, so put durable personal habits
here and repo-specific rules there. Edit freely — AMF only ever appends, it
never rewrites what's already here.
";

/// Resolve the review-memory doc path for `repo`, honoring a configured
/// override. A relative override is resolved against `repo`; an absolute one
/// is used as-is. Falls back to [`DEFAULT_REVIEW_MEMORY_PATH`] when `configured`
/// is `None` or blank.
pub fn review_memory_path(repo: &Path, configured: Option<&str>) -> PathBuf {
    let configured = configured.map(str::trim).filter(|s| !s.is_empty());
    let rel = Path::new(configured.unwrap_or(DEFAULT_REVIEW_MEMORY_PATH));
    if rel.is_absolute() {
        rel.to_path_buf()
    } else {
        repo.join(rel)
    }
}

/// Resolve the cross-project review-memory doc path, honoring a configured
/// override. Mirrors [`review_memory_path`] but anchors relative paths to the
/// AMF config dir (`~/.config/amf`) rather than a repo, since this doc belongs
/// to no single repo.
pub fn global_review_memory_path(configured: Option<&str>) -> PathBuf {
    global_review_memory_path_in(&crate::project::amf_config_dir(), configured)
}

/// [`global_review_memory_path`] with the config dir injected, so tests can
/// resolve against a temp dir instead of the developer's real `~/.config/amf`.
pub fn global_review_memory_path_in(config_dir: &Path, configured: Option<&str>) -> PathBuf {
    let configured = configured.map(str::trim).filter(|s| !s.is_empty());
    let rel = Path::new(configured.unwrap_or(DEFAULT_GLOBAL_REVIEW_MEMORY_FILE));
    if rel.is_absolute() {
        rel.to_path_buf()
    } else {
        config_dir.join(rel)
    }
}

/// Ensure the doc exists at `path`, creating it (and any missing parent
/// directories) with `scope`'s header template if it doesn't. No-op if it's
/// already there.
pub fn ensure_review_memory_doc(path: &Path, scope: MemoryScope) -> std::io::Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let header = match scope {
        MemoryScope::Project => HEADER_TEMPLATE,
        MemoryScope::Global => GLOBAL_HEADER_TEMPLATE,
    };
    std::fs::write(path, header)
}

/// Normalize a free-form category into a `## Heading`, e.g. `concurrency` ->
/// `## Concurrency`. Blank/whitespace-only categories fall back to `General`.
fn category_heading(category: &str) -> String {
    let trimmed = category.trim();
    let title = if trimmed.is_empty() {
        "General".to_string()
    } else {
        let mut chars = trimmed.chars();
        match chars.next() {
            Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            None => "General".to_string(),
        }
    };
    format!("## {title}")
}

/// Append `finding` under `category` in the doc at `path`, creating the doc
/// (with `scope`'s header) and/or the category section as needed. Dedup-aware:
/// if a case-insensitive, trimmed match of `finding` already appears anywhere
/// in the doc, this is a no-op. Returns whether the finding was newly appended.
///
/// Dedup is per-doc: promoting a project finding to the global doc appends it
/// there even though it also sits in the project doc. [`merge_memory_context`]
/// collapses that overlap at read time, so the reviewer never sees it twice.
pub fn append_finding(
    path: &Path,
    scope: MemoryScope,
    category: &str,
    finding: &str,
) -> std::io::Result<bool> {
    let finding = finding.trim();
    if finding.is_empty() {
        return Ok(false);
    }
    ensure_review_memory_doc(path, scope)?;
    let contents = std::fs::read_to_string(path)?;

    let already_present = contents.lines().any(|line| {
        line.trim()
            .trim_start_matches('-')
            .trim()
            .eq_ignore_ascii_case(finding)
    });
    if already_present {
        return Ok(false);
    }

    let heading = category_heading(category);
    let updated = match contents.find(&heading) {
        Some(section_start) => {
            let after_heading = section_start + heading.len();
            let insert_at = match contents[after_heading..].find("\n## ") {
                Some(offset) => after_heading + offset + 1,
                None => contents.len(),
            };
            let mut out = String::with_capacity(contents.len() + finding.len() + 8);
            out.push_str(&contents[..insert_at]);
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("- ");
            out.push_str(finding);
            out.push('\n');
            out.push_str(&contents[insert_at..]);
            out
        }
        None => {
            let mut out = contents;
            if !out.is_empty() && !out.ends_with("\n\n") {
                if !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push('\n');
            }
            out.push_str(&heading);
            out.push('\n');
            out.push_str("- ");
            out.push_str(finding);
            out.push('\n');
            out
        }
    };

    std::fs::write(path, updated)?;
    Ok(true)
}

const PROJECT_CONTEXT_LABEL: &str = "--- This repository's review memory ---";
const GLOBAL_CONTEXT_LABEL: &str = "--- Cross-project review memory (applies to every repo) ---";

/// Merge the project doc and the cross-project doc into the single context
/// block the AI reviewer is given.
///
/// When only one doc has content the result is that doc verbatim — the
/// single-doc case reads exactly as it did before the global layer existed.
/// When both do, each is introduced by a plain-text label rather than a
/// Markdown heading, so the labels can't be confused with the `## Category`
/// headings inside either doc.
///
/// Global findings the project doc already states are dropped, along with any
/// global section left empty by that pruning: the two docs overlap by design
/// (a rule gets promoted from one repo to all of them), and paying twice for
/// the same rule in every review's prompt is exactly the token waste this
/// feature is supposed to avoid.
pub fn merge_memory_context(project: &str, global: &str) -> String {
    let project = project.trim();
    let global = prune_duplicate_findings(global, project);
    let global = global.trim();

    match (project.is_empty(), global.is_empty()) {
        (true, true) => String::new(),
        (false, true) => project.to_string(),
        (true, false) => global.to_string(),
        (false, false) => {
            format!("{PROJECT_CONTEXT_LABEL}\n{project}\n\n{GLOBAL_CONTEXT_LABEL}\n{global}")
        }
    }
}

/// Drop every `- ` bullet in `doc` that already appears (case- and
/// whitespace-insensitively) as a line in `other`, then drop any `## ` section
/// left with no bullets under it. Prose outside a section, and sections that
/// keep at least one bullet, are preserved as written.
fn prune_duplicate_findings(doc: &str, other: &str) -> String {
    if other.trim().is_empty() {
        return doc.to_string();
    }
    let existing: Vec<String> = other
        .lines()
        .map(|line| line.trim().trim_start_matches('-').trim().to_lowercase())
        .filter(|line| !line.is_empty())
        .collect();
    let is_duplicate = |bullet: &str| existing.contains(&bullet.trim().to_lowercase());

    // Buffer each section so a heading can be dropped once it turns out every
    // bullet under it was a duplicate.
    let mut out: Vec<String> = Vec::new();
    let mut section: Option<(String, Vec<String>, bool)> = None;
    let flush = |section: Option<(String, Vec<String>, bool)>, out: &mut Vec<String>| {
        if let Some((heading, body, kept_bullet)) = section
            && kept_bullet
        {
            out.push(heading);
            out.extend(body);
        }
    };

    for line in doc.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("## ") {
            flush(section.take(), &mut out);
            section = Some((line.to_string(), Vec::new(), false));
            continue;
        }
        let duplicate = trimmed.strip_prefix("- ").is_some_and(&is_duplicate);
        match &mut section {
            Some((_, body, kept_bullet)) => {
                if duplicate {
                    continue;
                }
                if trimmed.starts_with("- ") {
                    *kept_bullet = true;
                }
                body.push(line.to_string());
            }
            // Before the first heading: keep the doc's own header/prose, but
            // still prune stray duplicate bullets.
            None => {
                if !duplicate {
                    out.push(line.to_string());
                }
            }
        }
    }
    flush(section.take(), &mut out);
    out.join("\n")
}

/// Count of `- ` bullet findings in a review-memory doc's raw contents. Used
/// by the compact flow to show "N -> M findings" before/after a proposed
/// rewrite, and to skip running the pass entirely on a doc with nothing to
/// compact.
pub fn count_findings(contents: &str) -> usize {
    contents
        .lines()
        .filter(|line| line.trim_start().starts_with("- "))
        .count()
}

/// Prompt asking an agent to compact a review-memory doc: merge near-duplicate
/// findings and prune stale or overly specific ones, without touching
/// hand-written prose or the doc's overall shape. Unlike [`append_finding`],
/// the response replaces the doc wholesale — the caller must show it to the
/// user for explicit approval before writing (Epic E "prevent review-memory
/// rot").
pub fn compact_prompt(contents: &str) -> String {
    format!(
        "You are compacting a team's code-review findings doc so it stays useful \
         over time instead of drifting and bloating.\n\n\
         Below is the current contents of the doc. It is Markdown: a top-level \
         header, `## Category` section headings, and findings as `- ` bullets \
         underneath. It may also contain hand-written prose paragraphs.\n\n\
         Rewrite it: merge findings that state the same rule in different words \
         into one clear bullet, and drop findings that are stale, superseded by a \
         more general bullet already in the doc, or too specific to a single past \
         PR to be a durable rule. Keep every section heading that still has \
         findings under it; drop a heading only if it ends up empty. Preserve the \
         top-level header and any hand-written prose paragraphs exactly as they \
         are — do not rewrite or remove them.\n\n\
         Output ONLY the full replacement document in the same Markdown shape as \
         the input (header, prose, `## Category` headings, `- ` bullets). No \
         commentary outside the document itself.\n\n---\n\n{contents}"
    )
}

/// Parse a distilled-findings response into `(category, finding)` pairs:
/// `## Heading` sections with `- ` bullets underneath — the same shape
/// [`append_finding`] itself writes, so the lookback bootstrap (Epic E) can
/// feed an agent's clustered output straight back through it. Lines before
/// the first heading fall under `General`; non-bullet lines (stray prose the
/// model wasn't asked for) are ignored rather than mistaken for a finding.
pub fn parse_findings_markdown(text: &str) -> Vec<(String, String)> {
    let mut findings = Vec::new();
    let mut category = "General".to_string();
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(heading) = trimmed.strip_prefix("## ") {
            category = heading.trim().to_string();
            continue;
        }
        if let Some(bullet) = trimmed.strip_prefix("- ") {
            let bullet = bullet.trim();
            if !bullet.is_empty() {
                findings.push((category.clone(), bullet.to_string()));
            }
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn review_memory_path_defaults_relative_to_repo() {
        let repo = Path::new("/repo");
        assert_eq!(
            review_memory_path(repo, None),
            PathBuf::from("/repo/.amf/review-memory.md")
        );
    }

    #[test]
    fn review_memory_path_honors_relative_override() {
        let repo = Path::new("/repo");
        assert_eq!(
            review_memory_path(repo, Some("docs/review-notes.md")),
            PathBuf::from("/repo/docs/review-notes.md")
        );
    }

    #[test]
    fn review_memory_path_honors_absolute_override() {
        let repo = Path::new("/repo");
        assert_eq!(
            review_memory_path(repo, Some("/elsewhere/notes.md")),
            PathBuf::from("/elsewhere/notes.md")
        );
    }

    #[test]
    fn review_memory_path_treats_blank_override_as_default() {
        let repo = Path::new("/repo");
        assert_eq!(
            review_memory_path(repo, Some("   ")),
            PathBuf::from("/repo/.amf/review-memory.md")
        );
    }

    #[test]
    fn ensure_review_memory_doc_creates_with_header() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".amf").join("review-memory.md");
        assert!(!path.exists());

        ensure_review_memory_doc(&path, MemoryScope::Project).unwrap();

        assert!(path.exists());
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.starts_with("# Review memory"));
    }

    #[test]
    fn ensure_review_memory_doc_is_noop_when_present() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("review-memory.md");
        std::fs::write(&path, "# Custom\n\nhand-written content\n").unwrap();

        ensure_review_memory_doc(&path, MemoryScope::Project).unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "# Custom\n\nhand-written content\n");
    }

    #[test]
    fn append_finding_creates_doc_and_section() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("review-memory.md");

        let appended = append_finding(
            &path,
            MemoryScope::Project,
            "concurrency",
            "Guard shared state behind a lock",
        )
        .unwrap();

        assert!(appended);
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("## Concurrency"));
        assert!(contents.contains("- Guard shared state behind a lock"));
    }

    #[test]
    fn append_finding_reuses_existing_section() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("review-memory.md");
        append_finding(
            &path,
            MemoryScope::Project,
            "naming",
            "Prefer full words over abbreviations",
        )
        .unwrap();

        append_finding(
            &path,
            MemoryScope::Project,
            "naming",
            "Avoid single-letter identifiers",
        )
        .unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        // Exactly one "## Naming" heading, both findings under it.
        assert_eq!(contents.matches("## Naming").count(), 1);
        assert!(contents.contains("- Prefer full words over abbreviations"));
        assert!(contents.contains("- Avoid single-letter identifiers"));
    }

    #[test]
    fn append_finding_does_not_bleed_into_next_section() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("review-memory.md");
        append_finding(&path, MemoryScope::Project, "naming", "Prefer full words").unwrap();
        append_finding(&path, MemoryScope::Project, "tests", "Cover the error path").unwrap();

        append_finding(&path, MemoryScope::Project, "naming", "Avoid abbreviations").unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        let naming_idx = contents.find("## Naming").unwrap();
        let tests_idx = contents.find("## Tests").unwrap();
        let avoid_idx = contents.find("Avoid abbreviations").unwrap();
        assert!(naming_idx < avoid_idx && avoid_idx < tests_idx);
    }

    #[test]
    fn append_finding_dedupes_case_and_whitespace_insensitively() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("review-memory.md");
        append_finding(&path, MemoryScope::Project, "tests", "Cover the error path").unwrap();

        let appended = append_finding(
            &path,
            MemoryScope::Project,
            "tests",
            "  cover THE error path  ",
        )
        .unwrap();

        assert!(!appended);
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents.matches("Cover the error path").count(), 1);
    }

    #[test]
    fn append_finding_ignores_blank_input() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("review-memory.md");

        let appended = append_finding(&path, MemoryScope::Project, "tests", "   ").unwrap();

        assert!(!appended);
        assert!(!path.exists());
    }

    #[test]
    fn append_finding_blank_category_falls_back_to_general() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("review-memory.md");

        append_finding(
            &path,
            MemoryScope::Project,
            "  ",
            "Some finding with no category",
        )
        .unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("## General"));
    }

    #[test]
    fn append_finding_preserves_hand_written_prose() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("review-memory.md");
        std::fs::write(
            &path,
            "# Review memory\n\n## Concurrency\n\nSome hand-written prose explaining context.\n\n- An existing finding\n",
        )
        .unwrap();

        append_finding(&path, MemoryScope::Project, "concurrency", "A new finding").unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("Some hand-written prose explaining context."));
        assert!(contents.contains("- An existing finding"));
        assert!(contents.contains("- A new finding"));
    }

    #[test]
    fn parse_findings_markdown_groups_by_heading() {
        let text = "## Concurrency\n- Guard shared state behind a lock\n- Avoid busy-waiting\n\n## Naming\n- Prefer full words\n";
        let findings = parse_findings_markdown(text);
        assert_eq!(
            findings,
            vec![
                (
                    "Concurrency".to_string(),
                    "Guard shared state behind a lock".to_string()
                ),
                ("Concurrency".to_string(), "Avoid busy-waiting".to_string()),
                ("Naming".to_string(), "Prefer full words".to_string()),
            ]
        );
    }

    #[test]
    fn parse_findings_markdown_defaults_to_general_before_first_heading() {
        let findings = parse_findings_markdown("- A finding with no heading yet\n");
        assert_eq!(
            findings,
            vec![(
                "General".to_string(),
                "A finding with no heading yet".to_string()
            )]
        );
    }

    #[test]
    fn parse_findings_markdown_ignores_non_bullet_prose() {
        let text =
            "## Tests\nSome intro sentence the model added anyway.\n- Cover the error path\n";
        let findings = parse_findings_markdown(text);
        assert_eq!(
            findings,
            vec![("Tests".to_string(), "Cover the error path".to_string())]
        );
    }

    #[test]
    fn parse_findings_markdown_empty_input_yields_no_findings() {
        assert!(parse_findings_markdown("").is_empty());
        assert!(parse_findings_markdown("## Tests\n").is_empty());
    }

    #[test]
    fn count_findings_counts_only_bullet_lines() {
        let text =
            "# Review memory\n\nSome prose.\n\n## Tests\n- One\n- Two\n\n## Naming\n- Three\n";
        assert_eq!(count_findings(text), 3);
    }

    #[test]
    fn count_findings_empty_doc_is_zero() {
        assert_eq!(count_findings(""), 0);
        assert_eq!(
            count_findings("# Review memory\n\nJust prose, no bullets.\n"),
            0
        );
    }

    #[test]
    fn global_review_memory_path_defaults_into_the_config_dir() {
        let config_dir = Path::new("/home/u/.config/amf");
        assert_eq!(
            global_review_memory_path_in(config_dir, None),
            PathBuf::from("/home/u/.config/amf/review-memory.md")
        );
        assert_eq!(
            global_review_memory_path_in(config_dir, Some("   ")),
            PathBuf::from("/home/u/.config/amf/review-memory.md")
        );
    }

    #[test]
    fn global_review_memory_path_honors_relative_and_absolute_overrides() {
        let config_dir = Path::new("/home/u/.config/amf");
        assert_eq!(
            global_review_memory_path_in(config_dir, Some("notes/lessons.md")),
            PathBuf::from("/home/u/.config/amf/notes/lessons.md")
        );
        assert_eq!(
            global_review_memory_path_in(config_dir, Some("/srv/shared/lessons.md")),
            PathBuf::from("/srv/shared/lessons.md")
        );
    }

    #[test]
    fn ensure_review_memory_doc_writes_the_scope_specific_header() {
        let dir = TempDir::new().unwrap();
        let global = dir.path().join("global.md");

        ensure_review_memory_doc(&global, MemoryScope::Global).unwrap();

        let contents = std::fs::read_to_string(&global).unwrap();
        assert!(contents.starts_with("# Review memory (cross-project)"));
        assert!(contents.contains("every repo you work in"));
    }

    #[test]
    fn append_finding_creates_a_global_doc_with_the_global_header() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("global.md");

        let appended =
            append_finding(&path, MemoryScope::Global, "tests", "Cover the error path").unwrap();

        assert!(appended);
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.starts_with("# Review memory (cross-project)"));
        assert!(contents.contains("- Cover the error path"));
    }

    #[test]
    fn memory_scope_labels_and_toggles() {
        assert_eq!(MemoryScope::default(), MemoryScope::Project);
        assert_eq!(MemoryScope::Project.label(), "project");
        assert_eq!(MemoryScope::Global.label(), "global");
        assert_eq!(MemoryScope::Project.toggled(), MemoryScope::Global);
        assert_eq!(MemoryScope::Global.toggled(), MemoryScope::Project);
    }

    #[test]
    fn review_memory_paths_selects_by_scope() {
        let paths = ReviewMemoryPaths {
            project: PathBuf::from("/repo/.amf/review-memory.md"),
            global: PathBuf::from("/home/u/.config/amf/review-memory.md"),
        };
        assert_eq!(
            paths.for_scope(MemoryScope::Project),
            Path::new("/repo/.amf/review-memory.md")
        );
        assert_eq!(
            paths.for_scope(MemoryScope::Global),
            Path::new("/home/u/.config/amf/review-memory.md")
        );
    }

    #[test]
    fn merge_memory_context_single_doc_is_verbatim() {
        let project = "# Review memory\n\n## Tests\n- Cover the error path\n";
        assert_eq!(merge_memory_context(project, ""), project.trim());
        assert_eq!(merge_memory_context("", project), project.trim());
        assert_eq!(merge_memory_context("", "   \n  "), "");
    }

    #[test]
    fn merge_memory_context_labels_both_docs() {
        let merged = merge_memory_context(
            "## Tests\n- Cover the error path\n",
            "## Naming\n- Prefer full words\n",
        );
        let project_at = merged.find(PROJECT_CONTEXT_LABEL).unwrap();
        let global_at = merged.find(GLOBAL_CONTEXT_LABEL).unwrap();
        assert!(project_at < global_at, "project doc comes first");
        assert!(merged.contains("- Cover the error path"));
        assert!(merged.contains("- Prefer full words"));
    }

    #[test]
    fn merge_memory_context_drops_global_findings_the_project_doc_already_states() {
        let merged = merge_memory_context(
            "## Tests\n- Cover the error path\n",
            "## Tests\n-   cover THE error path  \n- Assert on the message, not the type\n",
        );
        assert_eq!(merged.matches("Cover the error path").count(), 1);
        assert!(merged.contains("- Assert on the message, not the type"));
    }

    #[test]
    fn merge_memory_context_drops_a_global_section_left_empty_by_pruning() {
        let merged = merge_memory_context(
            "## Tests\n- Cover the error path\n",
            "# Review memory (cross-project)\n\n## Tests\n- Cover the error path\n\n## Naming\n- Prefer full words\n",
        );
        // The wholly-duplicated "Tests" section is gone from the global side,
        // but its heading survives on the project side.
        assert_eq!(merged.matches("## Tests").count(), 1);
        assert!(merged.find("## Tests").unwrap() < merged.find(GLOBAL_CONTEXT_LABEL).unwrap());
        assert!(merged.contains("## Naming"));
        assert!(merged.contains("- Prefer full words"));
        assert!(merged.contains("# Review memory (cross-project)"));
    }

    #[test]
    fn merge_memory_context_wholly_duplicated_global_doc_falls_back_to_project_only() {
        let project = "## Tests\n- Cover the error path\n";
        let merged = merge_memory_context(project, "## Tests\n- Cover the error path\n");
        assert_eq!(merged, project.trim());
        assert!(!merged.contains(GLOBAL_CONTEXT_LABEL));
    }

    #[test]
    fn compact_prompt_embeds_the_doc_and_instructs_full_replacement() {
        let contents = "# Review memory\n\n## Tests\n- Cover the error path\n";
        let prompt = compact_prompt(contents);
        assert!(prompt.contains(contents));
        assert!(prompt.contains("merge"));
        assert!(prompt.contains("Output ONLY the full replacement document"));
    }
}
