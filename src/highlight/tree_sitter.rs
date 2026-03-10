use std::sync::OnceLock;

use anyhow::{Context, Result};
use tree_sitter_highlight::{Highlight, HighlightConfiguration, HighlightEvent, Highlighter};

use super::detect::HighlightLanguage;
use super::model::{HighlightedLine, HighlightedSpan, HighlightedText, SyntaxClass};

const HIGHLIGHT_NAMES: [&str; 27] = [
    "attribute",
    "comment",
    "constant",
    "constant.builtin",
    "constructor",
    "embedded",
    "function",
    "function.builtin",
    "keyword",
    "module",
    "number",
    "operator",
    "property",
    "property.builtin",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation.special",
    "string",
    "string.special",
    "tag",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
    "none",
];

static REGISTRY: OnceLock<Result<Registry, String>> = OnceLock::new();

struct Registry {
    bash: HighlightConfiguration,
    json: HighlightConfiguration,
    rust: HighlightConfiguration,
    toml: HighlightConfiguration,
    yaml: HighlightConfiguration,
}

pub fn highlight_source(language: HighlightLanguage, source: &str) -> Result<HighlightedText> {
    let registry = registry()?;
    let config = registry.config(language);
    let mut highlighter = Highlighter::new();
    let mut lines = vec![HighlightedLine::default()];
    let mut classes = vec![SyntaxClass::Plain];

    let highlights = highlighter
        .highlight(config, source.as_bytes(), None, |_| None)
        .with_context(|| format!("failed to start tree-sitter highlighter for {language:?}"))?;

    for event in highlights {
        match event
            .with_context(|| format!("tree-sitter highlight stream failed for {language:?}"))?
        {
            HighlightEvent::Source { start, end } => {
                let text = &source[start..end];
                push_text(
                    &mut lines,
                    *classes.last().unwrap_or(&SyntaxClass::Plain),
                    text,
                );
            }
            HighlightEvent::HighlightStart(highlight) => {
                classes.push(class_for_highlight(highlight));
            }
            HighlightEvent::HighlightEnd => {
                if classes.len() > 1 {
                    classes.pop();
                }
            }
        }
    }

    if lines.is_empty() {
        lines.push(HighlightedLine::default());
    }

    Ok(HighlightedText {
        language_name: Some(language.display_name().to_string()),
        lines,
    })
}

fn registry() -> Result<&'static Registry> {
    REGISTRY
        .get_or_init(|| Registry::build().map_err(|err| format!("{err:#}")))
        .as_ref()
        .map_err(|err| anyhow::anyhow!(err.clone()))
}

impl Registry {
    fn build() -> Result<Self> {
        Ok(Self {
            bash: build_config(
                HighlightLanguage::Bash,
                tree_sitter_bash::LANGUAGE.into(),
                tree_sitter_bash::HIGHLIGHT_QUERY,
                "",
                "",
            )?,
            json: build_config(
                HighlightLanguage::Json,
                tree_sitter_json::LANGUAGE.into(),
                tree_sitter_json::HIGHLIGHTS_QUERY,
                "",
                "",
            )?,
            rust: build_config(
                HighlightLanguage::Rust,
                tree_sitter_rust::LANGUAGE.into(),
                tree_sitter_rust::HIGHLIGHTS_QUERY,
                tree_sitter_rust::INJECTIONS_QUERY,
                "",
            )?,
            toml: build_config(
                HighlightLanguage::Toml,
                tree_sitter_toml_ng::LANGUAGE.into(),
                tree_sitter_toml_ng::HIGHLIGHTS_QUERY,
                "",
                "",
            )?,
            yaml: build_config(
                HighlightLanguage::Yaml,
                tree_sitter_yaml::LANGUAGE.into(),
                tree_sitter_yaml::HIGHLIGHTS_QUERY,
                "",
                "",
            )?,
        })
    }

    fn config(&self, language: HighlightLanguage) -> &HighlightConfiguration {
        match language {
            HighlightLanguage::Bash => &self.bash,
            HighlightLanguage::Json => &self.json,
            HighlightLanguage::Rust => &self.rust,
            HighlightLanguage::Toml => &self.toml,
            HighlightLanguage::Yaml => &self.yaml,
        }
    }
}

fn build_config(
    language: HighlightLanguage,
    tree_sitter_language: tree_sitter::Language,
    highlights_query: &str,
    injections_query: &str,
    locals_query: &str,
) -> Result<HighlightConfiguration> {
    let mut config = HighlightConfiguration::new(
        tree_sitter_language,
        language.display_name(),
        highlights_query,
        injections_query,
        locals_query,
    )
    .with_context(|| format!("failed to build tree-sitter config for {language:?}"))?;
    config.configure(&HIGHLIGHT_NAMES);
    Ok(config)
}

fn class_for_highlight(highlight: Highlight) -> SyntaxClass {
    match highlight.0 {
        0 => SyntaxClass::Attribute,
        1 => SyntaxClass::Comment,
        2 => SyntaxClass::Constant,
        3 => SyntaxClass::ConstantBuiltin,
        4 => SyntaxClass::Constructor,
        5 => SyntaxClass::Embedded,
        6 => SyntaxClass::Function,
        7 => SyntaxClass::FunctionBuiltin,
        8 => SyntaxClass::Keyword,
        9 => SyntaxClass::Module,
        10 => SyntaxClass::Number,
        11 => SyntaxClass::Operator,
        12 => SyntaxClass::Property,
        13 => SyntaxClass::PropertyBuiltin,
        14 => SyntaxClass::Punctuation,
        15 => SyntaxClass::PunctuationBracket,
        16 => SyntaxClass::PunctuationDelimiter,
        17 => SyntaxClass::PunctuationSpecial,
        18 => SyntaxClass::String,
        19 => SyntaxClass::StringSpecial,
        20 => SyntaxClass::Tag,
        21 => SyntaxClass::Type,
        22 => SyntaxClass::TypeBuiltin,
        23 => SyntaxClass::Variable,
        24 => SyntaxClass::VariableBuiltin,
        25 => SyntaxClass::VariableParameter,
        _ => SyntaxClass::Plain,
    }
}

fn push_text(lines: &mut Vec<HighlightedLine>, class: SyntaxClass, text: &str) {
    let mut remaining = text;
    loop {
        if let Some(newline_index) = remaining.find('\n') {
            let (line_part, rest) = remaining.split_at(newline_index);
            if !line_part.is_empty() {
                push_span(current_line(lines), class, line_part);
            }
            lines.push(HighlightedLine::default());
            remaining = &rest[1..];
        } else {
            if !remaining.is_empty() {
                push_span(current_line(lines), class, remaining);
            }
            break;
        }
    }
}

fn current_line(lines: &mut Vec<HighlightedLine>) -> &mut HighlightedLine {
    if lines.is_empty() {
        lines.push(HighlightedLine::default());
    }
    lines.last_mut().expect("highlight lines should exist")
}

fn push_span(line: &mut HighlightedLine, class: SyntaxClass, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = line.spans.last_mut()
        && last.class == class
    {
        last.text.push_str(text);
        return;
    }
    line.spans.push(HighlightedSpan {
        text: text.to_string(),
        class,
    });
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn rust_highlighting_classifies_keywords() {
        let highlighted = crate::highlight::service::highlight_source(
            crate::highlight::model::HighlightRequest {
                path: Some(Path::new("src/main.rs")),
                language_hint: None,
                source: "fn main() {}",
            },
        );
        assert!(
            highlighted.lines[0]
                .spans
                .iter()
                .any(|span| span.class == SyntaxClass::Keyword && span.text.contains("fn"))
        );
    }

    #[test]
    fn unknown_language_falls_back_to_plain_text() {
        let highlighted = crate::highlight::service::highlight_source(
            crate::highlight::model::HighlightRequest {
                path: Some(Path::new("notes.txt")),
                language_hint: None,
                source: "plain text",
            },
        );
        assert_eq!(highlighted.lines.len(), 1);
        assert_eq!(highlighted.lines[0].spans[0].class, SyntaxClass::Plain);
    }

    #[test]
    fn rust_multiline_raw_string_keeps_inner_line_as_string() {
        let highlighted = crate::highlight::service::highlight_source(
            crate::highlight::model::HighlightRequest {
                path: Some(Path::new("src/query.rs")),
                language_hint: None,
                source: "let query = r#\"\nfrom users\n\"#;\n",
            },
        );

        assert!(
            highlighted.lines[1]
                .spans
                .iter()
                .any(|span| span.class == SyntaxClass::String && span.text.contains("from users"))
        );
    }
}
