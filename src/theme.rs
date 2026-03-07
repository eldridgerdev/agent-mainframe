use anyhow::Result;
use ratatui::style::Color;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeName {
    #[default]
    Default,
    Amf,
    Dracula,
    Nord,
    CatppuccinFrappe,
}

impl ThemeName {
    pub fn display_name(&self) -> &str {
        match self {
            ThemeName::Default => "Default",
            ThemeName::Amf => "AMF",
            ThemeName::Dracula => "Dracula",
            ThemeName::Nord => "Nord",
            ThemeName::CatppuccinFrappe => "Catppuccin Frappe",
        }
    }
}

impl std::fmt::Display for ThemeName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ThemeName::Default => write!(f, "default"),
            ThemeName::Amf => write!(f, "amf"),
            ThemeName::Dracula => write!(f, "dracula"),
            ThemeName::Nord => write!(f, "nord"),
            ThemeName::CatppuccinFrappe => write!(f, "catppuccin-frappe"),
        }
    }
}

impl std::str::FromStr for ThemeName {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "default" => Ok(ThemeName::Default),
            "amf" => Ok(ThemeName::Amf),
            "dracula" => Ok(ThemeName::Dracula),
            "nord" => Ok(ThemeName::Nord),
            "catppuccin-frappe" | "catppuccin_frappe" => Ok(ThemeName::CatppuccinFrappe),
            _ => Err(format!("Unknown theme: {}", s)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ColorDef {
    Named(String),
    Rgb { r: u8, g: u8, b: u8 },
}

impl ColorDef {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        ColorDef::Rgb { r, g, b }
    }

    pub fn named(name: &str) -> Self {
        ColorDef::Named(name.to_string())
    }

    pub fn to_color(&self) -> Color {
        match self {
            ColorDef::Named(name) => match name.as_str() {
                "black" => Color::Black,
                "red" => Color::Red,
                "green" => Color::Green,
                "yellow" => Color::Yellow,
                "blue" => Color::Blue,
                "magenta" => Color::Magenta,
                "cyan" => Color::Cyan,
                "white" => Color::White,
                "darkgray" | "dark_gray" => Color::DarkGray,
                "lightred" | "light_red" => Color::LightRed,
                "lightgreen" | "light_green" => Color::LightGreen,
                "lightyellow" | "light_yellow" => Color::LightYellow,
                "lightblue" | "light_blue" => Color::LightBlue,
                "lightmagenta" | "light_magenta" => Color::LightMagenta,
                "lightcyan" | "light_cyan" => Color::LightCyan,
                "reset" => Color::Reset,
                _ => Color::Reset,
            },
            ColorDef::Rgb { r, g, b } => Color::Rgb(*r, *g, *b),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    pub bg: ColorDef,
    pub fg: ColorDef,
    pub muted: ColorDef,
    pub accent: ColorDef,
    pub accent_alt: ColorDef,
    pub success: ColorDef,
    pub warning: ColorDef,
    pub error: ColorDef,
    pub info: ColorDef,
    pub border: ColorDef,
    pub border_accent: ColorDef,
    pub selection_bg: ColorDef,
    pub header_bg: ColorDef,
    pub leader_bg: ColorDef,
    pub leader_fg: ColorDef,
    pub scrollbar: ColorDef,
    pub project_name: ColorDef,
    pub feature_name: ColorDef,
    pub session_icon_claude: ColorDef,
    pub session_icon_opencode: ColorDef,
    pub session_icon_codex: ColorDef,
    pub session_icon_terminal: ColorDef,
    pub session_icon_nvim: ColorDef,
    pub session_icon_custom: ColorDef,
    pub status_active: ColorDef,
    pub status_idle: ColorDef,
    pub status_stopped: ColorDef,
    pub status_waiting: ColorDef,
    pub mode_vibeless: ColorDef,
    pub mode_vibe: ColorDef,
    pub mode_supervibe: ColorDef,
    pub mode_review: ColorDef,
    pub custom_status_text: ColorDef,
    pub usage_low: ColorDef,
    pub usage_medium: ColorDef,
    pub usage_high: ColorDef,
    #[serde(skip)]
    pub transparent: bool,
}

impl Default for Theme {
    fn default() -> Self {
        Self::load(&ThemeName::Default)
    }
}

impl Theme {
    pub fn load(name: &ThemeName) -> Self {
        match name {
            ThemeName::Default => Self::default_theme(),
            ThemeName::Amf => Self::amf(),
            ThemeName::Dracula => Self::dracula(),
            ThemeName::Nord => Self::nord(),
            ThemeName::CatppuccinFrappe => Self::catppuccin_frappe(),
        }
    }

    pub fn set_transparent(&mut self, transparent: bool) {
        self.transparent = transparent;
    }

    pub fn effective_bg(&self) -> Color {
        if self.transparent {
            Color::Reset
        } else {
            self.bg.to_color()
        }
    }

    pub fn effective_header_bg(&self) -> Color {
        if self.transparent {
            Color::Reset
        } else {
            self.header_bg.to_color()
        }
    }

    pub fn effective_selection_bg(&self) -> Color {
        self.selection_bg.to_color()
    }

    fn default_theme() -> Self {
        Self {
            name: "default".to_string(),
            bg: ColorDef::named("reset"),
            fg: ColorDef::named("white"),
            muted: ColorDef::named("darkgray"),
            accent: ColorDef::named("cyan"),
            accent_alt: ColorDef::named("magenta"),
            success: ColorDef::named("green"),
            warning: ColorDef::named("yellow"),
            error: ColorDef::named("red"),
            info: ColorDef::named("cyan"),
            border: ColorDef::named("white"),
            border_accent: ColorDef::named("cyan"),
            selection_bg: ColorDef::named("darkgray"),
            header_bg: ColorDef::rgb(76, 79, 105),
            leader_bg: ColorDef::named("yellow"),
            leader_fg: ColorDef::named("black"),
            scrollbar: ColorDef::rgb(60, 60, 60),
            project_name: ColorDef::named("cyan"),
            feature_name: ColorDef::named("white"),
            session_icon_claude: ColorDef::named("magenta"),
            session_icon_opencode: ColorDef::named("cyan"),
            session_icon_codex: ColorDef::named("lightblue"),
            session_icon_terminal: ColorDef::named("green"),
            session_icon_nvim: ColorDef::named("cyan"),
            session_icon_custom: ColorDef::named("yellow"),
            status_active: ColorDef::named("green"),
            status_idle: ColorDef::named("yellow"),
            status_stopped: ColorDef::named("red"),
            status_waiting: ColorDef::rgb(255, 165, 0),
            custom_status_text: ColorDef::named("cyan"),
            mode_vibeless: ColorDef::named("green"),
            mode_vibe: ColorDef::named("yellow"),
            mode_supervibe: ColorDef::named("magenta"),
            mode_review: ColorDef::named("magenta"),
            usage_low: ColorDef::named("green"),
            usage_medium: ColorDef::named("yellow"),
            usage_high: ColorDef::named("red"),
            transparent: false,
        }
    }

    fn amf() -> Self {
        Self {
            name: "amf".to_string(),
            bg: ColorDef::named("reset"),
            fg: ColorDef::named("white"),
            muted: ColorDef::named("darkgray"),
            accent: ColorDef::named("cyan"),
            accent_alt: ColorDef::named("magenta"),
            success: ColorDef::named("green"),
            warning: ColorDef::named("yellow"),
            error: ColorDef::named("red"),
            info: ColorDef::named("cyan"),
            border: ColorDef::named("white"),
            border_accent: ColorDef::named("cyan"),
            selection_bg: ColorDef::rgb(60, 60, 80),
            header_bg: ColorDef::rgb(40, 40, 60),
            leader_bg: ColorDef::named("yellow"),
            leader_fg: ColorDef::named("black"),
            scrollbar: ColorDef::rgb(60, 60, 60),
            project_name: ColorDef::named("cyan"),
            feature_name: ColorDef::named("white"),
            session_icon_claude: ColorDef::named("magenta"),
            session_icon_opencode: ColorDef::named("cyan"),
            session_icon_codex: ColorDef::named("lightblue"),
            session_icon_terminal: ColorDef::named("green"),
            session_icon_nvim: ColorDef::named("cyan"),
            session_icon_custom: ColorDef::named("yellow"),
            status_active: ColorDef::named("green"),
            status_idle: ColorDef::named("yellow"),
            status_stopped: ColorDef::named("red"),
            status_waiting: ColorDef::rgb(255, 165, 0),
            custom_status_text: ColorDef::named("cyan"),
            mode_vibeless: ColorDef::named("green"),
            mode_vibe: ColorDef::named("yellow"),
            mode_supervibe: ColorDef::named("magenta"),
            mode_review: ColorDef::named("magenta"),
            usage_low: ColorDef::named("green"),
            usage_medium: ColorDef::named("yellow"),
            usage_high: ColorDef::named("red"),
            transparent: false,
        }
    }

    fn dracula() -> Self {
        Self {
            name: "dracula".to_string(),
            bg: ColorDef::rgb(40, 42, 54),
            fg: ColorDef::rgb(248, 248, 242),
            muted: ColorDef::rgb(98, 114, 164),
            accent: ColorDef::rgb(139, 233, 253),
            accent_alt: ColorDef::rgb(255, 121, 198),
            success: ColorDef::rgb(80, 250, 123),
            warning: ColorDef::rgb(255, 184, 108),
            error: ColorDef::rgb(255, 85, 85),
            info: ColorDef::rgb(139, 233, 253),
            border: ColorDef::rgb(98, 114, 164),
            border_accent: ColorDef::rgb(139, 233, 253),
            selection_bg: ColorDef::rgb(68, 71, 90),
            header_bg: ColorDef::rgb(68, 71, 90),
            leader_bg: ColorDef::rgb(255, 184, 108),
            leader_fg: ColorDef::rgb(40, 42, 54),
            scrollbar: ColorDef::rgb(68, 71, 90),
            project_name: ColorDef::rgb(139, 233, 253),
            feature_name: ColorDef::rgb(248, 248, 242),
            session_icon_claude: ColorDef::rgb(255, 121, 198),
            session_icon_opencode: ColorDef::rgb(139, 233, 253),
            session_icon_codex: ColorDef::rgb(80, 250, 123),
            session_icon_terminal: ColorDef::rgb(80, 250, 123),
            session_icon_nvim: ColorDef::rgb(139, 233, 253),
            session_icon_custom: ColorDef::rgb(255, 184, 108),
            status_active: ColorDef::rgb(80, 250, 123),
            status_idle: ColorDef::rgb(255, 184, 108),
            status_stopped: ColorDef::rgb(255, 85, 85),
            status_waiting: ColorDef::rgb(241, 250, 140),
            custom_status_text: ColorDef::rgb(139, 233, 253),
            mode_vibeless: ColorDef::rgb(80, 250, 123),
            mode_vibe: ColorDef::rgb(255, 184, 108),
            mode_supervibe: ColorDef::rgb(255, 121, 198),
            mode_review: ColorDef::rgb(189, 147, 249),
            usage_low: ColorDef::rgb(80, 250, 123),
            usage_medium: ColorDef::rgb(255, 184, 108),
            usage_high: ColorDef::rgb(255, 85, 85),
            transparent: false,
        }
    }

    fn nord() -> Self {
        Self {
            name: "nord".to_string(),
            bg: ColorDef::rgb(46, 52, 64),
            fg: ColorDef::rgb(236, 239, 244),
            muted: ColorDef::rgb(129, 161, 193),
            accent: ColorDef::rgb(136, 192, 208),
            accent_alt: ColorDef::rgb(180, 142, 173),
            success: ColorDef::rgb(163, 190, 140),
            warning: ColorDef::rgb(235, 203, 139),
            error: ColorDef::rgb(191, 97, 106),
            info: ColorDef::rgb(136, 192, 208),
            border: ColorDef::rgb(129, 161, 193),
            border_accent: ColorDef::rgb(136, 192, 208),
            selection_bg: ColorDef::rgb(94, 129, 172),
            header_bg: ColorDef::rgb(59, 66, 82),
            leader_bg: ColorDef::rgb(235, 203, 139),
            leader_fg: ColorDef::rgb(46, 52, 64),
            scrollbar: ColorDef::rgb(76, 86, 106),
            project_name: ColorDef::rgb(136, 192, 208),
            feature_name: ColorDef::rgb(236, 239, 244),
            session_icon_claude: ColorDef::rgb(180, 142, 173),
            session_icon_opencode: ColorDef::rgb(136, 192, 208),
            session_icon_codex: ColorDef::rgb(163, 190, 140),
            session_icon_terminal: ColorDef::rgb(163, 190, 140),
            session_icon_nvim: ColorDef::rgb(136, 192, 208),
            session_icon_custom: ColorDef::rgb(235, 203, 139),
            status_active: ColorDef::rgb(163, 190, 140),
            status_idle: ColorDef::rgb(235, 203, 139),
            status_stopped: ColorDef::rgb(191, 97, 106),
            status_waiting: ColorDef::rgb(208, 135, 112),
            custom_status_text: ColorDef::rgb(136, 192, 208),
            mode_vibeless: ColorDef::rgb(163, 190, 140),
            mode_vibe: ColorDef::rgb(235, 203, 139),
            mode_supervibe: ColorDef::rgb(180, 142, 173),
            mode_review: ColorDef::rgb(180, 142, 173),
            usage_low: ColorDef::rgb(163, 190, 140),
            usage_medium: ColorDef::rgb(235, 203, 139),
            usage_high: ColorDef::rgb(191, 97, 106),
            transparent: false,
        }
    }

    fn catppuccin_frappe() -> Self {
        Self {
            name: "catppuccin-frappe".to_string(),
            bg: ColorDef::rgb(48, 52, 70),
            fg: ColorDef::rgb(198, 208, 245),
            muted: ColorDef::rgb(131, 139, 167),
            accent: ColorDef::rgb(140, 170, 238),
            accent_alt: ColorDef::rgb(244, 184, 228),
            success: ColorDef::rgb(166, 218, 149),
            warning: ColorDef::rgb(238, 212, 159),
            error: ColorDef::rgb(231, 130, 132),
            info: ColorDef::rgb(140, 170, 238),
            border: ColorDef::rgb(131, 139, 167),
            border_accent: ColorDef::rgb(140, 170, 238),
            selection_bg: ColorDef::rgb(81, 87, 109),
            header_bg: ColorDef::rgb(76, 79, 105),
            leader_bg: ColorDef::rgb(238, 212, 159),
            leader_fg: ColorDef::rgb(48, 52, 70),
            scrollbar: ColorDef::rgb(65, 69, 84),
            project_name: ColorDef::rgb(140, 170, 238),
            feature_name: ColorDef::rgb(198, 208, 245),
            session_icon_claude: ColorDef::rgb(244, 184, 228),
            session_icon_opencode: ColorDef::rgb(140, 170, 238),
            session_icon_codex: ColorDef::rgb(166, 218, 149),
            session_icon_terminal: ColorDef::rgb(166, 218, 149),
            session_icon_nvim: ColorDef::rgb(140, 170, 238),
            session_icon_custom: ColorDef::rgb(238, 212, 159),
            status_active: ColorDef::rgb(166, 218, 149),
            status_idle: ColorDef::rgb(238, 212, 159),
            status_stopped: ColorDef::rgb(231, 130, 132),
            status_waiting: ColorDef::rgb(254, 215, 102),
            custom_status_text: ColorDef::rgb(133, 193, 220),
            mode_vibeless: ColorDef::rgb(166, 218, 149),
            mode_vibe: ColorDef::rgb(238, 212, 159),
            mode_supervibe: ColorDef::rgb(244, 184, 228),
            mode_review: ColorDef::rgb(202, 158, 230),
            usage_low: ColorDef::rgb(166, 218, 149),
            usage_medium: ColorDef::rgb(238, 212, 159),
            usage_high: ColorDef::rgb(231, 130, 132),
            transparent: false,
        }
    }

    pub fn list() -> Vec<ThemeName> {
        vec![
            ThemeName::Default,
            ThemeName::Amf,
            ThemeName::Dracula,
            ThemeName::Nord,
            ThemeName::CatppuccinFrappe,
        ]
    }
}

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
