//! Explicit error collection for harness changes. Other setup callers retain
//! their historical best-effort behavior through the default context.
use std::path::Path;

#[derive(Default)]
pub(super) struct SetupIo {
    checked: bool,
    read_failed: bool,
    errors: Vec<String>,
}

impl SetupIo {
    pub(super) fn is_checked(&self) -> bool {
        self.checked
    }

    pub(super) fn checked() -> Self {
        Self {
            checked: true,
            ..Self::default()
        }
    }

    pub(super) fn finish(self) -> anyhow::Result<()> {
        anyhow::ensure!(self.errors.is_empty(), "{}", self.errors.join("; "));
        Ok(())
    }

    pub(super) fn failed_read(&mut self, path: &Path, error: impl std::fmt::Display) {
        if self.checked {
            self.read_failed = true;
            self.errors
                .push(format!("Cannot read {}: {error}", path.display()));
        }
    }

    pub(super) fn can_write(&self) -> bool {
        !self.read_failed
    }

    pub(super) fn result<T>(
        &mut self,
        path: &Path,
        result: std::io::Result<T>,
    ) -> std::io::Result<T> {
        if self.checked
            && let Err(error) = &result
        {
            self.errors
                .push(format!("Cannot update {}: {error}", path.display()));
        }
        result
    }

    fn update<T>(
        &mut self,
        path: &Path,
        operation: impl FnOnce() -> std::io::Result<T>,
    ) -> std::io::Result<T> {
        if !self.can_write() {
            return Err(std::io::Error::other(
                "Setup stopped after an unreadable input",
            ));
        }
        self.result(path, operation())
    }

    pub(super) fn read_to_string(&mut self, path: impl AsRef<Path>) -> std::io::Result<String> {
        let path = path.as_ref();
        let result = std::fs::read_to_string(path);
        if let Err(error) = &result
            && error.kind() != std::io::ErrorKind::NotFound
        {
            self.failed_read(path, error);
        }
        result
    }

    pub(super) fn json(&mut self, path: &Path) -> serde_json::Value {
        let content = match self.read_to_string(path) {
            Ok(content) => content,
            Err(_) => return serde_json::json!({}),
        };
        match serde_json::from_str::<serde_json::Value>(&content) {
            Ok(value) if value.is_object() => {
                let valid = value.get("hooks").is_none_or(|hooks| {
                    hooks
                        .as_object()
                        .is_some_and(|events| events.values().all(serde_json::Value::is_array))
                }) && value
                    .get("permissions")
                    .is_none_or(serde_json::Value::is_object)
                    && value
                        .pointer("/permissions/allow")
                        .is_none_or(serde_json::Value::is_array)
                    && value
                        .get("permissions_added")
                        .is_none_or(serde_json::Value::is_array);
                if self.checked && !valid {
                    self.failed_read(path, "Invalid hooks or permissions structure");
                    serde_json::json!({})
                } else {
                    value
                }
            }
            Ok(_) => {
                self.failed_read(path, "Expected a JSON object");
                serde_json::json!({})
            }
            Err(error) => {
                self.failed_read(path, error);
                serde_json::json!({})
            }
        }
    }

    pub(super) fn toml(&mut self, path: &Path) -> toml::Value {
        let content = match self.read_to_string(path) {
            Ok(content) => content,
            Err(_) => return toml::Value::Table(Default::default()),
        };
        match toml::from_str::<toml::Value>(&content) {
            Ok(value) if value.is_table() => {
                if self.checked
                    && let Err(error) = crate::codex_config::validate_notify_config(&value)
                {
                    self.failed_read(path, error);
                    return toml::Value::Table(Default::default());
                }
                value
            }
            Ok(_) => {
                self.failed_read(path, "Expected a TOML table");
                toml::Value::Table(Default::default())
            }
            Err(error) => {
                self.failed_read(path, error);
                toml::Value::Table(Default::default())
            }
        }
    }

    pub(super) fn create_dir_all(&mut self, path: impl AsRef<Path>) -> std::io::Result<()> {
        let path = path.as_ref();
        self.update(path, || std::fs::create_dir_all(path))
    }
    pub(super) fn write(
        &mut self,
        path: impl AsRef<Path>,
        content: impl AsRef<[u8]>,
    ) -> std::io::Result<()> {
        let path = path.as_ref();
        self.update(path, || std::fs::write(path, content))
    }
    pub(super) fn write_atomic(
        &mut self,
        path: &Path,
        content: &str,
        mode: Option<u32>,
    ) -> std::io::Result<()> {
        self.update(path, || super::write_atomic(path, content, mode))
    }
    pub(super) fn remove_file(&mut self, path: impl AsRef<Path>) -> std::io::Result<()> {
        let path = path.as_ref();
        self.update(path, || match std::fs::remove_file(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        })
    }
    pub(super) fn remove_dir_all(&mut self, path: impl AsRef<Path>) -> std::io::Result<()> {
        let path = path.as_ref();
        self.update(path, || match std::fs::remove_dir_all(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        })
    }
    pub(super) fn copy(
        &mut self,
        from: impl AsRef<Path>,
        to: impl AsRef<Path>,
    ) -> std::io::Result<u64> {
        let to = to.as_ref();
        self.update(to, || std::fs::copy(from, to))
    }
    pub(super) fn set_permissions(
        &mut self,
        path: impl AsRef<Path>,
        permissions: std::fs::Permissions,
    ) -> std::io::Result<()> {
        let path = path.as_ref();
        self.update(path, || std::fs::set_permissions(path, permissions))
    }
    pub(super) fn append_entry(&mut self, path: &Path, entry: &str) {
        use std::io::Write;
        let _ = self.update(path, || {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?
                .write_all(format!("{entry}\n").as_bytes())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_json_or_toml_read_blocks_writes_to_related_documents() {
        for (name, content) in [
            ("settings.json", "{broken"),
            ("config.toml", "notify = ["),
            ("config.toml", "notify = false"),
            ("config.toml", "notify = [\"command\", 1]"),
        ] {
            let dir = tempfile::TempDir::new().unwrap();
            let input = dir.path().join(name);
            let related = dir.path().join("related.md");
            std::fs::write(&input, content).unwrap();
            std::fs::write(&related, "user content").unwrap();
            let mut io = SetupIo::checked();
            if name.ends_with("json") {
                io.json(&input);
            } else {
                io.toml(&input);
            }
            assert!(io.write(&input, "{}").is_err());
            assert!(io.write(&related, "default-derived replacement").is_err());
            assert!(io.finish().is_err());
            assert_eq!(std::fs::read_to_string(input).unwrap(), content);
            assert_eq!(std::fs::read_to_string(related).unwrap(), "user content");
        }
    }
    #[test]
    fn missing_document_allows_checked_creation_but_unreadable_document_does_not() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        let mut io = SetupIo::checked();
        assert_eq!(io.json(&path), serde_json::json!({}));
        io.write(&path, "{}").unwrap();
        io.finish().unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        let mut io = SetupIo::checked();
        io.json(&path);
        assert!(
            io.write(dir.path().join("related.md"), "replacement")
                .is_err()
        );
        assert!(io.finish().is_err());
        assert!(!dir.path().join("related.md").exists());
    }
}
