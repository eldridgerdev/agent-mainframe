/// A small, curated set of Nerd Font icons useful for custom sessions.
///
/// AMF stores the glyph itself in config so rendering does not depend on a
/// shell helper or Nerd Fonts' CSS-style icon names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CustomSessionIcon {
    pub label: &'static str,
    pub nerd_font_name: &'static str,
    pub glyph: &'static str,
}

pub const CUSTOM_SESSION_ICONS: &[CustomSessionIcon] = &[
    CustomSessionIcon {
        label: "Server",
        nerd_font_name: "nf-md-server",
        glyph: "\u{f048b}",
    },
    CustomSessionIcon {
        label: "Database",
        nerd_font_name: "nf-md-database",
        glyph: "\u{f01bc}",
    },
    CustomSessionIcon {
        label: "Terminal",
        nerd_font_name: "nf-md-console",
        glyph: "\u{f018d}",
    },
    CustomSessionIcon {
        label: "Docker",
        nerd_font_name: "nf-md-docker",
        glyph: "\u{f0868}",
    },
    CustomSessionIcon {
        label: "Web",
        nerd_font_name: "nf-md-web",
        glyph: "\u{f059f}",
    },
    CustomSessionIcon {
        label: "API",
        nerd_font_name: "nf-md-api",
        glyph: "\u{f109b}",
    },
    CustomSessionIcon {
        label: "Code",
        nerd_font_name: "nf-md-code_braces",
        glyph: "\u{f0169}",
    },
    CustomSessionIcon {
        label: "Dashboard",
        nerd_font_name: "nf-md-monitor_dashboard",
        glyph: "\u{f0a07}",
    },
    CustomSessionIcon {
        label: "Cloud",
        nerd_font_name: "nf-md-cloud",
        glyph: "\u{f015f}",
    },
    CustomSessionIcon {
        label: "Rocket",
        nerd_font_name: "nf-md-rocket_launch",
        glyph: "\u{f14de}",
    },
    CustomSessionIcon {
        label: "Test",
        nerd_font_name: "nf-md-test_tube",
        glyph: "\u{f0668}",
    },
    CustomSessionIcon {
        label: "Bug",
        nerd_font_name: "nf-md-bug",
        glyph: "\u{f00e4}",
    },
    CustomSessionIcon {
        label: "Settings",
        nerd_font_name: "nf-md-cog",
        glyph: "\u{f0493}",
    },
    CustomSessionIcon {
        label: "Tools",
        nerd_font_name: "nf-md-wrench",
        glyph: "\u{f05b7}",
    },
];

pub fn custom_session_icon_index(value: &str) -> Option<usize> {
    let value = value.trim();
    CUSTOM_SESSION_ICONS
        .iter()
        .position(|icon| value == icon.glyph || value.eq_ignore_ascii_case(icon.nerd_font_name))
}

/// Resolve names accepted by older wizard hints while leaving custom glyphs
/// untouched.
pub fn resolve_custom_session_icon(value: &str) -> &str {
    custom_session_icon_index(value)
        .map(|index| CUSTOM_SESSION_ICONS[index].glyph)
        .unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_the_legacy_wizard_example_to_a_glyph() {
        assert_eq!(resolve_custom_session_icon("nf-md-server"), "\u{f048b}");
    }

    #[test]
    fn preserves_custom_glyphs() {
        assert_eq!(resolve_custom_session_icon("🚀"), "🚀");
    }
}
