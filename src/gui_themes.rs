//! Read-only GUI projections of the TUI catalog and independent custom files.
use std::{collections::BTreeMap, fs, path::Path};

use crate::theme::{ColorDef, Theme, ThemeName};
use serde::{Deserialize, Serialize};

mod color;

use color::{BLACK, Rgb, WHITE, best_on, contrast, ensure_contrast, ensure_contrast_by, mix, over};

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

fn rgb(color: &ColorDef) -> Rgb {
    Rgb::parse(&hex(color)).expect("hex() emits #rrggbb").0
}

/// WCAG AA for body text; every text role is floored here on every
/// background it is drawn on.
const TEXT_MIN: f64 = 4.5;
/// `text`, `text-muted` and `text-faint` (which gets [`TEXT_MIN`]) are
/// floored a step apart so they stay distinguishable from one another even
/// where a palette's own text is barely above AA on its raised surfaces
/// (catppuccin-latte `text` was 5.4:1 on `surface-3`).
const TEXT_STRONG_MIN: f64 = 6.0;
const MUTED_MIN: f64 = 5.0;
/// WCAG 1.4.11 non-text contrast: control boundaries. Also used for
/// disabled text, which WCAG exempts but which should still be legible.
const NON_TEXT_MIN: f64 = 3.0;
/// How far `surface-2` (footers, badges, selected rows) and `surface-3`
/// (chips, hover) step from `surface`. 1.10:1 is about ΔL* 4 on both light
/// and dark surfaces, the smallest step that reads as a distinct fill; these
/// targets sit a little above the floors the catalog test asserts (1.10 and
/// 1.20).
const SURFACE_2_TARGET: f64 = 1.12;
const SURFACE_3_TARGET: f64 = 1.25;
/// Below this the page and its cards/sidebar would read as one colour.
const PANEL_SEPARATION: f64 = 1.04;
/// The `20` alpha suffix of every `*-soft` tint.
const SOFT_ALPHA: f64 = 32.0 / 255.0;
/// The diff row tints in `gui/src/styles.css` (`.diff-added`/`.diff-removed`).
const DIFF_ADDED: (Rgb, f64) = (Rgb(46.0, 160.0, 67.0), 0.16);
const DIFF_REMOVED: (Rgb, f64) = (Rgb(248.0, 81.0, 73.0), 0.16);

/// `base` moved toward `toward` from mix fraction `start`, in 1% steps, until
/// it is at least `min_step` away from `base`. Returns the colour and the
/// fraction used.
fn step_from(base: Rgb, toward: Rgb, start: f64, min_step: f64) -> (Rgb, f64) {
    let mut t = start;
    loop {
        let candidate = mix(base, toward, t).rounded();
        if contrast(base, candidate) >= min_step || t >= 1.0 {
            return (candidate, t);
        }
        t = (t + 0.01).min(1.0);
    }
}

/// Derives the GUI's roles from a TUI palette instead of copying it. A TUI
/// palette is tuned for a terminal, where `shortcut_background` should be
/// loud and `text_muted` should recede; the GUI needs neutral raised
/// surfaces and secondary text that is still readable. Hues are kept: every
/// adjustment only moves a colour toward black or white (or toward `text`
/// for surfaces), the smallest amount that meets the floor.
fn project_tokens(theme: &Theme) -> BTreeMap<String, String> {
    let bg = rgb(&theme.background);
    let text_source = rgb(&theme.text);
    let mut panel = rgb(&theme.header_background);
    if contrast(panel, bg) < PANEL_SEPARATION {
        panel = step_from(bg, text_source, 0.02, PANEL_SEPARATION).0;
    }
    let (sidebar, surface) = (panel, panel);
    // Neutral raised surfaces: a little of the text colour mixed into the
    // card surface, never the key-hint or selection colours.
    // Sized by contrast step rather than a fixed mix: a fixed 6%/12% lands
    // near 1.10/1.22 on light surfaces but 1.15/1.35 on dark ones, and the
    // brighter dark `surface-3` would push `text-muted` up toward `text`.
    let (surface2, t2) = step_from(surface, text_source, 0.03, SURFACE_2_TARGET);
    let (surface3, _) = step_from(surface, text_source, t2 + 0.03, SURFACE_3_TARGET);
    // Accent first: the selected nav item and active feature row are its
    // soft tint, and every text role is drawn on those rows too.
    let chrome_bgs = [bg, sidebar, surface, surface2];
    let with_tint_on = |c: Rgb, base: &[Rgb]| {
        ensure_contrast_by(c, TEXT_MIN, |c| {
            let mut bgs = base.to_vec();
            bgs.extend([over(c, SOFT_ALPHA, surface), over(c, SOFT_ALPHA, sidebar)]);
            bgs
        })
    };
    let accent = with_tint_on(rgb(&theme.primary), &chrome_bgs);
    let active_rows = [
        over(accent, SOFT_ALPHA, sidebar),
        over(accent, SOFT_ALPHA, surface),
    ];
    let surfaces = [
        bg,
        sidebar,
        surface,
        surface2,
        surface3,
        active_rows[0],
        active_rows[1],
    ];

    let text = ensure_contrast(text_source, &surfaces, TEXT_STRONG_MIN);
    let muted_source = rgb(&theme.text_muted);
    let muted = ensure_contrast(muted_source, &surfaces, MUTED_MIN);
    let faint = ensure_contrast(mix(muted_source, bg, 0.3), &surfaces, TEXT_MIN);

    // Status and accent colours are drawn as text on the chrome and on their
    // own soft tint, so the tint is computed from the colour being floored.
    let text_bgs = [
        bg,
        sidebar,
        surface,
        surface2,
        active_rows[0],
        active_rows[1],
    ];
    let on_chrome = |c: Rgb| ensure_contrast(c, &text_bgs, TEXT_MIN);
    let with_tint = |c: Rgb| with_tint_on(c, &text_bgs);
    let green = with_tint(rgb(&theme.success));
    let amber = with_tint(rgb(&theme.warning));
    let red = with_tint(rgb(&theme.danger));
    // Merged-PR chips use the TUI's own merged-PR colour.
    let purple = with_tint(rgb(&theme.pr_merged));
    let on_fill = |fill: Rgb| best_on(fill, &[text, bg, BLACK, WHITE]);
    let accent_fg = on_fill(accent);
    // The primary button's hover fill: still text-safe, and still carries
    // `accent-fg`.
    let accent_hover = ensure_contrast_by(rgb(&theme.secondary), TEXT_MIN, |c| {
        let mut bgs = text_bgs.to_vec();
        bgs.extend([over(c, SOFT_ALPHA, surface), accent_fg]);
        bgs
    });

    let boundary_bgs = [bg, sidebar, surface, surface2];
    let border_strong = ensure_contrast(rgb(&theme.border_focus), &boundary_bgs, NON_TEXT_MIN);
    let border_control = ensure_contrast(mix(surface, text, 0.35), &boundary_bgs, NON_TEXT_MIN);
    let disabled_bg = surface3;
    let disabled_fg = ensure_contrast(
        mix(faint, disabled_bg, 0.5),
        &[disabled_bg, bg, sidebar, surface, surface2],
        NON_TEXT_MIN,
    );

    // Code is drawn on the card surface, the selected line, the diff row
    // tints and the review selection (see `gui/tests/syntaxContrast.test.ts`).
    let code_bgs = [
        surface,
        surface2,
        over(DIFF_ADDED.0, DIFF_ADDED.1, surface),
        over(DIFF_REMOVED.0, DIFF_REMOVED.1, surface),
        over(accent, SOFT_ALPHA, surface),
    ];
    let code = |c: &ColorDef| ensure_contrast(rgb(c), &code_bgs, TEXT_MIN);

    let mut tokens = BTreeMap::new();
    for (key, color) in [
        ("bg", bg),
        ("bg-sidebar", sidebar),
        ("surface", surface),
        ("surface-2", surface2),
        ("surface-3", surface3),
        ("border", rgb(&theme.border)),
        ("border-strong", border_strong),
        ("border-control", border_control),
        ("text", text),
        ("text-muted", muted),
        ("text-faint", faint),
        ("accent", accent),
        ("accent-hover", accent_hover),
        ("accent-fg", accent_fg),
        ("green", green),
        ("amber", amber),
        ("red", red),
        ("danger-fg", on_fill(red)),
        ("warning-fg", on_fill(amber)),
        ("success-fg", on_fill(green)),
        ("disabled-bg", disabled_bg),
        ("disabled-fg", disabled_fg),
        (
            "sb-harness-claude",
            on_chrome(rgb(&theme.session_icon_claude)),
        ),
        (
            "sb-harness-codex",
            on_chrome(rgb(&theme.session_icon_codex)),
        ),
        (
            "sb-harness-opencode",
            on_chrome(rgb(&theme.session_icon_opencode)),
        ),
        ("sb-harness-pi", accent),
        // `--kind-claude/codex/opencode/pi` alias the `sb-harness-*` tokens in
        // CSS; only the session kinds the sidebar has no token for are new.
        ("kind-nvim", on_chrome(rgb(&theme.session_icon_nvim))),
        ("kind-vscode", on_chrome(rgb(&theme.session_icon_vscode))),
        ("purple", purple),
        ("sb-accent-pr", on_chrome(rgb(&theme.info))),
        ("sb-accent-summary", on_chrome(rgb(&theme.info))),
        ("sb-accent-prompt", on_chrome(rgb(&theme.secondary))),
        ("sb-accent-status", amber),
        ("sb-accent-usage", green),
        ("sb-accent-plan", amber),
        ("sb-accent-issue", faint),
        ("sb-accent-work", accent),
        ("sb-accent-todos", green),
        ("sb-tone-ready", green),
        ("sb-tone-waiting", amber),
        ("sb-tone-busy", accent),
        ("sb-tone-pr-working", amber),
        ("sb-tone-muted", faint),
        ("sb-tone-detail", muted),
        ("sb-tone-todo", green),
        ("sb-tone-stopped", faint),
        ("sb-usage-low", green),
        ("sb-usage-medium", amber),
        ("sb-usage-high", red),
        ("sb-ctx-normal", green),
        ("sb-ctx-warning", amber),
        ("sb-ctx-critical", red),
        ("sb-track", surface3),
        ("syn-comment", code(&theme.text_muted)),
        ("syn-keyword", code(&theme.secondary)),
        ("syn-function", code(&theme.info)),
        ("syn-string", code(&theme.success)),
        ("syn-number", code(&theme.warning)),
        ("syn-type", code(&theme.info)),
        ("syn-property", code(&theme.primary)),
        ("syn-tag", code(&theme.danger)),
        ("syn-accent", code(&theme.secondary)),
        ("syn-builtin", code(&theme.danger)),
        ("syn-parameter", code(&theme.warning)),
        ("syn-punctuation", code(&theme.text_muted)),
    ] {
        tokens.insert(key.into(), color.hex());
    }
    for (key, role) in [
        ("accent-soft", "accent"),
        ("red-soft", "red"),
        ("amber-soft", "amber"),
        ("green-soft", "green"),
        ("purple-soft", "purple"),
    ] {
        tokens.insert(key.into(), format!("{}20", tokens[role]));
    }
    tokens
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
    let mut tokens = project_tokens(&theme);
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

/// Every colour token a custom file may set: the chrome palette plus each
/// `syn-*` and `sb-*` colour role declared in `gui/src/syntax.css` and
/// `gui/src/sessionSidebar.css` (not widths, nor the per-panel `sb-accent`).
/// Catalog themes assign only a subset, so this cannot be derived from them.
const CUSTOM_TOKENS: &[&str] = &[
    "bg",
    "bg-sidebar",
    "surface",
    "surface-2",
    "surface-3",
    "border",
    "border-strong",
    "border-control",
    "text",
    "text-muted",
    "text-faint",
    "accent",
    "accent-hover",
    "accent-fg",
    "accent-soft",
    "danger-fg",
    "warning-fg",
    "success-fg",
    "disabled-fg",
    "disabled-bg",
    "green",
    "amber",
    "red",
    "purple",
    "purple-soft",
    "kind-claude",
    "kind-codex",
    "kind-opencode",
    "kind-pi",
    "kind-nvim",
    "kind-vscode",
    "green-soft",
    "amber-soft",
    "red-soft",
    "backdrop",
    "terminal-bg",
    "syn-plain",
    "syn-comment",
    "syn-keyword",
    "syn-function",
    "syn-string",
    "syn-number",
    "syn-type",
    "syn-property",
    "syn-tag",
    "syn-accent",
    "syn-builtin",
    "syn-parameter",
    "syn-punctuation",
    "sb-harness-claude",
    "sb-harness-codex",
    "sb-harness-opencode",
    "sb-harness-pi",
    "sb-accent-status",
    "sb-accent-usage",
    "sb-accent-plan",
    "sb-accent-issue",
    "sb-accent-pr",
    "sb-accent-work",
    "sb-accent-summary",
    "sb-accent-prompt",
    "sb-accent-todos",
    "sb-tone-ready",
    "sb-tone-waiting",
    "sb-tone-busy",
    "sb-tone-pr-working",
    "sb-tone-muted",
    "sb-tone-detail",
    "sb-tone-todo",
    "sb-tone-stopped",
    "sb-usage-low",
    "sb-usage-medium",
    "sb-usage-high",
    "sb-track",
    "sb-ctx-normal",
    "sb-ctx-warning",
    "sb-ctx-critical",
];
fn valid_color(value: &str) -> bool {
    value.starts_with('#')
        && matches!(value.len(), 7 | 9)
        && value[1..].bytes().all(|b| b.is_ascii_hexdigit())
}
/// Fills in the derived roles a custom file leaves out, from the colours it
/// does set, the same way [`project_tokens`] derives them. A custom theme is
/// applied over the built-in light or dark palette, so without this a file
/// that sets a deep `red` but no `danger-fg` would get the built-in palette's
/// `danger-fg`, chosen for the built-in `red`. Roles whose inputs the file
/// does not set are left to the built-in palette, whose own pairs already
/// meet the floor. Explicit tokens are never changed.
fn fill_derived_tokens(tokens: &mut BTreeMap<String, String>) {
    let get = |tokens: &BTreeMap<String, String>, key: &str| match tokens
        .get(key)
        .and_then(|v| Rgb::parse(v))
    {
        Some((rgb, 1.0)) => Some(rgb),
        _ => None,
    };
    let mut derived: Vec<(&str, Rgb)> = vec![];
    let on_fill_candidates: Vec<Rgb> = ["text", "bg"]
        .into_iter()
        .filter_map(|key| get(tokens, key))
        .chain([BLACK, WHITE])
        .collect();
    for (fg, fill) in [
        ("accent-fg", "accent"),
        ("danger-fg", "red"),
        ("warning-fg", "amber"),
        ("success-fg", "green"),
    ] {
        if !tokens.contains_key(fg)
            && let Some(fill) = get(tokens, fill)
        {
            derived.push((fg, best_on(fill, &on_fill_candidates)));
        }
    }
    let boundary_bgs: Vec<Rgb> = ["bg", "bg-sidebar", "surface", "surface-2"]
        .into_iter()
        .filter_map(|key| get(tokens, key))
        .collect();
    if !tokens.contains_key("border-control")
        && let (Some(surface), Some(text)) = (get(tokens, "surface"), get(tokens, "text"))
    {
        derived.push((
            "border-control",
            ensure_contrast(mix(surface, text, 0.35), &boundary_bgs, NON_TEXT_MIN),
        ));
    }
    let disabled_bg = get(tokens, "disabled-bg").or_else(|| get(tokens, "surface-3"));
    if !tokens.contains_key("disabled-bg")
        && let Some(surface3) = disabled_bg
    {
        derived.push(("disabled-bg", surface3));
    }
    if !tokens.contains_key("disabled-fg")
        && let (Some(faint), Some(disabled_bg)) = (get(tokens, "text-faint"), disabled_bg)
    {
        let mut bgs = boundary_bgs.clone();
        bgs.push(disabled_bg);
        derived.push((
            "disabled-fg",
            ensure_contrast(mix(faint, disabled_bg, 0.5), &bgs, NON_TEXT_MIN),
        ));
    }
    for (key, color) in derived {
        tokens.insert(key.into(), color.hex());
    }
}

fn parse_custom(contents: &str) -> Result<GuiTheme, String> {
    let mut theme: GuiTheme = serde_json::from_str(contents).map_err(|e| e.to_string())?;
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
        if !CUSTOM_TOKENS.contains(&key.as_str()) {
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
    fill_derived_tokens(&mut theme.tokens);
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
    fn every_declared_colour_role_is_a_supported_custom_token() {
        for theme in Theme::list().into_iter().map(project) {
            for key in theme.tokens.keys() {
                assert!(CUSTOM_TOKENS.contains(&key.as_str()), "{key}");
            }
        }
        // The first `:root` block of each file declares its colour roles.
        let gui = Path::new(env!("CARGO_MANIFEST_DIR")).join("gui/src");
        for file in ["syntax.css", "sessionSidebar.css"] {
            let css = fs::read_to_string(gui.join(file)).unwrap();
            let root = css.split(":root {").nth(1).unwrap();
            let root = &root[..root.find('}').unwrap()];
            for line in root.lines() {
                let Some(name) = line.trim().strip_prefix("--") else {
                    continue;
                };
                let name = &name[..name.find(':').unwrap()];
                if !name.ends_with("width") {
                    let json = format!(
                        r##"{{"id":"x","name":"X","mode":"dark","tokens":{{"{name}":"#abcdef"}}}}"##
                    );
                    assert!(parse_custom(&json).is_ok(), "{file}: {name}");
                }
            }
        }
    }
    #[test]
    fn custom_files_get_derived_roles_for_the_colours_they_set() {
        let theme = parse_custom(
            r##"{"id":"x","name":"X","mode":"dark","tokens":{
                "bg":"#101418","surface":"#1a2028","surface-3":"#2a323c",
                "text":"#e8eef4","text-faint":"#98a4b0","accent-fg":"#ff00ff",
                "red":"#7a1010","amber":"#ffd17d"}}"##,
        )
        .unwrap();
        let t = &theme.tokens;
        let color = |key: &str| Rgb::parse(&t[key]).unwrap().0;
        // Explicit tokens are kept even when a derived one would differ.
        assert_eq!(t["accent-fg"], "#ff00ff");
        // A deep red gets light text; a light amber gets dark text.
        assert_eq!(t["danger-fg"], "#ffffff");
        assert!(contrast(color("warning-fg"), color("amber")) >= TEXT_MIN);
        assert!(contrast(color("warning-fg"), color("amber")) > 10.0);
        // No `green` in the file: `success-fg` stays with the built-in pair.
        assert!(!t.contains_key("success-fg"));
        assert_eq!(t["disabled-bg"], "#2a323c");
        for bg in ["disabled-bg", "surface", "bg"] {
            assert!(
                contrast(color("disabled-fg"), color(bg)) >= NON_TEXT_MIN,
                "{bg}"
            );
        }
        for bg in ["surface", "bg"] {
            assert!(
                contrast(color("border-control"), color(bg)) >= NON_TEXT_MIN,
                "{bg}"
            );
        }
        // A file that sets none of the inputs gets nothing derived.
        let bare = parse_custom(r#"{"id":"y","name":"Y","mode":"light"}"#).unwrap();
        assert!(bare.tokens.is_empty());
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

    /// The GUI's readability floor for every catalog theme: WCAG AA 4.5:1 for
    /// text, 3:1 for control boundaries and disabled text (WCAG exempts
    /// disabled controls; 3:1 keeps them legible), on every background each
    /// role is drawn on. Mirrors `gui/tests/syntaxContrast.test.ts` for code.
    #[test]
    fn every_catalog_theme_meets_the_gui_contrast_floor() {
        let mut failures = vec![];
        for name in Theme::list() {
            if name == ThemeName::Default {
                continue;
            }
            let theme = project(name);
            let id = &theme.id;
            // A missing or malformed token is reported, not a panic, so one
            // theme's gap doesn't hide every other theme's failures.
            let missing = std::cell::RefCell::new(vec![]);
            let parse = |key: &str| {
                theme
                    .tokens
                    .get(key)
                    .and_then(|v| Rgb::parse(v))
                    .or_else(|| {
                        missing.borrow_mut().push(key.to_owned());
                        None
                    })
            };
            let tok = |key: &str| -> Rgb {
                match parse(key) {
                    Some((rgb, 1.0)) => rgb,
                    Some(_) => {
                        missing.borrow_mut().push(key.to_owned());
                        Rgb(255.0, 0.0, 255.0)
                    }
                    None => Rgb(255.0, 0.0, 255.0),
                }
            };
            let soft = |key: &str, base: Rgb| -> Rgb {
                match parse(key) {
                    Some((rgb, alpha)) => over(rgb, alpha, base),
                    None => Rgb(255.0, 0.0, 255.0),
                }
            };
            let surfaces = [
                ("bg", tok("bg")),
                ("bg-sidebar", tok("bg-sidebar")),
                ("surface", tok("surface")),
                ("surface-2", tok("surface-2")),
                ("surface-3", tok("surface-3")),
            ];
            let [
                (_, bg),
                (_, sidebar),
                (_, surface),
                (_, surface2),
                (_, surface3),
            ] = surfaces;
            // 1.10:1 is about the smallest step that reads as a distinct
            // fill on both light and dark surfaces (≈ ΔL* 3–4 on a light
            // one); surface-3 (chips, hover) must step further than
            // surface-2 (footers, selected rows) so the two stay ordered.
            let (step2, step3) = (contrast(surface, surface2), contrast(surface, surface3));
            if step2 < 1.10 || step3 < 1.20 || step3 <= step2 {
                failures.push(format!(
                    "{id}: surface steps surface-2 {step2:.3} (≥1.10), surface-3 {step3:.3} (≥1.20)"
                ));
            }
            let mut check = |fg_name: &str, fg: Rgb, bg_name: &str, bg: Rgb, min: f64| {
                let ratio = contrast(fg, bg);
                if ratio < min {
                    failures.push(format!("{id}: {fg_name} on {bg_name} {ratio:.2} < {min}"));
                }
            };
            // The selected nav item and active feature row are the accent
            // tint; their counts, chips and ages are drawn on it.
            let active = [
                ("active/bg-sidebar", soft("accent-soft", sidebar)),
                ("active/surface", soft("accent-soft", surface)),
            ];
            for key in ["text", "text-muted", "text-faint"] {
                for (bg_name, bg) in surfaces.into_iter().chain(active) {
                    check(key, tok(key), bg_name, bg, 4.5);
                }
            }
            for key in ["kind-nvim", "kind-vscode"] {
                for (bg_name, bg) in surfaces.into_iter().take(4).chain(active) {
                    check(key, tok(key), bg_name, bg, 4.5);
                }
            }
            for key in ["accent", "green", "amber", "red", "purple"] {
                let fg = tok(key);
                let tint = format!("{key}-soft");
                check(key, fg, "surface", surface, 4.5);
                check(key, fg, "bg-sidebar", sidebar, 4.5);
                if key != "accent" {
                    for (bg_name, bg) in active {
                        check(key, fg, bg_name, bg, 4.5);
                    }
                }
                check(key, fg, "soft/surface", soft(&tint, surface), 4.5);
                check(key, fg, "soft/bg-sidebar", soft(&tint, sidebar), 4.5);
            }
            for (fg, fill) in [
                ("accent-fg", "accent"),
                ("accent-fg", "accent-hover"),
                ("danger-fg", "red"),
                ("warning-fg", "amber"),
                ("success-fg", "green"),
            ] {
                check(fg, tok(fg), fill, tok(fill), 4.5);
            }
            for key in ["border-control", "border-strong"] {
                check(key, tok(key), "surface", surface, 3.0);
                check(key, tok(key), "bg", bg, 3.0);
            }
            check(
                "border-control",
                tok("border-control"),
                "surface-2",
                surface2,
                3.0,
            );
            let disabled_bg = tok("disabled-bg");
            for (bg_name, bg) in [("disabled-bg", disabled_bg), ("surface", surface)] {
                check("disabled-fg", tok("disabled-fg"), bg_name, bg, 3.0);
            }
            let code = [
                ("surface", surface),
                ("surface-2", surface2),
                ("added", over(DIFF_ADDED.0, DIFF_ADDED.1, surface)),
                ("removed", over(DIFF_REMOVED.0, DIFF_REMOVED.1, surface)),
                ("selected", soft("accent-soft", surface)),
            ];
            for role in [
                "comment",
                "keyword",
                "function",
                "string",
                "number",
                "type",
                "property",
                "tag",
                "accent",
                "builtin",
                "parameter",
                "punctuation",
            ] {
                let key = format!("syn-{role}");
                let fg = tok(&key);
                for (bg_name, bg) in code {
                    check(&key, fg, bg_name, bg, 4.5);
                }
            }
            let mut missing = missing.into_inner();
            missing.sort();
            missing.dedup();
            for key in missing {
                failures.push(format!("{id}: missing token {key}"));
            }
        }
        assert!(
            failures.is_empty(),
            "{} failures:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}
