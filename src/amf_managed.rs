//! What AMF itself writes into a checkout, so views of "your changes" can tell
//! AMF's own files apart from the work being reviewed.
//!
//! This is the one place that list lives. The writers are spread out —
//! `app/setup.rs` (harness hooks, injected skills, `.gitignore` entries and
//! instruction blocks), `extension::generated_amf_subdir` (`.amf/`),
//! `app/plan_interview.rs` (plan-file ignore entries), `theme.rs` (OpenCode
//! themes) and `app/review` (Final Review bookkeeping) — so a new file AMF
//! starts writing into a workdir belongs here too, or it will show up in the
//! diff viewer as if the user had changed it.
//!
//! Two questions, deliberately separate:
//!
//! - [`is_amf_managed_path`]: is this a file AMF writes whole? Answered from the
//!   path alone.
//! - [`is_amf_generated_change`]: is this *diff entry* AMF's doing? A managed
//!   path only counts while it is untracked: once a file is committed or staged
//!   someone chose to keep it, so it is ordinary work. A shared file AMF edits
//!   in place (`.gitignore`, `AGENTS.md`, …) counts only when every line that
//!   differs is one AMF writes.

use crate::diff::{DiffFile, DiffFileStatus};

/// Skill directories AMF injects `amf-*` skills into, one per harness
/// (`app::setup::agent_skills_dir`).
const SKILL_DIRS: &[&str] = &[
    ".claude/skills/",
    ".opencode/skills/",
    ".agents/skills/",
    ".pi/skills/",
];

/// Single files AMF writes whole, relative to the workdir.
const MANAGED_FILES: &[&str] = &[
    // Claude Code hooks and AMF's record of what it added to them.
    ".claude/settings.local.json",
    ".claude/amf-hook-state.json",
    ".claude/latest-prompt.txt",
    // Review Mode notes and Final Review feedback.
    ".claude/review-notes.md",
    ".claude/review-notes-archive.md",
    ".claude/final-review-feedback.md",
    ".claude/final-review-feedback-archive.md",
    // OpenCode plugins.
    ".opencode/plugins/input-request.js",
    ".opencode/plugins/sidebar-state.js",
    ".opencode/plugins/change-tracker.js",
    // Codex notify hook.
    ".codex/amf-codex-notify.sh",
];

/// Lines AMF appends to a checkout's root `.gitignore`.
fn is_amf_root_ignore_line(line: &str) -> bool {
    matches!(
        line,
        "CLAUDE.local.md"
            | "AGENTS.md"
            | "# AMF:plan-instructions:begin"
            | "# AMF:plan-instructions:end"
    ) || (line.starts_with("AMF_PLAN") && line.ends_with(".md") && !line.contains('/'))
}

/// Lines AMF writes into `.claude/.gitignore`
/// (`app::setup::ensure_claude_runtime_ignores_with_io`). The file is judged by
/// its lines rather than its path because a user may keep their own there.
fn is_amf_claude_ignore_line(line: &str) -> bool {
    matches!(
        line,
        "notifications/"
            | "review-notes.md"
            | "review-notes-archive.md"
            | "final-review-feedback.md"
            | "final-review-feedback-archive.md"
            | "latest-prompt.txt"
            | "skills/amf-*/"
    )
}

/// True for a file AMF writes in its entirety: harness hooks and settings it
/// injects, the `amf-*` skills, `.amf/` (generated scratch, except the legacy
/// hand-written `.amf/config.json`), Review Mode notes and Final Review
/// bookkeeping. `path` is relative to the workdir, `/`-separated, as the diff
/// reports it.
pub fn is_amf_managed_path(path: &str) -> bool {
    if MANAGED_FILES.contains(&path) {
        return true;
    }
    if crate::app::review::is_review_bookkeeping_path(path) {
        return true;
    }
    if let Some(rest) = path.strip_prefix(".claude/notifications/") {
        return !rest.is_empty();
    }
    if let Some(rest) = path.strip_prefix(".amf/") {
        return !rest.is_empty() && rest != "config.json";
    }
    if let Some(theme) = path.strip_prefix(".opencode/themes/") {
        return theme == "amf.json" || (theme.starts_with("amf-") && theme.ends_with(".json"));
    }
    SKILL_DIRS.iter().any(|dir| {
        path.strip_prefix(dir)
            .is_some_and(|rest| rest.starts_with("amf-") && rest.contains('/'))
    })
}

/// True when this diff entry is AMF's own doing rather than a change to
/// review. See the module docs for the rule; anything it cannot judge (a
/// binary, unreadable content, a rename) is the user's.
pub fn is_amf_generated_change(file: &DiffFile) -> bool {
    match file.status {
        DiffFileStatus::Untracked => {
            is_amf_managed_path(&file.path) || shared_file_change_is_amf_only(file)
        }
        DiffFileStatus::Modified => shared_file_change_is_amf_only(file),
        _ => false,
    }
}

/// A file both the user and AMF write to, where only AMF's lines changed.
fn shared_file_change_is_amf_only(file: &DiffFile) -> bool {
    if file.is_binary {
        return false;
    }
    let Some(new) = file.new_content.as_deref() else {
        return false;
    };
    let old = match file.status {
        DiffFileStatus::Untracked => "",
        _ => match file.old_content.as_deref() {
            Some(old) => old,
            None => return false,
        },
    };
    if old == new {
        return false;
    }
    match file.path.as_str() {
        ".gitignore" => same_without(old, new, is_amf_root_ignore_line),
        ".claude/.gitignore" => same_without(old, new, is_amf_claude_ignore_line),
        "AGENTS.md" | "CLAUDE.local.md" => {
            let strip = crate::app::setup::strip_amf_managed_blocks;
            strip(old).trim() == strip(new).trim()
        }
        _ => false,
    }
}

/// `old` and `new` agree once AMF's lines (and blank lines, which AMF adds
/// around its own) are dropped.
fn same_without(old: &str, new: &str, is_amf_line: fn(&str) -> bool) -> bool {
    let keep = |text: &str| -> Vec<String> {
        text.lines()
            .map(str::trim_end)
            .filter(|line| !line.trim().is_empty() && !is_amf_line(line))
            .map(ToOwned::to_owned)
            .collect()
    };
    keep(old) == keep(new)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, status: DiffFileStatus, old: Option<&str>, new: Option<&str>) -> DiffFile {
        DiffFile {
            old_path: None,
            path: path.to_string(),
            status,
            additions: 1,
            deletions: 0,
            is_binary: false,
            old_content: old.map(str::to_string),
            new_content: new.map(str::to_string),
            patch: String::new(),
            hunks: Vec::new(),
        }
    }

    #[test]
    fn whole_files_amf_writes_are_managed_paths() {
        for path in [
            ".claude/settings.local.json",
            ".claude/amf-hook-state.json",
            ".claude/latest-prompt.txt",
            ".claude/notifications/abc.json",
            ".claude/review-notes.md",
            ".claude/final-review-progress.json",
            ".claude/final-review-snapshot.json",
            ".claude/skills/amf-interact/SKILL.md",
            ".claude/skills/amf-add-hook/SKILL.md",
            ".opencode/skills/amf-interact/scripts/ai_driver.py",
            ".agents/skills/amf-interact/SKILL.md",
            ".pi/skills/amf-interact/SKILL.md",
            ".opencode/plugins/sidebar-state.js",
            ".opencode/themes/amf.json",
            ".opencode/themes/amf-tokyonight.json",
            ".codex/amf-codex-notify.sh",
            ".amf/README.md",
            ".amf/session-status/x.json",
            ".amf/interview-docs/1-0/spec.md",
        ] {
            assert!(is_amf_managed_path(path), "{path} should be AMF-managed");
        }
    }

    #[test]
    fn user_files_beside_amf_ones_are_not_managed_paths() {
        for path in [
            "README.md",
            ".gitignore",
            ".claude/.gitignore",
            "AGENTS.md",
            "CLAUDE.md",
            "amf.json",
            ".claude/settings.json",
            ".claude/commands/deploy.md",
            ".claude/skills/my-skill/SKILL.md",
            ".claude/skills/amf-notes.md",
            ".opencode/plugins/mine.js",
            ".opencode/themes/solarized.json",
            ".codex/config.toml",
            ".amf/config.json",
            ".amf/",
            "src/.claude/settings.local.json",
        ] {
            assert!(!is_amf_managed_path(path), "{path} is the user's");
        }
    }

    #[test]
    fn a_managed_path_only_counts_while_untracked() {
        let untracked = file(
            ".claude/settings.local.json",
            DiffFileStatus::Untracked,
            None,
            Some("{}"),
        );
        assert!(is_amf_generated_change(&untracked));
        for status in [
            DiffFileStatus::Added,
            DiffFileStatus::Modified,
            DiffFileStatus::Deleted,
            DiffFileStatus::Renamed,
        ] {
            let tracked = file(
                ".claude/settings.local.json",
                status.clone(),
                Some("{}"),
                Some("{\"a\":1}"),
            );
            assert!(
                !is_amf_generated_change(&tracked),
                "a {status:?} managed path was committed or staged by someone"
            );
        }
    }

    #[test]
    fn a_gitignore_counts_only_when_every_changed_line_is_amfs() {
        let fresh = file(
            ".gitignore",
            DiffFileStatus::Untracked,
            None,
            Some("CLAUDE.local.md\n"),
        );
        assert!(is_amf_generated_change(&fresh));

        let appended = file(
            ".gitignore",
            DiffFileStatus::Modified,
            Some("target/"),
            Some("target/\nCLAUDE.local.md\nAMF_PLAN.md\nAMF_PLAN.todo-x.md\n"),
        );
        assert!(is_amf_generated_change(&appended));

        let mixed = file(
            ".gitignore",
            DiffFileStatus::Modified,
            Some("target/\n"),
            Some("target/\nCLAUDE.local.md\nnode_modules/\n"),
        );
        assert!(
            !is_amf_generated_change(&mixed),
            "node_modules/ is the user's"
        );

        let users_own = file(
            ".gitignore",
            DiffFileStatus::Untracked,
            None,
            Some("dist/\n"),
        );
        assert!(!is_amf_generated_change(&users_own));
    }

    #[test]
    fn a_claude_gitignore_counts_only_for_amfs_entries() {
        let fresh = file(
            ".claude/.gitignore",
            DiffFileStatus::Untracked,
            None,
            Some("notifications/\nlatest-prompt.txt\nskills/amf-*/\n"),
        );
        assert!(is_amf_generated_change(&fresh));
        let users_own = file(
            ".claude/.gitignore",
            DiffFileStatus::Untracked,
            None,
            Some("scratch/\n"),
        );
        assert!(!is_amf_generated_change(&users_own));
        let amf = file(
            ".claude/.gitignore",
            DiffFileStatus::Modified,
            Some("mine/\n"),
            Some("mine/\nnotifications/\nskills/amf-*/\n"),
        );
        assert!(is_amf_generated_change(&amf));
        let user = file(
            ".claude/.gitignore",
            DiffFileStatus::Modified,
            Some("mine/\n"),
            Some("mine/\nyours/\n"),
        );
        assert!(!is_amf_generated_change(&user));
    }

    #[test]
    fn instruction_files_count_only_for_amfs_managed_block() {
        let block = "<!-- AMF:plan-instructions:begin -->\n\n## Plan Mode\n\n\
                     <!-- AMF:plan-instructions:end -->\n";
        let only_block = file("AGENTS.md", DiffFileStatus::Untracked, None, Some(block));
        assert!(is_amf_generated_change(&only_block));

        let appended = file(
            "AGENTS.md",
            DiffFileStatus::Modified,
            Some("# Rules\n"),
            Some(&format!("# Rules\n\n{block}")),
        );
        assert!(is_amf_generated_change(&appended));

        let edited = file(
            "AGENTS.md",
            DiffFileStatus::Modified,
            Some("# Rules\n"),
            Some(&format!("# Rules\nNew rule\n\n{block}")),
        );
        assert!(!is_amf_generated_change(&edited));
    }

    #[test]
    fn unjudgeable_entries_are_the_users() {
        let mut binary = file(".gitignore", DiffFileStatus::Untracked, None, Some(""));
        binary.is_binary = true;
        assert!(!is_amf_generated_change(&binary));
        let unreadable = file(".gitignore", DiffFileStatus::Modified, None, Some("x"));
        assert!(!is_amf_generated_change(&unreadable));
        let readme = file("README.md", DiffFileStatus::Modified, Some("a"), Some("b"));
        assert!(!is_amf_generated_change(&readme));
    }
}
