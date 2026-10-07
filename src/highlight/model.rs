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

impl SyntaxClass {
    /// Stable kebab-case name for a class, mirroring the tree-sitter capture
    /// it came from (`function.builtin` -> `function-builtin`). The desktop GUI
    /// sends it to the frontend, which styles each name through theme tokens.
    /// `None` for plain text, which needs no styling.
    pub fn token_name(self) -> Option<&'static str> {
        Some(match self {
            SyntaxClass::Plain => return None,
            SyntaxClass::Attribute => "attribute",
            SyntaxClass::Comment => "comment",
            SyntaxClass::Constant => "constant",
            SyntaxClass::ConstantBuiltin => "constant-builtin",
            SyntaxClass::Constructor => "constructor",
            SyntaxClass::Embedded => "embedded",
            SyntaxClass::Function => "function",
            SyntaxClass::FunctionBuiltin => "function-builtin",
            SyntaxClass::Keyword => "keyword",
            SyntaxClass::Module => "module",
            SyntaxClass::Number => "number",
            SyntaxClass::Operator => "operator",
            SyntaxClass::Property => "property",
            SyntaxClass::PropertyBuiltin => "property-builtin",
            SyntaxClass::Punctuation => "punctuation",
            SyntaxClass::PunctuationBracket => "punctuation-bracket",
            SyntaxClass::PunctuationDelimiter => "punctuation-delimiter",
            SyntaxClass::PunctuationSpecial => "punctuation-special",
            SyntaxClass::String => "string",
            SyntaxClass::StringSpecial => "string-special",
            SyntaxClass::Tag => "tag",
            SyntaxClass::Type => "type",
            SyntaxClass::TypeBuiltin => "type-builtin",
            SyntaxClass::Variable => "variable",
            SyntaxClass::VariableBuiltin => "variable-builtin",
            SyntaxClass::VariableParameter => "variable-parameter",
        })
    }
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
