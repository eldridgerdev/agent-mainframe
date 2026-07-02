use std::path::Path;

use crate::project::VibeMode;

pub fn ensure_user_config_notify_hook(hook_path: &Path) {
    if cfg!(test) {
        return;
    }
    let Some(config_path) = dirs::home_dir().map(|home| home.join(".codex").join("config.toml"))
    else {
        return;
    };
    let _ = ensure_user_config_notify_hook_for(&config_path, hook_path);
}

pub fn launch_override_args(workdir: &Path, mode: &VibeMode) -> Vec<String> {
    let hook_path = workdir.join(".codex").join("amf-codex-notify.sh");
    let user_config_path = dirs::home_dir()
        .map(|home| home.join(".codex").join("config.toml"))
        .unwrap_or_default();

    launch_override_args_for(&user_config_path, &hook_path, workdir, mode)
}

pub fn configured_model() -> Option<String> {
    let config_path = dirs::home_dir()?.join(".codex").join("config.toml");
    configured_model_for(&config_path)
}

fn configured_model_for(config_path: &Path) -> Option<String> {
    read_launch_config(config_path)?
        .get("model")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(ToOwned::to_owned)
}

fn launch_override_args_for(
    config_path: &Path,
    hook_path: &Path,
    workdir: &Path,
    mode: &VibeMode,
) -> Vec<String> {
    let user_config = read_launch_config(config_path).unwrap_or_default();
    let launch_config = build_launch_config(&user_config, hook_path);
    let mut args = vec![
        "-C".to_string(),
        workdir.to_string_lossy().into_owned(),
        "--add-dir".to_string(),
        workdir.to_string_lossy().into_owned(),
    ];
    if matches!(mode, VibeMode::SuperVibe) {
        args.extend([
            "--sandbox".to_string(),
            "danger-full-access".to_string(),
            "--ask-for-approval".to_string(),
            "never".to_string(),
        ]);
    }
    args.extend(launch_config_to_args(launch_config));
    args
}

fn read_launch_config(config_path: &Path) -> Option<toml::map::Map<String, toml::Value>> {
    let config = std::fs::read_to_string(config_path)
        .ok()
        .and_then(|s| toml::from_str::<toml::Value>(&s).ok())?;

    config.as_table().cloned()
}

fn build_launch_config(
    user_config: &toml::map::Map<String, toml::Value>,
    hook_path: &Path,
) -> toml::map::Map<String, toml::Value> {
    let mut launch_config = toml::map::Map::new();
    launch_config.insert(
        "notify".to_string(),
        build_notify_override(user_config.get("notify"), hook_path),
    );
    launch_config
}

fn build_notify_override(notify: Option<&toml::Value>, hook_path: &Path) -> toml::Value {
    let hook_cmd = hook_path.to_string_lossy().into_owned();
    let mut entries = parse_notify_entries(notify).unwrap_or_default();
    if !entries.iter().any(|entry| entry == &hook_cmd) {
        entries.push(hook_cmd);
    }

    toml::Value::Array(entries.into_iter().map(toml::Value::String).collect())
}

fn parse_notify_entries(value: Option<&toml::Value>) -> Option<Vec<String>> {
    let Some(value) = value else {
        return Some(vec![]);
    };

    if let Some(arr) = value.as_array() {
        let values: Option<Vec<String>> = arr
            .iter()
            .map(|item| item.as_str().map(ToOwned::to_owned))
            .collect();
        return values;
    }

    value.as_str().map(|command| vec![command.to_string()])
}

fn launch_config_to_args(config: toml::map::Map<String, toml::Value>) -> Vec<String> {
    let mut args = Vec::new();
    for (key, value) in config {
        args.push("-c".to_string());
        args.push(format!("{key}={value}"));
    }
    args
}

fn ensure_user_config_notify_hook_for(config_path: &Path, hook_path: &Path) -> Option<()> {
    let mut config = if config_path.exists() {
        std::fs::read_to_string(config_path)
            .ok()
            .and_then(|s| toml::from_str::<toml::Value>(&s).ok())
            .filter(|value| value.is_table())?
    } else {
        toml::Value::Table(toml::map::Map::new())
    };

    let table = config.as_table_mut()?;
    let hook_cmd = hook_path.to_string_lossy().into_owned();
    let mut entries = parse_notify_entries(table.get("notify")).unwrap_or_default();
    if !entries.iter().any(|entry| entry == &hook_cmd) {
        entries.push(hook_cmd);
    }

    table.insert(
        "notify".to_string(),
        toml::Value::Array(entries.into_iter().map(toml::Value::String).collect()),
    );

    if let Some(parent) = config_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let rendered = toml::to_string_pretty(&config).ok()?;
    let _ = std::fs::write(config_path, rendered + "\n");
    Some(())
}

#[cfg(test)]
mod tests {
    use super::launch_override_args_for;
    use crate::project::VibeMode;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn launch_override_args_merges_existing_user_entries() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        let hook_path = dir.path().join("amf-codex-notify.sh");

        fs::write(&config_path, "notify = [\"/tmp/existing-hook.sh\"]\n").unwrap();
        let args = launch_override_args_for(&config_path, &hook_path, dir.path(), &VibeMode::Vibe);

        assert_eq!(args.len(), 6);
        assert_eq!(args[0], "-C");
        assert_eq!(args[1], dir.path().to_string_lossy());
        assert_eq!(args[2], "--add-dir");
        assert_eq!(args[3], dir.path().to_string_lossy());
        assert_eq!(args[4], "-c");
        assert!(
            args[5].contains("/tmp/existing-hook.sh"),
            "existing user notify entry should be preserved"
        );
        assert!(
            args[5].contains("amf-codex-notify.sh"),
            "AMF hook should be injected into the transient override"
        );
    }

    #[test]
    fn launch_override_args_falls_back_to_amf_hook_only() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("missing.toml");
        let hook_path = dir.path().join("amf-codex-notify.sh");

        let args = launch_override_args_for(&config_path, &hook_path, dir.path(), &VibeMode::Vibe);

        assert_eq!(args.len(), 6);
        assert_eq!(args[0], "-C");
        assert_eq!(args[1], dir.path().to_string_lossy());
        assert_eq!(args[2], "--add-dir");
        assert_eq!(args[3], dir.path().to_string_lossy());
        assert_eq!(args[4], "-c");
        assert!(args[5].contains("amf-codex-notify.sh"));
    }

    #[test]
    fn configured_model_reads_top_level_model() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, "model = \"gpt-5.5\"\n").unwrap();

        assert_eq!(
            super::configured_model_for(&config_path).as_deref(),
            Some("gpt-5.5")
        );
    }

    #[test]
    fn launch_override_args_supervibe_uses_full_access_without_approvals() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("missing.toml");
        let hook_path = dir.path().join("amf-codex-notify.sh");

        let args =
            launch_override_args_for(&config_path, &hook_path, dir.path(), &VibeMode::SuperVibe);

        assert!(
            args.windows(2)
                .any(|pair| pair == ["--sandbox", "danger-full-access"])
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--ask-for-approval", "never"])
        );
    }
}
