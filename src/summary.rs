use anyhow::Result;
use std::path::Path;

use crate::headless::HeadlessRunner;
use crate::project::AgentKind;
use crate::prompts::{PromptContext, render_template};
use crate::tmux::TmuxManager;

const SUMMARY_MAX_CHARS: usize = 60;
const MIN_CONTENT_LINES: usize = 5;

pub struct SummaryManager;

impl SummaryManager {
    /// `template` is the resolved `session.summary` prompt template (built-in
    /// default or a feature/project/global override), obtained on the UI
    /// thread via [`crate::app::App::resolve_headless_template`]; this runs on
    /// a worker thread and only interpolates it.
    pub fn generate_summary(
        tmux_session: &str,
        window: &str,
        workdir: &Path,
        agent: AgentKind,
        template: &str,
    ) -> Result<String> {
        let content = TmuxManager::capture_pane(tmux_session, window)?;

        summarize_content_with(&content, workdir, &agent, template, HeadlessRunner::run)
    }
}

fn summarize_content_with(
    content: &str,
    workdir: &Path,
    agent: &AgentKind,
    template: &str,
    run_headless: impl FnOnce(&AgentKind, &Path, &str, Option<&str>, bool) -> Result<String>,
) -> Result<String> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.len() < MIN_CONTENT_LINES {
        anyhow::bail!("Content too short for summary");
    }

    let recent_lines: String = lines[lines.len().saturating_sub(50)..].join("\n");

    let prompt = render_template(
        template,
        &PromptContext::new()
            .with("harness_name", agent.display_name())
            .with("max_chars", SUMMARY_MAX_CHARS.to_string())
            .with("recent_lines", recent_lines),
    );

    let summary = run_headless(agent, workdir, &prompt, None, true)?;

    let trimmed = summary.trim().lines().next().unwrap_or("").to_string();

    let truncated = if trimmed.len() > SUMMARY_MAX_CHARS {
        let mut end = SUMMARY_MAX_CHARS;
        while end > 0 && !trimmed.is_char_boundary(end) {
            end -= 1;
        }
        trimmed[..end].to_string()
    } else {
        trimmed
    };

    Ok(truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_uses_the_features_harness_in_restricted_headless_mode() {
        let content = (1..=MIN_CONTENT_LINES)
            .map(|line| format!("session line {line}"))
            .collect::<Vec<_>>()
            .join("\n");

        let summary = summarize_content_with(
            &content,
            Path::new("/tmp/feature"),
            &AgentKind::Codex,
            crate::prompts::PromptId::SessionSummary
                .spec()
                .default_template,
            |agent, workdir, prompt, model, restricted| {
                assert_eq!(agent, &AgentKind::Codex);
                assert_eq!(workdir, Path::new("/tmp/feature"));
                assert_eq!(model, None);
                assert!(restricted);
                assert!(prompt.contains("Summarize this Codex session"));
                assert!(prompt.contains("session line 5"));
                Ok("Implemented harness-aware summaries\nignored second line".into())
            },
        )
        .unwrap();

        assert_eq!(summary, "Implemented harness-aware summaries");
    }

    #[test]
    fn summary_renders_an_override_template_with_the_same_context_tokens() {
        let content = ["session line"; MIN_CONTENT_LINES].join("\n");
        let summary = summarize_content_with(
            &content,
            Path::new("/tmp/feature"),
            &AgentKind::Claude,
            "ONE LINE ONLY for {{harness_name}} <= {{max_chars}}:\n{{recent_lines}}",
            |_, _, prompt, _, _| {
                assert!(prompt.starts_with("ONE LINE ONLY for Claude <= 60:"));
                assert!(prompt.contains("session line"));
                Ok("ok".into())
            },
        )
        .unwrap();
        assert_eq!(summary, "ok");
    }

    #[test]
    fn summary_truncation_preserves_utf8_boundaries() {
        let content = ["line"; MIN_CONTENT_LINES].join("\n");
        let generated = format!("{}é", "a".repeat(SUMMARY_MAX_CHARS - 1));

        let summary = summarize_content_with(
            &content,
            Path::new("/tmp/feature"),
            &AgentKind::Pi,
            crate::prompts::PromptId::SessionSummary
                .spec()
                .default_template,
            |_, _, _, _, _| Ok(generated),
        )
        .unwrap();

        assert_eq!(summary, "a".repeat(SUMMARY_MAX_CHARS - 1));
    }
}
