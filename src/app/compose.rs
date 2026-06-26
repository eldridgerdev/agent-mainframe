use std::path::Path;
use std::os::fd::AsRawFd;
use std::sync::mpsc;

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

/// Fallback delay after a forwarded Ctrl+V when we cannot confirm the
/// image landed by watching the pane (e.g. on a non-WSL native clipboard
/// where ingestion is effectively instant, or if capture fails).
const COMPOSE_IMAGE_PASTE_DELAY: std::time::Duration = std::time::Duration::from_millis(350);

/// How long to wait for the harness to render the image placeholder
/// after a forwarded Ctrl+V before giving up and proceeding anyway. The
/// harness reads the *Windows* clipboard via PowerShell on WSL, which is
/// slow and variable, so we poll for the placeholder rather than guess a
/// fixed delay — otherwise Enter races ahead, the text submits on its
/// own, and the image is left sitting in the harness input box.
const COMPOSE_IMAGE_INGEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);
/// Poll cadence while waiting for the image placeholder to appear.
const COMPOSE_IMAGE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(120);

/// Count rendered image placeholders (`[Image #N]`) in the harness input
/// region, used to detect when a pasted image has been ingested. Only the
/// pane tail is inspected: the scrollback transcript may itself mention
/// "[Image #N]" (e.g. a conversation about image paste) and would
/// otherwise pollute the count.
fn count_image_placeholders(pane: &str) -> usize {
    const INPUT_TAIL_LINES: usize = 8;
    let lines: Vec<&str> = pane.lines().collect();
    let start = lines.len().saturating_sub(INPUT_TAIL_LINES);
    lines[start..]
        .iter()
        .map(|line| line.matches("[Image #").count())
        .sum()
}

/// Wait until the harness shows one more `[Image #N]` placeholder than
/// `baseline`, signalling the pasted image was ingested, then settle
/// briefly. Returns early on timeout as a best-effort fallback.
fn wait_for_image_ingested(session: &str, window: &str, baseline: usize) {
    let start = std::time::Instant::now();
    loop {
        let count = crate::tmux::TmuxManager::capture_pane(session, window)
            .map(|pane| count_image_placeholders(&pane))
            .unwrap_or(0);
        if count > baseline {
            // Let the harness settle the input line before the next
            // paste or the final Enter.
            std::thread::sleep(std::time::Duration::from_millis(150));
            return;
        }
        if start.elapsed() >= COMPOSE_IMAGE_INGEST_TIMEOUT {
            return;
        }
        std::thread::sleep(COMPOSE_IMAGE_POLL_INTERVAL);
    }
}

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
    pub fn start_compose_clipboard_paste(&mut self) {
        if self.compose_clipboard_paste.is_some() {
            self.push_toast_warning("Paste already in progress");
            return;
        }

        let Some((id, target)) = (match &mut self.mode {
            AppMode::Compose(state) => {
                let id = self.next_compose_clipboard_paste_id;
                self.next_compose_clipboard_paste_id =
                    self.next_compose_clipboard_paste_id.saturating_add(1);
                state.clipboard_paste_id = Some(id);
                Some((
                    id,
                    compose_target_key(&state.view.session, &state.view.window),
                ))
            }
            _ => None,
        }) else {
            return;
        };

        let wakeup = self.view_wakeup_tx();
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("compose-clipboard-paste".to_string())
            .spawn(move || {
                let result = crate::app::util::read_clipboard().map_err(|err| err.to_string());
                let _ = tx.send(result);
                let byte = 1u8;
                unsafe {
                    libc::write(
                        wakeup.as_raw_fd(),
                        &byte as *const u8 as *const _,
                        1,
                    )
                };
            })
            .expect("failed to start compose clipboard paste worker");
        self.compose_clipboard_paste = Some(ComposeClipboardPaste { id, target, rx });
    }

    pub fn poll_compose_clipboard_paste(&mut self) -> bool {
        let Some(paste) = &self.compose_clipboard_paste else {
            return false;
        };

        let result = match paste.rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("clipboard worker stopped before returning a result".to_string())
            }
        };

        let Some(paste) = self.compose_clipboard_paste.take() else {
            return false;
        };

        let should_apply = matches!(
            &self.mode,
            AppMode::Compose(state)
                if state.clipboard_paste_id == Some(paste.id)
                    && compose_target_key(&state.view.session, &state.view.window) == paste.target
        );

        if !should_apply {
            return true;
        }

        if let AppMode::Compose(state) = &mut self.mode {
            state.clipboard_paste_id = None;
        }

        match result {
            Ok(crate::app::util::ClipboardContent::Text(text)) if !text.is_empty() => {
                if let AppMode::Compose(state) = &mut self.mode {
                    let outcome = state.editor.insert_str(&text);
                    if outcome.text_changed {
                        state.refresh_suggestions();
                        state.request_cursor_scroll();
                    }
                }
            }
            Ok(crate::app::util::ClipboardContent::Image { data, mime }) => {
                let placeholder = if let AppMode::Compose(state) = &mut self.mode {
                    let placeholder = state.add_image(data, mime);
                    state.editor.insert_str(&placeholder);
                    state.refresh_suggestions();
                    state.request_cursor_scroll();
                    Some(placeholder)
                } else {
                    None
                };
                if let Some(placeholder) = placeholder {
                    self.push_toast_success(format!("Attached {placeholder} from clipboard"));
                }
            }
            Ok(_) => {}
            Err(e) => {
                self.push_toast_warning(format!("Clipboard error: {e}"));
            }
        }

        true
    }

    /// Whether typing in this agent view should open the compose box
    /// instead of forwarding keystrokes to tmux.
    pub fn compose_intercept_active(&self, view: &ViewState) -> bool {
        view.session_kind.is_agent_harness()
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

        if !kind.is_agent_harness() {
            self.push_toast_warning("Compose is only for agent harness sessions");
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

    /// Deliver the compose buffer to the agent session. Slash commands
    /// are typed as keystrokes so the harness parses them as commands;
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
        let session_kind = state.view.session_kind.clone();
        let key = compose_target_key(&session, &window);

        // Clear any leftover text in the harness input (e.g. typed
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
                        // The harness reads the clipboard when it receives
                        // Ctrl+V. On WSL that read goes through PowerShell
                        // and is slow, so wait for the rendered placeholder
                        // before moving on (next image's clipboard write or
                        // the final Enter); otherwise the image is dropped.
                        if crate::app::util::is_wsl() {
                            let baseline = crate::tmux::TmuxManager::capture_pane(&session, &window)
                                .map(|pane| count_image_placeholders(&pane))
                                .unwrap_or(0);
                            self.tmux.send_key_name(&session, &window, "C-v")?;
                            wait_for_image_ingested(&session, &window, baseline);
                        } else {
                            self.tmux.send_key_name(&session, &window, "C-v")?;
                            std::thread::sleep(COMPOSE_IMAGE_PASTE_DELAY);
                        }
                    }
                }
            }
            self.tmux.send_key_name(&session, &window, "Enter")?;
        }

        self.compose_drafts.remove(&key);
        self.mode = AppMode::Viewing(state.view);
        if session_kind == SessionKind::Codex {
            self.note_codex_prompt_submit(&session, &window);
        }
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

/// The available agent skills — global (`~/.claude/skills`) plus the
/// project's own (`{workdir}/.claude/skills`) — deduped by name with the
/// project copy winning. Same scan the compose catalog uses for skills, but
/// without the slash-command entries, for the prompt-library skill picker.
pub(crate) fn build_skill_catalog(workdir: Option<&Path>) -> Vec<ComposeCommandEntry> {
    let mut catalog: Vec<ComposeCommandEntry> = Vec::new();
    let push_unique = |catalog: &mut Vec<ComposeCommandEntry>, entry: ComposeCommandEntry| {
        if !catalog
            .iter()
            .any(|existing| existing.name.eq_ignore_ascii_case(&entry.name))
        {
            catalog.push(entry);
        }
    };

    // Project skills first so they win the name collision over global ones.
    if let Some(workdir) = workdir {
        for entry in scan_skills_dir(&workdir.join(".claude").join("skills")) {
            push_unique(&mut catalog, entry);
        }
    }
    if let Some(home) = dirs::home_dir() {
        for entry in scan_skills_dir(&home.join(".claude").join("skills")) {
            push_unique(&mut catalog, entry);
        }
    }

    catalog.sort_by(|a, b| a.name.cmp(&b.name));
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

#[cfg(test)]
mod compose_image_tests {
    use super::count_image_placeholders;

    #[test]
    fn counts_image_placeholders_in_pane() {
        assert_eq!(count_image_placeholders(""), 0);
        assert_eq!(count_image_placeholders("> just some text"), 0);
        assert_eq!(count_image_placeholders("> testing [Image #1]"), 1);
        // The pane includes scrollback, so a newly pasted image bumps the
        // count above the baseline even with prior placeholders present.
        let before = count_image_placeholders("history [Image #1]\n> caption");
        let after = count_image_placeholders("history [Image #1]\n> caption [Image #2]");
        assert_eq!(before, 1);
        assert_eq!(after, 2);
        assert!(after > before);
    }
}
