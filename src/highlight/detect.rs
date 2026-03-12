use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HighlightLanguage {
    Bash,
    C,
    Cpp,
    Css,
    Go,
    Html,
    Java,
    JavaScript,
    Json,
    Markdown,
    Python,
    Rust,
    Toml,
    Tsx,
    TypeScript,
    Yaml,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighlightInstallState {
    Available,
    Installed,
    Broken,
}

pub struct HighlightPackageSpec {
    pub repo_url: &'static str,
    pub revision: &'static str,
    pub grammars: &'static [HighlightGrammarSpec],
}

pub struct HighlightGrammarSpec {
    pub config_name: &'static str,
    pub symbol_name: &'static str,
    pub include_dir: &'static str,
    pub source_files: &'static [&'static str],
    pub highlights_query: &'static str,
    pub injections_query: &'static str,
    pub locals_query: &'static str,
    pub injection_aliases: &'static [&'static str],
}

const BASH_HINTS: &[&str] = &["bash", "shell", "sh", "zsh"];
const C_HINTS: &[&str] = &["c"];
const CPP_HINTS: &[&str] = &["c++", "cc", "cpp", "cxx", "hpp"];
const CSS_HINTS: &[&str] = &["css"];
const GO_HINTS: &[&str] = &["go", "golang"];
const HTML_HINTS: &[&str] = &["html", "htm"];
const JAVA_HINTS: &[&str] = &["java"];
const JAVASCRIPT_HINTS: &[&str] = &["javascript", "js", "jsx", "node", "deno", "bun"];
const JSON_HINTS: &[&str] = &["json", "jsonc", "jsonl"];
const MARKDOWN_HINTS: &[&str] = &["markdown", "md"];
const PYTHON_HINTS: &[&str] = &["py", "python"];
const RUST_HINTS: &[&str] = &["rust", "rs"];
const TOML_HINTS: &[&str] = &["toml"];
const TSX_HINTS: &[&str] = &["tsx"];
const TYPESCRIPT_HINTS: &[&str] = &["ts", "typescript"];
const YAML_HINTS: &[&str] = &["yaml", "yml"];

const BASH_EXTENSIONS: &[&str] = &["sh", "bash", "zsh"];
const C_EXTENSIONS: &[&str] = &["c"];
const CPP_EXTENSIONS: &[&str] = &["cc", "cpp", "cxx", "hh", "hpp", "hxx"];
const CSS_EXTENSIONS: &[&str] = &["css"];
const GO_EXTENSIONS: &[&str] = &["go"];
const HTML_EXTENSIONS: &[&str] = &["htm", "html"];
const JAVA_EXTENSIONS: &[&str] = &["java"];
const JAVASCRIPT_EXTENSIONS: &[&str] = &["cjs", "js", "jsx", "mjs"];
const JSON_EXTENSIONS: &[&str] = &["json", "jsonl"];
const MARKDOWN_EXTENSIONS: &[&str] = &["md", "markdown"];
const PYTHON_EXTENSIONS: &[&str] = &["py", "pyi", "pyw"];
const RUST_EXTENSIONS: &[&str] = &["rs"];
const TOML_EXTENSIONS: &[&str] = &["toml"];
const TSX_EXTENSIONS: &[&str] = &["tsx"];
const TYPESCRIPT_EXTENSIONS: &[&str] = &["cts", "mts", "ts"];
const YAML_EXTENSIONS: &[&str] = &["yml", "yaml"];

const BASH_FILE_NAMES: &[&str] = &[".bashrc", ".bash_profile", ".zshrc", ".zprofile"];
const TOML_FILE_NAMES: &[&str] = &["cargo.lock"];

const BASH_SHEBANGS: &[&str] = &["bash", "sh", "zsh"];
const JAVASCRIPT_SHEBANGS: &[&str] = &["node", "deno", "bun"];
const PYTHON_SHEBANGS: &[&str] = &["python", "python3"];

const BASH_GRAMMARS: &[HighlightGrammarSpec] = &[HighlightGrammarSpec {
    config_name: "bash",
    symbol_name: "tree_sitter_bash",
    include_dir: "src",
    source_files: &["src/parser.c", "src/scanner.c"],
    highlights_query: "queries/highlights.scm",
    injections_query: "",
    locals_query: "",
    injection_aliases: BASH_HINTS,
}];

const C_GRAMMARS: &[HighlightGrammarSpec] = &[HighlightGrammarSpec {
    config_name: "c",
    symbol_name: "tree_sitter_c",
    include_dir: "src",
    source_files: &["src/parser.c", "src/scanner.c"],
    highlights_query: "queries/highlights.scm",
    injections_query: "",
    locals_query: "",
    injection_aliases: C_HINTS,
}];

const CPP_GRAMMARS: &[HighlightGrammarSpec] = &[HighlightGrammarSpec {
    config_name: "cpp",
    symbol_name: "tree_sitter_cpp",
    include_dir: "src",
    source_files: &["src/parser.c", "src/scanner.c"],
    highlights_query: "queries/highlights.scm",
    injections_query: "",
    locals_query: "",
    injection_aliases: CPP_HINTS,
}];

const CSS_GRAMMARS: &[HighlightGrammarSpec] = &[HighlightGrammarSpec {
    config_name: "css",
    symbol_name: "tree_sitter_css",
    include_dir: "src",
    source_files: &["src/parser.c", "src/scanner.c"],
    highlights_query: "queries/highlights.scm",
    injections_query: "",
    locals_query: "",
    injection_aliases: CSS_HINTS,
}];

const GO_GRAMMARS: &[HighlightGrammarSpec] = &[HighlightGrammarSpec {
    config_name: "go",
    symbol_name: "tree_sitter_go",
    include_dir: "src",
    source_files: &["src/parser.c", "src/scanner.c"],
    highlights_query: "queries/highlights.scm",
    injections_query: "",
    locals_query: "",
    injection_aliases: GO_HINTS,
}];

const HTML_GRAMMARS: &[HighlightGrammarSpec] = &[HighlightGrammarSpec {
    config_name: "html",
    symbol_name: "tree_sitter_html",
    include_dir: "src",
    source_files: &["src/parser.c", "src/scanner.c"],
    highlights_query: "queries/highlights.scm",
    injections_query: "",
    locals_query: "",
    injection_aliases: HTML_HINTS,
}];

const JAVA_GRAMMARS: &[HighlightGrammarSpec] = &[HighlightGrammarSpec {
    config_name: "java",
    symbol_name: "tree_sitter_java",
    include_dir: "src",
    source_files: &["src/parser.c", "src/scanner.c"],
    highlights_query: "queries/highlights.scm",
    injections_query: "",
    locals_query: "",
    injection_aliases: JAVA_HINTS,
}];

const JAVASCRIPT_GRAMMARS: &[HighlightGrammarSpec] = &[HighlightGrammarSpec {
    config_name: "javascript",
    symbol_name: "tree_sitter_javascript",
    include_dir: "src",
    source_files: &["src/parser.c", "src/scanner.c"],
    highlights_query: "queries/highlights.scm",
    injections_query: "queries/injections.scm",
    locals_query: "queries/locals.scm",
    injection_aliases: JAVASCRIPT_HINTS,
}];

const JSON_GRAMMARS: &[HighlightGrammarSpec] = &[HighlightGrammarSpec {
    config_name: "json",
    symbol_name: "tree_sitter_json",
    include_dir: "src",
    source_files: &["src/parser.c"],
    highlights_query: "queries/highlights.scm",
    injections_query: "",
    locals_query: "",
    injection_aliases: JSON_HINTS,
}];

const MARKDOWN_GRAMMARS: &[HighlightGrammarSpec] = &[
    HighlightGrammarSpec {
        config_name: "markdown",
        symbol_name: "tree_sitter_markdown",
        include_dir: "tree-sitter-markdown/src",
        source_files: &[
            "tree-sitter-markdown/src/parser.c",
            "tree-sitter-markdown/src/scanner.c",
        ],
        highlights_query: "tree-sitter-markdown/queries/highlights.scm",
        injections_query: "tree-sitter-markdown/queries/injections.scm",
        locals_query: "",
        injection_aliases: &["markdown"],
    },
    HighlightGrammarSpec {
        config_name: "markdown_inline",
        symbol_name: "tree_sitter_markdown_inline",
        include_dir: "tree-sitter-markdown-inline/src",
        source_files: &[
            "tree-sitter-markdown-inline/src/parser.c",
            "tree-sitter-markdown-inline/src/scanner.c",
        ],
        highlights_query: "tree-sitter-markdown-inline/queries/highlights.scm",
        injections_query: "tree-sitter-markdown-inline/queries/injections.scm",
        locals_query: "",
        injection_aliases: &["markdown_inline", "markdown-inline", "md"],
    },
];

const PYTHON_GRAMMARS: &[HighlightGrammarSpec] = &[HighlightGrammarSpec {
    config_name: "python",
    symbol_name: "tree_sitter_python",
    include_dir: "src",
    source_files: &["src/parser.c", "src/scanner.c"],
    highlights_query: "queries/highlights.scm",
    injections_query: "queries/injections.scm",
    locals_query: "queries/locals.scm",
    injection_aliases: PYTHON_HINTS,
}];

const RUST_GRAMMARS: &[HighlightGrammarSpec] = &[HighlightGrammarSpec {
    config_name: "rust",
    symbol_name: "tree_sitter_rust",
    include_dir: "src",
    source_files: &["src/parser.c", "src/scanner.c"],
    highlights_query: "queries/highlights.scm",
    injections_query: "queries/injections.scm",
    locals_query: "",
    injection_aliases: RUST_HINTS,
}];

const TOML_GRAMMARS: &[HighlightGrammarSpec] = &[HighlightGrammarSpec {
    config_name: "toml",
    symbol_name: "tree_sitter_toml",
    include_dir: "src",
    source_files: &["src/parser.c", "src/scanner.c"],
    highlights_query: "queries/highlights.scm",
    injections_query: "",
    locals_query: "",
    injection_aliases: TOML_HINTS,
}];

const TSX_GRAMMARS: &[HighlightGrammarSpec] = &[HighlightGrammarSpec {
    config_name: "tsx",
    symbol_name: "tree_sitter_tsx",
    include_dir: "tsx/src",
    source_files: &["tsx/src/parser.c", "tsx/src/scanner.c"],
    highlights_query: "tsx/queries/highlights.scm",
    injections_query: "tsx/queries/injections.scm",
    locals_query: "tsx/queries/locals.scm",
    injection_aliases: TSX_HINTS,
}];

const TYPESCRIPT_GRAMMARS: &[HighlightGrammarSpec] = &[HighlightGrammarSpec {
    config_name: "typescript",
    symbol_name: "tree_sitter_typescript",
    include_dir: "typescript/src",
    source_files: &["typescript/src/parser.c", "typescript/src/scanner.c"],
    highlights_query: "typescript/queries/highlights.scm",
    injections_query: "typescript/queries/injections.scm",
    locals_query: "typescript/queries/locals.scm",
    injection_aliases: TYPESCRIPT_HINTS,
}];

const YAML_GRAMMARS: &[HighlightGrammarSpec] = &[HighlightGrammarSpec {
    config_name: "yaml",
    symbol_name: "tree_sitter_yaml",
    include_dir: "src",
    source_files: &["src/parser.c", "src/scanner.c"],
    highlights_query: "queries/highlights.scm",
    injections_query: "",
    locals_query: "",
    injection_aliases: YAML_HINTS,
}];

const BASH_PACKAGE: HighlightPackageSpec = HighlightPackageSpec {
    repo_url: "https://github.com/tree-sitter/tree-sitter-bash",
    revision: "a06c2e4415e9bc0346c6b86d401879ffb44058f7",
    grammars: BASH_GRAMMARS,
};

const C_PACKAGE: HighlightPackageSpec = HighlightPackageSpec {
    repo_url: "https://github.com/tree-sitter/tree-sitter-c",
    revision: "HEAD",
    grammars: C_GRAMMARS,
};

const CPP_PACKAGE: HighlightPackageSpec = HighlightPackageSpec {
    repo_url: "https://github.com/tree-sitter/tree-sitter-cpp",
    revision: "HEAD",
    grammars: CPP_GRAMMARS,
};

const CSS_PACKAGE: HighlightPackageSpec = HighlightPackageSpec {
    repo_url: "https://github.com/tree-sitter/tree-sitter-css",
    revision: "HEAD",
    grammars: CSS_GRAMMARS,
};

const GO_PACKAGE: HighlightPackageSpec = HighlightPackageSpec {
    repo_url: "https://github.com/tree-sitter/tree-sitter-go",
    revision: "HEAD",
    grammars: GO_GRAMMARS,
};

const HTML_PACKAGE: HighlightPackageSpec = HighlightPackageSpec {
    repo_url: "https://github.com/tree-sitter/tree-sitter-html",
    revision: "HEAD",
    grammars: HTML_GRAMMARS,
};

const JAVA_PACKAGE: HighlightPackageSpec = HighlightPackageSpec {
    repo_url: "https://github.com/tree-sitter/tree-sitter-java",
    revision: "HEAD",
    grammars: JAVA_GRAMMARS,
};

const JAVASCRIPT_PACKAGE: HighlightPackageSpec = HighlightPackageSpec {
    repo_url: "https://github.com/tree-sitter/tree-sitter-javascript",
    revision: "HEAD",
    grammars: JAVASCRIPT_GRAMMARS,
};

const JSON_PACKAGE: HighlightPackageSpec = HighlightPackageSpec {
    repo_url: "https://github.com/tree-sitter/tree-sitter-json",
    revision: "ee35a6ebefcef0c5c416c0d1ccec7370cfca5a24",
    grammars: JSON_GRAMMARS,
};

const MARKDOWN_PACKAGE: HighlightPackageSpec = HighlightPackageSpec {
    repo_url: "https://github.com/tree-sitter-grammars/tree-sitter-markdown",
    revision: "f969cd3ae3f9fbd4e43205431d0ae286014c05b5",
    grammars: MARKDOWN_GRAMMARS,
};

const PYTHON_PACKAGE: HighlightPackageSpec = HighlightPackageSpec {
    repo_url: "https://github.com/tree-sitter/tree-sitter-python",
    revision: "HEAD",
    grammars: PYTHON_GRAMMARS,
};

const RUST_PACKAGE: HighlightPackageSpec = HighlightPackageSpec {
    repo_url: "https://github.com/tree-sitter/tree-sitter-rust",
    revision: "18b0515fca567f5a10aee9978c6d2640e878671a",
    grammars: RUST_GRAMMARS,
};

const TOML_PACKAGE: HighlightPackageSpec = HighlightPackageSpec {
    repo_url: "https://github.com/tree-sitter-grammars/tree-sitter-toml",
    revision: "64b56832c2cffe41758f28e05c756a3a98d16f41",
    grammars: TOML_GRAMMARS,
};

const TSX_PACKAGE: HighlightPackageSpec = HighlightPackageSpec {
    repo_url: "https://github.com/tree-sitter/tree-sitter-typescript",
    revision: "HEAD",
    grammars: TSX_GRAMMARS,
};

const TYPESCRIPT_PACKAGE: HighlightPackageSpec = HighlightPackageSpec {
    repo_url: "https://github.com/tree-sitter/tree-sitter-typescript",
    revision: "HEAD",
    grammars: TYPESCRIPT_GRAMMARS,
};

const YAML_PACKAGE: HighlightPackageSpec = HighlightPackageSpec {
    repo_url: "https://github.com/tree-sitter-grammars/tree-sitter-yaml",
    revision: "7708026449bed86239b1cd5bce6e3c34dbca6415",
    grammars: YAML_GRAMMARS,
};

impl HighlightLanguage {
    pub const ALL: [HighlightLanguage; 16] = [
        HighlightLanguage::Bash,
        HighlightLanguage::C,
        HighlightLanguage::Cpp,
        HighlightLanguage::Css,
        HighlightLanguage::Go,
        HighlightLanguage::Html,
        HighlightLanguage::Java,
        HighlightLanguage::JavaScript,
        HighlightLanguage::Json,
        HighlightLanguage::Markdown,
        HighlightLanguage::Python,
        HighlightLanguage::Rust,
        HighlightLanguage::Toml,
        HighlightLanguage::Tsx,
        HighlightLanguage::TypeScript,
        HighlightLanguage::Yaml,
    ];

    pub fn display_name(self) -> &'static str {
        match self {
            HighlightLanguage::Bash => "bash",
            HighlightLanguage::C => "c",
            HighlightLanguage::Cpp => "c++",
            HighlightLanguage::Css => "css",
            HighlightLanguage::Go => "go",
            HighlightLanguage::Html => "html",
            HighlightLanguage::Java => "java",
            HighlightLanguage::JavaScript => "javascript",
            HighlightLanguage::Json => "json",
            HighlightLanguage::Markdown => "markdown",
            HighlightLanguage::Python => "python",
            HighlightLanguage::Rust => "rust",
            HighlightLanguage::Toml => "toml",
            HighlightLanguage::Tsx => "tsx",
            HighlightLanguage::TypeScript => "typescript",
            HighlightLanguage::Yaml => "yaml",
        }
    }

    pub fn picker_title(self) -> &'static str {
        match self {
            HighlightLanguage::Bash => "Bash",
            HighlightLanguage::C => "C",
            HighlightLanguage::Cpp => "C++",
            HighlightLanguage::Css => "CSS",
            HighlightLanguage::Go => "Go",
            HighlightLanguage::Html => "HTML",
            HighlightLanguage::Java => "Java",
            HighlightLanguage::JavaScript => "JavaScript",
            HighlightLanguage::Json => "JSON",
            HighlightLanguage::Markdown => "Markdown",
            HighlightLanguage::Python => "Python",
            HighlightLanguage::Rust => "Rust",
            HighlightLanguage::Toml => "TOML",
            HighlightLanguage::Tsx => "TSX",
            HighlightLanguage::TypeScript => "TypeScript",
            HighlightLanguage::Yaml => "YAML",
        }
    }

    pub fn picker_description(self) -> &'static str {
        match self {
            HighlightLanguage::Bash => "Shell scripts and dotfiles",
            HighlightLanguage::C => "C source files",
            HighlightLanguage::Cpp => "C++ source and header files",
            HighlightLanguage::Css => "CSS stylesheets",
            HighlightLanguage::Go => "Go source files",
            HighlightLanguage::Html => "HTML templates and documents",
            HighlightLanguage::Java => "Java source files",
            HighlightLanguage::JavaScript => "JavaScript, JSX, and Node scripts",
            HighlightLanguage::Json => "JSON, JSONL, and JSONC",
            HighlightLanguage::Markdown => "Markdown plus fenced-code injections",
            HighlightLanguage::Python => "Python source files and scripts",
            HighlightLanguage::Rust => "Rust source files",
            HighlightLanguage::Toml => "TOML and Cargo files",
            HighlightLanguage::Tsx => "TypeScript React and TSX files",
            HighlightLanguage::TypeScript => "TypeScript source files",
            HighlightLanguage::Yaml => "YAML and YML",
        }
    }

    pub fn extension_summary(self) -> &'static str {
        match self {
            HighlightLanguage::Bash => ".sh, .bash, .zsh",
            HighlightLanguage::C => ".c",
            HighlightLanguage::Cpp => ".cc, .cpp, .cxx, .hpp",
            HighlightLanguage::Css => ".css",
            HighlightLanguage::Go => ".go",
            HighlightLanguage::Html => ".html, .htm",
            HighlightLanguage::Java => ".java",
            HighlightLanguage::JavaScript => ".js, .jsx, .mjs, .cjs",
            HighlightLanguage::Json => ".json, .jsonl",
            HighlightLanguage::Markdown => ".md, .markdown",
            HighlightLanguage::Python => ".py, .pyi, .pyw",
            HighlightLanguage::Rust => ".rs",
            HighlightLanguage::Toml => ".toml, Cargo.lock",
            HighlightLanguage::Tsx => ".tsx",
            HighlightLanguage::TypeScript => ".ts, .mts, .cts",
            HighlightLanguage::Yaml => ".yml, .yaml",
        }
    }

    pub fn package_spec(self) -> &'static HighlightPackageSpec {
        match self {
            HighlightLanguage::Bash => &BASH_PACKAGE,
            HighlightLanguage::C => &C_PACKAGE,
            HighlightLanguage::Cpp => &CPP_PACKAGE,
            HighlightLanguage::Css => &CSS_PACKAGE,
            HighlightLanguage::Go => &GO_PACKAGE,
            HighlightLanguage::Html => &HTML_PACKAGE,
            HighlightLanguage::Java => &JAVA_PACKAGE,
            HighlightLanguage::JavaScript => &JAVASCRIPT_PACKAGE,
            HighlightLanguage::Json => &JSON_PACKAGE,
            HighlightLanguage::Markdown => &MARKDOWN_PACKAGE,
            HighlightLanguage::Python => &PYTHON_PACKAGE,
            HighlightLanguage::Rust => &RUST_PACKAGE,
            HighlightLanguage::Toml => &TOML_PACKAGE,
            HighlightLanguage::Tsx => &TSX_PACKAGE,
            HighlightLanguage::TypeScript => &TYPESCRIPT_PACKAGE,
            HighlightLanguage::Yaml => &YAML_PACKAGE,
        }
    }

    pub fn package_key(self) -> &'static str {
        match self {
            HighlightLanguage::Bash => "bash",
            HighlightLanguage::C => "c",
            HighlightLanguage::Cpp => "cpp",
            HighlightLanguage::Css => "css",
            HighlightLanguage::Go => "go",
            HighlightLanguage::Html => "html",
            HighlightLanguage::Java => "java",
            HighlightLanguage::JavaScript => "javascript",
            HighlightLanguage::Json => "json",
            HighlightLanguage::Markdown => "markdown",
            HighlightLanguage::Python => "python",
            HighlightLanguage::Rust => "rust",
            HighlightLanguage::Toml => "toml",
            HighlightLanguage::Tsx => "tsx",
            HighlightLanguage::TypeScript => "typescript",
            HighlightLanguage::Yaml => "yaml",
        }
    }

    pub fn source_dir(self) -> PathBuf {
        syntax_root_dir().join("sources").join(self.package_key())
    }

    pub fn library_path(self) -> PathBuf {
        syntax_root_dir()
            .join("parsers")
            .join(format!("{}{}", self.package_key(), shared_library_extension()))
    }

    pub fn install_state(self) -> HighlightInstallState {
        let source_exists = self.source_dir().exists();
        let library_exists = self.library_path().exists();
        match (source_exists, library_exists) {
            (true, true) => HighlightInstallState::Installed,
            (false, false) => HighlightInstallState::Available,
            _ => HighlightInstallState::Broken,
        }
    }

    fn matches_hint(self, hint: &str) -> bool {
        self.language_hints().contains(&hint)
    }

    fn matches_file_name(self, file_name: &str) -> bool {
        self.file_names().contains(&file_name)
    }

    fn matches_extension(self, extension: &str) -> bool {
        self.extensions().contains(&extension)
    }

    fn matches_shebang(self, first_line: &str) -> bool {
        self.shebang_tokens()
            .iter()
            .any(|token| first_line.contains(token))
    }

    fn language_hints(self) -> &'static [&'static str] {
        match self {
            HighlightLanguage::Bash => BASH_HINTS,
            HighlightLanguage::C => C_HINTS,
            HighlightLanguage::Cpp => CPP_HINTS,
            HighlightLanguage::Css => CSS_HINTS,
            HighlightLanguage::Go => GO_HINTS,
            HighlightLanguage::Html => HTML_HINTS,
            HighlightLanguage::Java => JAVA_HINTS,
            HighlightLanguage::JavaScript => JAVASCRIPT_HINTS,
            HighlightLanguage::Json => JSON_HINTS,
            HighlightLanguage::Markdown => MARKDOWN_HINTS,
            HighlightLanguage::Python => PYTHON_HINTS,
            HighlightLanguage::Rust => RUST_HINTS,
            HighlightLanguage::Toml => TOML_HINTS,
            HighlightLanguage::Tsx => TSX_HINTS,
            HighlightLanguage::TypeScript => TYPESCRIPT_HINTS,
            HighlightLanguage::Yaml => YAML_HINTS,
        }
    }

    fn extensions(self) -> &'static [&'static str] {
        match self {
            HighlightLanguage::Bash => BASH_EXTENSIONS,
            HighlightLanguage::C => C_EXTENSIONS,
            HighlightLanguage::Cpp => CPP_EXTENSIONS,
            HighlightLanguage::Css => CSS_EXTENSIONS,
            HighlightLanguage::Go => GO_EXTENSIONS,
            HighlightLanguage::Html => HTML_EXTENSIONS,
            HighlightLanguage::Java => JAVA_EXTENSIONS,
            HighlightLanguage::JavaScript => JAVASCRIPT_EXTENSIONS,
            HighlightLanguage::Json => JSON_EXTENSIONS,
            HighlightLanguage::Markdown => MARKDOWN_EXTENSIONS,
            HighlightLanguage::Python => PYTHON_EXTENSIONS,
            HighlightLanguage::Rust => RUST_EXTENSIONS,
            HighlightLanguage::Toml => TOML_EXTENSIONS,
            HighlightLanguage::Tsx => TSX_EXTENSIONS,
            HighlightLanguage::TypeScript => TYPESCRIPT_EXTENSIONS,
            HighlightLanguage::Yaml => YAML_EXTENSIONS,
        }
    }

    fn file_names(self) -> &'static [&'static str] {
        match self {
            HighlightLanguage::Bash => BASH_FILE_NAMES,
            HighlightLanguage::Toml => TOML_FILE_NAMES,
            _ => &[],
        }
    }

    fn shebang_tokens(self) -> &'static [&'static str] {
        match self {
            HighlightLanguage::Bash => BASH_SHEBANGS,
            HighlightLanguage::JavaScript => JAVASCRIPT_SHEBANGS,
            HighlightLanguage::Python => PYTHON_SHEBANGS,
            _ => &[],
        }
    }
}

pub fn detect_language(
    path: Option<&Path>,
    language_hint: Option<&str>,
    source: &str,
) -> Option<HighlightLanguage> {
    language_hint
        .and_then(parse_language_name)
        .or_else(|| path.and_then(language_from_path))
        .or_else(|| language_from_shebang(source))
}

fn parse_language_name(name: &str) -> Option<HighlightLanguage> {
    let normalized = name.trim().to_ascii_lowercase();
    HighlightLanguage::ALL
        .into_iter()
        .find(|language| language.matches_hint(normalized.as_str()))
}

fn language_from_path(path: &Path) -> Option<HighlightLanguage> {
    if let Some(file_name) = path.file_name().and_then(|name| name.to_str()) {
        let normalized = file_name.to_ascii_lowercase();
        if let Some(language) = HighlightLanguage::ALL
            .into_iter()
            .find(|language| language.matches_file_name(normalized.as_str()))
        {
            return Some(language);
        }
    }

    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)?;

    HighlightLanguage::ALL
        .into_iter()
        .find(|language| language.matches_extension(extension.as_str()))
}

fn language_from_shebang(source: &str) -> Option<HighlightLanguage> {
    let first_line = source.lines().next()?.trim();
    if !first_line.starts_with("#!") {
        return None;
    }

    HighlightLanguage::ALL
        .into_iter()
        .find(|language| language.matches_shebang(first_line))
}

fn syntax_root_dir() -> PathBuf {
    crate::project::amf_config_dir().join("tree-sitter")
}

#[cfg(target_os = "macos")]
fn shared_library_extension() -> &'static str {
    ".dylib"
}

#[cfg(not(target_os = "macos"))]
fn shared_library_extension() -> &'static str {
    ".so"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_language_from_extension() {
        assert_eq!(
            detect_language(Some(Path::new("src/main.rs")), None, ""),
            Some(HighlightLanguage::Rust)
        );
        assert_eq!(
            detect_language(Some(Path::new("src/main.py")), None, ""),
            Some(HighlightLanguage::Python)
        );
        assert_eq!(
            detect_language(Some(Path::new("web/app.ts")), None, ""),
            Some(HighlightLanguage::TypeScript)
        );
        assert_eq!(
            detect_language(Some(Path::new("web/app.tsx")), None, ""),
            Some(HighlightLanguage::Tsx)
        );
        assert_eq!(
            detect_language(Some(Path::new("web/app.jsx")), None, ""),
            Some(HighlightLanguage::JavaScript)
        );
        assert_eq!(
            detect_language(Some(Path::new("config/settings.json")), None, ""),
            Some(HighlightLanguage::Json)
        );
        assert_eq!(
            detect_language(Some(Path::new("README.md")), None, ""),
            Some(HighlightLanguage::Markdown)
        );
    }

    #[test]
    fn detects_language_from_special_file_name() {
        assert_eq!(
            detect_language(Some(Path::new("Cargo.lock")), None, ""),
            Some(HighlightLanguage::Toml)
        );
        assert_eq!(
            detect_language(Some(Path::new(".zshrc")), None, ""),
            Some(HighlightLanguage::Bash)
        );
    }

    #[test]
    fn detects_language_from_hint_before_path() {
        assert_eq!(
            detect_language(Some(Path::new("src/main.rs")), Some("yaml"), ""),
            Some(HighlightLanguage::Yaml)
        );
        assert_eq!(
            detect_language(Some(Path::new("src/main.rs")), Some("javascript"), ""),
            Some(HighlightLanguage::JavaScript)
        );
    }

    #[test]
    fn detects_language_from_shebang() {
        assert_eq!(
            detect_language(None, None, "#!/usr/bin/env bash\nprintf 'ok'\n"),
            Some(HighlightLanguage::Bash)
        );
        assert_eq!(
            detect_language(None, None, "#!/usr/bin/env node\nconsole.log('ok')\n"),
            Some(HighlightLanguage::JavaScript)
        );
        assert_eq!(
            detect_language(None, None, "#!/usr/bin/env python3\nprint('ok')\n"),
            Some(HighlightLanguage::Python)
        );
    }
}
