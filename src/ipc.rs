use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use crate::debug::{LogLevel, log_to_file};

/// Path to the AMF IPC socket.
/// Matches the state directory used by the debug log.
pub fn socket_path() -> PathBuf {
    dirs::state_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("amf")
        .join("amf.sock")
}

/// Directory used for temporary reply sockets for `notify-wait`.
pub fn reply_dir() -> PathBuf {
    std::env::temp_dir().join("amf-ipc-reply")
}

/// Owns the IPC socket lifetime. Removes the socket file on drop
/// so the filesystem is always cleaned up on normal exit or panic.
pub struct IpcGuard {
    pub rx: Receiver<serde_json::Value>,
    path: PathBuf,
}

impl Drop for IpcGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        log_to_file(
            LogLevel::Info,
            "ipc",
            &format!("Socket removed: {}", self.path.display()),
        );
    }
}

/// Start the IPC socket server. Removes any stale socket file,
/// binds a new `UnixListener`, and spawns a background thread
/// that accepts connections and forwards newline-delimited JSON
/// messages via the `IpcGuard`'s receiver.
///
/// Each connection is handled in its own short-lived thread.
/// The server thread exits when the listener errors or when the
/// receiver side is dropped (channel disconnected).
pub fn start(path: &Path) -> Result<IpcGuard> {
    // Remove stale socket from a previous run.
    let _ = std::fs::remove_file(path);

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("Failed to create IPC socket directory")?;
    }

    let listener =
        std::os::unix::net::UnixListener::bind(path).context("Failed to bind IPC socket")?;

    let (tx, rx) = channel::<serde_json::Value>();

    let path_buf = path.to_path_buf();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(s) => {
                    log_to_file(LogLevel::Debug, "ipc", "Accepted connection");
                    let tx = tx.clone();
                    std::thread::spawn(move || {
                        use std::io::BufRead;
                        for line in std::io::BufReader::new(s).lines() {
                            match line {
                                Ok(l) if !l.trim().is_empty() => {
                                    match serde_json::from_str::<serde_json::Value>(&l) {
                                        Ok(v) => {
                                            // Channel closed means
                                            // App is shutting down.
                                            if tx.send(v).is_err() {
                                                return;
                                            }
                                        }
                                        Err(e) => {
                                            log_to_file(
                                                LogLevel::Debug,
                                                "ipc",
                                                &format!("Invalid JSON: {e}"),
                                            );
                                        }
                                    }
                                }
                                Ok(_) => {}
                                Err(e) => {
                                    log_to_file(
                                        LogLevel::Debug,
                                        "ipc",
                                        &format!("Read error: {e}"),
                                    );
                                    break;
                                }
                            }
                        }
                    });
                }
                Err(e) => {
                    // A single accept error is not fatal; log and
                    // continue unless the listener itself is gone.
                    log_to_file(LogLevel::Warn, "ipc", &format!("Accept error: {e}"));
                    // EBADF / EINVAL mean the socket is closed.
                    use std::io::ErrorKind::*;
                    if matches!(e.kind(), InvalidInput | BrokenPipe) {
                        break;
                    }
                }
            }
        }
        log_to_file(
            LogLevel::Debug,
            "ipc",
            &format!("Listener thread exiting ({})", path_buf.display()),
        );
    });

    Ok(IpcGuard {
        rx,
        path: path.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tempfile::TempDir;

    fn wait(ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }

    #[test]
    fn send_and_receive() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("amf.sock");
        let guard = start(&path).unwrap();
        wait(10);

        send(&path, r#"{"type":"stop","session_id":"s1","cwd":"/tmp"}"#).unwrap();

        let msg = guard
            .rx
            .recv_timeout(Duration::from_secs(1))
            .expect("no message received within 1s");
        assert_eq!(msg["type"], "stop");
        assert_eq!(msg["session_id"], "s1");
    }

    #[test]
    fn socket_removed_on_drop() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("amf.sock");
        {
            let _guard = start(&path).unwrap();
            assert!(path.exists(), "socket should exist while guard is live");
        }
        assert!(!path.exists(), "socket should be removed after drop");
    }

    #[test]
    fn send_fails_when_no_server() {
        let path = std::path::PathBuf::from("/tmp/amf-ipc-test-no-server.sock");
        let result = send(&path, r#"{"test":true}"#);
        assert!(result.is_err());
    }

    #[test]
    fn invalid_json_is_dropped() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("amf.sock");
        let guard = start(&path).unwrap();
        wait(10);

        // Should succeed as a send (valid socket write) but produce
        // no message on the receiver because it is not valid JSON.
        send(&path, "not valid json").unwrap();
        wait(50);

        assert!(
            guard.rx.try_recv().is_err(),
            "invalid JSON should not produce a message"
        );
    }

    #[test]
    fn multiple_messages_in_sequence() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("amf.sock");
        let guard = start(&path).unwrap();
        wait(10);

        for i in 0..5 {
            send(&path, &format!(r#"{{"session_id":"s{i}","cwd":"/tmp"}}"#)).unwrap();
        }
        wait(100);

        let mut received = Vec::new();
        while let Ok(m) = guard.rx.try_recv() {
            received.push(m);
        }
        assert_eq!(received.len(), 5);
        let mut ids: Vec<String> = received
            .iter()
            .filter_map(|m| {
                m.get("session_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .collect();
        ids.sort();
        assert_eq!(
            ids,
            vec![
                "s0".to_string(),
                "s1".to_string(),
                "s2".to_string(),
                "s3".to_string(),
                "s4".to_string()
            ]
        );
    }

    #[test]
    fn clear_message_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("amf.sock");
        let guard = start(&path).unwrap();
        wait(10);

        send(&path, r#"{"type":"clear","session_id":"xyz","cwd":"/tmp"}"#).unwrap();

        let msg = guard
            .rx
            .recv_timeout(Duration::from_secs(1))
            .expect("no message received");
        assert_eq!(msg["type"], "clear");
        assert_eq!(msg["session_id"], "xyz");
    }

    #[test]
    fn empty_payload_does_not_send() {
        // amf notify exits early for empty stdin; mirror that here.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("amf.sock");
        let guard = start(&path).unwrap();
        wait(10);

        // send() with only whitespace produces an empty line after
        // trim; the server should not forward it.
        send(&path, "   ").unwrap_err();
        // or it connects but sends nothing — either way, no message.
        wait(50);
        assert!(guard.rx.try_recv().is_err());
    }
}

/// Connect to a running AMF instance and send a single
/// newline-terminated JSON payload. Used by the `amf notify`
/// subcommand so hook scripts can push messages without `nc`.
/// Returns an error (non-zero exit) when AMF is not running,
/// allowing shell scripts to fall back to file-based delivery.
pub fn send(path: &Path, payload: &str) -> Result<()> {
    use std::io::Write;
    let payload = payload.trim();
    if payload.is_empty() {
        anyhow::bail!("Payload is empty");
    }
    let mut stream = std::os::unix::net::UnixStream::connect(path).with_context(|| {
        format!(
            "AMF socket not found at {} — is amf running?",
            path.display()
        )
    })?;
    writeln!(stream, "{payload}").context("Failed to write to AMF socket")?;
    stream.flush().context("Failed to flush AMF socket")?;
    Ok(())
}

/// Send JSON payload and wait for a single JSON reply on a temporary
/// callback socket. Adds `request_id` and `reply_socket` fields to
/// the outbound payload if they are absent.
pub fn send_wait(path: &Path, payload: &str, timeout: Duration) -> Result<serde_json::Value> {
    use serde_json::json;
    use std::io::BufRead;

    let mut msg: serde_json::Value =
        serde_json::from_str(payload.trim()).context("Payload must be valid JSON")?;
    let obj = msg
        .as_object_mut()
        .context("Payload must be a JSON object")?;

    let request_id = obj
        .get("request_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let reply_path = reply_dir().join(format!("{request_id}.sock"));
    if let Some(parent) = reply_path.parent() {
        std::fs::create_dir_all(parent).context("Failed to create reply socket directory")?;
    }
    let _ = std::fs::remove_file(&reply_path);

    let listener = std::os::unix::net::UnixListener::bind(&reply_path)
        .with_context(|| format!("Failed to bind reply socket at {}", reply_path.display()))?;
    listener
        .set_nonblocking(true)
        .context("Failed to set reply listener nonblocking")?;

    obj.insert("request_id".to_string(), json!(request_id));
    obj.insert(
        "reply_socket".to_string(),
        json!(reply_path.display().to_string()),
    );

    let outbound = serde_json::to_string(&msg)?;
    let send_result = send(path, &outbound);
    if send_result.is_err() {
        let _ = std::fs::remove_file(&reply_path);
    }
    send_result?;

    let start = Instant::now();
    let result = loop {
        match listener.accept() {
            Ok((stream, _)) => {
                let mut reader = std::io::BufReader::new(stream);
                let mut line = String::new();
                reader
                    .read_line(&mut line)
                    .context("Failed to read reply from AMF")?;
                let reply: serde_json::Value =
                    serde_json::from_str(line.trim()).context("Reply was not valid JSON")?;
                break Ok(reply);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if start.elapsed() >= timeout {
                    break Err(anyhow::anyhow!("Timed out waiting for AMF reply"));
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => {
                break Err(anyhow::anyhow!("Failed waiting for AMF reply: {e}"));
            }
        }
    };

    let _ = std::fs::remove_file(&reply_path);
    result
}
