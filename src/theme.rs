use anyhow::Result;
use std::fs;
use std::path::Path;

pub struct ThemeManager;

impl ThemeManager {
    pub fn inject_opencode_themes(workdir: &Path) -> Result<()> {
        let themes_dir = workdir.join(".opencode").join("themes");

        fs::create_dir_all(&themes_dir)?;

        let theme_files = [
            ("amf.json", include_str!("../themes/opencode/amf.json")),
            (
                "amf-tokyonight.json",
                include_str!("../themes/opencode/amf-tokyonight.json"),
            ),
            (
                "amf-catppuccin.json",
                include_str!("../themes/opencode/amf-catppuccin.json"),
            ),
        ];

        for (filename, content) in theme_files {
            let theme_path = themes_dir.join(filename);

            if !theme_path.exists() {
                fs::write(&theme_path, content)?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_inject_opencode_themes() {
        let dir = TempDir::new().unwrap();
        let workdir = dir.path();

        ThemeManager::inject_opencode_themes(workdir).unwrap();

        let themes_dir = workdir.join(".opencode").join("themes");
        assert!(themes_dir.exists(), "Themes directory should exist");

        let amf_theme = themes_dir.join("amf.json");
        assert!(amf_theme.exists(), "amf.json should exist");

        let tokyonight_theme = themes_dir.join("amf-tokyonight.json");
        assert!(
            tokyonight_theme.exists(),
            "amf-tokyonight.json should exist"
        );

        let catppuccin_theme = themes_dir.join("amf-catppuccin.json");
        assert!(
            catppuccin_theme.exists(),
            "amf-catppuccin.json should exist"
        );

        let content = std::fs::read_to_string(&amf_theme).unwrap();
        assert!(
            content.contains("\"background\": \"none\""),
            "Theme main background should be transparent"
        );
        assert!(
            content.contains("\"backgroundPanel\":"),
            "Theme should have backgroundPanel defined"
        );
    }

    #[test]
    fn test_inject_opencode_themes_idempotent() {
        let dir = TempDir::new().unwrap();
        let workdir = dir.path();

        ThemeManager::inject_opencode_themes(workdir).unwrap();

        let amf_theme = workdir.join(".opencode").join("themes").join("amf.json");
        let original_content = std::fs::read_to_string(&amf_theme).unwrap();

        ThemeManager::inject_opencode_themes(workdir).unwrap();

        let new_content = std::fs::read_to_string(&amf_theme).unwrap();
        assert_eq!(
            original_content, new_content,
            "Second injection should not overwrite existing themes"
        );
    }
}
