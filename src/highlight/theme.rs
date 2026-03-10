use ratatui::style::{Modifier, Style};

use crate::theme::Theme;

use super::model::SyntaxClass;

pub fn style_for_class(class: SyntaxClass, theme: &Theme) -> Style {
    match class {
        SyntaxClass::Plain => Style::default().fg(theme.text.to_color()),
        SyntaxClass::Attribute => Style::default().fg(theme.warning.to_color()),
        SyntaxClass::Comment => Style::default()
            .fg(theme.text_muted.to_color())
            .add_modifier(Modifier::ITALIC),
        SyntaxClass::Constant | SyntaxClass::ConstantBuiltin => {
            Style::default().fg(theme.warning.to_color())
        }
        SyntaxClass::Constructor => Style::default()
            .fg(theme.info.to_color())
            .add_modifier(Modifier::BOLD),
        SyntaxClass::Embedded => Style::default().fg(theme.primary.to_color()),
        SyntaxClass::Function | SyntaxClass::FunctionBuiltin => {
            Style::default().fg(theme.primary.to_color())
        }
        SyntaxClass::Keyword => Style::default()
            .fg(theme.secondary.to_color())
            .add_modifier(Modifier::BOLD),
        SyntaxClass::Module => Style::default().fg(theme.info.to_color()),
        SyntaxClass::Number => Style::default().fg(theme.warning.to_color()),
        SyntaxClass::Operator => Style::default().fg(theme.text.to_color()),
        SyntaxClass::Property | SyntaxClass::PropertyBuiltin => {
            Style::default().fg(theme.text.to_color())
        }
        SyntaxClass::Punctuation
        | SyntaxClass::PunctuationBracket
        | SyntaxClass::PunctuationDelimiter => Style::default().fg(theme.text_muted.to_color()),
        SyntaxClass::PunctuationSpecial => Style::default().fg(theme.secondary.to_color()),
        SyntaxClass::String | SyntaxClass::StringSpecial => {
            Style::default().fg(theme.success.to_color())
        }
        SyntaxClass::Tag => Style::default()
            .fg(theme.danger.to_color())
            .add_modifier(Modifier::BOLD),
        SyntaxClass::Type | SyntaxClass::TypeBuiltin => Style::default().fg(theme.info.to_color()),
        SyntaxClass::Variable => Style::default().fg(theme.text.to_color()),
        SyntaxClass::VariableBuiltin => Style::default().fg(theme.warning.to_color()),
        SyntaxClass::VariableParameter => Style::default()
            .fg(theme.feature_title.to_color())
            .add_modifier(Modifier::ITALIC),
    }
}
