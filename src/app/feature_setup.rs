//! Shared state and pure transitions for compact feature setup dialogs.
//!
//! PR Triage and Final Review both open a small settings dialog before making
//! an isolated companion feature.  Keep the state transitions here so another
//! GitHub-backed workflow (such as issue fixing) can use the same dialog
//! contract without copying the PR-specific orchestration.

use crate::extension::FeaturePreset;
use crate::project::{AgentKind, VibeMode};

/// One editable row in a compact companion-feature setup dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureSetupRow {
    /// Apply a configured feature preset (or "Manual", which changes nothing).
    Preset,
    /// Which agent harness the feature runs.
    Harness,
    /// The feature's permission/vibe mode.
    Mode,
    /// Review mode (developer notes on every change).
    Review,
    /// Chrome/browser automation.
    Chrome,
    /// The branch name. Pre-filled and editable.
    Branch,
}

impl FeatureSetupRow {
    pub const ALL: [Self; 6] = [
        Self::Preset,
        Self::Harness,
        Self::Mode,
        Self::Review,
        Self::Chrome,
        Self::Branch,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Preset => "Preset",
            Self::Harness => "Harness",
            Self::Mode => "Vibe mode",
            Self::Review => "Review mode",
            Self::Chrome => "Chrome",
            Self::Branch => "Branch",
        }
    }
}

/// State shared by the compact setup dialogs used by PR Triage and Final
/// Review. `pending_batch` is retained for PR Triage's batch continuation; a
/// caller that has no batch operation leaves it false.
#[derive(Debug, Clone)]
pub struct FeatureSetupState {
    /// Index 0 is the implicit "Manual" choice; preset `i` is at `i + 1`.
    pub presets: Vec<FeaturePreset>,
    pub preset_index: usize,
    /// Harnesses allowed for the selected project.
    pub agents: Vec<AgentKind>,
    pub agent_index: usize,
    pub mode: VibeMode,
    pub review: bool,
    pub enable_chrome: bool,
    /// The isolated companion branch, pre-filled by the owning workflow.
    pub branch: String,
    /// Focused row index.
    pub row: usize,
    /// Inline validation/creation error.
    pub error: Option<String>,
    /// PR Triage's combined-batch continuation flag.
    pub pending_batch: bool,
}

impl FeatureSetupState {
    /// The chosen preset, or `None` for "Manual".
    pub fn selected_preset(&self) -> Option<&FeaturePreset> {
        self.preset_index
            .checked_sub(1)
            .and_then(|i| self.presets.get(i))
    }

    /// Display text for the preset row.
    pub fn preset_label(&self) -> String {
        self.selected_preset()
            .map_or_else(|| "Manual".to_string(), |preset| preset.name.clone())
    }

    /// The focused row, or `Branch` if state loaded from an older/invalid
    /// snapshot contains an out-of-range cursor.
    pub fn focused_row(&self) -> FeatureSetupRow {
        FeatureSetupRow::ALL
            .get(self.row)
            .copied()
            .unwrap_or(FeatureSetupRow::Branch)
    }

    pub fn agent(&self) -> AgentKind {
        self.agents
            .get(self.agent_index)
            .cloned()
            .unwrap_or_default()
    }

    /// Move the focused row, wrapping at either end.
    pub fn move_row(&mut self, delta: isize) {
        let len = FeatureSetupRow::ALL.len() as isize;
        self.row = ((self.row as isize + delta).rem_euclid(len)) as usize;
    }

    /// Adjust the focused setting. Returns the newly selected preset so the
    /// caller can apply it to the dependent rows without knowing about the
    /// dialog's ownership or `AppMode`.
    pub fn adjust(&mut self, delta: isize) -> Option<FeaturePreset> {
        self.error = None;
        let applied = match self.focused_row() {
            FeatureSetupRow::Preset => {
                let len = self.presets.len() as isize + 1;
                self.preset_index = ((self.preset_index as isize + delta).rem_euclid(len)) as usize;
                self.selected_preset().cloned()
            }
            FeatureSetupRow::Harness => {
                let len = self.agents.len().max(1) as isize;
                self.agent_index = ((self.agent_index as isize + delta).rem_euclid(len)) as usize;
                None
            }
            FeatureSetupRow::Mode => {
                let all = VibeMode::ALL;
                let current = all.iter().position(|mode| *mode == self.mode).unwrap_or(0);
                self.mode =
                    all[(current as isize + delta).rem_euclid(all.len() as isize) as usize].clone();
                None
            }
            FeatureSetupRow::Review => {
                self.review = !self.review;
                None
            }
            FeatureSetupRow::Chrome => {
                self.enable_chrome = !self.enable_chrome;
                None
            }
            FeatureSetupRow::Branch => None,
        };
        if let Some(preset) = &applied {
            self.apply_preset(preset);
        }
        applied
    }

    /// Apply a preset's settings while keeping the branch's meaningful suffix.
    pub fn apply_preset(&mut self, preset: &FeaturePreset) {
        if let Some(index) = self.agents.iter().position(|agent| *agent == preset.agent) {
            self.agent_index = index;
        }
        self.mode = preset.mode.clone();
        self.review = preset.review;
        self.enable_chrome = preset.enable_chrome;
        if let Some(prefix) = &preset.branch_prefix {
            let base = self
                .branch
                .rsplit('/')
                .next()
                .unwrap_or(&self.branch)
                .to_string();
            self.branch = format!("{prefix}{base}");
        }
    }

    pub fn on_branch_row(&self) -> bool {
        self.focused_row() == FeatureSetupRow::Branch
    }

    pub fn branch_push(&mut self, c: char) {
        if self.on_branch_row() {
            self.error = None;
            self.branch.push(c);
        }
    }

    pub fn branch_backspace(&mut self) {
        if self.on_branch_row() {
            self.error = None;
            self.branch.pop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> FeatureSetupState {
        FeatureSetupState {
            presets: Vec::new(),
            preset_index: 0,
            agents: vec![AgentKind::Claude],
            agent_index: 0,
            mode: VibeMode::Vibeless,
            review: false,
            enable_chrome: false,
            branch: "feature".to_string(),
            row: 0,
            error: Some("old error".to_string()),
            pending_batch: false,
        }
    }

    #[test]
    fn shared_setup_transitions_match_the_compact_dialog_contract() {
        let mut state = setup();
        state.move_row(-1);
        assert_eq!(state.focused_row(), FeatureSetupRow::Branch);
        state.branch_push('x');
        assert_eq!(state.branch, "featurex");
        state.branch_backspace();
        assert_eq!(state.branch, "feature");

        state.row = 2;
        assert_eq!(state.focused_row(), FeatureSetupRow::Mode);
        state.adjust(1);
        assert_eq!(state.mode, VibeMode::Vibe);
        assert!(state.error.is_none());
    }
}
