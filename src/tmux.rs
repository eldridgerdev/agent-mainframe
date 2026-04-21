use anyhow::{Context, Result, bail};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicBool, Ordering as AtomicOrdering},
    mpsc::{self, Receiver, RecvTimeoutError},
};
use std::time::Duration;

#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd};
#[cfg(unix)]
use std::os::unix::fs::FileTypeExt;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use uuid::Uuid;

use crate::debug::{LogLevel, log_to_file};
use crate::traits::TmuxOps;

pub struct TmuxManager;

static TMUX_CONTROL_MODE_ENABLED: AtomicBool = AtomicBool::new(true);

pub struct SpawnedTmuxCommand {
    pub child: Child,
    pub output_rx: Receiver<String>,
}

pub struct SpawnedTmuxControlClient {
    pub child: Child,
    writer: TmuxInputWriter,
    output_rx: Receiver<String>,
}

impl SpawnedTmuxControlClient {
    pub fn send_command(&mut self, command: &str) -> Result<()> {
        self.writer.write_command(command)
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<Option<String>> {
        match self.output_rx.recv_timeout(timeout) {
            Ok(line) => Ok(Some(line)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => {
                bail!("tmux control client output stream disconnected");
            }
        }
    }

    pub fn try_recv(&self) -> Result<Option<String>> {
        match self.output_rx.try_recv() {
            Ok(line) => Ok(Some(line)),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => {
                bail!("tmux control client output stream disconnected");
            }
        }
    }

    pub fn is_running(&mut self) -> Result<bool> {
        Ok(self
            .child
            .try_wait()
            .context("Failed to poll tmux control client process")?
            .is_none())
    }

    pub fn wait_for_token(&mut self, token: &str, timeout: Duration) -> Result<()> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if !self.is_running()? {
                bail!("tmux control client exited before acknowledging readiness");
            }

            let now = std::time::Instant::now();
            if now >= deadline {
                bail!("timed out waiting for tmux control client readiness");
            }

            let remaining = deadline.saturating_duration_since(now);
            match self.recv_timeout(remaining.min(Duration::from_millis(25)))? {
                Some(line) => {
                    log_to_file(
                        LogLevel::Debug,
                        "tmux",
                        &format!("tmux control view recv: {line}"),
                    );
                    if line.contains(token) {
                        return Ok(());
                    }
                }
                None => continue,
            }
        }
    }
}

impl Drop for SpawnedTmuxControlClient {
    fn drop(&mut self) {
        let _ = self.send_command("detach-client\n");
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct PersistentTmuxInputClient {
    child: Child,
    writer: TmuxInputWriter,
    output_rx: Receiver<String>,
}

impl Drop for PersistentTmuxInputClient {
    fn drop(&mut self) {
        let _ = self.send_command("detach-client\n");
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

enum TmuxInputWriter {
    Pipe(std::process::ChildStdin),
    #[cfg(unix)]
    Pty(File),
}

impl TmuxInputWriter {
    fn write_command(&mut self, command: &str) -> Result<()> {
        match self {
            Self::Pipe(stdin) => {
                stdin
                    .write_all(command.as_bytes())
                    .context("Failed to write tmux input command")?;
                stdin.flush().context("Failed to flush tmux input command")
            }
            #[cfg(unix)]
            Self::Pty(writer) => {
                writer
                    .write_all(command.as_bytes())
                    .context("Failed to write tmux PTY input command")?;
                writer
                    .flush()
                    .context("Failed to flush tmux PTY input command")
            }
        }
    }
}

impl PersistentTmuxInputClient {
    fn send_command(&mut self, command: &str) -> Result<()> {
        self.writer.write_command(command)
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
            match self
                .output_rx
                .recv_timeout(remaining.min(Duration::from_millis(25)))
            {
                Ok(line) => {
                    log_to_file(
                        LogLevel::Debug,
                        "tmux",
                        &format!("tmux control recv: {line}"),
                    );
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TmuxInputTransportMode {
    Direct,
    ControlPty,
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
        let socket_override = std::env::var_os("AMF_TMUX_SOCKET").map(PathBuf::from);
        let binary_override = std::env::var_os("AMF_TMUX_BIN");
        Self::detect_from_env(
            tmux_env.as_deref(),
            socket_override,
            binary_override,
            Self::env_flag_enabled("AMF_TMUX_DEDICATED_SOCKET")
                || TMUX_CONTROL_MODE_ENABLED.load(AtomicOrdering::Relaxed),
        )
    }

    fn detect_from_env(
        tmux_env: Option<&str>,
        socket_override: Option<PathBuf>,
        binary_override: Option<OsString>,
        using_dedicated_socket: bool,
    ) -> Self {
        let using_existing_tmux = tmux_env.is_some();
        let inherits_ambient_tmux = using_existing_tmux && !using_dedicated_socket;

        let binary = binary_override
            .or_else(|| {
                if inherits_ambient_tmux {
                    None
                } else {
                    Self::bundled_binary()
                }
            })
            .unwrap_or_else(|| OsString::from("tmux"));

        let (socket, manages_private_socket) = if let Some(socket) = socket_override {
            (Some(socket), false)
        } else if using_dedicated_socket {
            (Some(Self::dedicated_socket_path()), true)
        } else if let Some(socket) = tmux_env.and_then(Self::socket_from_tmux_env) {
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

    fn env_flag_enabled(key: &str) -> bool {
        std::env::var(key)
            .ok()
            .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "on" | "ON"))
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

    fn owns_tmux_env_client(&self, tmux_env: Option<&str>) -> bool {
        let Some(tmux_env) = tmux_env else {
            return false;
        };

        match &self.socket {
            Some(socket) => Self::socket_from_tmux_env(tmux_env)
                .is_some_and(|tmux_socket| tmux_socket == *socket),
            None => true,
        }
    }

    fn state_dir() -> PathBuf {
        dirs::state_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("amf")
    }

    fn private_socket_path() -> PathBuf {
        Self::state_dir().join("tmux.sock")
    }

    fn dedicated_socket_path() -> PathBuf {
        Self::state_dir().join("managed-tmux.sock")
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
    pub fn configure_control_mode(enabled: bool) {
        TMUX_CONTROL_MODE_ENABLED.store(enabled, AtomicOrdering::Relaxed);
    }

    fn input_transport_mode() -> TmuxInputTransportMode {
        if let Ok(value) = std::env::var("AMF_TMUX_INPUT_TRANSPORT") {
            match value.trim() {
                "control-pty" | "pty" => return TmuxInputTransportMode::ControlPty,
                _ => return TmuxInputTransportMode::Direct,
            }
        }

        if TmuxRuntime::env_flag_enabled("AMF_EXPERIMENTAL_PERSISTENT_TMUX_INPUT") {
            return TmuxInputTransportMode::ControlPty;
        }

        if TMUX_CONTROL_MODE_ENABLED.load(AtomicOrdering::Relaxed) {
            TmuxInputTransportMode::ControlPty
        } else {
            TmuxInputTransportMode::Direct
        }
    }

    pub fn uses_control_pty_input() -> bool {
        Self::input_transport_mode() == TmuxInputTransportMode::ControlPty
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

        if include_path && let Some(path) = runtime.launch_path_override() {
            parts.push(format!(
                "PATH={}",
                Self::shell_quote(&path.to_string_lossy())
            ));
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

    fn pane_default_terminal() -> Option<String> {
        if let Some(value) = std::env::var_os("AMF_TMUX_DEFAULT_TERMINAL") {
            let value = value.to_string_lossy().trim().to_string();
            return if value.is_empty() { None } else { Some(value) };
        }

        if cfg!(target_os = "macos") {
            // macOS's system terminfo database often lacks tmux-256color,
            // which makes shells and TUIs warn when a pane starts.
            Some("screen-256color".to_string())
        } else {
            None
        }
    }

    fn should_set_global_default_terminal() -> bool {
        Self::runtime().manages_private_socket
            || std::env::var_os("AMF_TMUX_DEFAULT_TERMINAL").is_some()
    }

    fn global_default_terminal_args(
        term: &str,
        bootstrap_session: Option<&str>,
    ) -> Vec<Vec<String>> {
        let mut commands = Vec::new();

        if let Some(bootstrap_session) = bootstrap_session {
            commands.push(vec![
                "new-session".to_string(),
                "-d".to_string(),
                "-s".to_string(),
                bootstrap_session.to_string(),
            ]);
        }

        commands.push(vec![
            "set-option".to_string(),
            "-g".to_string(),
            "default-terminal".to_string(),
            term.to_string(),
        ]);

        if let Some(bootstrap_session) = bootstrap_session {
            commands.push(vec![
                "kill-session".to_string(),
                "-t".to_string(),
                bootstrap_session.to_string(),
            ]);
        }

        commands
    }

    fn set_global_default_terminal_if_needed() -> Result<()> {
        let Some(term) = Self::pane_default_terminal() else {
            return Ok(());
        };
        if !Self::should_set_global_default_terminal() {
            return Ok(());
        }

        let bootstrap_session = Self::runtime()
            .manages_private_socket
            .then(|| format!("__amf-bootstrap-{}", Uuid::new_v4().simple()));

        if let Some(session) = bootstrap_session.as_deref() {
            Self::run_with_private_socket_recovery(
                &["new-session", "-d", "-s", session],
                "Failed to start tmux server for default terminal configuration",
                "tmux new-session failed",
            )?;
        }

        let result = Self::run_with_private_socket_recovery(
            &["set-option", "-g", "default-terminal", &term],
            "Failed to configure tmux default terminal",
            "tmux set-option failed",
        );

        if let Some(session) = bootstrap_session.as_deref() {
            let _ = Self::run(
                &["kill-session", "-t", session],
                "Failed to clean up tmux bootstrap session",
                "tmux kill-session failed",
            );
        }

        result
    }

    fn set_session_default_terminal_if_needed(session: &str) -> Result<()> {
        let Some(term) = Self::pane_default_terminal() else {
            return Ok(());
        };

        Self::run(
            &["set-option", "-t", session, "default-terminal", &term],
            "Failed to configure tmux session default terminal",
            "tmux set-option failed",
        )
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
        if !matches!(args.first().copied(), Some("new-session" | "set-option")) {
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
                let mut reader = BufReader::new(reader);
                let mut buf = [0u8; 8192];
                let mut line = Vec::with_capacity(8192);

                loop {
                    let Ok(n) = reader.read(&mut buf) else {
                        break;
                    };
                    if n == 0 {
                        break;
                    }

                    for &byte in &buf[..n] {
                        line.push(byte);
                        let is_line_end =
                            matches!(byte, b'\n' | b'\r') || line.ends_with(b"\x1b\\");
                        if is_line_end {
                            let text = String::from_utf8_lossy(&line).into_owned();
                            let _ = tx.send(text);
                            line.clear();
                        }
                    }
                }

                if !line.is_empty() {
                    let text = String::from_utf8_lossy(&line).into_owned();
                    let _ = tx.send(text);
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

    fn cleaned_control_sessions() -> &'static Mutex<HashSet<String>> {
        static CLEANED_CONTROL_SESSIONS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
        CLEANED_CONTROL_SESSIONS.get_or_init(|| Mutex::new(HashSet::new()))
    }

    fn bypassed_control_input_sessions() -> &'static Mutex<HashSet<String>> {
        static BYPASSED_CONTROL_INPUT_SESSIONS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
        BYPASSED_CONTROL_INPUT_SESSIONS.get_or_init(|| Mutex::new(HashSet::new()))
    }

    fn ensure_stale_control_clients_detached(session: &str) {
        {
            let Ok(mut cleaned) = Self::cleaned_control_sessions().lock() else {
                return;
            };
            if !cleaned.insert(session.to_string()) {
                return;
            }
        }

        let output = match Self::command()
            .args([
                "list-clients",
                "-t",
                session,
                "-F",
                "#{client_name}\t#{client_flags}",
            ])
            .output()
        {
            Ok(output) => output,
            Err(err) => {
                log_to_file(
                    LogLevel::Warn,
                    "tmux",
                    &format!("Failed to list tmux control clients for cleanup: {err}"),
                );
                return;
            }
        };

        if !output.status.success() {
            return;
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut detached = 0usize;
        for line in stdout.lines() {
            let Some((client_name, flags)) = line.split_once('\t') else {
                continue;
            };
            if flags.contains("control-mode")
                && (flags.contains("ignore-size") || flags.contains("no-output"))
            {
                let _ = Self::command()
                    .args(["detach-client", "-t", client_name])
                    .status();
                detached += 1;
            }
        }

        if detached > 0 {
            log_to_file(
                LogLevel::Warn,
                "tmux",
                &format!("Detached {detached} stale tmux control client(s) for session {session}"),
            );
        }
    }

    fn control_client_count(session: &str) -> usize {
        let Ok(output) = Self::command()
            .args(["list-clients", "-t", session, "-F", "#{client_flags}"])
            .output()
        else {
            return 0;
        };

        if !output.status.success() {
            return 0;
        }

        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|flags| flags.contains("control-mode"))
            .count()
    }

    fn should_bypass_control_input(session: &str) -> bool {
        const MAX_CONTROL_CLIENTS_BEFORE_DIRECT_FALLBACK: usize = 16;
        if let Ok(bypassed) = Self::bypassed_control_input_sessions().lock()
            && bypassed.contains(session)
        {
            return true;
        }

        let count = Self::control_client_count(session);
        if count <= MAX_CONTROL_CLIENTS_BEFORE_DIRECT_FALLBACK {
            return false;
        }

        if let Ok(mut bypassed) = Self::bypassed_control_input_sessions().lock() {
            bypassed.insert(session.to_string());
        }

        log_to_file(
            LogLevel::Warn,
            "tmux",
            &format!(
                "Bypassing persistent tmux control input for session {session}; found {count} control clients"
            ),
        );
        true
    }

    fn mark_control_input_bypassed(session: &str, reason: &str) {
        if let Ok(mut bypassed) = Self::bypassed_control_input_sessions().lock() {
            bypassed.insert(session.to_string());
        }
        log_to_file(
            LogLevel::Warn,
            "tmux",
            &format!("Bypassing persistent tmux control input for session {session}: {reason}"),
        );
    }

    #[cfg(unix)]
    fn open_pty(cols: u16, rows: u16) -> Result<(File, File)> {
        let mut master = -1;
        let mut slave = -1;
        let mut winsize = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let mut termios: libc::termios = unsafe { std::mem::zeroed() };

        unsafe {
            if libc::tcgetattr(libc::STDIN_FILENO, &mut termios) == -1 {
                return Err(std::io::Error::last_os_error())
                    .context("Failed to read terminal attributes for tmux PTY");
            }
            libc::cfmakeraw(&mut termios);
        }

        let result = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                &mut termios,
                &mut winsize,
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error())
                .context("Failed to open PTY for tmux control client");
        }

        let master = unsafe { File::from_raw_fd(master) };
        let slave = unsafe { File::from_raw_fd(slave) };
        Ok((master, slave))
    }

    #[cfg(unix)]
    fn spawn_input_client_control_pty(session: &str) -> Result<PersistentTmuxInputClient> {
        Self::ensure_stale_control_clients_detached(session);
        let (master, slave) = Self::open_pty(120, 40)?;
        let reader = master
            .try_clone()
            .context("Failed to clone tmux PTY reader")?;
        let stdin_slave = slave
            .try_clone()
            .context("Failed to clone tmux PTY stdin")?;
        let stdout_slave = slave
            .try_clone()
            .context("Failed to clone tmux PTY stdout")?;
        let slave_fd = slave.as_raw_fd();

        let mut child = Self::command();
        unsafe {
            child
                .args([
                    "-CC",
                    "attach-session",
                    "-f",
                    "no-output,ignore-size",
                    "-t",
                    session,
                ])
                .stdin(Stdio::from(stdin_slave))
                .stdout(Stdio::from(stdout_slave))
                .stderr(Stdio::from(slave))
                .pre_exec(move || {
                    if libc::setsid() == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::ioctl(slave_fd, libc::TIOCSCTTY.into(), 0) == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
        }
        child.env_remove("TMUX").env_remove("TMUX_PANE");

        let child = child
            .spawn()
            .with_context(|| format!("Failed to spawn PTY tmux input client for {session}"))?;

        let (tx, rx) = mpsc::channel();
        Self::stream_child_output(Some(reader), tx);

        let mut client = PersistentTmuxInputClient {
            child,
            writer: TmuxInputWriter::Pty(master),
            output_rx: rx,
        };
        let ready_token = format!("__AMF_INPUT_READY__{}__", std::process::id());
        client.send_command(&format!("display-message -p {ready_token}\n"))?;
        client.wait_for_token(&ready_token, Duration::from_millis(250))?;

        log_to_file(
            LogLevel::Debug,
            "tmux",
            &format!("Spawned ready PTY tmux input client for session {session}"),
        );

        Ok(client)
    }

    pub fn resolve_view_target_ids(session: &str, window: &str) -> Result<(String, String)> {
        let target = format!("{}:{}", session, window);
        let output = Self::command()
            .args([
                "display-message",
                "-t",
                &target,
                "-p",
                "#{window_id} #{pane_id}",
            ])
            .output()
            .context("Failed to resolve tmux view target IDs")?;

        if !output.status.success() {
            bail!(
                "{}",
                Self::command_error(&output, "tmux display-message failed")
            );
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let parts: Vec<&str> = stdout.split_whitespace().collect();
        if parts.len() == 2 {
            Ok((parts[0].to_string(), parts[1].to_string()))
        } else {
            bail!("tmux did not return window_id and pane_id for {target}");
        }
    }

    #[cfg(unix)]
    pub fn spawn_control_mode_view_client(
        session: &str,
        window: &str,
        pane_id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<SpawnedTmuxControlClient> {
        Self::ensure_stale_control_clients_detached(session);
        let (master, slave) = Self::open_pty(cols, rows)?;
        let reader = master
            .try_clone()
            .context("Failed to clone tmux control PTY reader")?;
        let stdin_slave = slave
            .try_clone()
            .context("Failed to clone tmux control PTY stdin")?;
        let stdout_slave = slave
            .try_clone()
            .context("Failed to clone tmux control PTY stdout")?;
        let slave_fd = slave.as_raw_fd();

        let mut child = Self::command();
        unsafe {
            child
                .args(["-CC", "attach-session", "-f", "ignore-size", "-t", session])
                .stdin(Stdio::from(stdin_slave))
                .stdout(Stdio::from(stdout_slave))
                .stderr(Stdio::from(slave))
                .pre_exec(move || {
                    if libc::setsid() == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::ioctl(slave_fd, libc::TIOCSCTTY.into(), 0) == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
        }
        child.env_remove("TMUX").env_remove("TMUX_PANE");

        let child = child.spawn().with_context(|| {
            format!("Failed to spawn PTY tmux control view client for {session}")
        })?;

        let (tx, rx) = mpsc::channel();
        Self::stream_child_output(Some(reader), tx);

        let mut client = SpawnedTmuxControlClient {
            child,
            writer: TmuxInputWriter::Pty(master),
            output_rx: rx,
        };
        let target = format!("{session}:{window}");
        let quoted_target = Self::tmux_command_quote(&target);
        client.send_command(&format!("select-window -t {quoted_target}\n"))?;
        client.send_command(&format!("refresh-client -A {pane_id}:on\n"))?;
        client.send_command(&format!("refresh-client -C {cols},{rows}\n"))?;
        let ready_token = format!("__AMF_VIEW_READY__{}__", std::process::id());
        client.send_command(&format!("display-message -p {ready_token}\n"))?;
        client.wait_for_token(&ready_token, Duration::from_millis(250))?;

        log_to_file(
            LogLevel::Debug,
            "tmux",
            &format!("Spawned ready PTY tmux control view client for {session}:{window}"),
        );

        Ok(client)
    }

    #[cfg(not(unix))]
    pub fn spawn_control_mode_view_client(
        _session: &str,
        _window: &str,
        _pane_id: &str,
        _cols: u16,
        _rows: u16,
    ) -> Result<SpawnedTmuxControlClient> {
        bail!("tmux PTY control view is only supported on Unix");
    }

    fn spawn_input_client(session: &str) -> Result<PersistentTmuxInputClient> {
        #[cfg(unix)]
        {
            return Self::spawn_input_client_control_pty(session);
        }

        #[allow(unreachable_code)]
        {
            bail!("tmux PTY control input is only supported on Unix");
        }
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
                    let client = match Self::spawn_input_client(session) {
                        Ok(client) => client,
                        Err(err) => {
                            Self::mark_control_input_bypassed(session, &err.to_string());
                            return Err(err);
                        }
                    };
                    entry.insert(client)
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
        let client = match clients.entry(session.to_string()) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let client = match Self::spawn_input_client(session) {
                    Ok(client) => client,
                    Err(err) => {
                        Self::mark_control_input_bypassed(session, &err.to_string());
                        return Err(err);
                    }
                };
                entry.insert(client)
            }
        };
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
        let runtime = Self::runtime();
        let output = Self::command().arg("-V").output().with_context(|| {
            format!(
                "tmux is not installed or bundled. Looked for '{}'.",
                runtime.binary.to_string_lossy()
            )
        })?;
        if !output.status.success() {
            bail!(
                "{}",
                Self::command_error(&output, "tmux is not working correctly")
            );
        }
        let socket = runtime
            .socket
            .as_ref()
            .map(|socket| socket.display().to_string())
            .unwrap_or_else(|| "<ambient default>".to_string());
        let ownership = if runtime.manages_private_socket {
            "managed"
        } else {
            "external"
        };
        log_to_file(
            LogLevel::Info,
            "tmux",
            &format!(
                "Using tmux binary '{}' with {ownership} socket {socket}",
                runtime.binary.to_string_lossy()
            ),
        );
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
        Self::set_global_default_terminal_if_needed()?;

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

        Self::set_session_default_terminal_if_needed(session)?;

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
        Self::set_global_default_terminal_if_needed()?;

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

        Self::set_session_default_terminal_if_needed(session)?;

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
        Self::set_session_default_terminal_if_needed(session)?;

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
        let tmux_env = std::env::var("TMUX").ok();
        Self::runtime().owns_tmux_env_client(tmux_env.as_deref())
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
        if Self::input_transport_mode() == TmuxInputTransportMode::Direct
            || Self::should_bypass_control_input(session)
        {
            return Self::send_literal_direct(session, window, text);
        }

        let target = format!("{}:{}", session, window);
        let quoted_target = Self::tmux_command_quote(&target);
        let quoted_text = Self::tmux_command_quote(text);
        match Self::send_via_input_client(session, |client| {
            client.send_command(&format!("send-keys -t {quoted_target} -l {quoted_text}\n"))
        }) {
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
        if Self::input_transport_mode() == TmuxInputTransportMode::Direct
            || Self::should_bypass_control_input(session)
        {
            return Self::send_key_name_direct(session, window, key_name);
        }

        let target = format!("{}:{}", session, window);
        let quoted_target = Self::tmux_command_quote(&target);
        match Self::send_via_input_client(session, |client| {
            client.send_command(&format!("send-keys -t {quoted_target} {key_name}\n"))
        }) {
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
                    format!(
                        "Failed to send key to tmux target {target} after input client fallback"
                    )
                })
            }
        }
    }

    /// Send keys to a specific window in a session
    pub fn send_keys(session: &str, window: &str, keys: &str) -> Result<()> {
        if Self::input_transport_mode() == TmuxInputTransportMode::Direct
            || Self::should_bypass_control_input(session)
        {
            return Self::send_keys_direct(session, window, keys);
        }

        let target = format!("{}:{}", session, window);
        let quoted_target = Self::tmux_command_quote(&target);
        let quoted_keys = Self::tmux_command_quote(keys);
        match Self::send_via_input_client(session, |client| {
            client.send_command(&format!(
                "send-keys -t {quoted_target} {quoted_keys} Enter\n"
            ))
        }) {
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
                    format!(
                        "Failed to send keys to tmux target {target} after input client fallback"
                    )
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
    use super::{TmuxInputTransportMode, TmuxManager, TmuxRuntime};
    use std::ffi::OsString;
    use std::fs;
    use std::process::{ExitStatus, Output};
    use std::sync::{Mutex, OnceLock};

    #[cfg(unix)]
    use std::os::unix::net::UnixListener;
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt;
    use tempfile::TempDir;

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    struct EnvGuard {
        values: Vec<(&'static str, Option<OsString>)>,
    }

    impl EnvGuard {
        fn new(keys: &[&'static str]) -> Self {
            Self {
                values: keys
                    .iter()
                    .map(|key| (*key, std::env::var_os(key)))
                    .collect(),
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in &self.values {
                unsafe {
                    if let Some(value) = value {
                        std::env::set_var(key, value);
                    } else {
                        std::env::remove_var(key);
                    }
                }
            }
        }
    }

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

    #[test]
    fn dedicated_socket_mode_ignores_ambient_tmux_socket() {
        let runtime =
            TmuxRuntime::detect_from_env(Some("/tmp/ambient-tmux.sock,1,0"), None, None, true);
        let socket = runtime.socket.as_ref().unwrap();

        assert_eq!(
            socket.file_name().and_then(|name| name.to_str()),
            Some("managed-tmux.sock")
        );
        assert!(runtime.manages_private_socket);
    }

    #[test]
    fn explicit_tmux_socket_overrides_dedicated_socket_mode() {
        let runtime = TmuxRuntime::detect_from_env(
            Some("/tmp/ambient-tmux.sock,1,0"),
            Some(std::path::PathBuf::from("/tmp/amf-explicit.sock")),
            None,
            true,
        );

        assert_eq!(
            runtime.socket.as_ref().unwrap(),
            &std::path::PathBuf::from("/tmp/amf-explicit.sock")
        );
        assert!(!runtime.manages_private_socket);
    }

    #[test]
    fn dedicated_socket_runtime_does_not_claim_outer_tmux_client() {
        let runtime = TmuxRuntime {
            binary: OsString::from("tmux"),
            socket: Some(std::path::PathBuf::from("/tmp/amf-managed.sock")),
            manages_private_socket: true,
        };

        assert!(!runtime.owns_tmux_env_client(Some("/tmp/outer-tmux.sock,1,0")));
    }

    #[test]
    fn runtime_claims_tmux_client_on_same_socket() {
        let runtime = TmuxRuntime {
            binary: OsString::from("tmux"),
            socket: Some(std::path::PathBuf::from("/tmp/amf-managed.sock")),
            manages_private_socket: true,
        };

        assert!(runtime.owns_tmux_env_client(Some("/tmp/amf-managed.sock,1,0")));
    }

    #[test]
    fn tmux_input_transport_defaults_to_control_mode() {
        let _lock = env_lock().lock().unwrap();
        let _env = EnvGuard::new(&[
            "AMF_EXPERIMENTAL_PERSISTENT_TMUX_INPUT",
            "AMF_TMUX_INPUT_TRANSPORT",
        ]);
        unsafe {
            std::env::remove_var("AMF_TMUX_INPUT_TRANSPORT");
            std::env::remove_var("AMF_EXPERIMENTAL_PERSISTENT_TMUX_INPUT");
        }
        assert_eq!(
            TmuxManager::input_transport_mode(),
            TmuxInputTransportMode::ControlPty
        );
    }

    #[test]
    fn tmux_input_transport_config_can_disable_control_mode() {
        let _lock = env_lock().lock().unwrap();
        let _env = EnvGuard::new(&[
            "AMF_EXPERIMENTAL_PERSISTENT_TMUX_INPUT",
            "AMF_TMUX_INPUT_TRANSPORT",
        ]);
        unsafe {
            std::env::remove_var("AMF_TMUX_INPUT_TRANSPORT");
            std::env::remove_var("AMF_EXPERIMENTAL_PERSISTENT_TMUX_INPUT");
        }
        TmuxManager::configure_control_mode(false);
        assert_eq!(
            TmuxManager::input_transport_mode(),
            TmuxInputTransportMode::Direct
        );
        TmuxManager::configure_control_mode(true);
    }

    #[test]
    fn managed_socket_global_default_terminal_bootstraps_with_new_session_first() {
        let commands = TmuxManager::global_default_terminal_args(
            "screen-256color",
            Some("__amf-bootstrap-test"),
        );

        assert_eq!(
            commands,
            vec![
                vec![
                    "new-session".to_string(),
                    "-d".to_string(),
                    "-s".to_string(),
                    "__amf-bootstrap-test".to_string(),
                ],
                vec![
                    "set-option".to_string(),
                    "-g".to_string(),
                    "default-terminal".to_string(),
                    "screen-256color".to_string(),
                ],
                vec![
                    "kill-session".to_string(),
                    "-t".to_string(),
                    "__amf-bootstrap-test".to_string(),
                ],
            ]
        );
    }

    #[test]
    fn ambient_socket_global_default_terminal_skips_bootstrap_session() {
        let commands = TmuxManager::global_default_terminal_args("screen-256color", None);

        assert_eq!(
            commands,
            vec![vec![
                "set-option".to_string(),
                "-g".to_string(),
                "default-terminal".to_string(),
                "screen-256color".to_string(),
            ]]
        );
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

    #[test]
    fn pane_default_terminal_can_be_overridden() {
        let _lock = env_lock().lock().unwrap();
        let _guard = EnvGuard::new(&["AMF_TMUX_DEFAULT_TERMINAL"]);
        unsafe {
            std::env::set_var("AMF_TMUX_DEFAULT_TERMINAL", "xterm-256color");
        }

        assert_eq!(
            TmuxManager::pane_default_terminal().as_deref(),
            Some("xterm-256color")
        );
    }

    #[test]
    fn empty_pane_default_terminal_override_disables_override() {
        let _lock = env_lock().lock().unwrap();
        let _guard = EnvGuard::new(&["AMF_TMUX_DEFAULT_TERMINAL"]);
        unsafe {
            std::env::set_var("AMF_TMUX_DEFAULT_TERMINAL", "");
        }

        assert_eq!(TmuxManager::pane_default_terminal(), None);
    }
}
