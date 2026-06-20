use std::path::Path;

use anyhow::Result;

use super::commands::scan_commands_recursive;
use super::*;

/// Built-in Claude Code slash commands. `interactive` marks commands
/// that open a CC-owned dialog the user must drive directly, so
/// submitting one drops the session into direct (passthrough) mode.
const BUILTIN_COMMANDS: &[(&str, &str, bool)] = &[
    ("add-dir", "Add a new working directory", false),
    ("agents", "Manage agent configurations", true),
    ("bashes", "List and manage background tasks", true),
    ("clear", "Clear conversation history and free up context", false),
    ("compact", "Summarize the conversation to free up context", false),
    ("config", "Open the settings panel", true),
    ("context", "Show current context usage", false),
    ("cost", "Show token usage and cost for this session", false),
    ("doctor", "Check the health of the Claude Code installation", false),
    ("exit", "Exit Claude Code", false),
    ("export", "Export the conversation", false),
    ("help", "Show available commands", false),
    ("hooks", "Manage hook configurations", true),
    ("ide", "Manage IDE integrations", true),
    ("init", "Generate a CLAUDE.md for this repository", false),
    ("login", "Sign in to your Anthropic account", true),
    ("logout", "Sign out of your Anthropic account", false),
    ("mcp", "Manage MCP server connections", true),
    ("memory", "Edit memory files", true),
    ("model", "Choose the model for this session", true),
    ("output-style", "Choose the output style", true),
    ("permissions", "Manage tool permissions", true),
    ("pr-comments", "Show comments on the current pull request", false),
    ("resume", "Resume a previous session", true),
    ("review", "Review a pull request", false),
    ("rewind", "Rewind the conversation", true),
    ("status", "Show Claude Code status", true),
    ("statusline", "Configure the status line", false),
    ("terminal-setup", "Configure terminal key bindings", false),
    ("todos", "List current todo items", false),
    ("usage", "Show plan usage limits", true),
    ("vim", "Toggle vim editing mode", false),
];

/// How long Claude Code gets to read an image off the clipboard after
/// the forwarded Ctrl+V, before the clipboard may be overwritten by
/// the next image in the same submission.
const COMPOSE_IMAGE_PASTE_DELAY: std::time::Duration = std::time::Duration::from_millis(350);

pub(crate) fn compose_target_key(session: &str, window: &str) -> String {
    format!("{session}:{window}")
}

/// Characters that begin a new "word" inside a command name, so a
/// query character landing right after one is a strong match. Covers
/// namespace separators (`stn:commit`) and the usual word boundaries.
fn is_command_boundary(c: char) -> bool {
    matches!(c, ':' | '-' | '_' | '/' | ' ' | '.')
}

/// Fuzzy-match `query` against `candidate`, both compared case-
/// insensitively. Returns a score (higher is better) when every
/// character of `query` appears in order within `candidate`, or `None`
/// when it does not. An empty query matches everything neutrally.
///
/// Scoring favors matches at the start of the name or just after a
/// namespace/word boundary and rewards consecutive runs, so typing
/// `commit` surfaces `stn:commit` while still ranking a literal
/// `commit` command above it.
pub(crate) fn fuzzy_score(query: &str, candidate: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }

    let query_lower: Vec<char> = query
        .chars()
        .flat_map(|c| c.to_lowercase())
        .filter(|c| !c.is_whitespace())
        .collect();
    if query_lower.is_empty() {
        return Some(0);
    }

    let candidate_chars: Vec<char> = candidate.chars().collect();
    let mut score = 0i32;
    let mut q = 0usize;
    let mut prev_match: Option<usize> = None;

    for (idx, ch) in candidate_chars.iter().enumerate() {
        if q >= query_lower.len() {
            break;
        }
        let cand_lower = ch.to_lowercase().next().unwrap_or(*ch);
        if cand_lower != query_lower[q] {
            continue;
        }

        score += 1;
        if idx == 0 {
            score += 8;
        } else if is_command_boundary(candidate_chars[idx - 1]) {
            score += 6;
        }
        if idx > 0 && prev_match == Some(idx - 1) {
            score += 4;
        }

        prev_match = Some(idx);
        q += 1;
    }

    if q == query_lower.len() {
        // Prefer tighter candidates so closer matches sort first.
        Some(score - (candidate_chars.len() as i32) / 8)
    } else {
        None
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ComposePart {
    Text(String),
    /// Index into `ComposeState::images`.
    Image(usize),
}

/// Split the compose buffer into text segments and image placeholders
/// in display order. Placeholders the user deleted simply do not
/// appear; placeholder-free text yields a single text part.
pub(crate) fn split_compose_parts(
    text: &str,
    images: &[crate::app::ComposeImage],
) -> Vec<ComposePart> {
    let mut parts = Vec::new();
    let mut rest = text;

    loop {
        let next = images
            .iter()
            .enumerate()
            .filter_map(|(idx, image)| rest.find(&image.placeholder).map(|pos| (pos, idx)))
            .min_by_key(|(pos, _)| *pos);

        let Some((pos, idx)) = next else {
            if !rest.is_empty() {
                parts.push(ComposePart::Text(rest.to_string()));
            }
            return parts;
        };

        let before = &rest[..pos];
        if !before.is_empty() {
            parts.push(ComposePart::Text(before.to_string()));
        }
        parts.push(ComposePart::Image(idx));
        rest = &rest[pos + images[idx].placeholder.len()..];
    }
}

impl App {
    /// Whether typing in this Claude view should open the compose box
    /// instead of forwarding keystrokes to tmux.
    pub fn compose_intercept_active(&self, view: &ViewState) -> bool {
        view.session_kind == SessionKind::Claude
            && !self
                .compose_direct_targets
                .contains(&compose_target_key(&view.session, &view.window))
    }

    /// Toggle compose interception for the currently viewed session.
    pub fn toggle_compose_intercept(&mut self) {
        let (session, window, kind) = match &self.mode {
            AppMode::Viewing(view) => (
                view.session.clone(),
                view.window.clone(),
                view.session_kind.clone(),
            ),
            _ => return,
        };

        if kind != SessionKind::Claude {
            self.push_toast_warning("Compose is only for Claude Code sessions");
            return;
        }

        let key = compose_target_key(&session, &window);
        if self.compose_direct_targets.remove(&key) {
            self.push_toast_success("Composer on — typing opens the compose box");
        } else {
            self.compose_direct_targets.insert(key);
            self.push_toast_warning("Composer off — leader+e to re-enable");
        }
    }

    /// Open the compose box over the current view, optionally seeded
    /// with the character that triggered the interception. Any saved
    /// draft for this session is restored first.
    pub fn open_compose_from_view(&mut self, seed: Option<char>) -> Result<()> {
        let view = match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::Viewing(view) => view,
            other => {
                self.mode = other;
                return Ok(());
            }
        };

        let workdir = self
            .store
            .projects
            .iter()
            .find(|project| project.name == view.project_name)
            .and_then(|project| {
                project
                    .features
                    .iter()
                    .find(|feature| feature.name == view.feature_name)
            })
            .map(|feature| feature.workdir.clone());

        let Some(workdir) = workdir else {
            self.mode = AppMode::Viewing(view);
            self.message = Some("Error: Could not resolve feature workdir".into());
            return Ok(());
        };

        let key = compose_target_key(&view.session, &view.window);
        let draft = self.compose_drafts.get(&key).cloned().unwrap_or_default();
        let mut text = draft.text;
        if let Some(c) = seed {
            text.push(c);
        }

        let catalog = build_compose_catalog(&workdir);
        let mut state = ComposeState::new(view, workdir, text, catalog);
        state.images = draft.images;
        self.mode = AppMode::Compose(state);
        Ok(())
    }

    /// Open the compose box over the current Viewing mode seeded with
    /// the given text. Used by the prompt library to inject a template
    /// for review before sending. Any saved draft's images are kept; the
    /// seed replaces the draft text.
    pub fn open_compose_seeded(&mut self, text: String) -> Result<()> {
        let view = match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::Viewing(view) => view,
            other => {
                self.mode = other;
                return Ok(());
            }
        };

        let workdir = self
            .store
            .projects
            .iter()
            .find(|project| project.name == view.project_name)
            .and_then(|project| {
                project
                    .features
                    .iter()
                    .find(|feature| feature.name == view.feature_name)
            })
            .map(|feature| feature.workdir.clone());

        let Some(workdir) = workdir else {
            self.mode = AppMode::Viewing(view);
            self.message = Some("Error: Could not resolve feature workdir".into());
            return Ok(());
        };

        let key = compose_target_key(&view.session, &view.window);
        let draft = self.compose_drafts.get(&key).cloned().unwrap_or_default();
        let catalog = build_compose_catalog(&workdir);
        let mut state = ComposeState::new(view, workdir, text, catalog);
        state.images = draft.images;
        self.mode = AppMode::Compose(state);
        self.push_toast_success("Prompt loaded — review and send");
        Ok(())
    }

    /// Escape hatch from inside the composer: close it (draft kept)
    /// and disable interception for this session until `leader+e`
    /// turns it back on.
    pub fn compose_switch_to_direct(&mut self) {
        let target = match &self.mode {
            AppMode::Compose(state) => {
                compose_target_key(&state.view.session, &state.view.window)
            }
            _ => return,
        };
        self.cancel_compose();
        self.compose_direct_targets.insert(target);
        self.push_toast_warning("Composer off — leader+e to re-enable");
    }

    /// Close the compose box, keeping any unsent text as a draft for
    /// the session so the next keystroke restores it.
    pub fn cancel_compose(&mut self) {
        let state = match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::Compose(state) => state,
            other => {
                self.mode = other;
                return;
            }
        };

        let key = compose_target_key(&state.view.session, &state.view.window);
        let text = state.editor.text();
        if text.is_empty() && state.images.is_empty() {
            self.compose_drafts.remove(&key);
        } else {
            self.compose_drafts.insert(
                key,
                ComposeDraft {
                    text: text.to_string(),
                    images: state.images.clone(),
                },
            );
        }

        self.mode = AppMode::Viewing(state.view);
    }

    /// Deliver the compose buffer to the Claude Code session. Slash
    /// commands are typed as keystrokes so CC parses them as commands;
    /// anything else is bracketed-pasted so newlines survive.
    pub fn submit_compose(&mut self) -> Result<()> {
        let state = match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::Compose(state) => state,
            other => {
                self.mode = other;
                return Ok(());
            }
        };

        let text = state.editor.text().trim().to_string();
        if text.is_empty() && state.images.is_empty() {
            self.mode = AppMode::Compose(state);
            self.message = Some("Nothing to send".into());
            return Ok(());
        }

        let session = state.view.session.clone();
        let window = state.view.window.clone();
        let key = compose_target_key(&session, &window);

        // Clear any leftover text in Claude Code's input (e.g. typed
        // during direct mode) so the submission cannot merge with it.
        self.tmux.send_key_name(&session, &window, "C-u")?;

        if state.is_slash_command() {
            let interactive = state
                .exact_command_match()
                .map(|entry| entry.interactive)
                .unwrap_or(false);

            self.tmux.send_literal(&session, &window, &text)?;
            self.tmux.send_key_name(&session, &window, "Enter")?;

            if interactive {
                self.compose_direct_targets.insert(key.clone());
                self.push_toast_warning("Composer off — leader+e to re-enable");
            } else {
                self.push_toast_success(format!("Sent {text}"));
            }
        } else {
            if !text.is_empty() {
                self.persist_startup_prompt(&state.workdir, &text);
            }
            let parts = split_compose_parts(&text, &state.images);
            for part in &parts {
                match part {
                    ComposePart::Text(segment) => {
                        self.tmux.paste_text(&session, &window, segment)?;
                    }
                    ComposePart::Image(idx) => {
                        let image = &state.images[*idx];
                        crate::app::util::copy_image_to_clipboard(&image.data, &image.mime)?;
                        // Claude Code reads the clipboard when it
                        // receives Ctrl+V; give it time to ingest the
                        // image before the clipboard changes again.
                        self.tmux.send_key_name(&session, &window, "C-v")?;
                        std::thread::sleep(COMPOSE_IMAGE_PASTE_DELAY);
                    }
                }
            }
            self.tmux.send_key_name(&session, &window, "Enter")?;
        }

        self.compose_drafts.remove(&key);
        self.mode = AppMode::Viewing(state.view);
        self.request_view_snapshot_pane_burst();
        Ok(())
    }
}

/// Assemble the slash command catalog: CC built-ins, then global and
/// project custom commands, then skills.
pub(crate) fn build_compose_catalog(workdir: &Path) -> Vec<ComposeCommandEntry> {
    let mut catalog: Vec<ComposeCommandEntry> = BUILTIN_COMMANDS
        .iter()
        .map(|(name, description, interactive)| ComposeCommandEntry {
            name: (*name).to_string(),
            description: (*description).to_string(),
            source: ComposeCommandSource::BuiltIn,
            interactive: *interactive,
        })
        .collect();

    let push_unique = |catalog: &mut Vec<ComposeCommandEntry>, entry: ComposeCommandEntry| {
        if !catalog
            .iter()
            .any(|existing| existing.name.eq_ignore_ascii_case(&entry.name))
        {
            catalog.push(entry);
        }
    };

    if let Some(home) = dirs::home_dir() {
        for entry in scan_command_dir(
            &home.join(".claude").join("commands"),
            ComposeCommandSource::Global,
        ) {
            push_unique(&mut catalog, entry);
        }
    }

    for entry in scan_command_dir(
        &workdir.join(".claude").join("commands"),
        ComposeCommandSource::Project,
    ) {
        push_unique(&mut catalog, entry);
    }

    if let Some(home) = dirs::home_dir() {
        for entry in scan_skills_dir(&home.join(".claude").join("skills")) {
            push_unique(&mut catalog, entry);
        }
    }
    for entry in scan_skills_dir(&workdir.join(".claude").join("skills")) {
        push_unique(&mut catalog, entry);
    }

    catalog
}

fn scan_command_dir(dir: &Path, source: ComposeCommandSource) -> Vec<ComposeCommandEntry> {
    let mut raw = Vec::new();
    scan_commands_recursive(dir, dir, source.label(), &mut raw);
    raw.sort_by(|a, b| a.name.cmp(&b.name));

    raw.into_iter()
        .map(|cmd| {
            let description = cmd
                .path
                .as_deref()
                .and_then(frontmatter_description)
                .unwrap_or_default();
            ComposeCommandEntry {
                name: cmd.name,
                description,
                source,
                interactive: false,
            }
        })
        .collect()
}

fn scan_skills_dir(dir: &Path) -> Vec<ComposeCommandEntry> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut skills: Vec<ComposeCommandEntry> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_dir() {
                return None;
            }
            let skill_md = path.join("SKILL.md");
            if !skill_md.is_file() {
                return None;
            }
            let name = path.file_name()?.to_str()?.to_string();
            let description = frontmatter_description(&skill_md).unwrap_or_default();
            Some(ComposeCommandEntry {
                name,
                description,
                source: ComposeCommandSource::Skill,
                interactive: false,
            })
        })
        .collect();

    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// Extract the `description:` value from a markdown file's YAML
/// frontmatter, if present.
fn frontmatter_description(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut lines = content.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }

    for line in lines {
        let trimmed = line.trim();
        if trimmed == "---" {
            return None;
        }
        if let Some(value) = trimmed.strip_prefix("description:") {
            let value = value.trim().trim_matches('"').trim_matches('\'');
            if value.is_empty() {
                return None;
            }
            return Some(value.to_string());
        }
    }
    None
}
