use anyhow::Result;

use super::*;
use crate::diff::{DiffFileStatus, DiffSnapshot};

/// Label given to every session this feature starts. Not deduplicated
/// against an existing session with the same base label -- the whole point
/// is a new session each time, so repeated presses number upward instead of
/// reusing one.
const FRESH_CONTEXT_LABEL: &str = "Fresh Context";

/// Trailing clarification ask appended to every seeded prompt, per the
/// brief's template -- the fresh session has no memory of this feature's
/// history, so it's told to check before it starts changing things.
const FRESH_CONTEXT_CLARIFY_ASK: &str =
    "Grill me with any questions to clarify before implementing";
const FRESH_CONTEXT_CONTINUATION_INSTRUCTION: &str = "Inspect the current work and continue from persisted artifacts, preserving the feature's existing intent";

/// Caps on the material folded into a seeded continuation prompt. A branch
/// with hundreds of changed files, or a multi-page prior prompt, would
/// otherwise produce a composer seed far larger than anything a person would
/// type -- these keep the useful signal (a sample of the diff, the gist of the
/// last prompt) without letting the seed grow without bound.
const MAX_CHANGED_FILES_LISTED: usize = 20;
const MAX_PROMPT_SOURCE_CHARS: usize = 600;

impl App {
    /// Open the fresh-context instruction prompt over the current session
    /// view (`Ctrl+Space` then `Shift+F`). Collecting the instruction first
    /// means the seeded prompt is complete when the new session opens,
    /// rather than asking the user to type over a placeholder there.
    pub fn open_fresh_context_prompt_from_view(&mut self) {
        self.open_fresh_context_prompt_with_prefill(
            String::new(),
            FreshContextPromptSource::Manual,
        );
    }

    /// Open the same editable fresh-context prompt used by the manual
    /// leader command, but with a generated continuation instruction already
    /// loaded into the input box.
    pub(crate) fn open_fresh_context_prompt_from_view_with_prefill(&mut self, prefill: String) {
        self.open_fresh_context_prompt_with_prefill(prefill, FreshContextPromptSource::ContextHint);
    }

    /// Open the editable fresh-context prompt with a conservative continuation
    /// instruction assembled from the current feature's persisted artifacts.
    pub(crate) fn open_fresh_context_prompt_from_view_with_context_hint(&mut self) {
        let view = match &self.mode {
            AppMode::Viewing(view) if view.session_kind.is_agent_harness() => view.clone(),
            _ => return,
        };
        let Some((pi, fi)) = view_project_feature_indices(&self.store, &view) else {
            self.push_toast_warning("Could not resolve the current feature");
            return;
        };

        let feature = &self.store.projects[pi].features[fi];
        let workdir = feature.workdir.clone();
        let relative_plan = plan::resolve_effective_plan(feature)
            .map(|plan| relative_display_path(&workdir, plan.path()));
        let changed_files = crate::diff::load_snapshot(&workdir, None, false)
            .map(|snapshot| changed_file_paths(&snapshot))
            .unwrap_or_default();
        let feature_summary = feature.summary.as_deref();
        let latest_prompt = self.continuation_latest_prompt(feature, &view.window);
        let prefill = build_fresh_context_prompt(
            relative_plan.as_deref(),
            &changed_files,
            feature_summary,
            latest_prompt,
            FRESH_CONTEXT_CONTINUATION_INSTRUCTION,
        );
        self.open_fresh_context_prompt_from_view_with_prefill(prefill);
    }

    /// Latest known prompt to fold into a continuation seed for the session
    /// currently being viewed (identified by its tmux window, not the
    /// feature). Codex keeps a per-session prompt cache, so a view of one of
    /// several Codex sessions gets that session's own prompt. When no
    /// session-scoped prompt is available the feature-wide cache is only
    /// trusted if a single agent session makes it unambiguous -- otherwise
    /// the seed would risk quoting a sibling session's prompt, so it's
    /// omitted.
    fn continuation_latest_prompt<'a>(
        &'a self,
        feature: &'a Feature,
        view_window: &str,
    ) -> Option<&'a str> {
        let session = feature
            .sessions
            .iter()
            .find(|session| session.tmux_window == view_window);

        let codex_session_prompt = session
            .and_then(|session| session.token_usage_source.as_ref())
            .filter(|source| {
                source.provider == crate::token_tracking::TokenUsageProvider::Codex
            })
            .and_then(|source| self.cached_codex_session_prompt(&feature.workdir, &source.id));
        if codex_session_prompt.is_some() {
            return codex_session_prompt;
        }

        let agent_session_count = feature
            .sessions
            .iter()
            .filter(|session| session.kind.is_agent_harness())
            .count();
        if agent_session_count > 1 {
            return None;
        }
        self.latest_prompt_for_session(&feature.tmux_session)
    }

    fn open_fresh_context_prompt_with_prefill(
        &mut self,
        prefill: String,
        source: FreshContextPromptSource,
    ) {
        let view = match &self.mode {
            AppMode::Viewing(view) if view.session_kind.is_agent_harness() => view.clone(),
            AppMode::Viewing(_) => {
                self.push_toast_warning("Fresh context sessions start from an agent session");
                return;
            }
            _ => return,
        };

        let Some((pi, fi)) = view_project_feature_indices(&self.store, &view) else {
            self.push_toast_warning("Could not resolve the current feature");
            return;
        };

        let feature_name = self.store.projects[pi].features[fi].name.clone();
        self.mode = AppMode::FreshContextPrompt(FreshContextPromptState {
            view,
            feature_name,
            input: prefill,
            source,
        });
    }

    /// Cancel the fresh-context prompt, returning to the session view
    /// unchanged.
    pub fn cancel_fresh_context_prompt(&mut self) {
        if let AppMode::FreshContextPrompt(state) = &self.mode {
            self.mode = AppMode::Viewing(state.view.clone());
        }
    }

    /// Start a brand-new agent-harness session in the current feature, using
    /// the typed instruction, seeded with a prompt pointing it at the
    /// feature's plan and the files changed on this branch -- so continuing
    /// related work doesn't have to carry the calling session's full token
    /// history forward. An empty instruction is a no-op cancel, matching TODO
    /// quick-capture. Left pre-filled and unsent in the compose box, matching
    /// Learning Mode's escalation pattern, so the user can still review
    /// before sending.
    pub fn commit_fresh_context_prompt(&mut self) -> Result<()> {
        let (view, instruction, source) = match &self.mode {
            AppMode::FreshContextPrompt(state) => (
                state.view.clone(),
                state.input.trim().to_string(),
                state.source,
            ),
            _ => return Ok(()),
        };

        // Reverting to `Viewing` up front means every early return below --
        // an unresolvable feature, a launch failure -- leaves the user back
        // where they started rather than stuck behind the prompt.
        self.mode = AppMode::Viewing(view.clone());

        if instruction.is_empty() {
            return Ok(());
        }

        let Some((pi, fi)) = view_project_feature_indices(&self.store, &view) else {
            self.push_toast_warning("Could not resolve the current feature");
            return Ok(());
        };

        let feature = &self.store.projects[pi].features[fi];
        let workdir = feature.workdir.clone();
        let harness = Some(feature.agent.clone());
        let label = fresh_context_session_label(feature);
        let relative_plan = plan::resolve_effective_plan(feature)
            .map(|plan| relative_display_path(&workdir, plan.path()));

        if relative_plan.is_none() {
            self.push_toast_warning("No plan file found for this feature -- starting without one");
        }

        let changed_files = match crate::diff::load_snapshot(&workdir, None, false) {
            Ok(snapshot) => changed_file_paths(&snapshot),
            Err(_) => Vec::new(),
        };

        let prompt = if source == FreshContextPromptSource::ContextHint {
            instruction.clone()
        } else {
            build_fresh_context_prompt(
                relative_plan.as_deref(),
                &changed_files,
                None,
                None,
                &instruction,
            )
        };

        // Recording that a fresh session was requested is not what's at
        // stake here -- unlike Learning Mode there is no answer row to link
        // it to -- so this warns and goes ahead rather than parking behind
        // the resource-confirmation dialog, which would replace the mode
        // this flow still needs to read `view` from.
        let si = self.create_agent_session_labeled(
            pi,
            fi,
            &label,
            harness,
            StartIntent::Warn("the fresh-context agent"),
        )?;

        self.selection = Selection::Session(pi, fi, si);
        self.enter_view_without_auto_compose()?;
        self.open_compose_seeded(prompt)?;
        Ok(())
    }
}

fn view_project_feature_indices(store: &ProjectStore, view: &ViewState) -> Option<(usize, usize)> {
    let pi = store
        .projects
        .iter()
        .position(|p| p.name == view.project_name)?;
    let fi = store.projects[pi]
        .features
        .iter()
        .position(|f| f.name == view.feature_name)?;
    Some((pi, fi))
}

fn fresh_context_session_label(feature: &Feature) -> String {
    if !feature
        .sessions
        .iter()
        .any(|s| s.label == FRESH_CONTEXT_LABEL)
    {
        return FRESH_CONTEXT_LABEL.to_string();
    }
    let mut n = 2;
    loop {
        let candidate = format!("{FRESH_CONTEXT_LABEL} {n}");
        if !feature.sessions.iter().any(|s| s.label == candidate) {
            return candidate;
        }
        n += 1;
    }
}

fn relative_display_path(workdir: &Path, path: &Path) -> String {
    path.strip_prefix(workdir)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// Changed/new files worth pointing a fresh session at: everything except
/// files that no longer exist on this branch.
fn changed_file_paths(snapshot: &DiffSnapshot) -> Vec<String> {
    snapshot
        .files
        .iter()
        .filter(|f| f.status != DiffFileStatus::Deleted)
        .map(|f| f.path.clone())
        .collect()
}

/// Clamp a free-text source (a feature summary, a prior prompt) to
/// [`MAX_PROMPT_SOURCE_CHARS`], appending an ellipsis when it had to be cut,
/// so one oversized field can't blow up the seeded prompt.
fn truncate_prompt_source(text: &str) -> String {
    if text.chars().count() <= MAX_PROMPT_SOURCE_CHARS {
        return text.to_string();
    }
    let kept: String = text.chars().take(MAX_PROMPT_SOURCE_CHARS).collect();
    format!("{}\u{2026}", kept.trim_end())
}

/// Build the fresh-context prompt per the brief's template, using the user's
/// own `instruction` in place of the brief's "(insert new prompt here)"
/// placeholder. Either input section is omitted when there's nothing to say
/// -- no plan file, or no changed files (e.g. a brand-new feature, or a
/// non-git project where the diff snapshot couldn't be loaded).
fn build_fresh_context_prompt(
    relative_plan: Option<&str>,
    changed_files: &[String],
    feature_summary: Option<&str>,
    latest_prompt: Option<&str>,
    instruction: &str,
) -> String {
    let mut prompt = String::new();
    if let Some(plan) = relative_plan {
        prompt.push_str(&format!("Read {plan} for full context on this feature. "));
    }
    if !changed_files.is_empty() {
        let listed = changed_files.len().min(MAX_CHANGED_FILES_LISTED);
        let mut files_line = changed_files[..listed].join(", ");
        if let Some(remaining) = changed_files.len().checked_sub(listed).filter(|n| *n > 0) {
            files_line.push_str(&format!(", and {remaining} more"));
        }
        prompt.push_str(&format!("Changed/new files to look at: {files_line}. "));
    }
    if let Some(summary) = feature_summary
        .map(str::trim)
        .filter(|summary| !summary.is_empty())
    {
        prompt.push_str(&format!(
            "Feature summary: {}. ",
            truncate_prompt_source(summary)
        ));
    }
    if let Some(latest_prompt) = latest_prompt
        .map(str::trim)
        .filter(|latest_prompt| !latest_prompt.is_empty())
    {
        prompt.push_str(&format!(
            "Latest known prompt: {}. ",
            truncate_prompt_source(latest_prompt)
        ));
    }
    prompt.push_str(instruction);
    prompt.push(' ');
    prompt.push_str(FRESH_CONTEXT_CLARIFY_ASK);
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::DiffFile;

    fn diff_file(path: &str, status: DiffFileStatus) -> DiffFile {
        DiffFile {
            old_path: None,
            path: path.to_string(),
            status,
            additions: 0,
            deletions: 0,
            is_binary: false,
            old_content: None,
            new_content: None,
            patch: String::new(),
            hunks: Vec::new(),
        }
    }

    fn snapshot(files: Vec<DiffFile>) -> DiffSnapshot {
        DiffSnapshot {
            branch: "feature".into(),
            base_ref: "main".into(),
            base_commit: "abc123".into(),
            files,
            total_additions: 0,
            total_deletions: 0,
        }
    }

    #[test]
    fn changed_file_paths_drops_deleted_files() {
        let snap = snapshot(vec![
            diff_file("src/added.rs", DiffFileStatus::Added),
            diff_file("src/gone.rs", DiffFileStatus::Deleted),
            diff_file("src/modified.rs", DiffFileStatus::Modified),
            diff_file("notes.md", DiffFileStatus::Untracked),
        ]);

        assert_eq!(
            changed_file_paths(&snap),
            vec![
                "src/added.rs".to_string(),
                "src/modified.rs".to_string(),
                "notes.md".to_string(),
            ]
        );
    }

    #[test]
    fn prompt_includes_plan_and_changed_files_when_both_present() {
        let prompt = build_fresh_context_prompt(
            Some("AMF_PLAN.md"),
            &["src/foo.rs".to_string(), "src/bar.rs".to_string()],
            None,
            None,
            "Fix the login bug.",
        );

        assert_eq!(
            prompt,
            "Read AMF_PLAN.md for full context on this feature. \
             Changed/new files to look at: src/foo.rs, src/bar.rs. \
             Fix the login bug. \
             Grill me with any questions to clarify before implementing"
        );
    }

    #[test]
    fn prompt_omits_plan_line_when_no_plan_file_exists() {
        let prompt = build_fresh_context_prompt(
            None,
            &["src/foo.rs".to_string()],
            None,
            None,
            "Fix the login bug.",
        );

        assert_eq!(
            prompt,
            "Changed/new files to look at: src/foo.rs. \
             Fix the login bug. \
             Grill me with any questions to clarify before implementing"
        );
    }

    #[test]
    fn prompt_omits_changed_files_line_when_there_are_none() {
        let prompt =
            build_fresh_context_prompt(Some("AMF_PLAN.md"), &[], None, None, "Fix the login bug.");

        assert_eq!(
            prompt,
            "Read AMF_PLAN.md for full context on this feature. \
             Fix the login bug. \
             Grill me with any questions to clarify before implementing"
        );
    }

    #[test]
    fn continuation_prompt_includes_summary_and_latest_prompt_sources() {
        let prompt = build_fresh_context_prompt(
            Some("docs/plan.md"),
            &["src/main.rs".to_string()],
            Some("Finish the sidebar work."),
            Some("Add the context hint."),
            FRESH_CONTEXT_CONTINUATION_INSTRUCTION,
        );

        assert!(prompt.contains("Read docs/plan.md for full context on this feature."));
        assert!(prompt.contains("Changed/new files to look at: src/main.rs."));
        assert!(prompt.contains("Feature summary: Finish the sidebar work."));
        assert!(prompt.contains("Latest known prompt: Add the context hint."));
        assert!(prompt.contains(FRESH_CONTEXT_CONTINUATION_INSTRUCTION));
    }

    #[test]
    fn continuation_prompt_gracefully_omits_unavailable_sources() {
        let prompt = build_fresh_context_prompt(
            None,
            &[],
            Some("  "),
            Some("\n"),
            FRESH_CONTEXT_CONTINUATION_INSTRUCTION,
        );

        assert_eq!(
            prompt,
            format!("{FRESH_CONTEXT_CONTINUATION_INSTRUCTION} {FRESH_CONTEXT_CLARIFY_ASK}")
        );
    }

    #[test]
    fn changed_files_list_is_capped_with_an_and_more_suffix() {
        let files: Vec<String> = (0..50).map(|n| format!("src/file_{n}.rs")).collect();
        let prompt = build_fresh_context_prompt(None, &files, None, None, "Continue.");

        assert!(prompt.contains("src/file_0.rs"));
        assert!(prompt.contains(&format!(
            "src/file_{}.rs, and 30 more. ",
            MAX_CHANGED_FILES_LISTED - 1
        )));
        assert!(!prompt.contains("src/file_20.rs"));
    }

    #[test]
    fn oversized_summary_and_latest_prompt_are_truncated() {
        let long_summary = "s".repeat(MAX_PROMPT_SOURCE_CHARS + 200);
        let long_prompt = "p".repeat(MAX_PROMPT_SOURCE_CHARS + 200);
        let prompt = build_fresh_context_prompt(
            None,
            &[],
            Some(&long_summary),
            Some(&long_prompt),
            FRESH_CONTEXT_CONTINUATION_INSTRUCTION,
        );

        // Each source contributes at most the cap plus the ellipsis, so the
        // whole seed stays within a bounded envelope rather than scaling with
        // the inputs.
        assert!(prompt.contains(&format!("{}\u{2026}", "s".repeat(MAX_PROMPT_SOURCE_CHARS))));
        assert!(prompt.contains(&format!("{}\u{2026}", "p".repeat(MAX_PROMPT_SOURCE_CHARS))));
        assert!(prompt.len() < 2 * (MAX_PROMPT_SOURCE_CHARS + 64) + 512);
    }

    #[test]
    fn prompt_is_just_the_instruction_and_clarify_ask_with_no_plan_and_no_changed_files() {
        // Covers both "not a git repo" (load_snapshot errors, so the caller
        // passes an empty slice) and a brand-new feature with nothing changed
        // yet.
        let prompt = build_fresh_context_prompt(None, &[], None, None, "Fix the login bug.");

        assert_eq!(
            prompt,
            "Fix the login bug. Grill me with any questions to clarify before implementing"
        );
    }

    #[test]
    fn fresh_context_session_label_numbers_upward_on_repeat_presses() {
        let mut feature = Feature::new(
            "feature".into(),
            "feature/handoff".into(),
            std::path::PathBuf::from("/tmp/handoff"),
            true,
            VibeMode::Vibeless,
            false,
            false,
            AgentKind::Codex,
            false,
            false,
        );
        assert_eq!(fresh_context_session_label(&feature), "Fresh Context");

        feature.add_session_named(SessionKind::Codex, "Fresh Context".into());
        assert_eq!(fresh_context_session_label(&feature), "Fresh Context 2");

        feature.add_session_named(SessionKind::Codex, "Fresh Context 2".into());
        assert_eq!(fresh_context_session_label(&feature), "Fresh Context 3");
    }

    #[test]
    fn relative_display_path_strips_the_workdir_prefix() {
        let workdir = Path::new("/repo/worktree");
        let plan = Path::new("/repo/worktree/AMF_PLAN.md");

        assert_eq!(relative_display_path(workdir, plan), "AMF_PLAN.md");
    }
}
