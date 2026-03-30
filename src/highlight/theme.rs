use ratatui::style::{Color, Modifier, Style};

use crate::theme::Theme;

use super::model::SyntaxClass;

struct SyntaxPalette {
    plain: Color,
    comment: Color,
    keyword: Color,
    function: Color,
    string: Color,
    number: Color,
    r#type: Color,
    property: Color,
    tag: Color,
    accent: Color,
    builtin: Color,
    parameter: Color,
    punctuation: Color,
}

pub fn style_for_class(class: SyntaxClass, theme: &Theme) -> Style {
    let palette = syntax_palette(theme);
    match class {
        SyntaxClass::Plain => Style::default().fg(palette.plain),
        SyntaxClass::Attribute => Style::default().fg(palette.accent),
        SyntaxClass::Comment => Style::default()
            .fg(palette.comment)
            .add_modifier(Modifier::ITALIC),
        SyntaxClass::Constant => Style::default().fg(palette.number),
        SyntaxClass::ConstantBuiltin => Style::default().fg(palette.builtin),
        SyntaxClass::Constructor => Style::default().fg(palette.function),
        SyntaxClass::Embedded => Style::default().fg(palette.accent),
        SyntaxClass::Function | SyntaxClass::FunctionBuiltin => {
            Style::default().fg(palette.function)
        }
        SyntaxClass::Keyword => Style::default()
            .fg(palette.keyword)
            .add_modifier(Modifier::BOLD),
        SyntaxClass::Module => Style::default().fg(palette.accent),
        SyntaxClass::Number => Style::default().fg(palette.number),
        SyntaxClass::Operator => Style::default().fg(palette.accent),
        SyntaxClass::Property => Style::default().fg(palette.property),
        SyntaxClass::PropertyBuiltin => Style::default().fg(palette.builtin),
        SyntaxClass::Punctuation
        | SyntaxClass::PunctuationBracket
        | SyntaxClass::PunctuationDelimiter => Style::default().fg(palette.punctuation),
        SyntaxClass::PunctuationSpecial => Style::default().fg(palette.accent),
        SyntaxClass::String | SyntaxClass::StringSpecial => Style::default().fg(palette.string),
        SyntaxClass::Tag => Style::default()
            .fg(palette.tag)
            .add_modifier(Modifier::BOLD),
        SyntaxClass::Type | SyntaxClass::TypeBuiltin => Style::default().fg(palette.r#type),
        SyntaxClass::Variable => Style::default().fg(palette.plain),
        SyntaxClass::VariableBuiltin => Style::default().fg(palette.builtin),
        SyntaxClass::VariableParameter => Style::default()
            .fg(palette.parameter)
            .add_modifier(Modifier::ITALIC),
    }
}

fn syntax_palette(theme: &Theme) -> SyntaxPalette {
    if is_light_theme(theme) {
        SyntaxPalette {
            plain: Color::Rgb(55, 65, 81),
            comment: Color::Rgb(120, 131, 155),
            keyword: Color::Rgb(123, 77, 255),
            function: Color::Rgb(36, 99, 235),
            string: Color::Rgb(5, 150, 105),
            number: Color::Rgb(217, 119, 6),
            r#type: Color::Rgb(8, 145, 178),
            property: Color::Rgb(14, 116, 144),
            tag: Color::Rgb(220, 38, 38),
            accent: Color::Rgb(168, 85, 247),
            builtin: Color::Rgb(190, 24, 93),
            parameter: Color::Rgb(194, 65, 12),
            punctuation: Color::Rgb(148, 163, 184),
        }
    } else {
        SyntaxPalette {
            plain: Color::Rgb(198, 208, 245),
            comment: Color::Rgb(122, 132, 163),
            keyword: Color::Rgb(187, 154, 247),
            function: Color::Rgb(122, 162, 247),
            string: Color::Rgb(158, 206, 106),
            number: Color::Rgb(255, 158, 100),
            r#type: Color::Rgb(224, 175, 104),
            property: Color::Rgb(247, 118, 142),
            tag: Color::Rgb(125, 207, 255),
            accent: Color::Rgb(179, 142, 244),
            builtin: Color::Rgb(255, 117, 127),
            parameter: Color::Rgb(242, 166, 112),
            punctuation: Color::Rgb(154, 168, 206),
        }
    }
}

fn is_light_theme(theme: &Theme) -> bool {
    luminance(theme.background.to_color()) >= 0.6
}

fn luminance(color: Color) -> f32 {
    let (r, g, b) = color_to_rgb(color);
    (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32) / 255.0
}

fn color_to_rgb(color: Color) -> (u8, u8, u8) {
    match color {
        Color::Black => (0, 0, 0),
        Color::Red => (205, 49, 49),
        Color::Green => (13, 188, 121),
        Color::Yellow => (229, 229, 16),
        Color::Blue => (36, 114, 200),
        Color::Magenta => (188, 63, 188),
        Color::Cyan => (17, 168, 205),
        Color::Gray => (204, 204, 204),
        Color::DarkGray => (118, 118, 118),
        Color::LightRed => (241, 76, 76),
        Color::LightGreen => (35, 209, 139),
        Color::LightYellow => (245, 245, 67),
        Color::LightBlue => (59, 142, 234),
        Color::LightMagenta => (214, 112, 214),
        Color::LightCyan => (41, 184, 219),
        Color::White => (242, 242, 242),
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Indexed(i) => (i, i, i),
        Color::Reset => (48, 52, 70),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_theme_uses_distinct_token_colors() {
        let theme = Theme::default();

        let keyword = style_for_class(SyntaxClass::Keyword, &theme).fg;
        let function = style_for_class(SyntaxClass::Function, &theme).fg;
        let string = style_for_class(SyntaxClass::String, &theme).fg;
        let r#type = style_for_class(SyntaxClass::Type, &theme).fg;
        let plain = style_for_class(SyntaxClass::Plain, &theme).fg;
        let property = style_for_class(SyntaxClass::Property, &theme).fg;
        let parameter = style_for_class(SyntaxClass::VariableParameter, &theme).fg;

        assert_ne!(keyword, function);
        assert_ne!(keyword, string);
        assert_ne!(function, r#type);
        assert_ne!(string, plain);
        assert_ne!(property, plain);
        assert_ne!(parameter, plain);
    }
}
