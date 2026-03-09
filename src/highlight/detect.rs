use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HighlightLanguage {
    Bash,
    Json,
    Rust,
    Toml,
    Yaml,
}

impl HighlightLanguage {
    pub fn display_name(self) -> &'static str {
        match self {
            HighlightLanguage::Bash => "bash",
            HighlightLanguage::Json => "json",
            HighlightLanguage::Rust => "rust",
            HighlightLanguage::Toml => "toml",
            HighlightLanguage::Yaml => "yaml",
        }
    }
}

pub fn detect_language(
    path: Option<&Path>,
    language_hint: Option<&str>,
    source: &str,
) -> Option<HighlightLanguage> {
    language_hint
        .and_then(parse_language_name)
        .or_else(|| path.and_then(language_from_path))
        .or_else(|| language_from_shebang(source))
}

fn parse_language_name(name: &str) -> Option<HighlightLanguage> {
    match name.trim().to_ascii_lowercase().as_str() {
        "bash" | "shell" | "sh" | "zsh" => Some(HighlightLanguage::Bash),
        "json" | "jsonc" | "jsonl" => Some(HighlightLanguage::Json),
        "rust" | "rs" => Some(HighlightLanguage::Rust),
        "toml" => Some(HighlightLanguage::Toml),
        "yaml" | "yml" => Some(HighlightLanguage::Yaml),
        _ => None,
    }
}

fn language_from_path(path: &Path) -> Option<HighlightLanguage> {
    if let Some(file_name) = path.file_name().and_then(|name| name.to_str()) {
        match file_name {
            "Cargo.lock" => return Some(HighlightLanguage::Toml),
            ".bashrc" | ".bash_profile" | ".zshrc" | ".zprofile" => {
                return Some(HighlightLanguage::Bash);
            }
            _ => {}
        }
    }

    match path.extension().and_then(|ext| ext.to_str()) {
        Some("rs") => Some(HighlightLanguage::Rust),
        Some("json") | Some("jsonl") => Some(HighlightLanguage::Json),
        Some("sh") | Some("bash") | Some("zsh") => Some(HighlightLanguage::Bash),
        Some("toml") => Some(HighlightLanguage::Toml),
        Some("yml") | Some("yaml") => Some(HighlightLanguage::Yaml),
        _ => None,
    }
}

fn language_from_shebang(source: &str) -> Option<HighlightLanguage> {
    let first_line = source.lines().next()?.trim();
    if !first_line.starts_with("#!") {
        return None;
    }
    if first_line.contains("bash") || first_line.contains("sh") || first_line.contains("zsh") {
        return Some(HighlightLanguage::Bash);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_language_from_extension() {
        assert_eq!(
            detect_language(Some(Path::new("src/main.rs")), None, ""),
            Some(HighlightLanguage::Rust)
        );
        assert_eq!(
            detect_language(Some(Path::new("config/settings.json")), None, ""),
            Some(HighlightLanguage::Json)
        );
    }

    #[test]
    fn detects_language_from_hint_before_path() {
        assert_eq!(
            detect_language(Some(Path::new("src/main.rs")), Some("yaml"), ""),
            Some(HighlightLanguage::Yaml)
        );
    }

    #[test]
    fn detects_language_from_shebang() {
        assert_eq!(
            detect_language(None, None, "#!/usr/bin/env bash\nprintf 'ok'\n"),
            Some(HighlightLanguage::Bash)
        );
    }
}
