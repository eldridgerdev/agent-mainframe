use std::path::Path;

pub fn notify_override_args(workdir: &Path) -> Vec<String> {
    let hook_path = workdir.join(".codex").join("amf-codex-notify.sh");
    let user_config_path = dirs::home_dir()
        .map(|home| home.join(".codex").join("config.toml"))
        .unwrap_or_default();

    notify_override_args_for(&user_config_path, &hook_path)
}

fn notify_override_args_for(config_path: &Path, hook_path: &Path) -> Vec<String> {
    let mut notify_entries = read_notify_entries(config_path).unwrap_or_default();
    let hook_cmd = hook_path.to_string_lossy().into_owned();
    if !notify_entries.iter().any(|entry| entry == &hook_cmd) {
        notify_entries.push(hook_cmd);
    }

    let notify_value = toml::Value::Array(
        notify_entries
            .into_iter()
            .map(toml::Value::String)
            .collect(),
    );

    vec!["-c".to_string(), format!("notify={notify_value}")]
}

fn read_notify_entries(config_path: &Path) -> Option<Vec<String>> {
    let config = std::fs::read_to_string(config_path)
        .ok()
        .and_then(|s| toml::from_str::<toml::Value>(&s).ok())?;

    parse_notify_entries(config.get("notify"))
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

#[cfg(test)]
mod tests {
    use super::notify_override_args_for;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn notify_override_args_merges_existing_user_entries() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        let hook_path = dir.path().join("amf-codex-notify.sh");

        fs::write(&config_path, "notify = [\"/tmp/existing-hook.sh\"]\n").unwrap();
        let args = notify_override_args_for(&config_path, &hook_path);

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
    fn notify_override_args_falls_back_to_amf_hook_only() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("missing.toml");
        let hook_path = dir.path().join("amf-codex-notify.sh");

        let args = notify_override_args_for(&config_path, &hook_path);

        assert_eq!(args.len(), 2);
        assert_eq!(args[0], "-c");
        assert!(args[1].contains("amf-codex-notify.sh"));
    }
}
