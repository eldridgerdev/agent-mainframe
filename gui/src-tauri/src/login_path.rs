//! Give a Finder-launched GUI the user's shell `PATH`.
//!
//! macOS starts apps from Finder, the Dock and Spotlight with launchd's
//! minimal `PATH` (`/usr/bin:/bin:/usr/sbin:/sbin`). Homebrew's `tmux` and the
//! harness CLIs (`claude`, `codex`, ...) live elsewhere, so without this every
//! feature start would fail to find them. A terminal launch already has the
//! full `PATH`; merging keeps whatever it had.
//!
//! The logic is Unix-generic so it can be tested on Linux; `main` only calls
//! it on macOS.

use std::ffi::OsString;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Printed right before `$PATH` so output from the user's rc files (banners,
/// `echo`s) can't be mistaken for it.
const MARKER: &str = "__AMF_LOGIN_PATH__";

/// A slow or interactive rc file must not hang the app's startup.
const TIMEOUT: Duration = Duration::from_secs(3);

/// Replace the process's `PATH` with the login shell's, keeping any existing
/// entries it lacks. Leaves `PATH` untouched when the shell can't be run,
/// times out, or reports nothing.
///
/// # Safety
///
/// Calls `std::env::set_var`, so it must run before any other thread starts;
/// `main` calls it first, before Tauri spawns anything.
pub unsafe fn adopt_login_shell_path() {
    let shell = std::env::var_os("SHELL")
        .filter(|shell| !shell.is_empty())
        .unwrap_or_else(|| OsString::from("/bin/zsh"));
    let Some(login_path) = login_shell_path(&shell) else {
        return;
    };
    let current = std::env::var("PATH").unwrap_or_default();
    let merged = merge_paths(&login_path, &current);
    if merged != current {
        // SAFETY: the caller guarantees no other thread is running yet.
        unsafe { std::env::set_var("PATH", merged) };
    }
}

/// Run `shell` as an interactive login shell and return the `PATH` it ends up
/// with, or `None` on any failure.
fn login_shell_path(shell: &OsString) -> Option<String> {
    let mut child = Command::new(shell)
        .arg("-ilc")
        .arg(format!("printf '\\n{MARKER}%s\\n' \"$PATH\""))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let deadline = Instant::now() + TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }

    let mut stdout = String::new();
    child.stdout.take()?.read_to_string(&mut stdout).ok()?;
    parse_marked_path(&stdout)
}

/// The value printed after the last marker, if non-empty.
fn parse_marked_path(output: &str) -> Option<String> {
    let (_, rest) = output.rsplit_once(MARKER)?;
    let path = rest.lines().next()?.trim();
    (!path.is_empty()).then(|| path.to_string())
}

/// `preferred`'s entries in order, then any of `existing`'s it doesn't have.
/// Empty entries are dropped: in `PATH` they mean the current directory.
fn merge_paths(preferred: &str, existing: &str) -> String {
    let mut merged: Vec<&str> = Vec::new();
    for entry in preferred.split(':').chain(existing.split(':')) {
        if !entry.is_empty() && !merged.contains(&entry) {
            merged.push(entry);
        }
    }
    merged.join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ignores_rc_file_noise_around_the_marker() {
        let output = format!("Welcome!\nlast login\n\n{MARKER}/opt/homebrew/bin:/usr/bin\n");
        assert_eq!(
            parse_marked_path(&output).as_deref(),
            Some("/opt/homebrew/bin:/usr/bin")
        );
    }

    #[test]
    fn parse_takes_the_last_marker_and_rejects_empty_or_missing() {
        let output = format!("{MARKER}/stale\n{MARKER}/fresh\n");
        assert_eq!(parse_marked_path(&output).as_deref(), Some("/fresh"));
        assert_eq!(parse_marked_path(&format!("{MARKER}\n")), None);
        assert_eq!(parse_marked_path("no marker here"), None);
    }

    #[test]
    fn merge_prefers_the_login_path_and_keeps_extra_entries() {
        assert_eq!(
            merge_paths("/opt/homebrew/bin:/usr/bin:/bin", "/usr/bin:/bin:/custom"),
            "/opt/homebrew/bin:/usr/bin:/bin:/custom"
        );
    }

    #[test]
    fn merge_drops_empty_entries_and_duplicates() {
        assert_eq!(merge_paths("/a::/b:/a", ":/b:/c:"), "/a:/b:/c");
    }

    #[test]
    fn login_shell_path_reads_a_real_shell() {
        let path = login_shell_path(&OsString::from("/bin/sh")).expect("sh reports a PATH");
        assert!(path.contains('/'), "unexpected PATH: {path}");
    }

    #[test]
    fn login_shell_path_is_none_for_a_missing_shell() {
        assert_eq!(
            login_shell_path(&OsString::from("/nonexistent/amf-shell")),
            None
        );
    }
}
