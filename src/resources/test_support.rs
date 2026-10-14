//! Process fixtures shared by tests that launch real processes.

use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::Duration;

/// Write a script a test is about to execute, without this process ever
/// holding it open for writing.
///
/// `fs::write` followed by an exec races every other test that spawns a
/// process: a fork on another thread inherits the still-open write descriptor,
/// and until that child execs the kernel refuses to run the file with
/// `ETXTBSY` ("Text file busy"), even though this thread closed its own handle
/// first. A shell writes the file instead, so the only writable descriptor
/// lives in a child that has exited by the time this returns.
pub(crate) fn write_executable_script(path: &Path, content: &str) {
    let mut writer = Command::new("/bin/sh")
        .args(["-c", r#"cat > "$1" && chmod 755 "$1""#, "sh"])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .expect("shell should launch to write the script");
    writer
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(content.as_bytes())
        .expect("script should reach the shell");
    let status = writer.wait().expect("shell should exit");
    assert!(status.success(), "writing {} failed", path.display());
}

/// Own the stand-in and its descendants, including when an assertion panics.
/// Killing only the shell would leave its sleeping children behind.
pub(crate) struct TestChild(Child);

impl TestChild {
    pub(crate) fn new(child: Child) -> Self {
        Self(child)
    }

    pub(crate) fn id(&self) -> u32 {
        self.0.id()
    }

    pub(crate) fn kill(&mut self) -> std::io::Result<()> {
        if self.0.try_wait()?.is_none() {
            super::procs::terminate_tree(self.id() as i64, Duration::from_millis(100));
        }
        Ok(())
    }

    pub(crate) fn wait(&mut self) -> std::io::Result<ExitStatus> {
        self.0.wait()
    }
}

impl Drop for TestChild {
    fn drop(&mut self) {
        let _ = self.kill();
        let _ = self.wait();
    }
}
