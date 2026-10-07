//! Syntax highlighting for the desktop GUI's diffs, Final Review and Learning
//! reader.
//!
//! Decision: highlighting happens here, in Rust, through the TUI's own
//! tree-sitter service (`crate::highlight`), and the frontend receives token
//! spans. Both interfaces therefore agree on a file's language (the same
//! path/shebang detection), use the same grammars and token classes, share
//! one on-disk parser install (`~/.config/amf/tree-sitter`) and work offline
//! once a parser is installed. The frontend bundle gains no highlighter or
//! grammars. Whole old/new file contents are only available on this side,
//! so each diff line is coloured with its full-file context: a line inside a
//! multi-line string or block comment is classified correctly even when the
//! hunk shows none of the lines that opened it.
//!
//! A file whose parser is not installed, whose language is unknown, that is
//! binary or that exceeds the size limits stays plain text, and its
//! [`SyntaxInfo`] says which, so the GUI can explain it and offer the
//! install the TUI's syntax-language picker performs.
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;

use crate::diff::{DiffFile, DiffLineKind};
use crate::gui_contract::{GuiError, GuiResult};
use crate::highlight::{
    self, HighlightInstallState, HighlightLanguage, HighlightRequest, HighlightedLine,
    HighlightedText,
};

/// Above this many lines one side of a file is left plain. Every highlighted
/// line becomes several DOM nodes, so a huge generated file costs more to
/// render than its colours are worth.
pub const MAX_HIGHLIGHT_LINES: usize = 10_000;
/// Above this many bytes one side of a file is left plain.
pub const MAX_HIGHLIGHT_BYTES: usize = 1024 * 1024;
/// Source bytes one diff/review projection may highlight in total. The
/// shared service caches by content, so this bounds the first, uncached
/// load of a very large change set; later files report `over_budget`.
pub const VIEW_HIGHLIGHT_BUDGET_BYTES: usize = 4 * 1024 * 1024;

/// A run of text and its token class, serialized as `["keyword", "fn"]`.
/// An empty class is plain text. The texts of a line's spans concatenate to
/// exactly that line's text, whitespace included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SyntaxSpan(pub &'static str, pub String);

/// Highlight spans for one rendered line; `None` renders the line's text
/// plain.
pub type SyntaxLine = Option<Vec<SyntaxSpan>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyntaxStatus {
    /// The installed parser coloured this file.
    Highlighted,
    /// No supported language matches the path or shebang.
    Unsupported,
    /// The language is supported, but its parser is not installed.
    NotInstalled,
    /// A partial or unloadable parser install: reinstalling repairs it.
    Broken,
    /// One side exceeds [`MAX_HIGHLIGHT_LINES`] or [`MAX_HIGHLIGHT_BYTES`].
    TooLarge,
    /// The view's [`VIEW_HIGHLIGHT_BUDGET_BYTES`] was spent on earlier files.
    OverBudget,
    /// Binary content has no text to colour.
    Binary,
}

/// Why a file is (or is not) highlighted, for the GUI's language badge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SyntaxInfo {
    /// The language's display title (`Rust`), when one was detected.
    pub language: Option<String>,
    /// The parser key an install request names (`rust`).
    pub language_key: Option<String>,
    pub status: SyntaxStatus,
}

impl SyntaxInfo {
    fn detected(detected: Option<(HighlightLanguage, HighlightInstallState)>) -> Self {
        let Some((language, state)) = detected else {
            return Self {
                language: None,
                language_key: None,
                status: SyntaxStatus::Unsupported,
            };
        };
        Self {
            language: Some(language.picker_title().to_string()),
            language_key: Some(language.package_key().to_string()),
            status: match state {
                // Installed but unloadable (a bad build, queries that no
                // longer compile) is repaired by reinstalling, as for a
                // partial install.
                HighlightInstallState::Installed if !highlight::parser_loads(language) => {
                    SyntaxStatus::Broken
                }
                HighlightInstallState::Installed => SyntaxStatus::Highlighted,
                HighlightInstallState::Available => SyntaxStatus::NotInstalled,
                HighlightInstallState::Broken => SyntaxStatus::Broken,
            },
        }
    }

    fn with_status(mut self, status: SyntaxStatus) -> Self {
        self.status = status;
        self
    }
}

/// Bytes of source a single projection may still highlight.
pub(crate) struct HighlightBudget {
    remaining: usize,
}

impl HighlightBudget {
    pub(crate) fn view() -> Self {
        Self {
            remaining: VIEW_HIGHLIGHT_BUDGET_BYTES,
        }
    }

    fn take(&mut self, bytes: usize) -> bool {
        if bytes > self.remaining {
            return false;
        }
        self.remaining -= bytes;
        true
    }
}

fn too_large(source: &str) -> bool {
    source.len() > MAX_HIGHLIGHT_BYTES || source.lines().count() > MAX_HIGHLIGHT_LINES
}

fn highlight(path: &Path, source: &str) -> HighlightedText {
    highlight::highlight_source(HighlightRequest {
        path: Some(path),
        language_hint: None,
        source,
    })
}

/// Both sides of one diff file, highlighted as whole files.
pub(crate) struct DiffHighlights {
    old: Option<HighlightedText>,
    new: Option<HighlightedText>,
    pub info: SyntaxInfo,
}

impl DiffHighlights {
    /// Highlights a hydrated diff file's base and current contents. Each side
    /// uses its own path, so a rename that changes the extension colours
    /// each side as its own language, as the TUI does.
    pub(crate) fn for_file(file: &DiffFile, budget: &mut HighlightBudget) -> Self {
        let new_path = Path::new(&file.path);
        let old_path = file.old_path.as_deref().map(Path::new).unwrap_or(new_path);
        let (path, source) = match (&file.new_content, &file.old_content) {
            (Some(new), _) => (new_path, new.as_str()),
            (None, Some(old)) => (old_path, old.as_str()),
            (None, None) => (new_path, ""),
        };
        let info = SyntaxInfo::detected(highlight::language_for_source(Some(path), source));
        let plain = |info: SyntaxInfo| Self {
            old: None,
            new: None,
            info,
        };
        if file.is_binary {
            return plain(info.with_status(SyntaxStatus::Binary));
        }
        if info.status != SyntaxStatus::Highlighted {
            return plain(info);
        }
        let sides = [file.old_content.as_deref(), file.new_content.as_deref()];
        if sides.iter().flatten().any(|side| too_large(side)) {
            return plain(info.with_status(SyntaxStatus::TooLarge));
        }
        if !budget.take(sides.iter().flatten().map(|side| side.len()).sum()) {
            return plain(info.with_status(SyntaxStatus::OverBudget));
        }
        Self {
            old: file
                .old_content
                .as_deref()
                .map(|source| highlight(old_path, source)),
            new: file
                .new_content
                .as_deref()
                .map(|source| highlight(new_path, source)),
            info,
        }
    }

    /// Spans for one diff row. `text` carries the row's `+`/`-`/space
    /// prefix, which stays a plain leading span. Removed rows read the base
    /// file, added rows the current file, and context rows the current file
    /// (base when the row has no current-side number), matching the TUI diff
    /// viewer.
    pub(crate) fn line(
        &self,
        kind: &DiffLineKind,
        text: &str,
        old_line: Option<usize>,
        new_line: Option<usize>,
    ) -> SyntaxLine {
        let (side, number) = match kind {
            DiffLineKind::Added => (self.new.as_ref(), new_line),
            DiffLineKind::Removed => (self.old.as_ref(), old_line),
            DiffLineKind::Context if new_line.is_some() && self.new.is_some() => {
                (self.new.as_ref(), new_line)
            }
            DiffLineKind::Context => (self.old.as_ref(), old_line),
            DiffLineKind::NoNewlineMarker => return None,
        };
        let line = side?.lines.get(number?.checked_sub(1)?)?;
        // The prefix is one ASCII byte, so slicing at 1 is a char boundary.
        let (prefix, body) = (text.get(..1)?, text.get(1..)?);
        if !matches!(prefix, "+" | "-" | " ") {
            return None;
        }
        let mut spans = spans_matching(line, body)?;
        match spans.first_mut() {
            Some(first) if first.0.is_empty() => first.1.insert_str(0, prefix),
            _ => spans.insert(0, SyntaxSpan("", prefix.to_string())),
        }
        Some(spans)
    }
}

/// Highlights a whole file shown line by line (the Learning reader's
/// repository view). Returns one entry per line plus the badge.
pub(crate) fn source_lines(path: &Path, lines: &[String]) -> (Vec<SyntaxLine>, SyntaxInfo) {
    let source = lines.join("\n");
    let info = SyntaxInfo::detected(highlight::language_for_source(Some(path), &source));
    let plain = |info| (vec![None; lines.len()], info);
    if info.status != SyntaxStatus::Highlighted {
        return plain(info);
    }
    if lines.len() > MAX_HIGHLIGHT_LINES || source.len() > MAX_HIGHLIGHT_BYTES {
        return plain(info.with_status(SyntaxStatus::TooLarge));
    }
    let highlighted = highlight(path, &source);
    let spans = lines
        .iter()
        .enumerate()
        .map(|(index, text)| {
            highlighted
                .lines
                .get(index)
                .and_then(|line| spans_matching(line, text))
        })
        .collect();
    (spans, info)
}

/// Converts one highlighted line to spans, but only when they reproduce
/// `expected` exactly, so the GUI can never show text that differs from the
/// diff it is colouring. Diff parsing drops a CRLF file's `\r`, which the
/// highlighter keeps, so a trailing `\r` is tolerated. A line with no
/// coloured token returns `None` and renders as its plain text.
fn spans_matching(line: &HighlightedLine, expected: &str) -> SyntaxLine {
    let mut spans: Vec<SyntaxSpan> = Vec::with_capacity(line.spans.len());
    let mut rest = expected;
    for span in &line.spans {
        let piece = span.text.as_str();
        let piece = if let Some(after) = rest.strip_prefix(piece) {
            rest = after;
            piece
        } else if piece.strip_suffix('\r') == Some(rest) {
            let piece = &piece[..piece.len() - 1];
            rest = "";
            piece
        } else {
            return None;
        };
        if piece.is_empty() {
            continue;
        }
        let class = span.class.token_name().unwrap_or("");
        match spans.last_mut() {
            Some(last) if last.0 == class => last.1.push_str(piece),
            _ => spans.push(SyntaxSpan(class, piece.to_string())),
        }
    }
    if !rest.is_empty() || spans.iter().all(|span| span.0.is_empty()) {
        return None;
    }
    Some(spans)
}

/// Progress of the one parser install the GUI process runs at a time. Parser
/// installs are process-wide (the highlight registry is), so this is too.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SyntaxInstallView {
    /// Display title of the language being, or last, installed.
    pub language: Option<String>,
    pub language_key: Option<String>,
    pub running: bool,
    /// The installer's latest progress line (a `git`/`cc` command).
    pub output: Option<String>,
    pub message: Option<String>,
    pub error: Option<String>,
    /// Increments as each install finishes, so a view refreshes once.
    pub completed: u64,
}

type InstallState = Arc<Mutex<SyntaxInstallView>>;

fn install_state() -> &'static InstallState {
    static STATE: OnceLock<InstallState> = OnceLock::new();
    STATE.get_or_init(InstallState::default)
}

pub fn install_status() -> SyntaxInstallView {
    install_state()
        .lock()
        .map(|view| view.clone())
        .unwrap_or_default()
}

/// Installs a language's parser with the TUI picker's own installer (clone
/// the grammar from GitHub, compile it with `cc`) on a background thread.
/// The caller polls [`install_status`]. Refuses a second install while one
/// runs and a parser that is already installed.
pub fn install(language_key: &str) -> GuiResult<SyntaxInstallView> {
    start_install(
        install_state(),
        language_key,
        |language| match language.install_state() {
            // An installed parser that fails to load may be reinstalled.
            HighlightInstallState::Installed if !highlight::parser_loads(language) => {
                HighlightInstallState::Broken
            }
            state => state,
        },
        highlight::install_language,
    )
}

fn start_install<S, I>(
    state: &InstallState,
    language_key: &str,
    install_state: S,
    installer: I,
) -> GuiResult<SyntaxInstallView>
where
    S: Fn(HighlightLanguage) -> HighlightInstallState,
    I: FnOnce(HighlightLanguage, Box<dyn FnMut(String) + Send>) -> anyhow::Result<String>
        + Send
        + 'static,
{
    let language = HighlightLanguage::ALL
        .into_iter()
        .find(|language| language.package_key() == language_key)
        .ok_or_else(|| GuiError::conflict(format!("AMF has no parser for {language_key}")))?;
    let mut view = state.lock().map_err(|_| poisoned())?;
    if view.running {
        return Err(GuiError::conflict(format!(
            "The {} parser is already being installed; wait for it to finish",
            view.language.as_deref().unwrap_or("syntax")
        )));
    }
    if install_state(language) == HighlightInstallState::Installed {
        return Err(GuiError::conflict(format!(
            "The {} parser is already installed; refresh to see highlighting",
            language.picker_title()
        )));
    }
    *view = SyntaxInstallView {
        language: Some(language.picker_title().to_string()),
        language_key: Some(language.package_key().to_string()),
        running: true,
        completed: view.completed,
        ..SyntaxInstallView::default()
    };
    let started = view.clone();
    drop(view);

    let shared = Arc::clone(state);
    std::thread::spawn(move || {
        let progress_state = Arc::clone(&shared);
        let progress = Box::new(move |line: String| {
            if let Ok(mut view) = progress_state.lock() {
                view.output = Some(line);
            }
        });
        let result = installer(language, progress);
        // Drop the loaded registry and cached highlights, as the TUI picker
        // does, so the next projection loads the new parser.
        highlight::reload_runtime_state();
        if let Ok(mut view) = shared.lock() {
            view.running = false;
            view.completed += 1;
            match result {
                Ok(message) => view.message = Some(message),
                Err(error) => view.error = Some(format!("{error:#}")),
            }
        }
    });
    Ok(started)
}

fn poisoned() -> GuiError {
    GuiError::from(anyhow::anyhow!("syntax install state is unavailable"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::highlight::{HighlightedSpan, SyntaxClass};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    fn line(spans: &[(SyntaxClass, &str)]) -> HighlightedLine {
        HighlightedLine {
            spans: spans
                .iter()
                .map(|(class, text)| HighlightedSpan {
                    text: text.to_string(),
                    class: *class,
                })
                .collect(),
        }
    }

    fn text(lines: Vec<HighlightedLine>) -> HighlightedText {
        HighlightedText {
            language_name: Some("rust".into()),
            lines,
        }
    }

    fn span(class: &'static str, text: &str) -> SyntaxSpan {
        SyntaxSpan(class, text.into())
    }

    #[test]
    fn spans_reproduce_the_line_and_name_token_classes() {
        let highlighted = line(&[
            (SyntaxClass::Plain, "    "),
            (SyntaxClass::Keyword, "let"),
            (SyntaxClass::Plain, " x = "),
            (SyntaxClass::String, "\"a\tb\""),
            (SyntaxClass::PunctuationDelimiter, ";"),
        ]);
        assert_eq!(
            spans_matching(&highlighted, "    let x = \"a\tb\";"),
            Some(vec![
                span("", "    "),
                span("keyword", "let"),
                span("", " x = "),
                span("string", "\"a\tb\""),
                span("punctuation-delimiter", ";"),
            ])
        );
    }

    #[test]
    fn mismatched_or_uncoloured_lines_render_plain() {
        let highlighted = line(&[(SyntaxClass::Keyword, "fn"), (SyntaxClass::Plain, " a")]);
        assert_eq!(spans_matching(&highlighted, "fn b"), None);
        assert_eq!(spans_matching(&highlighted, "fn a()"), None);
        assert_eq!(
            spans_matching(&line(&[(SyntaxClass::Plain, "just text")]), "just text"),
            None
        );
        assert_eq!(spans_matching(&HighlightedLine::default(), ""), None);
    }

    #[test]
    fn crlf_lines_drop_the_carriage_return_the_diff_text_lacks() {
        let joined = line(&[(SyntaxClass::Keyword, "fn"), (SyntaxClass::Plain, " a\r")]);
        assert_eq!(
            spans_matching(&joined, "fn a"),
            Some(vec![span("keyword", "fn"), span("", " a")])
        );
        let separate = line(&[(SyntaxClass::Comment, "// x"), (SyntaxClass::Plain, "\r")]);
        assert_eq!(
            spans_matching(&separate, "// x"),
            Some(vec![span("comment", "// x")])
        );
    }

    #[test]
    fn diff_rows_read_the_side_their_kind_belongs_to() {
        let highlights = DiffHighlights {
            old: Some(text(vec![
                line(&[(SyntaxClass::Comment, "/* opened")]),
                line(&[(SyntaxClass::Comment, "old body */")]),
            ])),
            new: Some(text(vec![
                line(&[(SyntaxClass::String, "\"opened")]),
                line(&[(SyntaxClass::String, "new body\"")]),
                line(&[(SyntaxClass::Keyword, "return")]),
            ])),
            info: SyntaxInfo::detected(None),
        };
        // A removed row inside a block comment that opened outside the hunk
        // is still a comment: it reads the base file's line 2.
        assert_eq!(
            highlights.line(&DiffLineKind::Removed, "-old body */", Some(2), None),
            Some(vec![span("", "-"), span("comment", "old body */")])
        );
        assert_eq!(
            highlights.line(&DiffLineKind::Added, "+new body\"", None, Some(2)),
            Some(vec![span("", "+"), span("string", "new body\"")])
        );
        assert_eq!(
            highlights.line(&DiffLineKind::Context, " return", Some(9), Some(3)),
            Some(vec![span("", " "), span("keyword", "return")])
        );
        // A context row without a current-side number falls back to base.
        assert_eq!(
            highlights.line(&DiffLineKind::Context, " /* opened", Some(1), None),
            Some(vec![span("", " "), span("comment", "/* opened")])
        );
        assert_eq!(
            highlights.line(
                &DiffLineKind::NoNewlineMarker,
                "\\ No newline",
                Some(1),
                Some(1)
            ),
            None
        );
        assert_eq!(
            highlights.line(&DiffLineKind::Added, "+beyond", None, Some(40)),
            None
        );
        // Text that is not the file's line (a stale row) is never coloured.
        assert_eq!(
            highlights.line(&DiffLineKind::Added, "+other", None, Some(3)),
            None
        );
    }

    fn file(path: &str, old: Option<&str>, new: Option<&str>) -> DiffFile {
        DiffFile {
            old_path: None,
            path: path.into(),
            status: crate::diff::DiffFileStatus::Modified,
            additions: 0,
            deletions: 0,
            is_binary: false,
            old_content: old.map(String::from),
            new_content: new.map(String::from),
            patch: String::new(),
            hunks: Vec::new(),
        }
    }

    #[test]
    fn badges_explain_why_a_file_stays_plain() {
        let mut budget = HighlightBudget::view();
        // The test sandbox has no parsers installed.
        let rust = DiffHighlights::for_file(&file("src/lib.rs", Some("a"), Some("b")), &mut budget);
        assert_eq!(
            rust.info,
            SyntaxInfo {
                language: Some("Rust".into()),
                language_key: Some("rust".into()),
                status: SyntaxStatus::NotInstalled,
            }
        );
        assert!(rust.old.is_none() && rust.new.is_none());

        let unknown = DiffHighlights::for_file(&file("notes.weird", None, Some("x")), &mut budget);
        assert_eq!(unknown.info.status, SyntaxStatus::Unsupported);
        assert_eq!(unknown.info.language, None);

        // A shebang names the language when the extension does not.
        let script = DiffHighlights::for_file(
            &file(
                "bin/deploy",
                None,
                Some("#!/usr/bin/env python3\nprint(1)\n"),
            ),
            &mut budget,
        );
        assert_eq!(script.info.language.as_deref(), Some("Python"));

        let mut binary = file("logo.png", None, None);
        binary.is_binary = true;
        let binary = DiffHighlights::for_file(&binary, &mut budget);
        assert_eq!(binary.info.status, SyntaxStatus::Binary);

        let (lines, info) = source_lines(
            Path::new("README.md"),
            &["# Title".to_string(), "text".to_string()],
        );
        assert_eq!(info.status, SyntaxStatus::NotInstalled);
        assert_eq!(lines, vec![None, None]);
    }

    #[test]
    fn size_limits_and_the_view_budget_are_enforced() {
        let huge = "x\n".repeat(MAX_HIGHLIGHT_LINES + 1);
        assert!(too_large(&huge));
        assert!(!too_large("fn main() {}\n"));
        assert!(too_large(&"x".repeat(MAX_HIGHLIGHT_BYTES + 1)));

        let mut budget = HighlightBudget { remaining: 10 };
        assert!(budget.take(6));
        assert!(!budget.take(5));
        assert!(budget.take(4));
        assert!(!budget.take(1));
    }

    fn wait_until(state: &InstallState, done: impl Fn(&SyntaxInstallView) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done(&state.lock().unwrap()) {
            assert!(Instant::now() < deadline, "install never finished");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn installs_run_one_at_a_time_and_report_their_result() {
        let state = InstallState::default();
        let (release, gate) = mpsc::channel::<()>();
        let started = start_install(
            &state,
            "python",
            |_| HighlightInstallState::Available,
            move |language, mut progress| {
                assert_eq!(language, HighlightLanguage::Python);
                progress("$ git clone tree-sitter-python".into());
                gate.recv().unwrap();
                Ok("Installed Python tree-sitter parser".into())
            },
        )
        .unwrap();
        assert!(started.running);
        assert_eq!(started.language.as_deref(), Some("Python"));
        wait_until(&state, |view| view.output.is_some());

        let duplicate = start_install(
            &state,
            "rust",
            |_| HighlightInstallState::Available,
            |_, _| unreachable!("a second install must not start"),
        )
        .unwrap_err();
        assert!(
            duplicate
                .message
                .contains("Python parser is already being installed")
        );

        release.send(()).unwrap();
        wait_until(&state, |view| !view.running);
        let done = state.lock().unwrap().clone();
        assert_eq!(done.completed, 1);
        assert_eq!(
            done.message.as_deref(),
            Some("Installed Python tree-sitter parser")
        );
        assert_eq!(done.error, None);

        // A failure is reported and frees the slot for a retry.
        start_install(
            &state,
            "rust",
            |_| HighlightInstallState::Broken,
            |_, _| Err(anyhow::anyhow!("cc: not found")),
        )
        .unwrap();
        wait_until(&state, |view| !view.running);
        let failed = state.lock().unwrap().clone();
        assert_eq!(failed.completed, 2);
        assert_eq!(failed.language.as_deref(), Some("Rust"));
        assert_eq!(failed.error.as_deref(), Some("cc: not found"));
        assert_eq!(failed.message, None);
    }

    #[test]
    fn installed_and_unknown_languages_are_refused() {
        let state = InstallState::default();
        let installed = start_install(
            &state,
            "rust",
            |_| HighlightInstallState::Installed,
            |_, _| unreachable!(),
        )
        .unwrap_err();
        assert!(
            installed
                .message
                .contains("Rust parser is already installed")
        );
        let unknown = start_install(
            &state,
            "cobol",
            |_| HighlightInstallState::Available,
            |_, _| unreachable!(),
        )
        .unwrap_err();
        assert!(unknown.message.contains("no parser for cobol"));
        assert!(!state.lock().unwrap().running);
    }
}
