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
    CatppuccinLatte,
    CatppuccinFrappe,
    CatppuccinMacchiato,
    CatppuccinMocha,
    GruvboxDark,
    GruvboxLight,
    GruvboxMaterialDarkHard,
    GruvboxMaterialDarkMedium,
    GruvboxMaterialDarkSoft,
    GruvboxMaterialLightHard,
    GruvboxMaterialLightMedium,
    GruvboxMaterialLightSoft,
    GruvboxMaterialMixDarkHard,
    GruvboxMaterialMixDarkMedium,
    GruvboxMaterialMixDarkSoft,
    GruvboxMaterialMixLightHard,
    GruvboxMaterialMixLightMedium,
    GruvboxMaterialMixLightSoft,
    GruvboxMaterialOriginalDarkHard,
    GruvboxMaterialOriginalDarkMedium,
    GruvboxMaterialOriginalDarkSoft,
    GruvboxMaterialOriginalLightHard,
    GruvboxMaterialOriginalLightMedium,
    GruvboxMaterialOriginalLightSoft,
}

impl ThemeName {
    fn gruvbox_material_display_name(&self) -> Option<&'static str> {
        match self {
            ThemeName::GruvboxMaterialDarkHard => Some("Gruvbox Material Dark Hard"),
            ThemeName::GruvboxMaterialDarkMedium => Some("Gruvbox Material Dark Medium"),
            ThemeName::GruvboxMaterialDarkSoft => Some("Gruvbox Material Dark Soft"),
            ThemeName::GruvboxMaterialLightHard => Some("Gruvbox Material Light Hard"),
            ThemeName::GruvboxMaterialLightMedium => Some("Gruvbox Material Light Medium"),
            ThemeName::GruvboxMaterialLightSoft => Some("Gruvbox Material Light Soft"),
            ThemeName::GruvboxMaterialMixDarkHard => Some("Gruvbox Material Mix Dark Hard"),
            ThemeName::GruvboxMaterialMixDarkMedium => Some("Gruvbox Material Mix Dark Medium"),
            ThemeName::GruvboxMaterialMixDarkSoft => Some("Gruvbox Material Mix Dark Soft"),
            ThemeName::GruvboxMaterialMixLightHard => Some("Gruvbox Material Mix Light Hard"),
            ThemeName::GruvboxMaterialMixLightMedium => Some("Gruvbox Material Mix Light Medium"),
            ThemeName::GruvboxMaterialMixLightSoft => Some("Gruvbox Material Mix Light Soft"),
            ThemeName::GruvboxMaterialOriginalDarkHard => {
                Some("Gruvbox Material Original Dark Hard")
            }
            ThemeName::GruvboxMaterialOriginalDarkMedium => {
                Some("Gruvbox Material Original Dark Medium")
            }
            ThemeName::GruvboxMaterialOriginalDarkSoft => {
                Some("Gruvbox Material Original Dark Soft")
            }
            ThemeName::GruvboxMaterialOriginalLightHard => {
                Some("Gruvbox Material Original Light Hard")
            }
            ThemeName::GruvboxMaterialOriginalLightMedium => {
                Some("Gruvbox Material Original Light Medium")
            }
            ThemeName::GruvboxMaterialOriginalLightSoft => {
                Some("Gruvbox Material Original Light Soft")
            }
            _ => None,
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            ThemeName::Default => "default",
            ThemeName::Amf => "amf",
            ThemeName::Dracula => "dracula",
            ThemeName::Nord => "nord",
            ThemeName::CatppuccinLatte => "catppuccin-latte",
            ThemeName::CatppuccinFrappe => "catppuccin-frappe",
            ThemeName::CatppuccinMacchiato => "catppuccin-macchiato",
            ThemeName::CatppuccinMocha => "catppuccin-mocha",
            ThemeName::GruvboxDark => "gruvbox-dark",
            ThemeName::GruvboxLight => "gruvbox-light",
            ThemeName::GruvboxMaterialDarkHard => "gruvbox-material-dark-hard",
            ThemeName::GruvboxMaterialDarkMedium => "gruvbox-material-dark-medium",
            ThemeName::GruvboxMaterialDarkSoft => "gruvbox-material-dark-soft",
            ThemeName::GruvboxMaterialLightHard => "gruvbox-material-light-hard",
            ThemeName::GruvboxMaterialLightMedium => "gruvbox-material-light-medium",
            ThemeName::GruvboxMaterialLightSoft => "gruvbox-material-light-soft",
            ThemeName::GruvboxMaterialMixDarkHard => "gruvbox-material-mix-dark-hard",
            ThemeName::GruvboxMaterialMixDarkMedium => "gruvbox-material-mix-dark-medium",
            ThemeName::GruvboxMaterialMixDarkSoft => "gruvbox-material-mix-dark-soft",
            ThemeName::GruvboxMaterialMixLightHard => "gruvbox-material-mix-light-hard",
            ThemeName::GruvboxMaterialMixLightMedium => "gruvbox-material-mix-light-medium",
            ThemeName::GruvboxMaterialMixLightSoft => "gruvbox-material-mix-light-soft",
            ThemeName::GruvboxMaterialOriginalDarkHard => "gruvbox-material-original-dark-hard",
            ThemeName::GruvboxMaterialOriginalDarkMedium => "gruvbox-material-original-dark-medium",
            ThemeName::GruvboxMaterialOriginalDarkSoft => "gruvbox-material-original-dark-soft",
            ThemeName::GruvboxMaterialOriginalLightHard => "gruvbox-material-original-light-hard",
            ThemeName::GruvboxMaterialOriginalLightMedium => {
                "gruvbox-material-original-light-medium"
            }
            ThemeName::GruvboxMaterialOriginalLightSoft => "gruvbox-material-original-light-soft",
        }
    }

    fn gruvbox_material_spec(&self) -> Option<GruvboxMaterialSpec> {
        use GruvboxMaterialContrast::{Hard, Medium, Soft};
        use GruvboxMaterialForeground::{Material, Mix, Original};
        use GruvboxMaterialMode::{Dark, Light};

        let spec = match self {
            ThemeName::GruvboxMaterialDarkHard => (Dark, Hard, Material),
            ThemeName::GruvboxMaterialDarkMedium => (Dark, Medium, Material),
            ThemeName::GruvboxMaterialDarkSoft => (Dark, Soft, Material),
            ThemeName::GruvboxMaterialLightHard => (Light, Hard, Material),
            ThemeName::GruvboxMaterialLightMedium => (Light, Medium, Material),
            ThemeName::GruvboxMaterialLightSoft => (Light, Soft, Material),
            ThemeName::GruvboxMaterialMixDarkHard => (Dark, Hard, Mix),
            ThemeName::GruvboxMaterialMixDarkMedium => (Dark, Medium, Mix),
            ThemeName::GruvboxMaterialMixDarkSoft => (Dark, Soft, Mix),
            ThemeName::GruvboxMaterialMixLightHard => (Light, Hard, Mix),
            ThemeName::GruvboxMaterialMixLightMedium => (Light, Medium, Mix),
            ThemeName::GruvboxMaterialMixLightSoft => (Light, Soft, Mix),
            ThemeName::GruvboxMaterialOriginalDarkHard => (Dark, Hard, Original),
            ThemeName::GruvboxMaterialOriginalDarkMedium => (Dark, Medium, Original),
            ThemeName::GruvboxMaterialOriginalDarkSoft => (Dark, Soft, Original),
            ThemeName::GruvboxMaterialOriginalLightHard => (Light, Hard, Original),
            ThemeName::GruvboxMaterialOriginalLightMedium => (Light, Medium, Original),
            ThemeName::GruvboxMaterialOriginalLightSoft => (Light, Soft, Original),
            _ => return None,
        };

        Some(GruvboxMaterialSpec {
            mode: spec.0,
            contrast: spec.1,
            foreground: spec.2,
        })
    }

    pub fn display_name(&self) -> &str {
        if let Some(name) = self.gruvbox_material_display_name() {
            return name;
        }

        match self {
            ThemeName::Default => "Default",
            ThemeName::Amf => "AMF",
            ThemeName::Dracula => "Dracula",
            ThemeName::Nord => "Nord",
            ThemeName::CatppuccinLatte => "Catppuccin Latte",
            ThemeName::CatppuccinFrappe => "Catppuccin Frappe",
            ThemeName::CatppuccinMacchiato => "Catppuccin Macchiato",
            ThemeName::CatppuccinMocha => "Catppuccin Mocha",
            ThemeName::GruvboxDark => "Gruvbox Dark",
            ThemeName::GruvboxLight => "Gruvbox Light",
            _ => unreachable!("Gruvbox Material display names are handled above"),
        }
    }
}

impl std::fmt::Display for ThemeName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
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
            "catppuccin-latte" | "catppuccin_latte" => Ok(ThemeName::CatppuccinLatte),
            "catppuccin-frappe" | "catppuccin_frappe" => Ok(ThemeName::CatppuccinFrappe),
            "catppuccin-macchiato" | "catppuccin_macchiato" => Ok(ThemeName::CatppuccinMacchiato),
            "catppuccin-mocha" | "catppuccin_mocha" => Ok(ThemeName::CatppuccinMocha),
            "gruvbox-dark" | "gruvbox_dark" => Ok(ThemeName::GruvboxDark),
            "gruvbox-light" | "gruvbox_light" => Ok(ThemeName::GruvboxLight),
            "gruvbox-material-dark-hard" | "gruvbox_material_dark_hard" => {
                Ok(ThemeName::GruvboxMaterialDarkHard)
            }
            "gruvbox-material-dark-medium" | "gruvbox_material_dark_medium" => {
                Ok(ThemeName::GruvboxMaterialDarkMedium)
            }
            "gruvbox-material-dark-soft" | "gruvbox_material_dark_soft" => {
                Ok(ThemeName::GruvboxMaterialDarkSoft)
            }
            "gruvbox-material-light-hard" | "gruvbox_material_light_hard" => {
                Ok(ThemeName::GruvboxMaterialLightHard)
            }
            "gruvbox-material-light-medium" | "gruvbox_material_light_medium" => {
                Ok(ThemeName::GruvboxMaterialLightMedium)
            }
            "gruvbox-material-light-soft" | "gruvbox_material_light_soft" => {
                Ok(ThemeName::GruvboxMaterialLightSoft)
            }
            "gruvbox-material-mix-dark-hard" | "gruvbox_material_mix_dark_hard" => {
                Ok(ThemeName::GruvboxMaterialMixDarkHard)
            }
            "gruvbox-material-mix-dark-medium" | "gruvbox_material_mix_dark_medium" => {
                Ok(ThemeName::GruvboxMaterialMixDarkMedium)
            }
            "gruvbox-material-mix-dark-soft" | "gruvbox_material_mix_dark_soft" => {
                Ok(ThemeName::GruvboxMaterialMixDarkSoft)
            }
            "gruvbox-material-mix-light-hard" | "gruvbox_material_mix_light_hard" => {
                Ok(ThemeName::GruvboxMaterialMixLightHard)
            }
            "gruvbox-material-mix-light-medium" | "gruvbox_material_mix_light_medium" => {
                Ok(ThemeName::GruvboxMaterialMixLightMedium)
            }
            "gruvbox-material-mix-light-soft" | "gruvbox_material_mix_light_soft" => {
                Ok(ThemeName::GruvboxMaterialMixLightSoft)
            }
            "gruvbox-material-original-dark-hard" | "gruvbox_material_original_dark_hard" => {
                Ok(ThemeName::GruvboxMaterialOriginalDarkHard)
            }
            "gruvbox-material-original-dark-medium" | "gruvbox_material_original_dark_medium" => {
                Ok(ThemeName::GruvboxMaterialOriginalDarkMedium)
            }
            "gruvbox-material-original-dark-soft" | "gruvbox_material_original_dark_soft" => {
                Ok(ThemeName::GruvboxMaterialOriginalDarkSoft)
            }
            "gruvbox-material-original-light-hard" | "gruvbox_material_original_light_hard" => {
                Ok(ThemeName::GruvboxMaterialOriginalLightHard)
            }
            "gruvbox-material-original-light-medium" | "gruvbox_material_original_light_medium" => {
                Ok(ThemeName::GruvboxMaterialOriginalLightMedium)
            }
            "gruvbox-material-original-light-soft" | "gruvbox_material_original_light_soft" => {
                Ok(ThemeName::GruvboxMaterialOriginalLightSoft)
            }
            _ => Err(format!("Unknown theme: {}", s)),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum GruvboxMaterialMode {
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy)]
enum GruvboxMaterialContrast {
    Hard,
    Medium,
    Soft,
}

#[derive(Debug, Clone, Copy)]
enum GruvboxMaterialForeground {
    Material,
    Mix,
    Original,
}

#[derive(Debug, Clone, Copy)]
struct GruvboxMaterialSpec {
    mode: GruvboxMaterialMode,
    contrast: GruvboxMaterialContrast,
    foreground: GruvboxMaterialForeground,
}

#[derive(Debug, Clone, Copy)]
struct Rgb(u8, u8, u8);

impl Rgb {
    fn color_def(self) -> ColorDef {
        ColorDef::rgb(self.0, self.1, self.2)
    }
}

#[derive(Debug, Clone, Copy)]
struct GruvboxMaterialBackground {
    bg0: Rgb,
    bg1: Rgb,
    bg3: Rgb,
    bg5: Rgb,
    bg_current_word: Rgb,
}

#[derive(Debug, Clone, Copy)]
struct GruvboxMaterialForegroundPalette {
    fg1: Rgb,
    red: Rgb,
    orange: Rgb,
    yellow: Rgb,
    green: Rgb,
    aqua: Rgb,
    blue: Rgb,
    purple: Rgb,
    grey2: Rgb,
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
    pub background: ColorDef,
    pub text: ColorDef,
    pub text_muted: ColorDef,
    pub primary: ColorDef,
    pub secondary: ColorDef,
    pub success: ColorDef,
    pub warning: ColorDef,
    pub danger: ColorDef,
    pub info: ColorDef,
    pub border: ColorDef,
    pub border_focus: ColorDef,
    pub selection: ColorDef,
    pub header_background: ColorDef,
    pub shortcut_background: ColorDef,
    pub shortcut_text: ColorDef,
    pub scrollbar: ColorDef,
    pub project_title: ColorDef,
    pub feature_title: ColorDef,
    pub session_icon_claude: ColorDef,
    pub session_icon_opencode: ColorDef,
    pub session_icon_codex: ColorDef,
    pub session_icon_terminal: ColorDef,
    pub session_icon_nvim: ColorDef,
    pub session_icon_vscode: ColorDef,
    pub session_icon_custom: ColorDef,
    pub status_active: ColorDef,
    pub status_idle: ColorDef,
    pub status_stopped: ColorDef,
    pub status_waiting: ColorDef,
    pub mode_vibeless: ColorDef,
    pub mode_vibe: ColorDef,
    pub mode_supervibe: ColorDef,
    pub mode_review: ColorDef,
    pub status_detail: ColorDef,
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
        if let Some(spec) = name.gruvbox_material_spec() {
            return Self::gruvbox_material(name.as_str(), spec);
        }

        match name {
            ThemeName::Default => Self::default_theme(),
            ThemeName::Amf => Self::amf(),
            ThemeName::Dracula => Self::dracula(),
            ThemeName::Nord => Self::nord(),
            ThemeName::CatppuccinLatte => Self::catppuccin_latte(),
            ThemeName::CatppuccinFrappe => Self::catppuccin_frappe(),
            ThemeName::CatppuccinMacchiato => Self::catppuccin_macchiato(),
            ThemeName::CatppuccinMocha => Self::catppuccin_mocha(),
            ThemeName::GruvboxDark => Self::gruvbox_dark(),
            ThemeName::GruvboxLight => Self::gruvbox_light(),
            _ => unreachable!("Gruvbox Material themes are handled above"),
        }
    }

    pub fn set_transparent(&mut self, transparent: bool) {
        self.transparent = transparent;
    }

    pub fn effective_bg(&self) -> Color {
        if self.transparent {
            Color::Reset
        } else {
            self.background.to_color()
        }
    }

    pub fn effective_header_bg(&self) -> Color {
        if self.transparent {
            Color::Reset
        } else {
            self.header_background.to_color()
        }
    }

    pub fn effective_selection_bg(&self) -> Color {
        self.selection.to_color()
    }

    fn default_theme() -> Self {
        Self {
            name: "default".to_string(),
            background: ColorDef::rgb(48, 52, 70),
            text: ColorDef::named("white"),
            text_muted: ColorDef::named("darkgray"),
            primary: ColorDef::named("cyan"),
            secondary: ColorDef::named("magenta"),
            success: ColorDef::named("green"),
            warning: ColorDef::named("yellow"),
            danger: ColorDef::named("red"),
            info: ColorDef::named("cyan"),
            border: ColorDef::named("white"),
            border_focus: ColorDef::named("cyan"),
            selection: ColorDef::named("darkgray"),
            header_background: ColorDef::rgb(76, 79, 105),
            shortcut_background: ColorDef::named("yellow"),
            shortcut_text: ColorDef::named("black"),
            scrollbar: ColorDef::rgb(60, 60, 60),
            project_title: ColorDef::named("cyan"),
            feature_title: ColorDef::named("white"),
            session_icon_claude: ColorDef::named("magenta"),
            session_icon_opencode: ColorDef::named("cyan"),
            session_icon_codex: ColorDef::named("lightblue"),
            session_icon_terminal: ColorDef::named("green"),
            session_icon_nvim: ColorDef::named("cyan"),
            session_icon_vscode: ColorDef::named("lightblue"),
            session_icon_custom: ColorDef::named("yellow"),
            status_active: ColorDef::named("green"),
            status_idle: ColorDef::named("yellow"),
            status_stopped: ColorDef::named("red"),
            status_waiting: ColorDef::rgb(255, 165, 0),
            status_detail: ColorDef::named("cyan"),
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
            background: ColorDef::rgb(46, 52, 64),
            text: ColorDef::named("white"),
            text_muted: ColorDef::named("darkgray"),
            primary: ColorDef::named("cyan"),
            secondary: ColorDef::named("magenta"),
            success: ColorDef::named("green"),
            warning: ColorDef::named("yellow"),
            danger: ColorDef::named("red"),
            info: ColorDef::named("cyan"),
            border: ColorDef::named("white"),
            border_focus: ColorDef::named("cyan"),
            selection: ColorDef::rgb(60, 60, 80),
            header_background: ColorDef::rgb(40, 40, 60),
            shortcut_background: ColorDef::named("yellow"),
            shortcut_text: ColorDef::named("black"),
            scrollbar: ColorDef::rgb(60, 60, 60),
            project_title: ColorDef::named("cyan"),
            feature_title: ColorDef::named("white"),
            session_icon_claude: ColorDef::named("magenta"),
            session_icon_opencode: ColorDef::named("cyan"),
            session_icon_codex: ColorDef::named("lightblue"),
            session_icon_terminal: ColorDef::named("green"),
            session_icon_nvim: ColorDef::named("cyan"),
            session_icon_vscode: ColorDef::named("lightblue"),
            session_icon_custom: ColorDef::named("yellow"),
            status_active: ColorDef::named("green"),
            status_idle: ColorDef::named("yellow"),
            status_stopped: ColorDef::named("red"),
            status_waiting: ColorDef::rgb(255, 165, 0),
            status_detail: ColorDef::named("cyan"),
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
            background: ColorDef::rgb(40, 42, 54),
            text: ColorDef::rgb(248, 248, 242),
            text_muted: ColorDef::rgb(98, 114, 164),
            primary: ColorDef::rgb(139, 233, 253),
            secondary: ColorDef::rgb(255, 121, 198),
            success: ColorDef::rgb(80, 250, 123),
            warning: ColorDef::rgb(255, 184, 108),
            danger: ColorDef::rgb(255, 85, 85),
            info: ColorDef::rgb(139, 233, 253),
            border: ColorDef::rgb(98, 114, 164),
            border_focus: ColorDef::rgb(139, 233, 253),
            selection: ColorDef::rgb(68, 71, 90),
            header_background: ColorDef::rgb(68, 71, 90),
            shortcut_background: ColorDef::rgb(255, 184, 108),
            shortcut_text: ColorDef::rgb(40, 42, 54),
            scrollbar: ColorDef::rgb(68, 71, 90),
            project_title: ColorDef::rgb(139, 233, 253),
            feature_title: ColorDef::rgb(248, 248, 242),
            session_icon_claude: ColorDef::rgb(255, 121, 198),
            session_icon_opencode: ColorDef::rgb(139, 233, 253),
            session_icon_codex: ColorDef::rgb(80, 250, 123),
            session_icon_terminal: ColorDef::rgb(80, 250, 123),
            session_icon_nvim: ColorDef::rgb(139, 233, 253),
            session_icon_vscode: ColorDef::rgb(189, 147, 249),
            session_icon_custom: ColorDef::rgb(255, 184, 108),
            status_active: ColorDef::rgb(80, 250, 123),
            status_idle: ColorDef::rgb(255, 184, 108),
            status_stopped: ColorDef::rgb(255, 85, 85),
            status_waiting: ColorDef::rgb(241, 250, 140),
            status_detail: ColorDef::rgb(139, 233, 253),
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
            background: ColorDef::rgb(46, 52, 64),
            text: ColorDef::rgb(236, 239, 244),
            text_muted: ColorDef::rgb(129, 161, 193),
            primary: ColorDef::rgb(136, 192, 208),
            secondary: ColorDef::rgb(180, 142, 173),
            success: ColorDef::rgb(163, 190, 140),
            warning: ColorDef::rgb(235, 203, 139),
            danger: ColorDef::rgb(191, 97, 106),
            info: ColorDef::rgb(136, 192, 208),
            border: ColorDef::rgb(129, 161, 193),
            border_focus: ColorDef::rgb(136, 192, 208),
            selection: ColorDef::rgb(94, 129, 172),
            header_background: ColorDef::rgb(59, 66, 82),
            shortcut_background: ColorDef::rgb(235, 203, 139),
            shortcut_text: ColorDef::rgb(46, 52, 64),
            scrollbar: ColorDef::rgb(76, 86, 106),
            project_title: ColorDef::rgb(136, 192, 208),
            feature_title: ColorDef::rgb(236, 239, 244),
            session_icon_claude: ColorDef::rgb(180, 142, 173),
            session_icon_opencode: ColorDef::rgb(136, 192, 208),
            session_icon_codex: ColorDef::rgb(163, 190, 140),
            session_icon_terminal: ColorDef::rgb(163, 190, 140),
            session_icon_nvim: ColorDef::rgb(136, 192, 208),
            session_icon_vscode: ColorDef::rgb(94, 129, 172),
            session_icon_custom: ColorDef::rgb(235, 203, 139),
            status_active: ColorDef::rgb(163, 190, 140),
            status_idle: ColorDef::rgb(235, 203, 139),
            status_stopped: ColorDef::rgb(191, 97, 106),
            status_waiting: ColorDef::rgb(208, 135, 112),
            status_detail: ColorDef::rgb(136, 192, 208),
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

    fn catppuccin_latte() -> Self {
        Self {
            name: "catppuccin-latte".to_string(),
            background: ColorDef::rgb(239, 241, 245),
            text: ColorDef::rgb(76, 79, 105),
            text_muted: ColorDef::rgb(108, 111, 133),
            primary: ColorDef::rgb(30, 102, 245),
            secondary: ColorDef::rgb(114, 135, 253),
            success: ColorDef::rgb(64, 160, 43),
            warning: ColorDef::rgb(223, 142, 29),
            danger: ColorDef::rgb(210, 15, 57),
            info: ColorDef::rgb(32, 159, 181),
            border: ColorDef::rgb(156, 160, 176),
            border_focus: ColorDef::rgb(114, 135, 253),
            selection: ColorDef::rgb(204, 208, 218),
            header_background: ColorDef::rgb(230, 233, 239),
            shortcut_background: ColorDef::rgb(30, 102, 245),
            shortcut_text: ColorDef::rgb(239, 241, 245),
            scrollbar: ColorDef::rgb(172, 176, 190),
            project_title: ColorDef::rgb(30, 102, 245),
            feature_title: ColorDef::rgb(76, 79, 105),
            session_icon_claude: ColorDef::rgb(136, 57, 239),
            session_icon_opencode: ColorDef::rgb(32, 159, 181),
            session_icon_codex: ColorDef::rgb(64, 160, 43),
            session_icon_terminal: ColorDef::rgb(64, 160, 43),
            session_icon_nvim: ColorDef::rgb(23, 146, 153),
            session_icon_vscode: ColorDef::rgb(4, 165, 229),
            session_icon_custom: ColorDef::rgb(254, 100, 11),
            status_active: ColorDef::rgb(64, 160, 43),
            status_idle: ColorDef::rgb(223, 142, 29),
            status_stopped: ColorDef::rgb(210, 15, 57),
            status_waiting: ColorDef::rgb(254, 100, 11),
            status_detail: ColorDef::rgb(32, 159, 181),
            mode_vibeless: ColorDef::rgb(64, 160, 43),
            mode_vibe: ColorDef::rgb(223, 142, 29),
            mode_supervibe: ColorDef::rgb(234, 118, 203),
            mode_review: ColorDef::rgb(114, 135, 253),
            usage_low: ColorDef::rgb(64, 160, 43),
            usage_medium: ColorDef::rgb(223, 142, 29),
            usage_high: ColorDef::rgb(210, 15, 57),
            transparent: false,
        }
    }

    fn catppuccin_frappe() -> Self {
        Self {
            name: "catppuccin-frappe".to_string(),
            background: ColorDef::rgb(48, 52, 70),
            text: ColorDef::rgb(198, 208, 245),
            text_muted: ColorDef::rgb(165, 173, 206),
            primary: ColorDef::rgb(140, 170, 238),
            secondary: ColorDef::rgb(186, 187, 241),
            success: ColorDef::rgb(166, 218, 149),
            warning: ColorDef::rgb(229, 200, 144),
            danger: ColorDef::rgb(231, 130, 132),
            info: ColorDef::rgb(129, 200, 190),
            border: ColorDef::rgb(115, 121, 148),
            border_focus: ColorDef::rgb(186, 187, 241),
            selection: ColorDef::rgb(81, 87, 109),
            header_background: ColorDef::rgb(41, 44, 60),
            shortcut_background: ColorDef::rgb(140, 170, 238),
            shortcut_text: ColorDef::rgb(48, 52, 70),
            scrollbar: ColorDef::rgb(98, 104, 128),
            project_title: ColorDef::rgb(140, 170, 238),
            feature_title: ColorDef::rgb(198, 208, 245),
            session_icon_claude: ColorDef::rgb(202, 158, 230),
            session_icon_opencode: ColorDef::rgb(133, 193, 220),
            session_icon_codex: ColorDef::rgb(166, 218, 149),
            session_icon_terminal: ColorDef::rgb(166, 218, 149),
            session_icon_nvim: ColorDef::rgb(129, 200, 190),
            session_icon_vscode: ColorDef::rgb(153, 209, 219),
            session_icon_custom: ColorDef::rgb(239, 159, 118),
            status_active: ColorDef::rgb(166, 218, 149),
            status_idle: ColorDef::rgb(229, 200, 144),
            status_stopped: ColorDef::rgb(231, 130, 132),
            status_waiting: ColorDef::rgb(239, 159, 118),
            status_detail: ColorDef::rgb(133, 193, 220),
            mode_vibeless: ColorDef::rgb(166, 218, 149),
            mode_vibe: ColorDef::rgb(229, 200, 144),
            mode_supervibe: ColorDef::rgb(244, 184, 228),
            mode_review: ColorDef::rgb(186, 187, 241),
            usage_low: ColorDef::rgb(166, 218, 149),
            usage_medium: ColorDef::rgb(229, 200, 144),
            usage_high: ColorDef::rgb(231, 130, 132),
            transparent: false,
        }
    }

    fn catppuccin_macchiato() -> Self {
        Self {
            name: "catppuccin-macchiato".to_string(),
            background: ColorDef::rgb(36, 39, 58),
            text: ColorDef::rgb(202, 211, 245),
            text_muted: ColorDef::rgb(165, 173, 203),
            primary: ColorDef::rgb(138, 173, 244),
            secondary: ColorDef::rgb(183, 189, 248),
            success: ColorDef::rgb(166, 218, 149),
            warning: ColorDef::rgb(238, 212, 159),
            danger: ColorDef::rgb(237, 135, 150),
            info: ColorDef::rgb(125, 196, 228),
            border: ColorDef::rgb(110, 115, 141),
            border_focus: ColorDef::rgb(183, 189, 248),
            selection: ColorDef::rgb(54, 58, 79),
            header_background: ColorDef::rgb(30, 32, 48),
            shortcut_background: ColorDef::rgb(138, 173, 244),
            shortcut_text: ColorDef::rgb(36, 39, 58),
            scrollbar: ColorDef::rgb(91, 96, 120),
            project_title: ColorDef::rgb(138, 173, 244),
            feature_title: ColorDef::rgb(202, 211, 245),
            session_icon_claude: ColorDef::rgb(198, 160, 246),
            session_icon_opencode: ColorDef::rgb(125, 196, 228),
            session_icon_codex: ColorDef::rgb(166, 218, 149),
            session_icon_terminal: ColorDef::rgb(166, 218, 149),
            session_icon_nvim: ColorDef::rgb(139, 213, 202),
            session_icon_vscode: ColorDef::rgb(145, 215, 227),
            session_icon_custom: ColorDef::rgb(245, 169, 127),
            status_active: ColorDef::rgb(166, 218, 149),
            status_idle: ColorDef::rgb(238, 212, 159),
            status_stopped: ColorDef::rgb(237, 135, 150),
            status_waiting: ColorDef::rgb(245, 169, 127),
            status_detail: ColorDef::rgb(125, 196, 228),
            mode_vibeless: ColorDef::rgb(166, 218, 149),
            mode_vibe: ColorDef::rgb(238, 212, 159),
            mode_supervibe: ColorDef::rgb(245, 189, 230),
            mode_review: ColorDef::rgb(183, 189, 248),
            usage_low: ColorDef::rgb(166, 218, 149),
            usage_medium: ColorDef::rgb(238, 212, 159),
            usage_high: ColorDef::rgb(237, 135, 150),
            transparent: false,
        }
    }

    fn catppuccin_mocha() -> Self {
        Self {
            name: "catppuccin-mocha".to_string(),
            background: ColorDef::rgb(30, 30, 46),
            text: ColorDef::rgb(205, 214, 244),
            text_muted: ColorDef::rgb(166, 173, 200),
            primary: ColorDef::rgb(137, 180, 250),
            secondary: ColorDef::rgb(180, 190, 254),
            success: ColorDef::rgb(166, 227, 161),
            warning: ColorDef::rgb(249, 226, 175),
            danger: ColorDef::rgb(243, 139, 168),
            info: ColorDef::rgb(116, 199, 236),
            border: ColorDef::rgb(108, 112, 134),
            border_focus: ColorDef::rgb(180, 190, 254),
            selection: ColorDef::rgb(49, 50, 68),
            header_background: ColorDef::rgb(24, 24, 37),
            shortcut_background: ColorDef::rgb(137, 180, 250),
            shortcut_text: ColorDef::rgb(30, 30, 46),
            scrollbar: ColorDef::rgb(88, 91, 112),
            project_title: ColorDef::rgb(137, 180, 250),
            feature_title: ColorDef::rgb(205, 214, 244),
            session_icon_claude: ColorDef::rgb(203, 166, 247),
            session_icon_opencode: ColorDef::rgb(116, 199, 236),
            session_icon_codex: ColorDef::rgb(166, 227, 161),
            session_icon_terminal: ColorDef::rgb(166, 227, 161),
            session_icon_nvim: ColorDef::rgb(148, 226, 213),
            session_icon_vscode: ColorDef::rgb(137, 220, 235),
            session_icon_custom: ColorDef::rgb(250, 179, 135),
            status_active: ColorDef::rgb(166, 227, 161),
            status_idle: ColorDef::rgb(249, 226, 175),
            status_stopped: ColorDef::rgb(243, 139, 168),
            status_waiting: ColorDef::rgb(250, 179, 135),
            status_detail: ColorDef::rgb(116, 199, 236),
            mode_vibeless: ColorDef::rgb(166, 227, 161),
            mode_vibe: ColorDef::rgb(249, 226, 175),
            mode_supervibe: ColorDef::rgb(245, 194, 231),
            mode_review: ColorDef::rgb(180, 190, 254),
            usage_low: ColorDef::rgb(166, 227, 161),
            usage_medium: ColorDef::rgb(249, 226, 175),
            usage_high: ColorDef::rgb(243, 139, 168),
            transparent: false,
        }
    }

    fn gruvbox_dark() -> Self {
        // Gruvbox dark (medium contrast) – morhetz/gruvbox
        Self {
            name: "gruvbox-dark".to_string(),
            // bg: #282828
            background: ColorDef::rgb(40, 40, 40),
            // fg1: #ebdbb2
            text: ColorDef::rgb(235, 219, 178),
            // fg4: #a89984
            text_muted: ColorDef::rgb(168, 153, 132),
            // bright_blue: #83a598
            primary: ColorDef::rgb(131, 165, 152),
            // bright_purple: #d3869b
            secondary: ColorDef::rgb(211, 134, 155),
            // bright_green: #b8bb26
            success: ColorDef::rgb(184, 187, 38),
            // bright_yellow: #fabd2f
            warning: ColorDef::rgb(250, 189, 47),
            // bright_red: #fb4934
            danger: ColorDef::rgb(251, 73, 52),
            // bright_aqua: #8ec07c
            info: ColorDef::rgb(142, 192, 124),
            // bg3: #665c54
            border: ColorDef::rgb(102, 92, 84),
            // bright_blue: #83a598
            border_focus: ColorDef::rgb(131, 165, 152),
            // bg2: #504945
            selection: ColorDef::rgb(80, 73, 69),
            // bg1: #3c3836
            header_background: ColorDef::rgb(60, 56, 54),
            // bright_yellow: #fabd2f
            shortcut_background: ColorDef::rgb(250, 189, 47),
            // bg: #282828
            shortcut_text: ColorDef::rgb(40, 40, 40),
            // bg2: #504945
            scrollbar: ColorDef::rgb(80, 73, 69),
            // bright_blue: #83a598
            project_title: ColorDef::rgb(131, 165, 152),
            // fg1: #ebdbb2
            feature_title: ColorDef::rgb(235, 219, 178),
            // bright_purple: #d3869b
            session_icon_claude: ColorDef::rgb(211, 134, 155),
            // bright_aqua: #8ec07c
            session_icon_opencode: ColorDef::rgb(142, 192, 124),
            // bright_green: #b8bb26
            session_icon_codex: ColorDef::rgb(184, 187, 38),
            // bright_green: #b8bb26
            session_icon_terminal: ColorDef::rgb(184, 187, 38),
            // bright_blue: #83a598
            session_icon_nvim: ColorDef::rgb(131, 165, 152),
            // blue: #458588
            session_icon_vscode: ColorDef::rgb(69, 133, 136),
            // bright_orange: #fe8019
            session_icon_custom: ColorDef::rgb(254, 128, 25),
            // bright_green: #b8bb26
            status_active: ColorDef::rgb(184, 187, 38),
            // bright_yellow: #fabd2f
            status_idle: ColorDef::rgb(250, 189, 47),
            // bright_red: #fb4934
            status_stopped: ColorDef::rgb(251, 73, 52),
            // bright_orange: #fe8019
            status_waiting: ColorDef::rgb(254, 128, 25),
            // bright_blue: #83a598
            status_detail: ColorDef::rgb(131, 165, 152),
            // bright_green: #b8bb26
            mode_vibeless: ColorDef::rgb(184, 187, 38),
            // bright_yellow: #fabd2f
            mode_vibe: ColorDef::rgb(250, 189, 47),
            // bright_orange: #fe8019
            mode_supervibe: ColorDef::rgb(254, 128, 25),
            // bright_purple: #d3869b
            mode_review: ColorDef::rgb(211, 134, 155),
            // bright_green: #b8bb26
            usage_low: ColorDef::rgb(184, 187, 38),
            // bright_yellow: #fabd2f
            usage_medium: ColorDef::rgb(250, 189, 47),
            // bright_red: #fb4934
            usage_high: ColorDef::rgb(251, 73, 52),
            transparent: false,
        }
    }

    fn gruvbox_light() -> Self {
        // Gruvbox light (medium contrast) – morhetz/gruvbox
        // In light mode the bg/fg roles flip: light* colors become backgrounds,
        // dark* colors become foreground. Accent colors use the normal (non-bright)
        // variants for proper contrast on a pale background.
        Self {
            name: "gruvbox-light".to_string(),
            // light0: #fbf1c7
            background: ColorDef::rgb(251, 241, 199),
            // dark0: #282828
            text: ColorDef::rgb(40, 40, 40),
            // dark3: #665c54
            text_muted: ColorDef::rgb(102, 92, 84),
            // blue: #458588
            primary: ColorDef::rgb(69, 133, 136),
            // purple: #b16286
            secondary: ColorDef::rgb(177, 98, 134),
            // green: #98971a
            success: ColorDef::rgb(152, 151, 26),
            // yellow: #d79921
            warning: ColorDef::rgb(215, 153, 33),
            // red: #cc241d
            danger: ColorDef::rgb(204, 36, 29),
            // aqua: #689d6a
            info: ColorDef::rgb(104, 157, 106),
            // light3: #bdae93
            border: ColorDef::rgb(189, 174, 147),
            // blue: #458588
            border_focus: ColorDef::rgb(69, 133, 136),
            // light2: #d5c4a1
            selection: ColorDef::rgb(213, 196, 161),
            // light1: #ebdbb2
            header_background: ColorDef::rgb(235, 219, 178),
            // yellow: #d79921
            shortcut_background: ColorDef::rgb(215, 153, 33),
            // light0: #fbf1c7
            shortcut_text: ColorDef::rgb(251, 241, 199),
            // light2: #d5c4a1
            scrollbar: ColorDef::rgb(213, 196, 161),
            // blue: #458588
            project_title: ColorDef::rgb(69, 133, 136),
            // dark0: #282828
            feature_title: ColorDef::rgb(40, 40, 40),
            // purple: #b16286
            session_icon_claude: ColorDef::rgb(177, 98, 134),
            // aqua: #689d6a
            session_icon_opencode: ColorDef::rgb(104, 157, 106),
            // green: #98971a
            session_icon_codex: ColorDef::rgb(152, 151, 26),
            // green: #98971a
            session_icon_terminal: ColorDef::rgb(152, 151, 26),
            // blue: #458588
            session_icon_nvim: ColorDef::rgb(69, 133, 136),
            // faded_blue: #076678
            session_icon_vscode: ColorDef::rgb(7, 102, 120),
            // orange: #d65d0e
            session_icon_custom: ColorDef::rgb(214, 93, 14),
            // green: #98971a
            status_active: ColorDef::rgb(152, 151, 26),
            // yellow: #d79921
            status_idle: ColorDef::rgb(215, 153, 33),
            // red: #cc241d
            status_stopped: ColorDef::rgb(204, 36, 29),
            // orange: #d65d0e
            status_waiting: ColorDef::rgb(214, 93, 14),
            // blue: #458588
            status_detail: ColorDef::rgb(69, 133, 136),
            // green: #98971a
            mode_vibeless: ColorDef::rgb(152, 151, 26),
            // yellow: #d79921
            mode_vibe: ColorDef::rgb(215, 153, 33),
            // orange: #d65d0e
            mode_supervibe: ColorDef::rgb(214, 93, 14),
            // purple: #b16286
            mode_review: ColorDef::rgb(177, 98, 134),
            // green: #98971a
            usage_low: ColorDef::rgb(152, 151, 26),
            // yellow: #d79921
            usage_medium: ColorDef::rgb(215, 153, 33),
            // red: #cc241d
            usage_high: ColorDef::rgb(204, 36, 29),
            transparent: false,
        }
    }

    fn gruvbox_material(name: &str, spec: GruvboxMaterialSpec) -> Self {
        let bg = gruvbox_material_background(spec.mode, spec.contrast);
        let fg = gruvbox_material_foreground(spec.mode, spec.foreground);

        Self {
            name: name.to_string(),
            background: bg.bg0.color_def(),
            text: fg.fg1.color_def(),
            text_muted: fg.grey2.color_def(),
            primary: fg.blue.color_def(),
            secondary: fg.purple.color_def(),
            success: fg.green.color_def(),
            warning: fg.yellow.color_def(),
            danger: fg.red.color_def(),
            info: fg.aqua.color_def(),
            border: bg.bg5.color_def(),
            border_focus: fg.blue.color_def(),
            selection: bg.bg_current_word.color_def(),
            header_background: bg.bg1.color_def(),
            shortcut_background: fg.yellow.color_def(),
            shortcut_text: bg.bg0.color_def(),
            scrollbar: bg.bg3.color_def(),
            project_title: fg.blue.color_def(),
            feature_title: fg.fg1.color_def(),
            session_icon_claude: fg.purple.color_def(),
            session_icon_opencode: fg.aqua.color_def(),
            session_icon_codex: fg.green.color_def(),
            session_icon_terminal: fg.green.color_def(),
            session_icon_nvim: fg.blue.color_def(),
            session_icon_vscode: fg.blue.color_def(),
            session_icon_custom: fg.orange.color_def(),
            status_active: fg.green.color_def(),
            status_idle: fg.yellow.color_def(),
            status_stopped: fg.red.color_def(),
            status_waiting: fg.orange.color_def(),
            status_detail: fg.blue.color_def(),
            mode_vibeless: fg.green.color_def(),
            mode_vibe: fg.yellow.color_def(),
            mode_supervibe: fg.orange.color_def(),
            mode_review: fg.purple.color_def(),
            usage_low: fg.green.color_def(),
            usage_medium: fg.yellow.color_def(),
            usage_high: fg.red.color_def(),
            transparent: false,
        }
    }

    pub fn list() -> Vec<ThemeName> {
        vec![
            ThemeName::Default,
            ThemeName::Amf,
            ThemeName::Dracula,
            ThemeName::Nord,
            ThemeName::CatppuccinLatte,
            ThemeName::CatppuccinFrappe,
            ThemeName::CatppuccinMacchiato,
            ThemeName::CatppuccinMocha,
            ThemeName::GruvboxDark,
            ThemeName::GruvboxLight,
            ThemeName::GruvboxMaterialDarkHard,
            ThemeName::GruvboxMaterialDarkMedium,
            ThemeName::GruvboxMaterialDarkSoft,
            ThemeName::GruvboxMaterialLightHard,
            ThemeName::GruvboxMaterialLightMedium,
            ThemeName::GruvboxMaterialLightSoft,
            ThemeName::GruvboxMaterialMixDarkHard,
            ThemeName::GruvboxMaterialMixDarkMedium,
            ThemeName::GruvboxMaterialMixDarkSoft,
            ThemeName::GruvboxMaterialMixLightHard,
            ThemeName::GruvboxMaterialMixLightMedium,
            ThemeName::GruvboxMaterialMixLightSoft,
            ThemeName::GruvboxMaterialOriginalDarkHard,
            ThemeName::GruvboxMaterialOriginalDarkMedium,
            ThemeName::GruvboxMaterialOriginalDarkSoft,
            ThemeName::GruvboxMaterialOriginalLightHard,
            ThemeName::GruvboxMaterialOriginalLightMedium,
            ThemeName::GruvboxMaterialOriginalLightSoft,
        ]
    }
}

fn gruvbox_material_background(
    mode: GruvboxMaterialMode,
    contrast: GruvboxMaterialContrast,
) -> GruvboxMaterialBackground {
    use GruvboxMaterialContrast::{Hard, Medium, Soft};
    use GruvboxMaterialMode::{Dark, Light};

    let (bg0, bg1, bg3, bg5, bg_current_word) = match (mode, contrast) {
        (Dark, Hard) => (
            Rgb(0x1d, 0x20, 0x21),
            Rgb(0x28, 0x28, 0x28),
            Rgb(0x3c, 0x38, 0x36),
            Rgb(0x50, 0x49, 0x45),
            Rgb(0x32, 0x30, 0x2f),
        ),
        (Dark, Medium) => (
            Rgb(0x28, 0x28, 0x28),
            Rgb(0x32, 0x30, 0x2f),
            Rgb(0x45, 0x40, 0x3d),
            Rgb(0x5a, 0x52, 0x4c),
            Rgb(0x3c, 0x38, 0x36),
        ),
        (Dark, Soft) => (
            Rgb(0x32, 0x30, 0x2f),
            Rgb(0x3c, 0x38, 0x36),
            Rgb(0x50, 0x49, 0x45),
            Rgb(0x66, 0x5c, 0x54),
            Rgb(0x45, 0x40, 0x3d),
        ),
        (Light, Hard) => (
            Rgb(0xf9, 0xf5, 0xd7),
            Rgb(0xf5, 0xed, 0xca),
            Rgb(0xf2, 0xe5, 0xbc),
            Rgb(0xeb, 0xdb, 0xb2),
            Rgb(0xf3, 0xea, 0xc7),
        ),
        (Light, Medium) => (
            Rgb(0xfb, 0xf1, 0xc7),
            Rgb(0xf4, 0xe8, 0xbe),
            Rgb(0xee, 0xe0, 0xb7),
            Rgb(0xdd, 0xcc, 0xab),
            Rgb(0xf2, 0xe5, 0xbc),
        ),
        (Light, Soft) => (
            Rgb(0xf2, 0xe5, 0xbc),
            Rgb(0xed, 0xde, 0xb5),
            Rgb(0xe6, 0xd5, 0xae),
            Rgb(0xd5, 0xc4, 0xa1),
            Rgb(0xeb, 0xdb, 0xb2),
        ),
    };

    GruvboxMaterialBackground {
        bg0,
        bg1,
        bg3,
        bg5,
        bg_current_word,
    }
}

fn gruvbox_material_foreground(
    mode: GruvboxMaterialMode,
    foreground: GruvboxMaterialForeground,
) -> GruvboxMaterialForegroundPalette {
    use GruvboxMaterialForeground::{Material, Mix, Original};
    use GruvboxMaterialMode::{Dark, Light};

    let (fg1, red, orange, yellow, green, aqua, blue, purple, grey2) = match (mode, foreground) {
        (Dark, Material) => (
            Rgb(0xdd, 0xc7, 0xa1),
            Rgb(0xea, 0x69, 0x62),
            Rgb(0xe7, 0x8a, 0x4e),
            Rgb(0xd8, 0xa6, 0x57),
            Rgb(0xa9, 0xb6, 0x65),
            Rgb(0x89, 0xb4, 0x82),
            Rgb(0x7d, 0xae, 0xa3),
            Rgb(0xd3, 0x86, 0x9b),
            Rgb(0xa8, 0x99, 0x84),
        ),
        (Light, Material) => (
            Rgb(0x4f, 0x38, 0x29),
            Rgb(0xc1, 0x4a, 0x4a),
            Rgb(0xc3, 0x5e, 0x0a),
            Rgb(0xb4, 0x71, 0x09),
            Rgb(0x6c, 0x78, 0x2e),
            Rgb(0x4c, 0x7a, 0x5d),
            Rgb(0x45, 0x70, 0x7a),
            Rgb(0x94, 0x5e, 0x80),
            Rgb(0x7c, 0x6f, 0x64),
        ),
        (Dark, Mix) => (
            Rgb(0xe2, 0xcc, 0xa9),
            Rgb(0xf2, 0x59, 0x4b),
            Rgb(0xf2, 0x85, 0x34),
            Rgb(0xe9, 0xb1, 0x43),
            Rgb(0xb0, 0xb8, 0x46),
            Rgb(0x8b, 0xba, 0x7f),
            Rgb(0x80, 0xaa, 0x9e),
            Rgb(0xd3, 0x86, 0x9b),
            Rgb(0xa8, 0x99, 0x84),
        ),
        (Light, Mix) => (
            Rgb(0x51, 0x40, 0x36),
            Rgb(0xaf, 0x25, 0x28),
            Rgb(0xb9, 0x4c, 0x07),
            Rgb(0xb4, 0x73, 0x0e),
            Rgb(0x72, 0x76, 0x1e),
            Rgb(0x47, 0x7a, 0x5b),
            Rgb(0x26, 0x6b, 0x79),
            Rgb(0x92, 0x4f, 0x79),
            Rgb(0x7c, 0x6f, 0x64),
        ),
        (Dark, Original) => (
            Rgb(0xeb, 0xdb, 0xb2),
            Rgb(0xfb, 0x49, 0x34),
            Rgb(0xfe, 0x80, 0x19),
            Rgb(0xfa, 0xbd, 0x2f),
            Rgb(0xb8, 0xbb, 0x26),
            Rgb(0x8e, 0xc0, 0x7c),
            Rgb(0x83, 0xa5, 0x98),
            Rgb(0xd3, 0x86, 0x9b),
            Rgb(0xa8, 0x99, 0x84),
        ),
        (Light, Original) => (
            Rgb(0x3c, 0x38, 0x36),
            Rgb(0x9d, 0x00, 0x06),
            Rgb(0xaf, 0x3a, 0x03),
            Rgb(0xb5, 0x76, 0x14),
            Rgb(0x79, 0x74, 0x0e),
            Rgb(0x42, 0x7b, 0x58),
            Rgb(0x07, 0x66, 0x78),
            Rgb(0x8f, 0x3f, 0x71),
            Rgb(0x7c, 0x6f, 0x64),
        ),
    };

    GruvboxMaterialForegroundPalette {
        fg1,
        red,
        orange,
        yellow,
        green,
        aqua,
        blue,
        purple,
        grey2,
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
            (
                "amf-gruvbox.json",
                include_str!("../themes/opencode/amf-gruvbox.json"),
            ),
        ];

        for (filename, content) in theme_files {
            let theme_path = themes_dir.join(filename);
            fs::write(&theme_path, content)?;
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

        let gruvbox_theme = themes_dir.join("amf-gruvbox.json");
        assert!(gruvbox_theme.exists(), "amf-gruvbox.json should exist");

        let content = std::fs::read_to_string(&amf_theme).unwrap();
        assert!(
            content.contains("\"background\": {"),
            "Theme main background should be defined"
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
        std::fs::write(&amf_theme, "{\"custom\":true}").unwrap();

        ThemeManager::inject_opencode_themes(workdir).unwrap();

        let new_content = std::fs::read_to_string(&amf_theme).unwrap();
        assert!(
            new_content.contains("\"background\": {"),
            "Second injection should refresh AMF-managed theme files"
        );
    }

    #[test]
    fn test_theme_names_round_trip() {
        for theme_name in Theme::list() {
            let config_name = theme_name.to_string();
            let parsed: ThemeName = config_name.parse().unwrap();

            assert_eq!(parsed, theme_name);
            assert_eq!(Theme::load(&parsed).name, config_name);
            assert!(!parsed.display_name().is_empty());
        }
    }
}
