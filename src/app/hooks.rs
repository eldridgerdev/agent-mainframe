use anyhow::Result;
use std::path::{Path, PathBuf};

use super::*;

impl App {
    /// Run a lifecycle hook script non-blocking.
    /// Expands leading `~/` to the home directory.
    /// If `choice` is provided it is set as `AMF_HOOK_CHOICE`
    /// in the child environment.
    pub fn run_lifecycle_hook(&self, script: &str, workdir: &Path, choice: Option<&str>) {
        let expanded = if script.starts_with("~/") {
            dirs::home_dir()
                .map(|h| format!("{}/{}", h.display(), &script[2..]))
                .unwrap_or_else(|| script.to_string())
        } else {
            script.to_string()
        };

        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg(&expanded).current_dir(workdir);
        if let Some(c) = choice {
            cmd.env("AMF_HOOK_CHOICE", c);
        }
        let _ = cmd.spawn();
    }

    /// Enter `HookPrompt` mode when the hook config has a
    /// `prompt` field. Does nothing (returns `false`) for
    /// plain `Script` configs so the caller can fall through
    /// to immediate execution.
    pub fn start_hook_prompt(
        &mut self,
        script: String,
        workdir: PathBuf,
        title: String,
        options: Vec<String>,
        next: HookNext,
    ) {
        self.mode = AppMode::HookPrompt(HookPromptState {
            script,
            workdir,
            title,
            options,
            selected: 0,
            next,
        });
    }

    /// Called when the user presses Enter in `HookPrompt` mode.
    pub fn confirm_hook_prompt(&mut self) -> Result<()> {
        let state = match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::HookPrompt(s) => s,
            other => {
                self.mode = other;
                return Ok(());
            }
        };

        let choice = state
            .options
            .get(state.selected)
            .cloned()
            .unwrap_or_default();

        match state.next {
            HookNext::WorktreeCreated {
                project_name,
                branch,
                mode,
                review,
                agent,
                enable_chrome,
                enable_notes,
            } => {
                self.start_worktree_hook(
                    &state.script,
                    state.workdir,
                    project_name,
                    branch,
                    mode,
                    review,
                    agent,
                    enable_chrome,
                    enable_notes,
                    Some(choice),
                );
            }
            HookNext::StartFeature { pi, fi } => {
                self.run_lifecycle_hook(&state.script, &state.workdir, Some(&choice));
                self.do_start_feature(pi, fi)?;
            }
            HookNext::StopFeature { pi, fi } => {
                self.run_lifecycle_hook(&state.script, &state.workdir, Some(&choice));
                self.do_stop_feature(pi, fi)?;
            }
        }
        Ok(())
    }

    pub fn start_worktree_hook(
        &mut self,
        script: &str,
        workdir: PathBuf,
        project_name: String,
        branch: String,
        mode: VibeMode,
        review: bool,
        agent: AgentKind,
        enable_chrome: bool,
        enable_notes: bool,
        choice: Option<String>,
    ) {
        let expanded = if script.starts_with("~/") {
            dirs::home_dir()
                .map(|h| format!("{}/{}", h.display(), &script[2..]))
                .unwrap_or_else(|| script.to_string())
        } else {
            script.to_string()
        };

        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c")
            .arg(&expanded)
            .current_dir(&workdir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        if let Some(ref c) = choice {
            cmd.env("AMF_HOOK_CHOICE", c);
        }
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        let mut child = cmd.spawn().ok();

        if let Some(ref mut c) = child {
            if let Some(stdout) = c.stdout.take() {
                let tx2 = tx.clone();
                std::thread::spawn(move || {
                    use std::io::BufRead;
                    for line in std::io::BufReader::new(stdout).lines() {
                        if let Ok(l) = line {
                            let _ = tx2.send(l);
                        }
                    }
                });
            }
            if let Some(stderr) = c.stderr.take() {
                std::thread::spawn(move || {
                    use std::io::BufRead;
                    for line in std::io::BufReader::new(stderr).lines() {
                        if let Ok(l) = line {
                            let _ = tx.send(l);
                        }
                    }
                });
            }
        }

        self.mode = AppMode::RunningHook(RunningHookState {
            script: script.to_string(),
            workdir,
            project_name,
            branch,
            mode,
            review,
            agent,
            enable_chrome,
            enable_notes,
            child,
            output: String::new(),
            success: None,
            output_rx: Some(rx),
        });
    }

    pub fn poll_running_hook(&mut self) -> Result<()> {
        let state = match &mut self.mode {
            AppMode::RunningHook(s) => s,
            _ => return Ok(()),
        };

        // Drain any lines the reader threads have sent.
        if let Some(ref rx) = state.output_rx {
            while let Ok(line) = rx.try_recv() {
                state.output.push_str(&line);
                state.output.push('\n');
            }
        }

        if let Some(ref mut child) = state.child {
            match child.try_wait() {
                Ok(Some(status)) => {
                    state.success = Some(status.success());
                    if let Some(code) = status.code() {
                        state
                            .output
                            .push_str(&format!("\nProcess exited with code: {}", code));
                    }
                    state.child = None;
                }
                Ok(None) => {}
                Err(e) => {
                    state.success = Some(false);
                    state.output.push_str(&format!("\nError: {}", e));
                    state.child = None;
                }
            }
        }

        Ok(())
    }

    pub fn complete_running_hook(&mut self) -> Result<()> {
        let (
            workdir,
            project_name,
            branch,
            mode,
            review,
            agent,
            enable_chrome,
            enable_notes,
            success,
        ) = {
            match &self.mode {
                AppMode::RunningHook(s) => (
                    s.workdir.clone(),
                    s.project_name.clone(),
                    s.branch.clone(),
                    s.mode.clone(),
                    s.review,
                    s.agent.clone(),
                    s.enable_chrome,
                    s.enable_notes,
                    s.success,
                ),
                _ => return Ok(()),
            }
        };

        let is_worktree = workdir
            != self
                .store
                .find_project(&project_name)
                .map(|p| p.repo.clone())
                .unwrap_or_default();

        if enable_notes {
            let claude_dir = workdir.join(".claude");
            if !claude_dir.exists() {
                let _ = std::fs::create_dir_all(&claude_dir);
            }
            let notes_path = claude_dir.join("notes.md");
            if !notes_path.exists() {
                let _ = std::fs::write(
                    &notes_path,
                    "# Notes\n\nWrite instructions for Claude here.\n",
                );
            }
        }

        let feature = Feature::new(
            branch.clone(),
            branch.clone(),
            workdir.clone(),
            is_worktree,
            mode,
            review,
            agent,
            enable_chrome,
            enable_notes,
        );

        self.store.add_feature(&project_name, feature);
        self.save()?;

        if let Some(pi) = self
            .store
            .projects
            .iter()
            .position(|p| p.name == project_name)
        {
            let fi = self.store.projects[pi].features.len().saturating_sub(1);
            self.store.projects[pi].collapsed = false;
            self.selection = Selection::Feature(pi, fi);
        }

        self.mode = AppMode::Normal;

        if let Some(pi) = self
            .store
            .projects
            .iter()
            .position(|p| p.name == project_name)
        {
            let fi = self.store.projects[pi].features.len().saturating_sub(1);
            self.ensure_feature_running(pi, fi)?;
            self.save()?;
        }

        if success.unwrap_or(false) {
            self.message = Some(format!(
                "Created and started feature '{}' (hook succeeded)",
                branch
            ));
        } else {
            self.message = Some(format!(
                "Created and started feature '{}' (hook failed)",
                branch
            ));
        }

        Ok(())
    }

    pub fn hide_running_hook(&mut self) {
        if let AppMode::RunningHook(state) = std::mem::replace(&mut self.mode, AppMode::Normal) {
            let key = state.key();
            let bg = BackgroundHook::from_running_state(state);
            self.background_hooks.insert(key, bg);
            self.message = Some("Hook moved to background".to_string());
        }
    }

    pub fn poll_background_hooks(&mut self) -> Result<()> {
        let mut completed = Vec::new();

        for (key, hook) in self.background_hooks.iter_mut() {
            if let Some(ref rx) = hook.output_rx {
                while let Ok(line) = rx.try_recv() {
                    hook.output.push_str(&line);
                    hook.output.push('\n');
                }
            }

            if let Some(ref mut child) = hook.child {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        hook.success = Some(status.success());
                        hook.child = None;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        hook.success = Some(false);
                        hook.output.push_str(&format!("\nError: {}", e));
                        hook.child = None;
                    }
                }
            }

            if hook.child.is_none() {
                completed.push(key.clone());
            }
        }

        for key in completed {
            if let Some(hook) = self.background_hooks.remove(&key) {
                if hook.success.unwrap_or(false) {
                    let _ = self.save();
                    self.message = Some("Hook completed".to_string());
                } else {
                    self.message = Some("Hook failed".to_string());
                }
            }
        }

        Ok(())
    }

    pub fn is_hook_running(&self, workdir: &PathBuf) -> bool {
        self.background_hooks
            .values()
            .any(|hook| &hook.workdir == workdir)
    }
}
