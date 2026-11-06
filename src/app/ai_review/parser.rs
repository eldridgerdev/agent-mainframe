use super::AiReviewFinding;

/// Render a parsed unified diff with an explicit source coordinate on every
/// addressable row. Context rows show both coordinates but lead with RIGHT so
/// findings naturally target the current file; removals expose LEFT only and
/// additions RIGHT only. If parsing fails, retain the raw diff so the review
/// can still run, while response validation will conservatively downgrade any
/// location that cannot be resolved.
pub(super) fn annotated_diff_for_ai_review(diff: &str) -> String {
    use crate::diff::{DiffLineKind, DiffSide, line_locations_in_hunk};

    let Ok(files) = crate::diff::parse_unified_diff(diff) else {
        return diff.trim_end().to_string();
    };
    let mut out = String::new();
    for file in files {
        out.push_str(&format!("File: {}\n", file.path));
        for hunk in &file.hunks {
            out.push_str(&hunk.header);
            out.push('\n');
            for (line, location) in hunk.lines.iter().zip(line_locations_in_hunk(hunk)) {
                let label = match (line.kind.clone(), location) {
                    (DiffLineKind::Context, Some(location)) => format!(
                        "RIGHT:{} LEFT:{}",
                        location
                            .line_on(DiffSide::New)
                            .expect("context has new line"),
                        location
                            .line_on(DiffSide::Old)
                            .expect("context has old line")
                    ),
                    (DiffLineKind::Added, Some(location)) => format!(
                        "RIGHT:{}",
                        location
                            .line_on(DiffSide::New)
                            .expect("addition has new line")
                    ),
                    (DiffLineKind::Removed, Some(location)) => format!(
                        "LEFT:{}",
                        location
                            .line_on(DiffSide::Old)
                            .expect("removal has old line")
                    ),
                    (DiffLineKind::NoNewlineMarker, None) => "MARKER".to_string(),
                    _ => continue,
                };
                out.push_str(&format!("[{label}] {}\n", line.text));
            }
        }
        out.push('\n');
    }
    out.trim_end().to_string()
}

/// If `output` is entirely wrapped in one fenced code block (a common way
/// models "helpfully" package a response even when told not to add anything
/// outside the requested format, e.g. ` ```markdown ... ``` `), strip the
/// fence and return the inner text. Otherwise returns `output` unchanged.
fn strip_outer_code_fence(output: &str) -> &str {
    let trimmed = output.trim();
    let Some(after_open) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let Some(close_at) = after_open.rfind("```") else {
        return trimmed;
    };
    // Skip an optional language tag on the opening fence line (```markdown).
    let body_start = after_open.find('\n').map_or(0, |i| i + 1);
    if body_start > close_at {
        return trimmed;
    }
    after_open[body_start..close_at].trim()
}

/// Recognize a finding heading: 1-4 leading `#` characters followed by
/// whitespace and the heading text. [`ai_review_prompt`] asks for exactly
/// [`AI_FINDING_HEADING_PREFIX`] (`###`), but models don't reliably hold a
/// specific heading level (`##`/`#` are common substitutions), so the parser
/// accepts any small heading level rather than silently dropping every
/// finding over that one mismatch.
fn strip_finding_heading(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let hashes = trimmed.chars().take_while(|&c| c == '#').count();
    if !(1..=4).contains(&hashes) {
        return None;
    }
    let rest = trimmed[hashes..].trim_start();
    (!rest.is_empty()).then_some(rest)
}

/// Parse the AI reviewer's fixed-format output ([`ai_review_prompt`]) into its
/// overall summary and findings. Tolerant of common formatting drift: an
/// outer code fence around the whole response is stripped first
/// ([`strip_outer_code_fence`]), and any small markdown heading level starts a
/// section ([`strip_finding_heading`], not just the requested `###`). A
/// case-insensitive `Summary` section is separated from findings; a
/// `path|RIGHT|line` / `path|LEFT|line` finding heading is explicitly anchored.
/// Legacy `path:line` remains parseable with an unspecified side for safe
/// migration; the validation pass must resolve it unambiguously. Anything else
/// (`General`, malformed) stays pathless. Empty sections are dropped rather
/// than erroring, so a partially-malformed response still yields whatever
/// content did parse.
pub(super) fn parse_ai_review_output(output: &str) -> (Option<String>, Vec<AiReviewFinding>) {
    fn flush(
        current: Option<(&str, Vec<&str>)>,
        summary: &mut Option<String>,
        out: &mut Vec<AiReviewFinding>,
    ) {
        let Some((heading, lines)) = current else {
            return;
        };
        let body = lines.join("\n").trim().to_string();
        if body.is_empty() {
            return;
        }
        if heading.trim().eq_ignore_ascii_case("summary") {
            if summary.is_none() {
                *summary = Some(body);
            }
            return;
        }
        let heading = heading.trim();
        let explicit = heading.rsplit_once('|').and_then(|(path_and_side, line)| {
            let (path, side) = path_and_side.rsplit_once('|')?;
            let side = match side.trim().to_ascii_uppercase().as_str() {
                "LEFT" => crate::diff::DiffSide::Old,
                "RIGHT" => crate::diff::DiffSide::New,
                _ => return None,
            };
            let line = line.trim().parse::<u32>().ok()?;
            (!path.is_empty()).then(|| (path.to_string(), line, side))
        });
        let (path, line, side) = match explicit {
            Some((path, line, side)) => (Some(path), Some(line), Some(side)),
            None => match heading.rsplit_once(':') {
                Some((path, line)) if !path.is_empty() => match line.trim().parse::<u32>() {
                    Ok(line) => (Some(path.to_string()), Some(line), None),
                    Err(_) => (None, None, None),
                },
                _ => (None, None, None),
            },
        };
        out.push(AiReviewFinding {
            path,
            line,
            side,
            body,
            diff_hunk: None,
            skipped: false,
            published: false,
        });
    }

    let output = strip_outer_code_fence(output);
    let mut summary = None;
    let mut findings = Vec::new();
    let mut current: Option<(&str, Vec<&str>)> = None;
    for raw_line in output.lines() {
        match strip_finding_heading(raw_line) {
            Some(heading) => {
                flush(current.take(), &mut summary, &mut findings);
                current = Some((heading, Vec::new()));
            }
            None => {
                if let Some((_, lines)) = current.as_mut() {
                    lines.push(raw_line);
                }
            }
        }
    }
    flush(current, &mut summary, &mut findings);
    (summary, findings)
}

#[cfg(test)]
pub(super) fn parse_ai_findings(output: &str) -> Vec<AiReviewFinding> {
    parse_ai_review_output(output).1
}
