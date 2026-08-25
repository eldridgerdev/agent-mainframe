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

impl App {
    /// Open the fresh-context instruction prompt over the current session
    /// view (`Ctrl+Space` then `Shift+F`). Collecting the instruction first
    /// means the seeded prompt is complete when the new session opens,
    /// rather than asking the user to type over a placeholder there.
    pub fn open_fresh_context_prompt_from_view(&mut self) {
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
            input: String::new(),
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
        let (view, instruction) = match &self.mode {
            AppMode::FreshContextPrompt(state) => {
                (state.view.clone(), state.input.trim().to_string())
            }
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

        let prompt =
            build_fresh_context_prompt(relative_plan.as_deref(), &changed_files, &instruction);

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
    path.strip_prefix(workdir).unwrap_or(path).display().to_string()
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

/// Build the fresh-context prompt per the brief's template, using the user's
/// own `instruction` in place of the brief's "(insert new prompt here)"
/// placeholder. Either input section is omitted when there's nothing to say
/// -- no plan file, or no changed files (e.g. a brand-new feature, or a
/// non-git project where the diff snapshot couldn't be loaded).
fn build_fresh_context_prompt(
    relative_plan: Option<&str>,
    changed_files: &[String],
    instruction: &str,
) -> String {
    let mut prompt = String::new();
    if let Some(plan) = relative_plan {
        prompt.push_str(&format!("Read {plan} for full context on this feature. "));
    }
    if !changed_files.is_empty() {
        prompt.push_str(&format!(
            "Changed/new files to look at: {}. ",
            changed_files.join(", ")
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
        let prompt =
            build_fresh_context_prompt(None, &["src/foo.rs".to_string()], "Fix the login bug.");

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
            build_fresh_context_prompt(Some("AMF_PLAN.md"), &[], "Fix the login bug.");

        assert_eq!(
            prompt,
            "Read AMF_PLAN.md for full context on this feature. \
             Fix the login bug. \
             Grill me with any questions to clarify before implementing"
        );
    }

    #[test]
    fn prompt_is_just_the_instruction_and_clarify_ask_with_no_plan_and_no_changed_files() {
        // Covers both "not a git repo" (load_snapshot errors, so the caller
        // passes an empty slice) and a brand-new feature with nothing changed
        // yet.
        let prompt = build_fresh_context_prompt(None, &[], "Fix the login bug.");

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
