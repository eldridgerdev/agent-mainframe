use super::AI_FINDING_HUNK_CONTEXT_LINES;

pub(super) fn resolve_ai_review_location(
    file: &crate::diff::DiffFile,
    line: u32,
    side: Option<crate::diff::DiffSide>,
) -> Option<(crate::diff::DiffSide, crate::diff::DiffLineLocation)> {
    use crate::diff::DiffSide;

    let line = line as usize;
    if let Some(side) = side {
        return file
            .resolve_source_line(side, line)
            .map(|location| (side, location));
    }

    let old = file.resolve_source_line(DiffSide::Old, line);
    let new = file.resolve_source_line(DiffSide::New, line);
    match (old, new) {
        (Some(old), Some(new)) if old == new => Some((DiffSide::New, new)),
        (Some(_), Some(_)) => None,
        (Some(old), None) => Some((DiffSide::Old, old)),
        (None, Some(new)) => Some((DiffSide::New, new)),
        (None, None) => None,
    }
}

/// Reconstruct a GitHub-style `diff_hunk` string (the `@@ ... @@` header plus
/// a small window around an already-resolved source coordinate).
pub(in crate::app) fn diff_hunk_for_location(
    files: &[crate::diff::DiffFile],
    path: &str,
    side: crate::diff::DiffSide,
    location: crate::diff::DiffLineLocation,
) -> Option<String> {
    let file = files.iter().find(|f| f.path == path)?;
    let hunk = file.hunk_for_location(location)?;
    let line = location.line_on(side)?;

    super::pr_review::window_parsed_hunk(
        hunk,
        line,
        side == crate::diff::DiffSide::Old,
        AI_FINDING_HUNK_CONTEXT_LINES,
    )
}
