//! Shared preparation for the TUI and desktop supervised-edit explanation.
use std::path::Path;

use super::{App, ReviewAction};
use crate::headless::ReasoningLevel;
use crate::project::AgentKind;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct ExplanationCall {
    pub prompt: String,
    pub model: Option<String>,
    pub reasoning: Option<ReasoningLevel>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ExplanationPreparation {
    SavedNote(String),
    Call(ExplanationCall),
}

pub(crate) fn find_review_note(workdir: &Path, relative_path: &str) -> Option<String> {
    super::review::load_review_notes_for_explanation(workdir).remove(relative_path)
}

impl App {
    pub(crate) fn prepare_edit_explanation(
        &self,
        workdir: &Path,
        relative_path: &str,
        old_snippet: &str,
        new_snippet: &str,
    ) -> ExplanationPreparation {
        self.prepare_edit_explanation_with_config(
            workdir,
            relative_path,
            old_snippet,
            new_snippet,
            &self.config,
        )
    }

    pub(crate) fn prepare_edit_explanation_with_config(
        &self,
        workdir: &Path,
        relative_path: &str,
        old_snippet: &str,
        new_snippet: &str,
        config: &super::AppConfig,
    ) -> ExplanationPreparation {
        if let Some(note) = find_review_note(workdir, relative_path) {
            return ExplanationPreparation::SavedNote(note.trim().to_owned());
        }
        let repo = crate::worktree::WorktreeManager::repo_root(workdir)
            .unwrap_or_else(|_| workdir.to_owned());
        let ctx = crate::prompts::PromptContext::new()
            .with("file_path", relative_path)
            .with("old_snippet", old_snippet)
            .with("new_snippet", new_snippet);
        ExplanationPreparation::Call(ExplanationCall {
            prompt: self.resolve_headless_prompt(
                crate::prompts::PromptId::ReviewDiffExplain,
                &AgentKind::Claude,
                &repo,
                workdir,
                &ctx,
            ),
            model: config.review_model_for(ReviewAction::DiffExplain),
            reasoning: config.review_reasoning_for(ReviewAction::DiffExplain),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::prompt_overrides::OverrideScope;

    #[test]
    fn preparation_uses_shared_override_precedence_snippets_and_review_model_effort() {
        let (dir, mut gui, _) = crate::gui_diff::tests::fixture();
        let repo = dir.path().join("repo");
        let app = gui.app_for_workflow();
        app.config
            .review_models
            .insert("diff_explain".into(), "model-override".into());
        app.config
            .review_reasonings
            .insert("diff_explain".into(), ReasoningLevel::High);
        let db = app.db.as_ref().unwrap();
        db.upsert_prompt_override(
            "review.diff_explain",
            &OverrideScope::Global,
            Some(&AgentKind::Claude),
            "global {{file_path}} {{old_snippet}} {{new_snippet}}",
        )
        .unwrap();
        let call = |app: &App| match app.prepare_edit_explanation(&repo, "code.txt", "old", "new") {
            ExplanationPreparation::Call(call) => call,
            other => panic!("{other:?}"),
        };
        let global = call(app);
        assert_eq!(global.prompt, "global code.txt old new");
        assert_eq!(global.model.as_deref(), Some("model-override"));
        assert_eq!(global.reasoning, Some(ReasoningLevel::High));
        std::fs::write(
            repo.join("amf.json"),
            r#"{"prompt_overrides":{"review.diff_explain":{"template":"project {{file_path}}"}}}"#,
        )
        .unwrap();
        assert_eq!(call(app).prompt, "project code.txt");
        db.upsert_prompt_override(
            "review.diff_explain",
            &OverrideScope::Feature {
                workdir: repo.to_str().unwrap().into(),
            },
            Some(&AgentKind::Claude),
            "feature {{new_snippet}}",
        )
        .unwrap();
        assert_eq!(call(app).prompt, "feature new");
    }
}
