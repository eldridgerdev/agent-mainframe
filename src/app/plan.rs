use std::path::{Path, PathBuf};

use crate::project::Feature;

pub(crate) const DEFAULT_PLAN_FILE: &str = "AMF_PLAN.md";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EffectivePlan {
    Default(PathBuf),
    Selected(PathBuf),
}

impl EffectivePlan {
    pub(crate) fn path(&self) -> &Path {
        match self {
            Self::Default(path) | Self::Selected(path) => path,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectedPlanPathError {
    Missing,
    OutsideWorktree,
    NotAFile,
    NotMarkdown,
}

/// Resolve the one plan AMF should present for a feature.
///
/// The conventional worktree plan always wins. A manual selection is only
/// effective while it continues to satisfy the same worktree and file-type
/// boundary enforced when the picker persists it.
pub(crate) fn resolve_effective_plan(feature: &Feature) -> Option<EffectivePlan> {
    let default = feature.workdir.join(DEFAULT_PLAN_FILE);
    if default.is_file() {
        return Some(EffectivePlan::Default(default));
    }

    let selected = feature.selected_plan_path.as_deref()?;
    validate_selected_plan_path(&feature.workdir, selected)
        .ok()
        .map(EffectivePlan::Selected)
}

/// Return a canonical absolute plan path when `candidate` names a Markdown
/// file contained by the feature worktree.
///
/// Canonicalizing both sides before the boundary check rejects `..` traversal
/// and symlinks that point outside the worktree. Callers persist the returned
/// path, never the unchecked picker input.
pub(crate) fn validate_selected_plan_path(
    workdir: &Path,
    candidate: &Path,
) -> Result<PathBuf, SelectedPlanPathError> {
    let workdir = workdir
        .canonicalize()
        .map_err(|_| SelectedPlanPathError::Missing)?;
    let candidate = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        workdir.join(candidate)
    };
    let candidate = candidate
        .canonicalize()
        .map_err(|_| SelectedPlanPathError::Missing)?;

    if !candidate.starts_with(&workdir) {
        return Err(SelectedPlanPathError::OutsideWorktree);
    }
    if !candidate.is_file() {
        return Err(SelectedPlanPathError::NotAFile);
    }
    if !is_markdown_path(&candidate) {
        return Err(SelectedPlanPathError::NotMarkdown);
    }

    Ok(candidate)
}

pub(crate) fn is_markdown_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{AgentKind, Feature, VibeMode};
    use std::fs;
    use tempfile::TempDir;

    fn feature(workdir: &Path, selected_plan_path: Option<PathBuf>) -> Feature {
        let mut feature = Feature::new(
            "feature".into(),
            "feature/plan".into(),
            workdir.to_path_buf(),
            true,
            VibeMode::Vibeless,
            false,
            false,
            AgentKind::Codex,
            false,
            false,
        );
        feature.selected_plan_path = selected_plan_path;
        feature
    }

    #[test]
    fn default_plan_takes_precedence_over_a_selected_plan() {
        let workdir = TempDir::new().unwrap();
        let default = workdir.path().join(DEFAULT_PLAN_FILE);
        let selected = workdir.path().join("notes.md");
        fs::write(&default, "# Current\n").unwrap();
        fs::write(&selected, "# Older\n").unwrap();

        let resolved = resolve_effective_plan(&feature(workdir.path(), Some(selected))).unwrap();

        assert_eq!(resolved, EffectivePlan::Default(default));
    }

    #[test]
    fn valid_selected_plan_is_used_when_the_default_is_missing() {
        let workdir = TempDir::new().unwrap();
        let selected = workdir.path().join("docs").join("accepted.md");
        fs::create_dir_all(selected.parent().unwrap()).unwrap();
        fs::write(&selected, "# Accepted\n").unwrap();

        let resolved = resolve_effective_plan(&feature(workdir.path(), Some(selected))).unwrap();

        assert_eq!(
            resolved,
            EffectivePlan::Selected(
                workdir
                    .path()
                    .join("docs/accepted.md")
                    .canonicalize()
                    .unwrap()
            )
        );
    }

    #[test]
    fn missing_or_invalid_selected_plan_resolves_to_none() {
        let workdir = TempDir::new().unwrap();
        let missing = feature(workdir.path(), Some(workdir.path().join("missing.md")));
        assert_eq!(resolve_effective_plan(&missing), None);

        let non_markdown = workdir.path().join("notes.txt");
        fs::write(&non_markdown, "not markdown\n").unwrap();
        let invalid = feature(workdir.path(), Some(non_markdown));
        assert_eq!(resolve_effective_plan(&invalid), None);
    }

    #[test]
    fn validation_rejects_traversal_directories_and_non_markdown_files() {
        let parent = TempDir::new().unwrap();
        let workdir = parent.path().join("worktree");
        fs::create_dir(&workdir).unwrap();
        let outside = parent.path().join("outside.md");
        fs::write(&outside, "# Outside\n").unwrap();
        let directory = workdir.join("docs.md");
        fs::create_dir(&directory).unwrap();
        let text = workdir.join("notes.txt");
        fs::write(&text, "notes\n").unwrap();

        assert_eq!(
            validate_selected_plan_path(&workdir, Path::new("../outside.md")),
            Err(SelectedPlanPathError::OutsideWorktree)
        );
        assert_eq!(
            validate_selected_plan_path(&workdir, &directory),
            Err(SelectedPlanPathError::NotAFile)
        );
        assert_eq!(
            validate_selected_plan_path(&workdir, &text),
            Err(SelectedPlanPathError::NotMarkdown)
        );
    }

    #[cfg(unix)]
    #[test]
    fn validation_rejects_a_symlink_that_escapes_the_worktree() {
        use std::os::unix::fs::symlink;

        let parent = TempDir::new().unwrap();
        let workdir = parent.path().join("worktree");
        fs::create_dir(&workdir).unwrap();
        let outside = parent.path().join("outside.md");
        fs::write(&outside, "# Outside\n").unwrap();
        let link = workdir.join("linked.md");
        symlink(&outside, &link).unwrap();

        assert_eq!(
            validate_selected_plan_path(&workdir, &link),
            Err(SelectedPlanPathError::OutsideWorktree)
        );
    }
}
