use anyhow::Result;
use std::path::Path;

use crate::claude::ClaudeLauncher;
use crate::project::AgentKind;
use crate::tmux::TmuxManager;

const SUMMARY_MAX_CHARS: usize = 60;
const MIN_CONTENT_LINES: usize = 5;

pub struct SummaryManager;

impl SummaryManager {
    pub fn generate_summary(
        tmux_session: &str,
        window: &str,
        workdir: &Path,
        agent: AgentKind,
    ) -> Result<String> {
        let content = TmuxManager::capture_pane(tmux_session, window)?;

        let lines: Vec<&str> = content.lines().collect();
        if lines.len() < MIN_CONTENT_LINES {
            anyhow::bail!("Content too short for summary");
        }

        let recent_lines: String = lines[lines.len().saturating_sub(50)..].join("\n");

        let prompt = format!(
            "Summarize this {} session in one line (max {} chars). \
             Focus on what was done or what's blocking. \
             Be concise and specific. \
             Example: 'Refactored auth module, waiting on test fix'\n\n\
             Session output:\n{}",
            agent.display_name(),
            SUMMARY_MAX_CHARS,
            recent_lines
        );

        let summary = ClaudeLauncher::run_headless(workdir, &prompt)?;

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
}
