//! A [`TmuxOps`] decorator that moves a sandboxed feature's panes into its
//! container.
//!
//! For a session in the [`SandboxRegistry`] the decorator (1) makes sure the
//! feature's containers are running, (2) creates each window running
//! `docker exec` into the sandbox as its initial command (never a host shell), and
//! (3) types agent launch commands into that shell instead of running the
//! host-resolved binaries. Everything else, and every session not in the
//! registry, goes straight to the inner implementation, so non-sandboxed
//! projects behave exactly as before.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::Result;

use super::args::{Names, SandboxSpec, exec_shell_command, sh_quote};
use super::runtime;
use crate::project::AgentKind;
use crate::traits::TmuxOps;

/// Which tmux sessions are sandboxed, shared between the `App` (which keeps it
/// in step with the project store) and the decorator.
#[derive(Default)]
pub struct SandboxRegistry {
    specs: Mutex<HashMap<String, SandboxSpec>>,
}

impl SandboxRegistry {
    pub fn replace_all(&self, specs: HashMap<String, SandboxSpec>) {
        *self.specs.lock().unwrap() = specs;
    }

    pub fn get(&self, tmux_session: &str) -> Option<SandboxSpec> {
        self.specs.lock().unwrap().get(tmux_session).cloned()
    }

    pub fn len(&self) -> usize {
        self.specs.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub struct SandboxTmux {
    inner: Box<dyn TmuxOps>,
    registry: Arc<SandboxRegistry>,
}

impl SandboxTmux {
    pub fn new(inner: Box<dyn TmuxOps>, registry: Arc<SandboxRegistry>) -> Self {
        Self { inner, registry }
    }

    /// Type `program` (already shell-quoted) into a sandboxed pane with the
    /// same session variables the host launch sets, minus host-only paths.
    fn sandbox_launch(
        &self,
        session: &str,
        window: &str,
        feature_session_id: &str,
        program: String,
    ) -> Result<()> {
        let cmd = format!(
            "printf '\\033[2J\\033[H'; env AMF_ACTIVE='1' AMF_SESSION={} AMF_TMUX_SESSION={} AMF_TMUX_WINDOW={} AMF_FEATURE_SESSION_ID={} {program}",
            sh_quote(session),
            sh_quote(session),
            sh_quote(window),
            sh_quote(feature_session_id),
        );
        self.inner.send_keys(session, window, &cmd)
    }
}

fn quoted_args(args: &[String]) -> String {
    args.iter().map(|a| format!(" {}", sh_quote(a))).collect()
}

impl TmuxOps for SandboxTmux {
    fn check_harness_available(&self, kind: &AgentKind) -> Result<()> {
        self.inner.check_harness_available(kind)
    }
    fn session_exists(&self, session: &str) -> bool {
        self.inner.session_exists(session)
    }
    fn window_exists(&self, session: &str, window: &str) -> bool {
        self.inner.window_exists(session, window)
    }
    fn list_sessions(&self) -> Result<Vec<String>> {
        self.inner.list_sessions()
    }
    fn window_activity(&self) -> Vec<(String, String, i64)> {
        self.inner.window_activity()
    }
    fn list_panes(&self) -> Vec<(String, String, i64)> {
        self.inner.list_panes()
    }

    fn create_session_with_window(
        &self,
        session: &str,
        first_window: &str,
        workdir: &Path,
    ) -> Result<()> {
        let Some(spec) = self.registry.get(session) else {
            return self
                .inner
                .create_session_with_window(session, first_window, workdir);
        };
        // Containers first: a failure here leaves no half-made tmux session.
        runtime::ensure_running(&spec)?;
        // The pane starts as `docker exec`, never as a host shell, so the
        // user's shell startup never runs in the untrusted checkout and a
        // dead container closes the pane instead of leaving a host shell.
        let names = Names::for_session(&spec.tmux_session);
        self.inner.create_session_with_command(
            session,
            first_window,
            workdir,
            &exec_shell_command(&names, &spec.workdir),
        )
    }

    fn create_session_with_command(
        &self,
        session: &str,
        first_window: &str,
        workdir: &Path,
        command: &str,
    ) -> Result<()> {
        self.inner
            .create_session_with_command(session, first_window, workdir, command)
    }

    fn create_window_with_command(
        &self,
        session: &str,
        window: &str,
        workdir: &Path,
        command: &str,
    ) -> Result<()> {
        self.inner
            .create_window_with_command(session, window, workdir, command)
    }

    fn set_session_env(&self, session: &str, key: &str, value: &str) -> Result<()> {
        self.inner.set_session_env(session, key, value)
    }

    fn create_window(&self, session: &str, window: &str, workdir: &Path) -> Result<()> {
        let Some(spec) = self.registry.get(session) else {
            return self.inner.create_window(session, window, workdir);
        };
        runtime::ensure_running(&spec)?;
        // Windows other than the feature root start in their own directory.
        let names = Names::for_session(&spec.tmux_session);
        self.inner.create_window_with_command(
            session,
            window,
            workdir,
            &exec_shell_command(&names, workdir),
        )
    }

    fn launch_claude(
        &self,
        session: &str,
        window: &str,
        feature_session_id: &str,
        resume_id: Option<String>,
        extra_args: Vec<String>,
    ) -> Result<()> {
        if self.registry.get(session).is_none() {
            return self.inner.launch_claude(
                session,
                window,
                feature_session_id,
                resume_id,
                extra_args,
            );
        }
        let mut program = String::from("claude");
        if let Some(id) = resume_id {
            program.push_str(&format!(" --resume {}", sh_quote(&id)));
        }
        program.push_str(&quoted_args(&extra_args));
        self.sandbox_launch(session, window, feature_session_id, program)
    }

    fn launch_opencode(&self, session: &str, window: &str, feature_session_id: &str) -> Result<()> {
        self.launch_opencode_with_session(session, window, feature_session_id, None)
    }

    fn launch_opencode_with_session(
        &self,
        session: &str,
        window: &str,
        feature_session_id: &str,
        resume_id: Option<String>,
    ) -> Result<()> {
        if self.registry.get(session).is_none() {
            return self.inner.launch_opencode_with_session(
                session,
                window,
                feature_session_id,
                resume_id,
            );
        }
        let program = match resume_id {
            Some(id) => format!("opencode -s {}", sh_quote(&id)),
            None => "opencode".to_string(),
        };
        self.sandbox_launch(session, window, feature_session_id, program)
    }

    fn launch_codex(
        &self,
        session: &str,
        window: &str,
        feature_session_id: &str,
        resume_id: Option<String>,
        extra_args: Vec<String>,
    ) -> Result<()> {
        if self.registry.get(session).is_none() {
            return self.inner.launch_codex(
                session,
                window,
                feature_session_id,
                resume_id,
                extra_args,
            );
        }
        let mut program = String::from("codex");
        program.push_str(&quoted_args(&extra_args));
        if let Some(id) = resume_id {
            program.push_str(&format!(" resume {}", sh_quote(&id)));
        }
        self.sandbox_launch(session, window, feature_session_id, program)
    }

    fn launch_pi(&self, session: &str, window: &str, feature_session_id: &str) -> Result<()> {
        if self.registry.get(session).is_none() {
            return self.inner.launch_pi(session, window, feature_session_id);
        }
        self.sandbox_launch(session, window, feature_session_id, "pi".into())
    }

    fn run_shell_command(&self, session: &str, window: &str, command: &str) -> Result<()> {
        if self.registry.get(session).is_none() {
            return self.inner.run_shell_command(session, window, command);
        }
        // No launch script: it would live on the host, out of the sandbox's
        // sight, and the sandbox must not be given the host's temp dir.
        self.inner.send_keys(session, window, command)
    }

    fn send_keys(&self, session: &str, window: &str, keys: &str) -> Result<()> {
        self.inner.send_keys(session, window, keys)
    }
    fn send_literal(&self, session: &str, window: &str, text: &str) -> Result<()> {
        self.inner.send_literal(session, window, text)
    }
    fn capture_pane(&self, session: &str, window: &str) -> Result<String> {
        self.inner.capture_pane(session, window)
    }
    fn capture_pane_history(&self, session: &str, window: &str, lines: u32) -> Result<String> {
        self.inner.capture_pane_history(session, window, lines)
    }
    fn paste_text(&self, session: &str, window: &str, text: &str) -> Result<()> {
        self.inner.paste_text(session, window, text)
    }
    fn send_key_name(&self, session: &str, window: &str, key_name: &str) -> Result<()> {
        self.inner.send_key_name(session, window, key_name)
    }
    fn resize_pane(&self, session: &str, window: &str, cols: u16, rows: u16) -> Result<()> {
        self.inner.resize_pane(session, window, cols, rows)
    }
    fn select_window(&self, session: &str, window: &str) -> Result<()> {
        self.inner.select_window(session, window)
    }
    fn kill_window(&self, session: &str, window: &str) -> Result<()> {
        self.inner.kill_window(session, window)
    }

    fn kill_session(&self, session: &str) -> Result<()> {
        let result = self.inner.kill_session(session);
        if self.registry.get(session).is_some() {
            runtime::teardown(session);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container::ContainerSettings;
    use crate::traits::MockTmuxOps;
    use std::path::PathBuf;

    fn registry_with(session: &str) -> Arc<SandboxRegistry> {
        let reg = Arc::new(SandboxRegistry::default());
        let spec = SandboxSpec {
            tmux_session: session.into(),
            mounts: vec![PathBuf::from("/code/repo")],
            protect: vec![],
            workdir: PathBuf::from("/code/repo"),
            home_volume: "amf-sbx-home-x".into(),
            settings: ContainerSettings::default(),
        };
        reg.replace_all(HashMap::from([(session.to_string(), spec)]));
        reg
    }

    #[test]
    fn unregistered_sessions_pass_straight_through() {
        let mut inner = MockTmuxOps::new();
        inner
            .expect_launch_claude()
            .withf(|s, w, id, resume, args| {
                s == "amf-host"
                    && w == "claude"
                    && id == "sid"
                    && resume.is_none()
                    && args.is_empty()
            })
            .times(1)
            .returning(|_, _, _, _, _| Ok(()));
        inner.expect_send_keys().never();
        let t = SandboxTmux::new(Box::new(inner), registry_with("amf-sandboxed"));
        t.launch_claude("amf-host", "claude", "sid", None, vec![])
            .unwrap();
    }

    #[test]
    fn sandboxed_launch_types_into_the_pane_and_never_uses_host_binaries() {
        let mut inner = MockTmuxOps::new();
        inner.expect_launch_claude().never();
        inner
            .expect_send_keys()
            .withf(|s, w, keys| {
                s == "amf-sandboxed"
                    && w == "claude"
                    && keys.contains(" claude --resume 'abc' '--model' 'x y'")
                    && keys.contains("AMF_FEATURE_SESSION_ID='sid'")
                    && !keys.contains("AMF_BIN")
                    && !keys.contains("/home/")
            })
            .times(1)
            .returning(|_, _, _| Ok(()));
        let t = SandboxTmux::new(Box::new(inner), registry_with("amf-sandboxed"));
        t.launch_claude(
            "amf-sandboxed",
            "claude",
            "sid",
            Some("abc".into()),
            vec!["--model".into(), "x y".into()],
        )
        .unwrap();
    }

    #[test]
    fn sandboxed_shell_commands_skip_the_host_launch_script() {
        let mut inner = MockTmuxOps::new();
        inner.expect_run_shell_command().never();
        inner
            .expect_send_keys()
            .withf(|_, _, k| k == "npm test")
            .times(1)
            .returning(|_, _, _| Ok(()));
        let t = SandboxTmux::new(Box::new(inner), registry_with("amf-sandboxed"));
        t.run_shell_command("amf-sandboxed", "dev", "npm test")
            .unwrap();
    }

    #[test]
    fn registry_replace_all_drops_removed_sessions() {
        let reg = registry_with("a");
        assert!(reg.get("a").is_some());
        reg.replace_all(HashMap::new());
        assert!(reg.get("a").is_none());
        assert!(reg.is_empty());
    }
}
