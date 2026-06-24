//! The agent-skill picker for prompt-editing surfaces. Launched with a
//! hotkey from the prompt editor body or a text fill field, it lists the
//! workspace's available skills and, on selection, inserts the chosen
//! skill's `/skill-name` invocation at the editor cursor. The skill list is
//! the same one the compose `/command` popup draws from (global + project
//! `.claude/skills`), so what you can invoke in a session you can inject here.

use crate::app::state::{AppMode, SkillPickerState};
use crate::app::App;

impl App {
    /// Open the skill picker over the current prompt-editing mode. The active
    /// `PromptEditor` / `PlaceholderFill` is stashed as `return_to` so the
    /// cursor and buffer are preserved across the picker. No-op (with a hint)
    /// when no skills are installed.
    pub fn open_skill_picker(&mut self) {
        // Only meaningful over an editing surface that owns a text editor.
        if !matches!(
            self.mode,
            AppMode::PromptEditor(_) | AppMode::PlaceholderFill(_)
        ) {
            return;
        }

        let skills = self.available_skills();
        if skills.is_empty() {
            self.push_toast_warning("No agent skills found in ~/.claude/skills or .claude/skills");
            return;
        }

        let return_to = std::mem::replace(&mut self.mode, AppMode::Normal);
        let filtered = (0..skills.len()).collect();
        self.mode = AppMode::SkillPicker(SkillPickerState {
            skills,
            filtered,
            query: String::new(),
            selected: 0,
            return_to: Box::new(return_to),
        });
    }

    /// The workspace's available skills: global plus the selected feature's
    /// project skills, deduped by name (project wins). Global skills are
    /// always included even without a project context.
    fn available_skills(&self) -> Vec<crate::app::state::ComposeCommandEntry> {
        let workdir = self.resolve_skill_workdir();
        crate::app::compose::build_skill_catalog(workdir.as_deref())
    }

    /// Best-effort project directory to scan for `.claude/skills`: the
    /// selected feature's worktree, if any. `None` falls back to global-only.
    fn resolve_skill_workdir(&self) -> Option<std::path::PathBuf> {
        self.selected_feature().map(|(_, f)| f.workdir.clone())
    }

    /// Re-rank the skill list against the current query (fuzzy, best first).
    /// An empty query lists every skill in name order. Keeps the highlight on
    /// the top match.
    pub fn skill_picker_filter(&mut self) {
        if let AppMode::SkillPicker(state) = &mut self.mode {
            let query = state.query.trim();
            if query.is_empty() {
                state.filtered = (0..state.skills.len()).collect();
            } else {
                let mut scored: Vec<(i32, usize)> = state
                    .skills
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, skill)| {
                        crate::app::compose::fuzzy_score(query, &skill.name).map(|score| (score, idx))
                    })
                    .collect();
                // Highest score first; ties keep name order (already sorted).
                scored.sort_by(|a, b| b.0.cmp(&a.0));
                state.filtered = scored.into_iter().map(|(_, idx)| idx).collect();
            }
            state.selected = 0;
        }
    }

    /// Move the highlight down one (wrapping).
    pub fn skill_picker_select_next(&mut self) {
        if let AppMode::SkillPicker(state) = &mut self.mode
            && !state.filtered.is_empty()
        {
            state.selected = (state.selected + 1) % state.filtered.len();
        }
    }

    /// Move the highlight up one (wrapping).
    pub fn skill_picker_select_prev(&mut self) {
        if let AppMode::SkillPicker(state) = &mut self.mode
            && !state.filtered.is_empty()
        {
            state.selected = state
                .selected
                .checked_sub(1)
                .unwrap_or(state.filtered.len() - 1);
        }
    }

    /// Insert the highlighted skill's `/skill-name` invocation at the editor
    /// cursor of the surface the picker was opened from, then return to it.
    /// The agent expands the skill when the prompt is delivered, so injecting
    /// the invocation token keeps templates compact and never goes stale.
    pub fn insert_selected_skill(&mut self) {
        let (name, return_to) = match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::SkillPicker(state) => {
                let name = state.selected_skill().map(|s| s.name.clone());
                (name, state.return_to)
            }
            other => {
                self.mode = other;
                return;
            }
        };

        let mut return_to = return_to;
        if let Some(name) = name {
            let token = format!("/{name} ");
            match return_to.as_mut() {
                AppMode::PromptEditor(editor) => {
                    editor.editor.insert_str(&token);
                }
                AppMode::PlaceholderFill(fill) => {
                    fill.input.insert_str(&token);
                }
                _ => {}
            }
            self.push_toast_success(format!("Inserted /{name}"));
        }
        self.mode = *return_to;
    }

    /// Close the picker without inserting, restoring the editing surface.
    pub fn cancel_skill_picker(&mut self) {
        if let AppMode::SkillPicker(state) =
            std::mem::replace(&mut self.mode, AppMode::Normal)
        {
            self.mode = *state.return_to;
        }
    }
}
