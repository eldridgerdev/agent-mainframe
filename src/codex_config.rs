use std::path::Path;

pub fn launch_override_args(workdir: &Path) -> Vec<String> {
    let hook_path = workdir.join(".codex").join("amf-codex-notify.sh");
    let user_config_path = dirs::home_dir()
        .map(|home| home.join(".codex").join("config.toml"))
        .unwrap_or_default();

    launch_override_args_for(&user_config_path, &hook_path)
}

fn launch_override_args_for(config_path: &Path, hook_path: &Path) -> Vec<String> {
    let user_config = read_launch_config(config_path).unwrap_or_default();
    let launch_config = build_launch_config(&user_config, hook_path);
    launch_config_to_args(launch_config)
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

#[cfg(test)]
mod tests {
    use super::launch_override_args_for;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn launch_override_args_merges_existing_user_entries() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        let hook_path = dir.path().join("amf-codex-notify.sh");

        fs::write(&config_path, "notify = [\"/tmp/existing-hook.sh\"]\n").unwrap();
        let args = launch_override_args_for(&config_path, &hook_path);

        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-c");
        assert!(
            args[1].contains("/tmp/existing-hook.sh"),
            "existing user notify entry should be preserved"
        );
        assert!(
            args[1].contains("amf-codex-notify.sh"),
            "AMF hook should be injected into the transient override"
        );
    }

    #[test]
    fn launch_override_args_falls_back_to_amf_hook_only() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("missing.toml");
        let hook_path = dir.path().join("amf-codex-notify.sh");

        let args = launch_override_args_for(&config_path, &hook_path);

        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-c");
        assert!(args[1].contains("amf-codex-notify.sh"));
    }
}
