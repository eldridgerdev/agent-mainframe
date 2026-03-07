use anyhow::Result;
use std::path::PathBuf;

use super::*;
use crate::tmux::TmuxManager;

impl App {
    pub fn trigger_final_review(&mut self) -> Result<()> {
        // Extract everything we need before mutating self.
        let (workdir, repo, session, feature_name) = match &self.mode {
            AppMode::Viewing(view) => {
                let pi = self
                    .store
                    .projects
                    .iter()
                    .position(|p| p.name == view.project_name);
                let pi = match pi {
                    Some(pi) => pi,
                    None => {
                        return Ok(());
                    }
                };
                let fi = self.store.projects[pi]
                    .features
                    .iter()
                    .position(|f| f.name == view.feature_name);
                let fi = match fi {
                    Some(fi) => fi,
                    None => {
                        return Ok(());
                    }
                };
                let feature = &self.store.projects[pi].features[fi];
                let repo = self.store.projects[pi].repo.clone();
                (
                    feature.workdir.clone(),
                    repo,
                    view.session.clone(),
                    feature.name.clone(),
                )
            }
            _ => return Ok(()),
        };

        // Look in workdir (feature worktree), then repo root, then
        // the directory of the running AMF binary (handles the case
        // where final-review.sh hasn't been committed yet but exists
        // in the worktree AMF was built from).
        let script_suffix = ["plugins", "diff-review", "scripts", "final-review.sh"];
        let amf_root = std::env::current_exe().ok().and_then(|exe| {
            // exe is at <root>/target/{debug,release}/amf — go up 3
            exe.parent()?.parent()?.parent().map(PathBuf::from)
        });
        let script_path = [Some(workdir.clone()), Some(repo.clone()), amf_root]
            .into_iter()
            .flatten()
            .map(|base| script_suffix.iter().fold(base, |p, s| p.join(s)))
            .find(|p| p.exists());

        let script = match script_path {
            Some(p) => p,
            None => {
                self.exit_view();
                self.message = Some(format!(
                    "final-review.sh not found in {}, {}, or AMF binary dir",
                    workdir.display(),
                    repo.display(),
                ));
                return Ok(());
            }
        };

        // Check if the "terminal" window exists in the current session.
        // If not, create a new "Review" session for this feature.
        let windows = TmuxManager::list_windows(&session).unwrap_or_default();
        let has_terminal = windows.iter().any(|w| w == "terminal");

        let (target_session, target_window) = if has_terminal {
            (session.clone(), "terminal".to_string())
        } else {
            let review_session = format!("amf-{}-Review", feature_name);
            if !TmuxManager::session_exists(&review_session) {
                TmuxManager::create_session_with_window(&review_session, "review", &workdir)?;
            }
            (review_session, "review".to_string())
        };

        // Run the script directly in the feature's terminal pane.
        // Wrapping in display-popup would cause nested-popup failures
        // since final-review.sh opens its own popups for vimdiff/notes.
        // After the script exits, switch back to the AMF session so the
        // user doesn't get stranded in the feature's terminal.
        let amf_session = TmuxManager::current_session().unwrap_or_default();
        let switch_back = if amf_session.is_empty() {
            String::new()
        } else {
            format!("; tmux switch-client -t '{}'", amf_session)
        };
        let cmd = format!(
            "bash '{}' '{}'{}",
            script.to_string_lossy(),
            workdir.to_string_lossy(),
            switch_back,
        );
        if let Err(e) = TmuxManager::send_literal(&target_session, &target_window, &cmd) {
            self.exit_view();
            self.message = Some(format!("Failed to send review command: {e}"));
            return Ok(());
        }
        if let Err(e) = TmuxManager::send_key_name(&target_session, &target_window, "Enter") {
            self.exit_view();
            self.message = Some(format!("Failed to start review: {e}"));
            return Ok(());
        }

        // Switch to the session so the popup is visible.
        if TmuxManager::is_inside_tmux() {
            TmuxManager::switch_client(&target_session)?;
        } else {
            self.should_switch = Some(target_session);
        }

        Ok(())
    }
}
