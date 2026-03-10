use std::path::Path;

#[derive(Debug, Clone, Copy)]
pub struct HighlightRequest<'a> {
    pub path: Option<&'a Path>,
    pub language_hint: Option<&'a str>,
    pub source: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightedText {
    pub language_name: Option<String>,
    pub lines: Vec<HighlightedLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HighlightedLine {
    pub spans: Vec<HighlightedSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightedSpan {
    pub text: String,
    pub class: SyntaxClass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyntaxClass {
    Plain,
    Attribute,
    Comment,
    Constant,
    ConstantBuiltin,
    Constructor,
    Embedded,
    Function,
    FunctionBuiltin,
    Keyword,
    Module,
    Number,
    Operator,
    Property,
    PropertyBuiltin,
    Punctuation,
    PunctuationBracket,
    PunctuationDelimiter,
    PunctuationSpecial,
    String,
    StringSpecial,
    Tag,
    Type,
    TypeBuiltin,
    Variable,
    VariableBuiltin,
    VariableParameter,
}

impl HighlightedText {
    pub fn plain(language_name: Option<String>, source: &str) -> Self {
        let mut lines = Vec::new();
        for line in source.split('\n') {
            let mut highlighted = HighlightedLine::default();
            if !line.is_empty() {
                highlighted.spans.push(HighlightedSpan {
                    text: line.to_string(),
                    class: SyntaxClass::Plain,
                });
            }
            lines.push(highlighted);
        }
        if lines.is_empty() {
            lines.push(HighlightedLine::default());
        }
        Self {
            language_name,
            lines,
        }
    }
}
