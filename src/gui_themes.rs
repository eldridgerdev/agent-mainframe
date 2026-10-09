//! Read-only GUI projections of the TUI catalog and independent custom files.
use std::{collections::BTreeMap, fs, path::Path};

use crate::theme::{ColorDef, Theme, ThemeName};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    Light,
    Dark,
    System,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuiTheme {
    pub id: String,
    pub name: String,
    pub mode: ThemeMode,
    #[serde(default)]
    pub tokens: BTreeMap<String, String>,
    #[serde(default)]
    pub terminal: BTreeMap<String, String>,
}

#[derive(Serialize)]
pub struct ThemeCatalog {
    pub tui_theme: String,
    pub themes: Vec<GuiTheme>,
    pub errors: Vec<String>,
    pub directory: String,
}

fn hex(color: &ColorDef) -> String {
    use ratatui::style::Color;
    let (r, g, b) = match color.to_color() {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Black => (0, 0, 0),
        Color::Red => (205, 49, 49),
        Color::Green => (13, 188, 121),
        Color::Yellow => (229, 229, 16),
        Color::Blue => (36, 114, 200),
        Color::Magenta => (188, 63, 188),
        Color::Cyan => (17, 168, 205),
        Color::White => (229, 229, 229),
        Color::DarkGray => (102, 102, 102),
        Color::Gray => (170, 170, 170),
        Color::LightRed => (241, 76, 76),
        Color::LightGreen => (35, 209, 139),
        Color::LightYellow => (245, 245, 67),
        Color::LightBlue => (59, 142, 234),
        Color::LightMagenta => (214, 112, 214),
        Color::LightCyan => (41, 184, 219),
        _ => (255, 255, 255),
    };
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn project(name: ThemeName) -> GuiTheme {
    let id = serde_json::to_value(name)
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned();
    if name == ThemeName::Default {
        return GuiTheme {
            id,
            name: "AMF default (system)".into(),
            mode: ThemeMode::System,
            tokens: BTreeMap::new(),
            terminal: BTreeMap::new(),
        };
    }
    let theme = Theme::load(&name);
    let bg = hex(&theme.background);
    let light = if let ColorDef::Rgb { r, g, b } = theme.background {
        u32::from(r) + u32::from(g) + u32::from(b) > 384
    } else {
        false
    };
    let mut tokens = BTreeMap::new();
    for (key, color) in [
        ("bg", &theme.background),
        ("bg-sidebar", &theme.header_background),
        ("surface", &theme.header_background),
        ("surface-2", &theme.shortcut_background),
        ("surface-3", &theme.selection),
        ("border", &theme.border),
        ("border-strong", &theme.border_focus),
        ("text", &theme.text),
        ("text-muted", &theme.text_muted),
        ("text-faint", &theme.text_muted),
        ("accent", &theme.primary),
        ("accent-hover", &theme.secondary),
        ("accent-fg", &theme.background),
        ("green", &theme.success),
        ("amber", &theme.warning),
        ("red", &theme.danger),
        ("sb-harness-claude", &theme.session_icon_claude),
        ("sb-harness-codex", &theme.session_icon_codex),
        ("sb-harness-opencode", &theme.session_icon_opencode),
        ("sb-accent-pr", &theme.info),
        ("sb-accent-summary", &theme.info),
        ("sb-accent-prompt", &theme.secondary),
        ("syn-comment", &theme.text_muted),
        ("syn-keyword", &theme.secondary),
        ("syn-function", &theme.info),
        ("syn-string", &theme.success),
        ("syn-number", &theme.warning),
        ("syn-type", &theme.info),
        ("syn-property", &theme.primary),
        ("syn-tag", &theme.danger),
        ("syn-accent", &theme.secondary),
        ("syn-builtin", &theme.danger),
        ("syn-parameter", &theme.warning),
        ("syn-punctuation", &theme.text_muted),
    ] {
        tokens.insert(key.into(), hex(color));
    }
    for (key, role) in [
        ("accent-soft", "accent"),
        ("red-soft", "red"),
        ("amber-soft", "amber"),
        ("green-soft", "green"),
    ] {
        tokens.insert(key.into(), format!("{}20", tokens[role]));
    }
    let mut terminal = BTreeMap::new();
    for (key, color) in [
        ("background", &theme.background),
        ("foreground", &theme.text),
        ("cursor", &theme.primary),
        ("cursorAccent", &theme.background),
        ("selectionBackground", &theme.selection),
        ("selectionForeground", &theme.text),
        ("black", &theme.background),
        ("red", &theme.danger),
        ("green", &theme.success),
        ("yellow", &theme.warning),
        ("blue", &theme.info),
        ("magenta", &theme.secondary),
        ("cyan", &theme.primary),
        ("white", &theme.text),
    ] {
        terminal.insert(key.into(), hex(color));
    }
    for (bright, base) in [
        ("brightBlack", "black"),
        ("brightRed", "red"),
        ("brightGreen", "green"),
        ("brightYellow", "yellow"),
        ("brightBlue", "blue"),
        ("brightMagenta", "magenta"),
        ("brightCyan", "cyan"),
        ("brightWhite", "white"),
    ] {
        terminal.insert(bright.into(), terminal[base].clone());
    }
    // ANSI black/white remain usable as backgrounds and foregrounds in a
    // light terminal too; the page background is not necessarily ANSI black.
    terminal.insert(
        "black".into(),
        hex(if light {
            &theme.text
        } else {
            &theme.background
        }),
    );
    terminal.insert(
        "white".into(),
        hex(if light {
            &theme.background
        } else {
            &theme.text
        }),
    );
    terminal.insert("brightBlack".into(), hex(&theme.text_muted));
    terminal.insert(
        "brightWhite".into(),
        hex(if light {
            &theme.header_background
        } else {
            &theme.text
        }),
    );
    tokens.insert("terminal-bg".into(), bg);
    GuiTheme {
        id,
        name: name.display_name().into(),
        mode: if light {
            ThemeMode::Light
        } else {
            ThemeMode::Dark
        },
        tokens,
        terminal,
    }
}

const EXTRA_TOKENS: &str =
    "terminal-bg accent-soft red-soft amber-soft green-soft backdrop syn-plain";
fn valid_color(value: &str) -> bool {
    value.starts_with('#')
        && matches!(value.len(), 7 | 9)
        && value[1..].bytes().all(|b| b.is_ascii_hexdigit())
}
fn parse_custom(contents: &str) -> Result<GuiTheme, String> {
    let theme: GuiTheme = serde_json::from_str(contents).map_err(|e| e.to_string())?;
    if theme.id.is_empty()
        || !theme
            .id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        || theme.name.trim().is_empty()
    {
        return Err(
            "id must use letters, digits, hyphens or underscores; name must be nonempty".into(),
        );
    }
    let known = project(ThemeName::Dracula);
    for (key, value) in &theme.tokens {
        if !known.tokens.contains_key(key) && !EXTRA_TOKENS.split_whitespace().any(|k| k == key) {
            return Err(format!("unknown token: {key}"));
        }
        if !valid_color(value) {
            return Err(format!(
                "invalid colour for {key}: use #RRGGBB or #RRGGBBAA"
            ));
        }
    }
    for (key, value) in &theme.terminal {
        if !known.terminal.contains_key(key) {
            return Err(format!("unknown terminal key: {key}"));
        }
        if !valid_color(value) {
            return Err(format!(
                "invalid terminal colour for {key}: use #RRGGBB or #RRGGBBAA"
            ));
        }
    }
    Ok(theme)
}

fn load_at(directory: &Path, tui_theme: ThemeName) -> ThemeCatalog {
    let mut catalog = ThemeCatalog {
        tui_theme: serde_json::to_value(tui_theme)
            .unwrap()
            .as_str()
            .unwrap()
            .into(),
        themes: Theme::list().into_iter().map(project).collect(),
        errors: vec![],
        directory: directory.display().to_string(),
    };
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return catalog,
        Err(e) => {
            catalog.errors.push(format!("{}: {e}", directory.display()));
            return catalog;
        }
    };
    let mut paths = vec![];
    for entry in entries {
        match entry {
            Ok(entry) => paths.push(entry.path()),
            Err(e) => catalog.errors.push(e.to_string()),
        }
    }
    paths.sort();
    for path in paths
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
    {
        let result = fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|s| parse_custom(&s));
        match result {
            Ok(mut theme) => {
                theme.id = format!("custom:{}", theme.id);
                if catalog.themes.iter().any(|t| t.id == theme.id) {
                    catalog.errors.push(format!(
                        "{}: duplicate theme id {}",
                        path.display(),
                        theme.id
                    ));
                } else {
                    catalog.themes.push(theme);
                }
            }
            Err(e) => catalog.errors.push(format!("{}: {e}", path.display())),
        }
    }
    catalog
}

pub fn load() -> ThemeCatalog {
    load_from_config_dir(&crate::project::amf_config_dir())
}

fn load_from_config_dir(config_dir: &Path) -> ThemeCatalog {
    // Read only the theme field: the normal config loader can migrate/write
    // config.json. GUI appearance must never write the TUI configuration.
    let config_path = config_dir.join("config.json");
    let result = fs::read_to_string(&config_path)
        .map_err(|e| e.to_string())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).map_err(|e| e.to_string()))
        .and_then(|v| {
            serde_json::from_value::<ThemeName>(
                v.get("theme")
                    .cloned()
                    .unwrap_or(serde_json::json!("default")),
            )
            .map_err(|e| e.to_string())
        });
    let mut catalog = load_at(
        &config_dir.join("gui-themes"),
        result.as_ref().copied().unwrap_or_default(),
    );
    if config_path.exists()
        && let Err(error) = result
    {
        catalog.errors.push(format!(
            "{}: {error}; following the default theme",
            config_path.display()
        ));
    }
    catalog
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn following_tui_never_creates_or_rewrites_its_config() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_from_config_dir(dir.path()).tui_theme, "default");
        assert!(!dir.path().join("config.json").exists());
        let contents = r#"{"theme":"dracula", "unknown_setting":true}"#;
        fs::write(dir.path().join("config.json"), contents).unwrap();
        assert_eq!(load_from_config_dir(dir.path()).tui_theme, "dracula");
        assert_eq!(
            fs::read_to_string(dir.path().join("config.json")).unwrap(),
            contents
        );
        fs::write(dir.path().join("config.json"), "broken").unwrap();
        let catalog = load_from_config_dir(dir.path());
        assert_eq!(catalog.tui_theme, "default");
        assert_eq!(catalog.errors.len(), 1);
        assert_eq!(
            fs::read_to_string(dir.path().join("config.json")).unwrap(),
            "broken"
        );
    }
    #[test]
    fn entire_tui_catalog_is_projected() {
        for name in Theme::list() {
            let theme = project(name);
            assert!(!theme.name.is_empty());
            assert!(theme.tokens.values().all(|v| valid_color(v)));
            assert!(theme.terminal.values().all(|v| valid_color(v)));
        }
        let latte = project(ThemeName::CatppuccinLatte);
        assert_ne!(latte.terminal["black"], latte.terminal["background"]);
        assert_ne!(latte.terminal["brightBlack"], latte.terminal["black"]);
        assert!(matches!(
            project(ThemeName::CatppuccinLatte).mode,
            ThemeMode::Light
        ));
        assert!(matches!(project(ThemeName::Dracula).mode, ThemeMode::Dark));
    }
    #[test]
    fn custom_files_fail_independently_and_duplicates_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("a.json"),
            r##"{"id":"ocean","name":"Ocean","mode":"dark","tokens":{"accent":"#12aabb"}}"##,
        )
        .unwrap();
        fs::copy(dir.path().join("a.json"), dir.path().join("b.json")).unwrap();
        fs::write(dir.path().join("broken.json"), "{").unwrap();
        let catalog = load_at(dir.path(), ThemeName::Nord);
        assert_eq!(catalog.tui_theme, "nord");
        assert_eq!(catalog.themes.len(), Theme::list().len() + 1);
        assert_eq!(catalog.errors.len(), 2);
        assert!(catalog.themes.last().unwrap().terminal.is_empty());
    }
    #[test]
    fn invalid_keys_and_colours_are_rejected() {
        for extra in [
            r##", "tokens":{"bogus":"#abcdef"}"##,
            r##", "tokens":{"accent":"red"}"##,
            r##", "terminal":{"oops":"#abcdef"}"##,
            r##", "terminal":{"red":"url(x)"}"##,
            r##", "unknown":true"##,
        ] {
            assert!(
                parse_custom(&format!(r#"{{"id":"x","name":"X","mode":"light"{extra}}}"#)).is_err()
            );
        }
    }
}
