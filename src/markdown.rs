use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::theme::Theme;

// Keep a note of the alternatives that were evaluated for this feature:
// - tui-markdown: https://docs.rs/tui-markdown/latest/tui_markdown/
// - markdown-reader: https://docs.rs/crate/markdown-reader/latest
// We keep rendering in-house on top of pulldown-cmark so AMF owns
// view-mode keybindings, workflows, and future plan-specific behavior.

const MARKDOWN_VIEW_CANDIDATES: &[&str] = &[
    ".claude/plan.md",
    ".claude/context.md",
    ".claude/review-notes.md",
    ".claude/notes.md",
    "PLAN.md",
    "plan.md",
];

pub fn collect_markdown_view_paths(workdir: &Path) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut paths = Vec::new();

    for candidate in MARKDOWN_VIEW_CANDIDATES
        .iter()
        .map(|candidate| workdir.join(candidate))
    {
        if candidate.is_file() && seen.insert(candidate.clone()) {
            paths.push(candidate);
        }
    }

    for extra_dir in [workdir.to_path_buf(), workdir.join(".claude")] {
        let Ok(entries) = std::fs::read_dir(&extra_dir) else {
            continue;
        };

        let mut extras: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.is_file())
            .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
            .collect();
        extras.sort();

        for path in extras {
            if seen.insert(path.clone()) {
                paths.push(path);
            }
        }
    }

    paths
}

pub fn render_markdown(markdown: &str, theme: &Theme) -> Vec<Line<'static>> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_FOOTNOTES);

    let parser = Parser::new_ext(markdown, options);
    let mut renderer = MarkdownRenderer::new(theme);
    for event in parser {
        renderer.handle(event);
    }
    renderer.finish()
}

#[derive(Clone, Copy)]
enum ListKind {
    Bullet,
    Ordered(u64),
}

struct MarkdownRenderer<'a> {
    theme: &'a Theme,
    lines: Vec<Line<'static>>,
    current: Vec<Span<'static>>,
    inline_styles: Vec<Style>,
    list_stack: Vec<ListKind>,
    blockquote_depth: usize,
    pending_item_prefix: Option<String>,
    in_code_block: bool,
    table_cell_index: usize,
}

impl<'a> MarkdownRenderer<'a> {
    fn new(theme: &'a Theme) -> Self {
        Self {
            theme,
            lines: Vec::new(),
            current: Vec::new(),
            inline_styles: Vec::new(),
            list_stack: Vec::new(),
            blockquote_depth: 0,
            pending_item_prefix: None,
            in_code_block: false,
            table_cell_index: 0,
        }
    }

    fn handle(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start_tag(tag),
            Event::End(tag) => self.end_tag(tag),
            Event::Text(text) => {
                if self.in_code_block {
                    self.push_code_block_text(&text);
                } else {
                    self.push_text(&text);
                }
            }
            Event::Code(code) => {
                self.ensure_prefix();
                self.current.push(Span::styled(
                    code.into_string(),
                    Style::default()
                        .fg(self.theme.warning.to_color())
                        .bg(self.theme.effective_header_bg()),
                ));
            }
            Event::InlineHtml(html) | Event::Html(html) => {
                self.push_text(&html);
            }
            Event::SoftBreak => {
                if self.in_code_block {
                    self.flush_current_line();
                } else {
                    self.push_text(" ");
                }
            }
            Event::HardBreak => {
                self.flush_current_line();
            }
            Event::Rule => {
                self.ensure_blank_line();
                self.lines.push(Line::from(Span::styled(
                    "────────────────────────",
                    Style::default().fg(self.theme.text_muted.to_color()),
                )));
                self.lines.push(Line::raw(""));
            }
            Event::TaskListMarker(checked) => {
                self.ensure_prefix();
                let (marker, color) = if checked {
                    ("[x] ", self.theme.success.to_color())
                } else {
                    ("[ ] ", self.theme.text_muted.to_color())
                };
                self.current.push(Span::styled(
                    marker,
                    Style::default()
                        .fg(color)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            Event::FootnoteReference(label) => {
                self.ensure_prefix();
                self.current.push(Span::styled(
                    format!("[{}]", label),
                    Style::default()
                        .fg(self.theme.info.to_color())
                        .add_modifier(Modifier::ITALIC),
                ));
            }
            Event::InlineMath(math) | Event::DisplayMath(math) => {
                self.ensure_prefix();
                self.current.push(Span::styled(
                    math.into_string(),
                    Style::default().fg(self.theme.secondary.to_color()),
                ));
            }
        }
    }

    fn start_tag(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {}
            Tag::Heading { level, .. } => {
                self.ensure_blank_line();
                let hashes = "#".repeat(heading_level_number(level) as usize);
                self.current.push(Span::styled(
                    format!("{hashes} "),
                    Style::default()
                        .fg(self.theme.primary.to_color())
                        .add_modifier(Modifier::BOLD),
                ));
                self.inline_styles.push(
                    Style::default()
                        .fg(self.theme.primary.to_color())
                        .add_modifier(Modifier::BOLD),
                );
            }
            Tag::BlockQuote(_) => {
                self.ensure_blank_line();
                self.blockquote_depth += 1;
            }
            Tag::CodeBlock(kind) => {
                self.ensure_blank_line();
                self.in_code_block = true;
                let label = match kind {
                    CodeBlockKind::Fenced(lang) if !lang.is_empty() => {
                        format!("```{}", lang.into_string())
                    }
                    _ => "```".to_string(),
                };
                self.lines.push(Line::from(Span::styled(
                    label,
                    Style::default().fg(self.theme.text_muted.to_color()),
                )));
            }
            Tag::List(start) => {
                self.ensure_blank_line();
                let kind = start.map(ListKind::Ordered).unwrap_or(ListKind::Bullet);
                self.list_stack.push(kind);
            }
            Tag::Item => {
                self.flush_current_line();
                self.pending_item_prefix = Some(self.next_list_prefix());
            }
            Tag::Emphasis => {
                self.inline_styles
                    .push(Style::default().add_modifier(Modifier::ITALIC));
            }
            Tag::Strong => {
                self.inline_styles
                    .push(Style::default().add_modifier(Modifier::BOLD));
            }
            Tag::Strikethrough => {
                self.inline_styles
                    .push(Style::default().add_modifier(Modifier::CROSSED_OUT));
            }
            Tag::Link { .. } => {
                self.inline_styles.push(
                    Style::default()
                        .fg(self.theme.info.to_color())
                        .add_modifier(Modifier::UNDERLINED),
                );
            }
            Tag::Image { .. } => {
                self.inline_styles.push(
                    Style::default()
                        .fg(self.theme.secondary.to_color())
                        .add_modifier(Modifier::ITALIC),
                );
            }
            Tag::Table(_) => {
                self.ensure_blank_line();
            }
            Tag::TableHead => {
                self.inline_styles.push(
                    Style::default()
                        .fg(self.theme.primary.to_color())
                        .add_modifier(Modifier::BOLD),
                );
            }
            Tag::TableRow => {
                self.flush_current_line();
                self.table_cell_index = 0;
            }
            Tag::TableCell => {
                if self.table_cell_index > 0 {
                    self.push_text(" | ");
                }
                self.table_cell_index += 1;
            }
            _ => {}
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush_current_line();
                self.lines.push(Line::raw(""));
            }
            TagEnd::Heading(_) => {
                self.flush_current_line();
                self.lines.push(Line::raw(""));
                self.inline_styles.pop();
            }
            TagEnd::BlockQuote(_) => {
                self.flush_current_line();
                self.lines.push(Line::raw(""));
                self.blockquote_depth = self.blockquote_depth.saturating_sub(1);
            }
            TagEnd::CodeBlock => {
                self.flush_current_line();
                self.lines.push(Line::from(Span::styled(
                    "```",
                    Style::default().fg(self.theme.text_muted.to_color()),
                )));
                self.lines.push(Line::raw(""));
                self.in_code_block = false;
            }
            TagEnd::List(_) => {
                self.flush_current_line();
                self.list_stack.pop();
                self.lines.push(Line::raw(""));
            }
            TagEnd::Item => {
                self.flush_current_line();
            }
            TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Link
            | TagEnd::Image
            | TagEnd::TableHead => {
                self.inline_styles.pop();
            }
            TagEnd::Table => {
                self.flush_current_line();
                self.lines.push(Line::raw(""));
            }
            TagEnd::TableRow => {
                self.flush_current_line();
            }
            TagEnd::TableCell => {}
            _ => {}
        }
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        self.flush_current_line();
        while matches!(self.lines.last(), Some(line) if line.spans.is_empty()) {
            self.lines.pop();
        }
        if self.lines.is_empty() {
            self.lines.push(Line::raw("(empty markdown file)"));
        }
        self.lines
    }

    fn push_text(&mut self, text: &str) {
        for (idx, part) in text.split('\n').enumerate() {
            if idx > 0 {
                self.flush_current_line();
            }
            if part.is_empty() {
                continue;
            }
            self.ensure_prefix();
            self.current
                .push(Span::styled(part.to_string(), self.current_style()));
        }
    }

    fn push_code_block_text(&mut self, text: &str) {
        for part in text.split('\n') {
            if !part.is_empty() {
                self.current.push(Span::styled(
                    format!("  {}", part),
                    Style::default().fg(self.theme.warning.to_color()),
                ));
            }
            self.flush_current_line();
        }
    }

    fn current_style(&self) -> Style {
        let mut style = Style::default().fg(self.theme.text.to_color());
        for extra in &self.inline_styles {
            style = style.patch(*extra);
        }
        style
    }

    fn next_list_prefix(&mut self) -> String {
        let indent = "  ".repeat(self.list_stack.len().saturating_sub(1));
        let marker = match self.list_stack.last_mut() {
            Some(ListKind::Ordered(next)) => {
                let marker = format!("{next}. ");
                *next += 1;
                marker
            }
            _ => "- ".to_string(),
        };
        format!("{indent}{marker}")
    }

    fn ensure_prefix(&mut self) {
        if !self.current.is_empty() {
            return;
        }

        for _ in 0..self.blockquote_depth {
            self.current.push(Span::styled(
                "│ ",
                Style::default().fg(self.theme.secondary.to_color()),
            ));
        }

        if let Some(prefix) = self.pending_item_prefix.take() {
            self.current.push(Span::styled(
                prefix,
                Style::default()
                    .fg(self.theme.warning.to_color())
                    .add_modifier(Modifier::BOLD),
            ));
        }
    }

    fn flush_current_line(&mut self) {
        if self.current.is_empty() {
            return;
        }
        self.lines.push(Line::from(std::mem::take(&mut self.current)));
    }

    fn ensure_blank_line(&mut self) {
        self.flush_current_line();
        if !matches!(self.lines.last(), Some(line) if line.spans.is_empty()) && !self.lines.is_empty()
        {
            self.lines.push(Line::raw(""));
        }
    }
}

fn heading_level_number(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_path_prefers_plan_then_context() {
        let dir = tempfile::TempDir::new().unwrap();
        let claude_dir = dir.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::write(claude_dir.join("context.md"), "# Context").unwrap();
        std::fs::write(claude_dir.join("plan.md"), "# Plan").unwrap();

        let found = collect_markdown_view_paths(dir.path());
        assert_eq!(found.first(), Some(&claude_dir.join("plan.md")));
    }

    #[test]
    fn markdown_path_collection_includes_other_markdown_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let claude_dir = dir.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::write(claude_dir.join("notes.md"), "# Notes").unwrap();
        std::fs::write(dir.path().join("RETRO.md"), "# Retro").unwrap();

        let found = collect_markdown_view_paths(dir.path());
        assert!(found.contains(&claude_dir.join("notes.md")));
        assert!(found.contains(&dir.path().join("RETRO.md")));
    }

    #[test]
    fn render_markdown_keeps_headings_and_lists() {
        let theme = Theme::default();
        let lines = render_markdown("# Title\n\n1. Ship it\n2. Test it", &theme);
        let rendered = lines
            .iter()
            .flat_map(|line| line.spans.iter().map(|span| span.content.as_ref()))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("# "));
        assert!(rendered.contains("Title"));
        assert!(rendered.contains("1. "));
        assert!(rendered.contains("Ship it"));
    }

    #[test]
    fn render_markdown_keeps_task_lists_and_code_blocks() {
        let theme = Theme::default();
        let lines = render_markdown("- [x] Done\n\n```rust\nfn main() {}\n```", &theme);
        let rendered = lines
            .iter()
            .flat_map(|line| line.spans.iter().map(|span| span.content.as_ref()))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("[x]"));
        assert!(rendered.contains("```rust"));
        assert!(rendered.contains("fn main() {}"));
    }
}
