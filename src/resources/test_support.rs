//! Process fixtures shared by editor ownership tests.

use std::process::{Child, ExitStatus};
use std::time::Duration;

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
