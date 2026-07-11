//! Review-findings memory doc: a version-controlled Markdown file that
//! accumulates a team's recurring code-review findings, grouped by category.
//!
//! This is the shared substrate for PR-review Epic E: the AI reviewer reads
//! it as context before each review, the lookback bootstrap seeds it from
//! history, and the "add to memory" pane key appends to it incrementally. See
//! `docs/backlog/pr-comment-review-plan.md` (Epic E).
//!
//! AMF only ever *appends* here (dedup-aware, grouped by category) — it never
//! rewrites existing prose, so hand-edits are safe across runs.

use std::path::{Path, PathBuf};

/// Default location of the review-memory doc, relative to the repo root.
pub const DEFAULT_REVIEW_MEMORY_PATH: &str = ".amf/review-memory.md";

const HEADER_TEMPLATE: &str = "\
# Review memory

Recurring findings from code review, grouped by category. AMF appends new
findings here (from PR triage and AI review) and reads this file as context
before each AI review, so the same issue doesn't need rediscovering every
time. Edit freely — AMF only ever appends, it never rewrites what's already
here.
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

/// Ensure the doc exists at `path`, creating it (and any missing parent
/// directories) with the header template if it doesn't. No-op if it's
/// already there.
pub fn ensure_review_memory_doc(path: &Path) -> std::io::Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, HEADER_TEMPLATE)
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
/// and/or the category section as needed. Dedup-aware: if a
/// case-insensitive, trimmed match of `finding` already appears anywhere in
/// the doc, this is a no-op. Returns whether the finding was newly appended.
pub fn append_finding(path: &Path, category: &str, finding: &str) -> std::io::Result<bool> {
    let finding = finding.trim();
    if finding.is_empty() {
        return Ok(false);
    }
    ensure_review_memory_doc(path)?;
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

        ensure_review_memory_doc(&path).unwrap();

        assert!(path.exists());
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.starts_with("# Review memory"));
    }

    #[test]
    fn ensure_review_memory_doc_is_noop_when_present() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("review-memory.md");
        std::fs::write(&path, "# Custom\n\nhand-written content\n").unwrap();

        ensure_review_memory_doc(&path).unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "# Custom\n\nhand-written content\n");
    }

    #[test]
    fn append_finding_creates_doc_and_section() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("review-memory.md");

        let appended =
            append_finding(&path, "concurrency", "Guard shared state behind a lock").unwrap();

        assert!(appended);
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains("## Concurrency"));
        assert!(contents.contains("- Guard shared state behind a lock"));
    }

    #[test]
    fn append_finding_reuses_existing_section() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("review-memory.md");
        append_finding(&path, "naming", "Prefer full words over abbreviations").unwrap();

        append_finding(&path, "naming", "Avoid single-letter identifiers").unwrap();

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
        append_finding(&path, "naming", "Prefer full words").unwrap();
        append_finding(&path, "tests", "Cover the error path").unwrap();

        append_finding(&path, "naming", "Avoid abbreviations").unwrap();

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
        append_finding(&path, "tests", "Cover the error path").unwrap();

        let appended = append_finding(&path, "tests", "  cover THE error path  ").unwrap();

        assert!(!appended);
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents.matches("Cover the error path").count(), 1);
    }

    #[test]
    fn append_finding_ignores_blank_input() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("review-memory.md");

        let appended = append_finding(&path, "tests", "   ").unwrap();

        assert!(!appended);
        assert!(!path.exists());
    }

    #[test]
    fn append_finding_blank_category_falls_back_to_general() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("review-memory.md");

        append_finding(&path, "  ", "Some finding with no category").unwrap();

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

        append_finding(&path, "concurrency", "A new finding").unwrap();

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
        let text = "## Tests\nSome intro sentence the model added anyway.\n- Cover the error path\n";
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
}
