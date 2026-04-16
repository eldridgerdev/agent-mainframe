use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Output, Stdio};
use std::sync::{
    Mutex, OnceLock,
    mpsc::{self, Receiver, RecvTimeoutError},
};
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::fs::FileTypeExt;

use crate::debug::{LogLevel, log_to_file};
use crate::traits::TmuxOps;

pub struct TmuxManager;

pub struct SpawnedTmuxCommand {
    pub child: Child,
    pub output_rx: Receiver<String>,
}

struct PersistentTmuxInputClient {
    child: Child,
    stdin: ChildStdin,
    output_rx: Receiver<String>,
}

impl PersistentTmuxInputClient {
    fn send_command(&mut self, command: &str) -> Result<()> {
        self.stdin
            .write_all(command.as_bytes())
            .context("Failed to write tmux input command")?;
        self.stdin
            .flush()
            .context("Failed to flush tmux input command")
    }

    fn is_running(&mut self) -> Result<bool> {
        Ok(self
            .child
            .try_wait()
            .context("Failed to poll tmux input client process")?
            .is_none())
    }

    fn wait_for_token(&mut self, token: &str, timeout: Duration) -> Result<()> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if !self.is_running()? {
                bail!("tmux input client exited before acknowledging readiness");
            }

            let now = std::time::Instant::now();
            if now >= deadline {
                bail!("timed out waiting for tmux input client readiness");
            }

            let remaining = deadline.saturating_duration_since(now);
            match self.output_rx.recv_timeout(remaining.min(Duration::from_millis(50))) {
                Ok(line) => {
                    if line.contains(token) {
                        return Ok(());
                    }
                }
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => {
                    bail!("tmux input client output stream disconnected");
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
struct TmuxRuntime {
    binary: OsString,
    socket: Option<PathBuf>,
    manages_private_socket: bool,
}

impl TmuxRuntime {
    fn detect() -> Self {
        let tmux_env = std::env::var("TMUX").ok();
        let using_existing_tmux = tmux_env.is_some();

        let binary = std::env::var_os("AMF_TMUX_BIN")
            .or_else(|| {
                if using_existing_tmux {
                    None
                } else {
                    Self::bundled_binary()
                }
            })
            .unwrap_or_else(|| OsString::from("tmux"));

        let (socket, manages_private_socket) =
            if let Some(socket) = std::env::var_os("AMF_TMUX_SOCKET").map(PathBuf::from) {
                (Some(socket), false)
            } else if let Some(socket) = tmux_env.as_deref().and_then(Self::socket_from_tmux_env) {
                (Some(socket), false)
            } else if using_existing_tmux {
                (None, false)
            } else {
                (Some(Self::private_socket_path()), true)
            };

        Self {
            binary,
            socket,
            manages_private_socket,
        }
    }

    fn bundled_binary() -> Option<OsString> {
        let exe = std::env::current_exe().ok()?;
        let dir = exe.parent()?;
        let bundled = dir.join("tmux");
        if bundled.exists() {
            Some(bundled.into_os_string())
        } else {
            None
        }
    }

    fn socket_from_tmux_env(value: &str) -> Option<PathBuf> {
        let socket = value.split(',').next()?.trim();
        if socket.is_empty() {
            None
        } else {
            Some(PathBuf::from(socket))
        }
    }

    fn private_socket_path() -> PathBuf {
        dirs::state_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("amf")
            .join("tmux.sock")
    }

    fn launch_path_override(&self) -> Option<OsString> {
        Self::prepend_binary_dir_to_path(&self.binary)
    }

    fn prepend_binary_dir_to_path(binary: &OsString) -> Option<OsString> {
        let binary_path = PathBuf::from(binary);
        let dir = binary_path.parent()?;
        let current_path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![dir.to_path_buf()];

        // Add mise install directories
        if let Some(home) = dirs::home_dir() {
            let mise_base = home.join(".local/share/mise/installs");
            // Add codex and node (dynamically find versions in case they change)
            if let Ok(entries) = std::fs::read_dir(&mise_base) {
                for entry in entries.flatten() {
                    let tool_dir = entry.path();
                    if let Ok(versions) = std::fs::read_dir(&tool_dir) {
                        for version in versions.flatten() {
                            let bin_dir = version.path().join("bin");
                            if bin_dir.exists() {
                                paths.push(bin_dir);
                            }
                        }
                    }
                }
            }
        }

        paths.extend(std::env::split_paths(&current_path));
        std::env::join_paths(paths).ok()
    }
}

impl TmuxManager {
    fn persistent_input_enabled() -> bool {
        std::env::var("AMF_EXPERIMENTAL_PERSISTENT_TMUX_INPUT")
            .ok()
            .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "on" | "ON"))
    }

    fn runtime() -> &'static TmuxRuntime {
        static RUNTIME: OnceLock<TmuxRuntime> = OnceLock::new();
        RUNTIME.get_or_init(TmuxRuntime::detect)
    }

    fn command() -> Command {
        let runtime = Self::runtime();
        let mut command = Command::new(&runtime.binary);
        if let Some(socket) = &runtime.socket {
            if let Some(parent) = socket.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            command.arg("-S").arg(socket);
        }
        command.env("AMF_TMUX_BIN", &runtime.binary);
        if let Some(socket) = &runtime.socket {
            command.env("AMF_TMUX_SOCKET", socket);
        }
        command
    }

    fn shell_quote(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\\''"))
    }

    fn shell_env_parts(include_path: bool) -> Vec<String> {
        let runtime = Self::runtime();
        let mut parts = vec![format!(
            "AMF_TMUX_BIN={}",
            Self::shell_quote(&runtime.binary.to_string_lossy())
        )];

        if let Some(socket) = &runtime.socket {
            parts.push(format!(
                "AMF_TMUX_SOCKET={}",
                Self::shell_quote(&socket.to_string_lossy())
            ));
        }

        if include_path
            && let Some(path) = runtime.launch_path_override()
        {
            parts.push(format!("PATH={}", Self::shell_quote(&path.to_string_lossy())));
        }

        parts
    }

    fn shell_env_with(extra: &[(&str, &str)]) -> String {
        let mut parts = Self::shell_env_parts(false);
        for (key, value) in extra {
            parts.push(format!("{key}={}", Self::shell_quote(value)));
        }
        format!("env {}", parts.join(" "))
    }

    fn shell_launch_env_with(extra: &[(&str, &str)]) -> String {
        let mut parts = Self::shell_env_parts(true);
        for (key, value) in extra {
            parts.push(format!("{key}={}", Self::shell_quote(value)));
        }
        format!("env {}", parts.join(" "))
    }

    pub fn shell_env_prefix(extra: &[(&str, &str)]) -> String {
        Self::shell_env_with(extra)
    }

    pub fn shell_tmux_command(args: &[&str]) -> String {
        let runtime = Self::runtime();
        let mut parts = vec![Self::shell_quote(&runtime.binary.to_string_lossy())];
        if let Some(socket) = &runtime.socket {
            parts.push("-S".to_string());
            parts.push(Self::shell_quote(&socket.to_string_lossy()));
        }
        parts.extend(args.iter().map(|arg| Self::shell_quote(arg)));
        parts.join(" ")
    }

    fn output(args: &[&str], context: &str) -> Result<Output> {
        Self::command()
            .args(args)
            .output()
            .with_context(|| context.to_string())
    }

    fn command_error(output: &Output, fallback: &str) -> String {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if !stderr.is_empty() {
            return format!("{fallback}: {stderr}");
        }

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !stdout.is_empty() {
            return format!("{fallback}: {stdout}");
        }

        fallback.to_string()
    }

    fn run(args: &[&str], context: &str, failure: &str) -> Result<()> {
        let output = Self::output(args, context)?;
        if output.status.success() {
            Ok(())
        } else {
            bail!("{}", Self::command_error(&output, failure));
        }
    }

    fn output_indicates_socket_startup_failure(output: &Output) -> bool {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = format!("{stderr}\n{stdout}").to_ascii_lowercase();

        detail.contains("server exited unexpectedly")
            || detail.contains("error connecting to")
            || detail.contains("no server running")
            || detail.contains("couldn't create socket")
    }

    fn private_socket_server_responding() -> bool {
        Self::command()
            .arg("list-sessions")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    fn remove_stale_socket_file(socket: &Path) -> bool {
        let metadata = match fs::symlink_metadata(socket) {
            Ok(metadata) => metadata,
            Err(_) => return false,
        };

        #[cfg(unix)]
        if !metadata.file_type().is_socket() {
            log_to_file(
                LogLevel::Warn,
                "tmux",
                &format!(
                    "Refusing to remove AMF tmux socket path because it is not a socket: {}",
                    socket.display()
                ),
            );
            return false;
        }

        match fs::remove_file(socket) {
            Ok(()) => {
                log_to_file(
                    LogLevel::Warn,
                    "tmux",
                    &format!(
                        "Removed stale AMF tmux socket and will retry session start: {}",
                        socket.display()
                    ),
                );
                true
            }
            Err(err) => {
                log_to_file(
                    LogLevel::Warn,
                    "tmux",
                    &format!(
                        "Failed to remove stale AMF tmux socket {}: {err}",
                        socket.display()
                    ),
                );
                false
            }
        }
    }

    fn should_retry_after_private_socket_cleanup(args: &[&str], output: &Output) -> bool {
        if !matches!(args.first(), Some(&"new-session")) {
            return false;
        }

        let runtime = Self::runtime();
        let Some(socket) = runtime.socket.as_ref() else {
            return false;
        };

        runtime.manages_private_socket
            && socket.exists()
            && Self::output_indicates_socket_startup_failure(output)
            && !Self::private_socket_server_responding()
    }

    fn run_with_private_socket_recovery(args: &[&str], context: &str, failure: &str) -> Result<()> {
        let output = Self::output(args, context)?;
        if output.status.success() {
            return Ok(());
        }

        if Self::should_retry_after_private_socket_cleanup(args, &output) {
            if let Some(socket) = Self::runtime().socket.as_ref() {
                if Self::remove_stale_socket_file(socket) {
                    let retry_output = Self::output(args, context)?;
                    if retry_output.status.success() {
                        return Ok(());
                    }
                    bail!("{}", Self::command_error(&retry_output, failure));
                }
            }
        }

        bail!("{}", Self::command_error(&output, failure));
    }

    fn stream_child_output<R: Read + Send + 'static>(reader: Option<R>, tx: mpsc::Sender<String>) {
        if let Some(reader) = reader {
            std::thread::spawn(move || {
                for line in BufReader::new(reader)
                    .lines()
                    .map_while(std::result::Result::ok)
                {
                    let _ = tx.send(line);
                }
            });
        }
    }

    fn drain_child_output<R: Read + Send + 'static>(reader: Option<R>) {
        if let Some(reader) = reader {
            std::thread::spawn(move || for _ in BufReader::new(reader).lines() {});
        }
    }

    fn spawn(args: &[&str], context: &str) -> Result<SpawnedTmuxCommand> {
        let mut child = Self::command()
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| context.to_string())?;

        let (tx, rx) = mpsc::channel();
        Self::stream_child_output(child.stdout.take(), tx.clone());
        Self::stream_child_output(child.stderr.take(), tx);

        Ok(SpawnedTmuxCommand {
            child,
            output_rx: rx,
        })
    }

    fn tmux_command_quote(value: &str) -> String {
        let mut quoted = String::with_capacity(value.len() + 2);
        quoted.push('"');
        for ch in value.chars() {
            match ch {
                '\\' => quoted.push_str("\\\\"),
                '"' => quoted.push_str("\\\""),
                '\n' => quoted.push_str("\\n"),
                '\r' => quoted.push_str("\\r"),
                '\t' => quoted.push_str("\\t"),
                _ => quoted.push(ch),
            }
        }
        quoted.push('"');
        quoted
    }

    fn input_clients() -> &'static Mutex<HashMap<String, PersistentTmuxInputClient>> {
        static INPUT_CLIENTS: OnceLock<Mutex<HashMap<String, PersistentTmuxInputClient>>> =
            OnceLock::new();
        INPUT_CLIENTS.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn spawn_input_client(session: &str) -> Result<PersistentTmuxInputClient> {
        let mut child = Self::command()
            .args(["-C", "attach-session", "-t", session])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("Failed to spawn tmux input client for {session}"))?;

        let stdin = child
            .stdin
            .take()
            .context("Failed to open tmux input stdin")?;

        let (tx, rx) = mpsc::channel();
        Self::stream_child_output(child.stdout.take(), tx.clone());
        Self::stream_child_output(child.stderr.take(), tx);

        let mut client = PersistentTmuxInputClient {
            child,
            stdin,
            output_rx: rx,
        };
        let ready_token = format!("__AMF_INPUT_READY__{session}__");
        client.send_command(&format!("display-message -p {ready_token}\n"))?;
        client.wait_for_token(&ready_token, Duration::from_millis(500))?;

        log_to_file(
            LogLevel::Debug,
            "tmux",
            &format!("Spawned persistent tmux input client for session {session}"),
        );

        Ok(client)
    }

    fn remove_input_client(session: &str) {
        if let Ok(mut clients) = Self::input_clients().lock()
            && let Some(mut client) = clients.remove(session)
        {
            let _ = client.child.kill();
        }
    }

    fn send_via_input_client<F>(session: &str, mut send: F) -> Result<()>
    where
        F: FnMut(&mut PersistentTmuxInputClient) -> Result<()>,
    {
        {
            let mut clients = Self::input_clients()
                .lock()
                .expect("tmux input client mutex poisoned");
            let needs_respawn = match clients.get_mut(session) {
                Some(client) => !client.is_running()?,
                None => false,
            };
            if needs_respawn {
                log_to_file(
                    LogLevel::Warn,
                    "tmux",
                    &format!("Respawning dead tmux input client for session {session}"),
                );
                if let Some(mut client) = clients.remove(session) {
                    let _ = client.child.kill();
                }
            }
            let client = match clients.entry(session.to_string()) {
                std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(Self::spawn_input_client(session)?)
                }
            };
            if send(client).is_ok() {
                return Ok(());
            }
        }

        Self::remove_input_client(session);

        let mut clients = Self::input_clients()
            .lock()
            .expect("tmux input client mutex poisoned");
        let client = clients
            .entry(session.to_string())
            .or_insert(Self::spawn_input_client(session)?);
        send(client)
    }

    fn send_literal_direct(session: &str, window: &str, text: &str) -> Result<()> {
        let target = format!("{}:{}", session, window);
        Self::run(
            &["send-keys", "-t", &target, "-l", text],
            "Failed to send literal text to tmux",
            "tmux send-keys failed",
        )
    }

    fn send_key_name_direct(session: &str, window: &str, key_name: &str) -> Result<()> {
        let target = format!("{}:{}", session, window);
        Self::run(
            &["send-keys", "-t", &target, key_name],
            "Failed to send key to tmux",
            "tmux send-keys failed",
        )
    }

    fn send_keys_direct(session: &str, window: &str, keys: &str) -> Result<()> {
        let target = format!("{}:{}", session, window);
        Self::run(
            &["send-keys", "-t", &target, keys, "Enter"],
            "Failed to send keys to tmux",
            "tmux send-keys failed",
        )
    }

    /// Check if tmux is available
    pub fn check_available() -> Result<()> {
        let output = Self::command().arg("-V").output().with_context(|| {
            format!(
                "tmux is not installed or bundled. Looked for '{}'.",
                Self::runtime().binary.to_string_lossy()
            )
        })?;
        if !output.status.success() {
            bail!(
                "{}",
                Self::command_error(&output, "tmux is not working correctly")
            );
        }
        Ok(())
    }

    /// Check if a tmux session exists
    pub fn session_exists(session: &str) -> bool {
        Self::command()
            .args(["has-session", "-t", session])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Create a new tmux session with a Claude Code window and a terminal window
    pub fn create_session(session: &str, workdir: &Path) -> Result<()> {
        if Self::session_exists(session) {
            bail!("tmux session '{}' already exists", session);
        }

        let workdir_str = workdir.to_string_lossy();

        // Create detached session with first window named "claude"
        Self::run_with_private_socket_recovery(
            &[
                "new-session",
                "-d",
                "-s",
                session,
                "-n",
                "claude",
                "-c",
                &workdir_str,
            ],
            "Failed to create tmux session",
            "tmux new-session failed",
        )?;

        // Create second window named "terminal"
        let target = format!("{}:", session);
        Self::run(
            &[
                "new-window",
                "-t",
                &target,
                "-n",
                "terminal",
                "-c",
                &workdir_str,
            ],
            "Failed to create terminal window",
            "tmux new-window failed",
        )?;

        // Select the first window (claude)
        let claude_window = format!("{}:claude", session);
        Self::run(
            &["select-window", "-t", &claude_window],
            "Failed to select tmux window",
            "tmux select-window failed",
        )?;

        // Set status bar hint for navigating back
        Self::run(
            &[
                "set-option",
                "-t",
                session,
                "status-right",
                " #[fg=cyan]prefix+s#[default]: sessions ",
            ],
            "Failed to set tmux status hint",
            "tmux set-option failed",
        )?;

        Ok(())
    }

    /// Create a new tmux session with a single named first
    /// window.
    pub fn create_session_with_window(
        session: &str,
        first_window: &str,
        workdir: &Path,
    ) -> Result<()> {
        if Self::session_exists(session) {
            bail!("tmux session '{}' already exists", session);
        }

        let workdir_str = workdir.to_string_lossy();

        Self::run_with_private_socket_recovery(
            &[
                "new-session",
                "-d",
                "-s",
                session,
                "-n",
                first_window,
                "-c",
                &workdir_str,
            ],
            "Failed to create tmux session",
            "tmux new-session failed",
        )?;

        // Set status bar hint
        Self::run(
            &[
                "set-option",
                "-t",
                session,
                "status-right",
                " #[fg=cyan]prefix+s#[default]: sessions ",
            ],
            "Failed to set tmux status hint",
            "tmux set-option failed",
        )?;

        Ok(())
    }

    /// Add a new window to an existing tmux session.
    pub fn create_window(session: &str, window_name: &str, workdir: &Path) -> Result<()> {
        let workdir_str = workdir.to_string_lossy();

        let target = format!("{}:", session);
        Self::run(
            &[
                "new-window",
                "-t",
                &target,
                "-n",
                window_name,
                "-c",
                &workdir_str,
            ],
            "Failed to create tmux window",
            "tmux new-window failed",
        )?;

        Ok(())
    }

    /// Select a specific window in a tmux session.
    pub fn select_window(session: &str, window: &str) -> Result<()> {
        let target = format!("{}:{}", session, window);
        Self::run(
            &["select-window", "-t", &target],
            "Failed to select tmux window",
            "tmux select-window failed",
        )
    }

    /// Kill a single window in a tmux session.
    pub fn kill_window(session: &str, window: &str) -> Result<()> {
        let target = format!("{}:{}", session, window);
        Self::run(
            &["kill-window", "-t", &target],
            "Failed to kill tmux window",
            "tmux kill-window failed",
        )
    }

    /// List window names for a tmux session.
    pub fn list_windows(session: &str) -> Result<Vec<String>> {
        let output = Self::command()
            .args(["list-windows", "-t", session, "-F", "#{window_name}"])
            .output()
            .context("Failed to list tmux windows")?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(String::from)
            .collect())
    }

    /// Launch Claude Code in a specific window of a session
    pub fn launch_claude(
        session: &str,
        window: &str,
        resume_session_id: Option<&str>,
        extra_args: &[&str],
    ) -> Result<()> {
        let target = format!("{}:{}", session, window);

        // Use `env` to set AMF_SESSION so PreToolUse/Stop hooks
        // can identify the session. `env VAR=val cmd` works in
        // all shells including fish (unlike `VAR=val cmd`).
        let mut cmd_str = format!(
            "{} claude",
            Self::shell_launch_env_with(&[("AMF_SESSION", session)])
        );
        if let Some(sid) = resume_session_id {
            cmd_str.push_str(&format!(" --resume {}", sid));
        }
        for arg in extra_args {
            cmd_str.push(' ');
            cmd_str.push_str(arg);
        }

        Self::run(
            &["send-keys", "-t", &target, &cmd_str, "Enter"],
            "Failed to send claude command to tmux",
            "tmux send-keys failed",
        )
    }

    /// Launch opencode in a specific window of a session
    pub fn launch_opencode(session: &str, window: &str) -> Result<()> {
        Self::launch_opencode_with_session(session, window, None)
    }

    /// Launch opencode in a specific window, optionally resuming a session
    pub fn launch_opencode_with_session(
        session: &str,
        window: &str,
        resume_session_id: Option<&str>,
    ) -> Result<()> {
        let target = format!("{}:{}", session, window);
        let cmd = match resume_session_id {
            Some(id) => format!("{} opencode -s {}", Self::shell_launch_env_with(&[]), id),
            None => format!("{} opencode", Self::shell_launch_env_with(&[])),
        };

        Self::run(
            &["send-keys", "-t", &target, &cmd, "Enter"],
            "Failed to send opencode command to tmux",
            "tmux send-keys failed",
        )
    }

    /// Launch codex in a specific window of a session
    pub fn launch_codex(
        session: &str,
        window: &str,
        resume_session_id: Option<&str>,
    ) -> Result<()> {
        let target = format!("{}:{}", session, window);
        let cmd = match resume_session_id {
            Some(id) => format!(
                "{} codex resume {}",
                Self::shell_launch_env_with(&[("AMF_SESSION", session)]),
                id
            ),
            None => format!(
                "{} codex",
                Self::shell_launch_env_with(&[("AMF_SESSION", session)])
            ),
        };

        Self::run(
            &["send-keys", "-t", &target, &cmd, "Enter"],
            "Failed to send codex command to tmux",
            "tmux send-keys failed",
        )
    }

    /// Launch pi in a specific window of a session
    pub fn launch_pi(session: &str, window: &str) -> Result<()> {
        let target = format!("{}:{}", session, window);
        let cmd = format!(
            "{} pi",
            Self::shell_launch_env_with(&[("AMF_SESSION", session)])
        );

        Self::run(
            &["send-keys", "-t", &target, &cmd, "Enter"],
            "Failed to send pi command to tmux",
            "tmux send-keys failed",
        )
    }

    /// Check if we're currently running inside a tmux session
    pub fn is_inside_tmux() -> bool {
        std::env::var("TMUX").is_ok()
    }

    /// Get the name of the current tmux session (only works inside tmux)
    pub fn current_session() -> Option<String> {
        let output = Self::command()
            .args(["display-message", "-p", "#{session_name}"])
            .output()
            .ok()?;
        if output.status.success() {
            let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if name.is_empty() { None } else { Some(name) }
        } else {
            None
        }
    }

    /// Switch the tmux client to a different session (only works inside tmux)
    pub fn switch_client(session: &str) -> Result<()> {
        Self::run(
            &["switch-client", "-t", session],
            "Failed to switch tmux client",
            "tmux switch-client failed",
        )
    }

    /// Attach to a session (replaces current terminal)
    pub fn attach_session(session: &str) -> Result<()> {
        if !Self::session_exists(session) {
            bail!("tmux session '{}' does not exist", session);
        }

        let output = Self::output(
            &["switch-client", "-t", session],
            "Failed to switch tmux client",
        )?;

        if output.status.success() {
            return Ok(());
        }

        Self::run(
            &["attach-session", "-t", session],
            "Failed to attach to tmux session",
            "tmux attach-session failed",
        )
    }

    /// Set an environment variable in a tmux session so it is
    /// inherited by all processes started in that session.
    pub fn set_session_env(session: &str, key: &str, value: &str) -> Result<()> {
        Self::run(
            &["set-environment", "-t", session, key, value],
            "Failed to set tmux session environment",
            "tmux set-environment failed",
        )
    }

    /// Kill a tmux session
    pub fn kill_session(session: &str) -> Result<()> {
        if !Self::session_exists(session) {
            return Ok(());
        }

        Self::remove_input_client(session);
        Self::run(
            &["kill-session", "-t", session],
            "Failed to kill tmux session",
            "tmux kill-session failed",
        )
    }

    pub fn spawn_kill_session(session: &str) -> Result<Option<SpawnedTmuxCommand>> {
        if !Self::session_exists(session) {
            return Ok(None);
        }

        Ok(Some(Self::spawn(
            &["kill-session", "-t", session],
            "Failed to spawn tmux kill-session",
        )?))
    }

    /// List all amf-* tmux sessions
    pub fn list_sessions() -> Result<Vec<String>> {
        let output = Self::command()
            .args(["list-sessions", "-F", "#{session_name}"])
            .output();

        match output {
            Ok(o) if o.status.success() => {
                let sessions: Vec<String> = String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .filter(|s| s.starts_with("amf-"))
                    .map(String::from)
                    .collect();
                Ok(sessions)
            }
            _ => Ok(Vec::new()),
        }
    }

    /// Capture the current pane content of a session's window
    pub fn capture_pane(session: &str, window: &str) -> Result<String> {
        let target = format!("{}:{}", session, window);
        let output = Self::command()
            .args(["capture-pane", "-t", &target, "-p"])
            .output()
            .context("Failed to capture pane")?;

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Capture pane content with ANSI escape sequences preserved
    pub fn capture_pane_ansi(session: &str, window: &str) -> Result<String> {
        Self::capture_pane_ansi_from_line(session, window, 0, 0)
    }

    /// Capture pane content with ANSI, starting from a specific line offset.
    /// top_skip: number of lines to skip from the top of the visible pane
    /// extra_lines: additional lines to capture from scrollback to fill the gap
    pub fn capture_pane_ansi_from_line(
        session: &str,
        window: &str,
        top_skip: u16,
        extra_lines: u16,
    ) -> Result<String> {
        let target = format!("{}:{}", session, window);
        let total_skip = top_skip as i32 + extra_lines as i32;
        let output = if total_skip > 0 {
            let start = format!("-{}", total_skip);
            Self::command()
                .args(["capture-pane", "-t", &target, "-e", "-p", "-S", &start])
                .output()
                .context("Failed to capture pane with ANSI")?
        } else {
            Self::command()
                .args(["capture-pane", "-t", &target, "-e", "-p"])
                .output()
                .context("Failed to capture pane with ANSI")?
        };

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Capture pane content with ANSI sequences, including scrollback history
    /// Returns the raw content and the number of lines captured
    pub fn capture_pane_with_history(
        session: &str,
        window: &str,
        history_lines: i32,
    ) -> Result<(String, usize)> {
        let target = format!("{}:{}", session, window);
        let start = format!("-{}", history_lines);
        let output = Self::command()
            .args(["capture-pane", "-t", &target, "-e", "-p", "-S", &start])
            .output()
            .context("Failed to capture pane with history")?;

        let content = String::from_utf8_lossy(&output.stdout).to_string();
        let lines = content.lines().count();
        Ok((content, lines))
    }

    /// Check if the pane is using alternate screen mode (like vim/opencode)
    pub fn is_alternate_screen(session: &str, window: &str) -> bool {
        let target = format!("{}:{}", session, window);
        Self::command()
            .args(["display-message", "-t", &target, "-p", "#{alternate_on}"])
            .output()
            .map(|o| {
                let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                s == "1"
            })
            .unwrap_or(false)
    }

    /// Get the cursor position in a tmux pane (x, y)
    pub fn cursor_position(session: &str, window: &str) -> Result<(u16, u16)> {
        let target = format!("{}:{}", session, window);
        let output = Self::command()
            .args([
                "display-message",
                "-t",
                &target,
                "-p",
                "#{cursor_x} #{cursor_y}",
            ])
            .output()
            .context("Failed to get cursor position")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let parts: Vec<&str> = stdout.split_whitespace().collect();
        if parts.len() == 2 {
            let x: u16 = parts[0].parse().unwrap_or(0);
            let y: u16 = parts[1].parse().unwrap_or(0);
            Ok((x, y))
        } else {
            Ok((0, 0))
        }
    }

    /// Start piping pane output to a FIFO path.
    /// Returns Ok(()) if the pipe-pane command succeeds.
    pub fn start_pipe_pane(session: &str, window: &str, fifo_path: &Path) -> Result<()> {
        let target = format!("{}:{}", session, window);
        let cmd = format!("cat > {}", Self::shell_quote(&fifo_path.to_string_lossy()));
        Self::run(
            &["pipe-pane", "-t", &target, &cmd],
            "Failed to start pipe-pane",
            "tmux pipe-pane failed",
        )
    }

    /// Stop piping pane output (pass empty command to cancel).
    pub fn stop_pipe_pane(session: &str, window: &str) -> Result<()> {
        let target = format!("{}:{}", session, window);
        Self::run(
            &["pipe-pane", "-t", &target],
            "Failed to stop pipe-pane",
            "tmux pipe-pane cancel failed",
        )
    }

    /// Resize a tmux pane to match the TUI rendering area
    pub fn resize_pane(session: &str, window: &str, cols: u16, rows: u16) -> Result<()> {
        let target = format!("{}:{}", session, window);
        Self::run(
            &[
                "resize-window",
                "-t",
                &target,
                "-x",
                &cols.to_string(),
                "-y",
                &rows.to_string(),
            ],
            "Failed to resize tmux pane",
            "tmux resize-window failed",
        )
    }

    /// Send literal text to a tmux pane (no key name interpretation)
    pub fn send_literal(session: &str, window: &str, text: &str) -> Result<()> {
        if !Self::persistent_input_enabled() {
            return Self::send_literal_direct(session, window, text);
        }

        let target = format!("{}:{}", session, window);
        let quoted_target = Self::tmux_command_quote(&target);
        let quoted_text = Self::tmux_command_quote(text);
        match Self::send_via_input_client(session, |client| {
            client.send_command(&format!(
                "send-keys -t {quoted_target} -l {quoted_text}\n"
            ))
        })
        {
            Ok(()) => Ok(()),
            Err(err) => {
                log_to_file(
                    LogLevel::Warn,
                    "tmux",
                    &format!(
                        "Persistent tmux input client failed for {target}; falling back to direct send-keys: {err}"
                    ),
                );
                Self::send_literal_direct(session, window, text).with_context(|| {
                    format!(
                        "Failed to send literal text to tmux target {target} after input client fallback"
                    )
                })
            }
        }
    }

    /// Paste text into a tmux pane using bracketed paste mode.
    /// Uses set-buffer + paste-buffer -p so the inner
    /// application receives proper bracketed paste sequences.
    pub fn paste_text(session: &str, window: &str, text: &str) -> Result<()> {
        let target = format!("{}:{}", session, window);

        // Load text into a tmux paste buffer
        Self::run(
            &["set-buffer", "--", text],
            "Failed to set tmux buffer",
            "tmux set-buffer failed",
        )?;

        // Paste with -p flag for bracketed paste indicators
        Self::run(
            &["paste-buffer", "-t", &target, "-p"],
            "Failed to paste buffer to tmux",
            "tmux paste-buffer failed",
        )
    }

    /// Send a named key (e.g. Enter, Up, BSpace) to a tmux pane
    pub fn send_key_name(session: &str, window: &str, key_name: &str) -> Result<()> {
        if !Self::persistent_input_enabled() {
            return Self::send_key_name_direct(session, window, key_name);
        }

        let target = format!("{}:{}", session, window);
        let quoted_target = Self::tmux_command_quote(&target);
        match Self::send_via_input_client(session, |client| {
            client.send_command(&format!("send-keys -t {quoted_target} {key_name}\n"))
        })
        {
            Ok(()) => Ok(()),
            Err(err) => {
                log_to_file(
                    LogLevel::Warn,
                    "tmux",
                    &format!(
                        "Persistent tmux input client failed for {target}; falling back to direct send-keys: {err}"
                    ),
                );
                Self::send_key_name_direct(session, window, key_name).with_context(|| {
                    format!("Failed to send key to tmux target {target} after input client fallback")
                })
            }
        }
    }

    /// Send keys to a specific window in a session
    pub fn send_keys(session: &str, window: &str, keys: &str) -> Result<()> {
        if !Self::persistent_input_enabled() {
            return Self::send_keys_direct(session, window, keys);
        }

        let target = format!("{}:{}", session, window);
        let quoted_target = Self::tmux_command_quote(&target);
        let quoted_keys = Self::tmux_command_quote(keys);
        match Self::send_via_input_client(session, |client| {
            client.send_command(&format!("send-keys -t {quoted_target} {quoted_keys} Enter\n"))
        })
        {
            Ok(()) => Ok(()),
            Err(err) => {
                log_to_file(
                    LogLevel::Warn,
                    "tmux",
                    &format!(
                        "Persistent tmux input client failed for {target}; falling back to direct send-keys: {err}"
                    ),
                );
                Self::send_keys_direct(session, window, keys).with_context(|| {
                    format!("Failed to send keys to tmux target {target} after input client fallback")
                })
            }
        }
    }

    /// Enter tmux copy mode for a pane
    pub fn enter_copy_mode(session: &str, window: &str) -> Result<()> {
        let target = format!("{}:{}", session, window);
        Self::run(
            &["copy-mode", "-t", &target],
            "Failed to enter tmux copy mode",
            "tmux copy-mode failed",
        )
    }

    /// Exit tmux copy mode for a pane (send q to exit)
    pub fn exit_copy_mode(session: &str, window: &str) -> Result<()> {
        let target = format!("{}:{}", session, window);
        Self::run(
            &["send-keys", "-t", &target, "-X", "cancel"],
            "Failed to exit tmux copy mode",
            "tmux send-keys failed",
        )
    }
}

// ── TmuxOps trait implementation ─────────────────────────────────────────────

impl TmuxOps for TmuxManager {
    fn session_exists(&self, session: &str) -> bool {
        TmuxManager::session_exists(session)
    }

    fn list_sessions(&self) -> Result<Vec<String>> {
        TmuxManager::list_sessions()
    }

    fn create_session_with_window(
        &self,
        session: &str,
        first_window: &str,
        workdir: &Path,
    ) -> Result<()> {
        TmuxManager::create_session_with_window(session, first_window, workdir)
    }

    fn set_session_env(&self, session: &str, key: &str, value: &str) -> Result<()> {
        TmuxManager::set_session_env(session, key, value)
    }

    fn create_window(&self, session: &str, window: &str, workdir: &Path) -> Result<()> {
        TmuxManager::create_window(session, window, workdir)
    }

    fn launch_claude(
        &self,
        session: &str,
        window: &str,
        resume_id: Option<String>,
        extra_args: Vec<String>,
    ) -> Result<()> {
        let refs: Vec<&str> = extra_args.iter().map(|s| s.as_str()).collect();
        TmuxManager::launch_claude(session, window, resume_id.as_deref(), &refs)
    }

    fn launch_opencode(&self, session: &str, window: &str) -> Result<()> {
        TmuxManager::launch_opencode(session, window)
    }

    fn launch_opencode_with_session(
        &self,
        session: &str,
        window: &str,
        resume_id: Option<String>,
    ) -> Result<()> {
        TmuxManager::launch_opencode_with_session(session, window, resume_id.as_deref())
    }

    fn launch_codex(&self, session: &str, window: &str, resume_id: Option<String>) -> Result<()> {
        TmuxManager::launch_codex(session, window, resume_id.as_deref())
    }

    fn launch_pi(&self, session: &str, window: &str) -> Result<()> {
        TmuxManager::launch_pi(session, window)
    }

    fn send_keys(&self, session: &str, window: &str, keys: &str) -> Result<()> {
        TmuxManager::send_keys(session, window, keys)
    }

    fn send_literal(&self, session: &str, window: &str, text: &str) -> Result<()> {
        TmuxManager::send_literal(session, window, text)
    }

    fn paste_text(&self, session: &str, window: &str, text: &str) -> Result<()> {
        TmuxManager::paste_text(session, window, text)
    }

    fn send_key_name(&self, session: &str, window: &str, key_name: &str) -> Result<()> {
        TmuxManager::send_key_name(session, window, key_name)
    }

    fn resize_pane(&self, session: &str, window: &str, cols: u16, rows: u16) -> Result<()> {
        TmuxManager::resize_pane(session, window, cols, rows)
    }

    fn select_window(&self, session: &str, window: &str) -> Result<()> {
        TmuxManager::select_window(session, window)
    }

    fn kill_session(&self, session: &str) -> Result<()> {
        TmuxManager::kill_session(session)
    }
}

#[cfg(test)]
mod tests {
    use super::TmuxManager;
    use std::fs;
    use std::process::{ExitStatus, Output};

    #[cfg(unix)]
    use std::os::unix::net::UnixListener;
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt;
    use tempfile::TempDir;

    #[cfg(unix)]
    fn failure_status() -> ExitStatus {
        ExitStatus::from_raw(1)
    }

    #[cfg(windows)]
    fn failure_status() -> ExitStatus {
        use std::os::windows::process::ExitStatusExt;
        ExitStatus::from_raw(1)
    }

    fn output_with(stderr: &str, stdout: &str) -> Output {
        Output {
            status: failure_status(),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[test]
    fn detects_tmux_socket_startup_failures_from_stderr() {
        let output = output_with("server exited unexpectedly", "");
        assert!(TmuxManager::output_indicates_socket_startup_failure(
            &output
        ));
    }

    #[test]
    fn ignores_unrelated_tmux_failures() {
        let output = output_with("can't find pane", "");
        assert!(!TmuxManager::output_indicates_socket_startup_failure(
            &output
        ));
    }

    #[cfg(unix)]
    #[test]
    fn removes_stale_socket_files() {
        let temp_dir = TempDir::new().unwrap();
        let socket_path = temp_dir.path().join("tmux.sock");

        let listener = UnixListener::bind(&socket_path).unwrap();
        drop(listener);

        assert!(socket_path.exists());
        assert!(TmuxManager::remove_stale_socket_file(&socket_path));
        assert!(!socket_path.exists());
    }

    #[test]
    fn does_not_remove_regular_files() {
        let temp_dir = TempDir::new().unwrap();
        let socket_path = temp_dir.path().join("tmux.sock");
        fs::write(&socket_path, "not a socket").unwrap();

        assert!(socket_path.exists());
        assert!(!TmuxManager::remove_stale_socket_file(&socket_path));
        assert!(socket_path.exists());
    }

    #[test]
    fn shell_env_prefix_does_not_export_path() {
        let prefix = TmuxManager::shell_env_prefix(&[("AMF_SESSION", "amf-test")]);
        assert!(prefix.starts_with("env "));
        assert!(prefix.contains("AMF_TMUX_BIN="));
        assert!(prefix.contains("AMF_SESSION='amf-test'"));
        assert!(!prefix.contains("PATH="));
    }
}
