use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    highlight::{HighlightRequest, highlight_source, style_for_class},
    theme::Theme,
};

const MARKDOWN_VIEW_CANDIDATES: &[&str] = &[
    ".claude/plan.md",
    ".claude/context.md",
    ".claude/review-notes.md",
    ".claude/notes.md",
    "PLAN.md",
    "plan.md",
];
const SIDEBAR_PLAN_CANDIDATES: &[&str] = &[".claude/plan.md", "PLAN.md", "plan.md"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkdownViewScope {
    Worktree,
    RepoRoot,
    Other,
}

#[derive(Clone)]
pub struct RenderedMarkdown {
    pub lines: Vec<Line<'static>>,
}

#[derive(Clone, Copy)]
enum ListKind {
    Bullet,
    Ordered(u64),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EmittedBlockKind {
    Paragraph,
    ListItem,
    Heading,
    CodeBlock,
    Table,
    Rule,
}

#[derive(Clone)]
struct Prefix {
    spans: Vec<Span<'static>>,
    width: usize,
}

#[derive(Clone)]
enum InlineNode {
    Text {
        text: String,
        style: Style,
        atomic: bool,
    },
    Break,
}

struct TextBlock {
    kind: TextBlockKind,
    nodes: Vec<InlineNode>,
    first_prefix: Prefix,
    rest_prefix: Prefix,
}

enum TextBlockKind {
    Paragraph { list_item: bool },
    Heading { level: u8 },
}

struct CodeBlockState {
    language: Option<String>,
    code: String,
    prefix: Prefix,
}

struct TableState {
    alignments: Vec<Alignment>,
    rows: Vec<Vec<String>>,
    current_row: Vec<String>,
    current_cell: String,
    header_rows: usize,
    prefix: Prefix,
}

impl TableState {
    fn new(alignments: Vec<Alignment>, prefix: Prefix) -> Self {
        Self {
            alignments,
            rows: Vec::new(),
            current_row: Vec::new(),
            current_cell: String::new(),
            header_rows: 0,
            prefix,
        }
    }

    fn start_cell(&mut self) {
        self.current_cell.clear();
    }

    fn push_cell_text(&mut self, text: &str) {
        self.current_cell.push_str(text);
    }

    fn finish_cell(&mut self) {
        self.current_row.push(self.current_cell.trim().to_string());
        self.current_cell.clear();
    }

    fn start_row(&mut self) {
        self.current_row.clear();
    }

    fn finish_row(&mut self) {
        self.rows.push(std::mem::take(&mut self.current_row));
    }
}

pub fn collect_markdown_view_paths(workdir: &Path, repo_root: Option<&Path>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut paths = Vec::new();

    collect_markdown_paths_in_root(workdir, &mut seen, &mut paths);

    if let Some(repo_root) = repo_root
        && repo_root != workdir
    {
        collect_markdown_paths_in_root(repo_root, &mut seen, &mut paths);
    }

    paths
}

pub fn preferred_plan_markdown_path(workdir: &Path, repo_root: Option<&Path>) -> Option<PathBuf> {
    for candidate in SIDEBAR_PLAN_CANDIDATES {
        let path = workdir.join(candidate);
        if path.is_file() {
            return Some(path);
        }
    }

    if let Some(repo_root) = repo_root
        && repo_root != workdir
    {
        for candidate in SIDEBAR_PLAN_CANDIDATES {
            let path = repo_root.join(candidate);
            if path.is_file() {
                return Some(path);
            }
        }
    }

    None
}

pub fn read_plan_preview(workdir: &Path, repo_root: Option<&Path>) -> Option<String> {
    let path = preferred_plan_markdown_path(workdir, repo_root)?;
    let content = std::fs::read_to_string(path).ok()?;
    compact_plan_preview(&content)
}

fn compact_plan_preview(content: &str) -> Option<String> {
    const MAX_LINES: usize = 6;
    const MAX_CHARS: usize = 280;

    let mut lines = Vec::new();
    let mut used_chars = 0;

    for raw_line in content.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() || trimmed == "---" {
            continue;
        }

        let normalized = trimmed
            .trim_start_matches('#')
            .trim()
            .trim_end_matches('\r');
        if normalized.is_empty() {
            continue;
        }

        let remaining = MAX_CHARS.saturating_sub(used_chars);
        if remaining == 0 {
            break;
        }

        let normalized_chars = normalized.chars().count();
        let mut line = normalized.to_string();
        if normalized_chars > remaining {
            line = normalized
                .chars()
                .take(remaining.saturating_sub(1))
                .collect::<String>();
            line.push('…');
        }

        used_chars += line.chars().count();
        lines.push(line);

        if lines.len() >= MAX_LINES || normalized_chars > remaining {
            break;
        }
    }

    if lines.is_empty() {
        return None;
    }

    let has_more_content = content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && *line != "---")
        .count()
        > lines.len();
    if has_more_content
        && let Some(last) = lines.last_mut()
        && !last.ends_with('…')
    {
        last.push('…');
    }

    Some(lines.join("\n"))
}

fn collect_markdown_paths_in_root(
    root: &Path,
    seen: &mut HashSet<PathBuf>,
    paths: &mut Vec<PathBuf>,
) {
    for candidate in MARKDOWN_VIEW_CANDIDATES
        .iter()
        .map(|candidate| root.join(candidate))
    {
        if candidate.is_file() && seen.insert(candidate.clone()) {
            paths.push(candidate);
        }
    }

    let mut extras = Vec::new();
    collect_markdown_paths_recursive(root, root, &mut extras);
    extras.sort();

    for path in extras {
        if seen.insert(path.clone()) {
            paths.push(path);
        }
    }
}

fn collect_markdown_paths_recursive(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.filter_map(|entry| entry.ok()) {
        let path = entry.path();

        if path.is_dir() {
            if should_skip_markdown_dir(root, &path) {
                continue;
            }
            collect_markdown_paths_recursive(root, &path, out);
            continue;
        }

        if path.is_file() && path.extension().is_some_and(|ext| ext == "md") {
            out.push(path);
        }
    }
}

fn should_skip_markdown_dir(root: &Path, path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };

    if matches!(name, ".git" | "target" | ".worktrees") {
        return true;
    }

    if name.starts_with('.') && path != root.join(".claude") {
        return true;
    }

    false
}

pub fn markdown_view_label(path: &Path, workdir: &Path, repo_root: Option<&Path>) -> String {
    match markdown_view_scope(path, workdir, repo_root) {
        MarkdownViewScope::Worktree => {
            let relative = path.strip_prefix(workdir).unwrap_or(path);
            if repo_root.is_some() {
                format!("[worktree] {}", relative.display())
            } else {
                relative.display().to_string()
            }
        }
        MarkdownViewScope::RepoRoot => {
            let relative = repo_root
                .and_then(|root| path.strip_prefix(root).ok())
                .unwrap_or(path);
            format!("[repo root] {}", relative.display())
        }
        MarkdownViewScope::Other => path.display().to_string(),
    }
}

pub fn markdown_view_scope(
    path: &Path,
    workdir: &Path,
    repo_root: Option<&Path>,
) -> MarkdownViewScope {
    if path.strip_prefix(workdir).is_ok() {
        return MarkdownViewScope::Worktree;
    }

    if let Some(repo_root) = repo_root
        && path.strip_prefix(repo_root).is_ok()
    {
        return MarkdownViewScope::RepoRoot;
    }

    MarkdownViewScope::Other
}

pub fn markdown_view_relative_label(
    path: &Path,
    workdir: &Path,
    repo_root: Option<&Path>,
) -> String {
    if let Ok(relative) = path.strip_prefix(workdir) {
        return relative.display().to_string();
    }

    if let Some(repo_root) = repo_root
        && let Ok(relative) = path.strip_prefix(repo_root)
    {
        return relative.display().to_string();
    }

    path.display().to_string()
}

pub fn render_markdown(
    markdown: &str,
    theme: &Theme,
    width: usize,
    source_path: Option<&Path>,
) -> RenderedMarkdown {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_FOOTNOTES);

    let parser = Parser::new_ext(markdown, options);
    let mut renderer = MarkdownRenderer::new(theme, width.max(24), source_path);
    for event in parser {
        renderer.handle(event);
    }
    RenderedMarkdown {
        lines: renderer.finish(),
    }
}

struct MarkdownRenderer<'a> {
    theme: &'a Theme,
    width: usize,
    source_path: Option<&'a Path>,
    lines: Vec<Line<'static>>,
    inline_styles: Vec<Style>,
    list_stack: Vec<ListKind>,
    blockquote_depth: usize,
    current_text: Option<TextBlock>,
    current_code: Option<CodeBlockState>,
    current_table: Option<TableState>,
    current_item_prefix: Option<(Prefix, Prefix)>,
    last_block_kind: Option<EmittedBlockKind>,
    in_table_head: bool,
}

impl<'a> MarkdownRenderer<'a> {
    fn new(theme: &'a Theme, width: usize, source_path: Option<&'a Path>) -> Self {
        Self {
            theme,
            width,
            source_path,
            lines: Vec::new(),
            inline_styles: Vec::new(),
            list_stack: Vec::new(),
            blockquote_depth: 0,
            current_text: None,
            current_code: None,
            current_table: None,
            current_item_prefix: None,
            last_block_kind: None,
            in_table_head: false,
        }
    }

    fn handle(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start_tag(tag),
            Event::End(tag) => self.end_tag(tag),
            Event::Text(text) => self.push_text(&text),
            Event::Code(code) => self.push_inline_code(&code),
            Event::InlineHtml(html) | Event::Html(html) => self.push_text(&html),
            Event::SoftBreak => self.push_soft_break(),
            Event::HardBreak => self.push_hard_break(),
            Event::Rule => self.render_rule(),
            Event::TaskListMarker(checked) => self.push_task_marker(checked),
            Event::FootnoteReference(label) => {
                self.push_inline_text(
                    format!("[{}]", label),
                    Style::default()
                        .fg(self.theme.info.to_color())
                        .add_modifier(Modifier::ITALIC),
                    true,
                );
            }
            Event::InlineMath(math) | Event::DisplayMath(math) => {
                self.push_inline_text(
                    math.into_string(),
                    Style::default().fg(self.theme.secondary.to_color()),
                    true,
                );
            }
        }
    }

    fn start_tag(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                self.finish_text_block();
                let (first_prefix, rest_prefix, list_item) = self.current_block_prefixes();
                self.current_text = Some(TextBlock {
                    kind: TextBlockKind::Paragraph { list_item },
                    nodes: Vec::new(),
                    first_prefix,
                    rest_prefix,
                });
            }
            Tag::Heading { level, .. } => {
                self.finish_text_block();
                let prefix = self.quote_prefix();
                self.current_text = Some(TextBlock {
                    kind: TextBlockKind::Heading {
                        level: heading_level_number(level),
                    },
                    nodes: Vec::new(),
                    first_prefix: prefix.clone(),
                    rest_prefix: prefix,
                });
            }
            Tag::BlockQuote(_) => {
                self.finish_text_block();
                self.blockquote_depth += 1;
            }
            Tag::CodeBlock(kind) => {
                self.finish_text_block();
                let prefix = self.continuation_prefix();
                let language = match kind {
                    CodeBlockKind::Fenced(lang) => {
                        let lang = lang.trim();
                        (!lang.is_empty()).then(|| lang.to_string())
                    }
                    CodeBlockKind::Indented => None,
                };
                self.current_code = Some(CodeBlockState {
                    language,
                    code: String::new(),
                    prefix,
                });
            }
            Tag::List(start) => {
                self.finish_text_block();
                let kind = start.map(ListKind::Ordered).unwrap_or(ListKind::Bullet);
                self.list_stack.push(kind);
            }
            Tag::Item => {
                self.finish_text_block();
                self.current_item_prefix = Some(self.next_item_prefixes());
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
            Tag::Table(alignments) => {
                self.finish_text_block();
                self.current_table = Some(TableState::new(
                    alignments.to_vec(),
                    self.continuation_prefix(),
                ));
            }
            Tag::TableHead => {
                self.in_table_head = true;
            }
            Tag::TableRow => {
                if let Some(table) = &mut self.current_table {
                    table.start_row();
                }
            }
            Tag::TableCell => {
                if let Some(table) = &mut self.current_table {
                    table.start_cell();
                }
            }
            _ => {}
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::Heading(_) => self.finish_text_block(),
            TagEnd::BlockQuote(_) => {
                self.finish_text_block();
                self.blockquote_depth = self.blockquote_depth.saturating_sub(1);
            }
            TagEnd::CodeBlock => self.finish_code_block(),
            TagEnd::List(_) => {
                self.finish_text_block();
                self.list_stack.pop();
            }
            TagEnd::Item => {
                self.finish_text_block();
                self.current_item_prefix = None;
            }
            TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Link
            | TagEnd::Image => {
                self.inline_styles.pop();
            }
            TagEnd::Table => self.finish_table(),
            TagEnd::TableHead => {
                if let Some(table) = &mut self.current_table {
                    table.header_rows = table.rows.len();
                }
                self.in_table_head = false;
            }
            TagEnd::TableRow => {
                if let Some(table) = &mut self.current_table {
                    table.finish_row();
                }
            }
            TagEnd::TableCell => {
                if let Some(table) = &mut self.current_table {
                    table.finish_cell();
                }
            }
            _ => {}
        }
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        self.finish_text_block();
        self.finish_code_block();
        self.finish_table();
        while matches!(self.lines.last(), Some(line) if line.spans.is_empty()) {
            self.lines.pop();
        }
        if self.lines.is_empty() {
            self.lines.push(Line::from(Span::styled(
                "(empty markdown file)",
                Style::default().fg(self.theme.text_muted.to_color()),
            )));
        }
        self.lines
    }

    fn push_text(&mut self, text: &str) {
        if let Some(code) = &mut self.current_code {
            code.code.push_str(text);
            return;
        }

        if let Some(table) = &mut self.current_table {
            table.push_cell_text(text);
            return;
        }

        if self.current_text.is_none() {
            let (first_prefix, rest_prefix, list_item) = self.current_block_prefixes();
            self.current_text = Some(TextBlock {
                kind: TextBlockKind::Paragraph { list_item },
                nodes: Vec::new(),
                first_prefix,
                rest_prefix,
            });
        }

        self.push_inline_text(text.to_string(), self.current_style(), false);
    }

    fn push_inline_code(&mut self, code: &str) {
        if let Some(table) = &mut self.current_table {
            table.push_cell_text(code);
            return;
        }

        self.push_inline_text(code.to_string(), self.inline_code_style(), true);
    }

    fn push_soft_break(&mut self) {
        if let Some(code) = &mut self.current_code {
            code.code.push('\n');
        } else {
            self.push_inline_text(" ".to_string(), self.current_style(), false);
        }
    }

    fn push_hard_break(&mut self) {
        if let Some(code) = &mut self.current_code {
            code.code.push('\n');
        } else if let Some(block) = &mut self.current_text {
            block.nodes.push(InlineNode::Break);
        }
    }

    fn push_task_marker(&mut self, checked: bool) {
        let (symbol, color) = if checked {
            ("☑", self.theme.success.to_color())
        } else {
            ("☐", self.theme.text_muted.to_color())
        };
        self.push_inline_text(
            format!("{symbol} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
            true,
        );
    }

    fn push_inline_text(&mut self, text: String, style: Style, atomic: bool) {
        if text.is_empty() {
            return;
        }
        if let Some(block) = &mut self.current_text {
            block.nodes.push(InlineNode::Text {
                text,
                style,
                atomic,
            });
        }
    }

    fn current_style(&self) -> Style {
        let mut style = Style::default().fg(self.theme.text.to_color());
        for extra in &self.inline_styles {
            style = style.patch(*extra);
        }
        style
    }

    fn inline_code_style(&self) -> Style {
        Style::default()
            .fg(self.theme.warning.to_color())
            .bg(self.theme.effective_header_bg())
    }

    fn finish_text_block(&mut self) {
        let Some(block) = self.current_text.take() else {
            return;
        };

        match block.kind {
            TextBlockKind::Paragraph { list_item } => {
                self.begin_block(if list_item {
                    EmittedBlockKind::ListItem
                } else {
                    EmittedBlockKind::Paragraph
                });
                let rendered = wrap_inline_nodes(
                    &block.nodes,
                    self.width,
                    &block.first_prefix,
                    &block.rest_prefix,
                );
                self.lines.extend(rendered);
            }
            TextBlockKind::Heading { level } => {
                self.begin_block(EmittedBlockKind::Heading);
                let style = heading_style(level, self.theme);
                let heading_nodes = block
                    .nodes
                    .into_iter()
                    .map(|node| match node {
                        InlineNode::Text {
                            text,
                            style: base,
                            atomic,
                        } => InlineNode::Text {
                            text,
                            style: base.patch(style),
                            atomic,
                        },
                        InlineNode::Break => InlineNode::Break,
                    })
                    .collect::<Vec<_>>();
                let rendered = wrap_inline_nodes(
                    &heading_nodes,
                    self.width,
                    &block.first_prefix,
                    &block.rest_prefix,
                );
                let underline_char = if level <= 2 { '━' } else { '─' };
                let underline_width = plain_inline_width(&heading_nodes)
                    .min(self.width.saturating_sub(block.first_prefix.width))
                    .max(6);

                self.lines.extend(rendered);
                if level <= 3 {
                    let mut spans = block.first_prefix.spans.clone();
                    spans.push(Span::styled(
                        underline_char.to_string().repeat(underline_width),
                        Style::default().fg(self.theme.primary.to_color()),
                    ));
                    self.lines.push(Line::from(spans));
                }
            }
        }
    }

    fn finish_code_block(&mut self) {
        let Some(code) = self.current_code.take() else {
            return;
        };

        self.begin_block(EmittedBlockKind::CodeBlock);

        let frame_style = Style::default().fg(self.theme.border_focus.to_color());
        let label_style = Style::default()
            .fg(self.theme.secondary.to_color())
            .add_modifier(Modifier::BOLD);
        let code_bg = self.theme.effective_header_bg();

        let available = self.width.saturating_sub(code.prefix.width).max(10);
        let label = code.language.as_deref().unwrap_or("text");
        let top = format_box_bar("╭─", label, available, '─');
        let bottom = format_box_bar("╰─", "", available, '─');

        self.lines.push(prefix_line(
            &code.prefix,
            vec![Span::styled(top, frame_style.patch(label_style))],
        ));

        let highlighted = highlight_source(HighlightRequest {
            path: None,
            language_hint: code.language.as_deref(),
            source: &code.code,
        });

        let first_line_prefix =
            combine_prefixes(&code.prefix, &styled_prefix("│ ".to_string(), frame_style));
        let rest_line_prefix =
            combine_prefixes(&code.prefix, &styled_prefix("│ ".to_string(), frame_style));

        for highlighted_line in highlighted.lines {
            let mut nodes = Vec::new();
            if highlighted_line.spans.is_empty() {
                nodes.push(InlineNode::Text {
                    text: " ".to_string(),
                    style: Style::default().bg(code_bg),
                    atomic: true,
                });
            } else {
                for span in highlighted_line.spans {
                    nodes.push(InlineNode::Text {
                        text: span.text,
                        style: style_for_class(span.class, self.theme).bg(code_bg),
                        atomic: true,
                    });
                }
            }
            let rendered =
                wrap_inline_nodes(&nodes, self.width, &first_line_prefix, &rest_line_prefix);
            self.lines.extend(rendered);
        }

        self.lines.push(prefix_line(
            &code.prefix,
            vec![Span::styled(bottom, frame_style)],
        ));
    }

    fn finish_table(&mut self) {
        let Some(table) = self.current_table.take() else {
            return;
        };
        if table.rows.is_empty() {
            return;
        }

        self.begin_block(EmittedBlockKind::Table);

        let cols = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        if cols == 0 {
            return;
        }

        let mut rows = table.rows;
        for row in &mut rows {
            while row.len() < cols {
                row.push(String::new());
            }
        }

        let mut widths = vec![3usize; cols];
        for row in &rows {
            for (idx, cell) in row.iter().enumerate() {
                widths[idx] = widths[idx].max(display_width(cell).max(1));
            }
        }

        let border_overhead = cols * 3 + 1;
        let available = self
            .width
            .saturating_sub(table.prefix.width)
            .saturating_sub(border_overhead)
            .max(cols * 3);

        while widths.iter().sum::<usize>() > available {
            if let Some((idx, _)) = widths
                .iter()
                .enumerate()
                .filter(|(_, width)| **width > 3)
                .max_by_key(|(_, width)| **width)
            {
                widths[idx] -= 1;
            } else {
                break;
            }
        }

        let top = table_border('┌', '┬', '┐', &widths);
        let mid = table_border('├', '┼', '┤', &widths);
        let bottom = table_border('└', '┴', '┘', &widths);
        let border_style = Style::default().fg(self.theme.border.to_color());

        self.lines.push(prefix_line(
            &table.prefix,
            vec![Span::styled(top, border_style)],
        ));

        for (row_idx, row) in rows.iter().enumerate() {
            let mut line_spans = table.prefix.spans.clone();
            line_spans.push(Span::styled("│", border_style));
            for (col_idx, cell) in row.iter().enumerate() {
                let padded = pad_aligned(
                    &truncate_to_width(cell, widths[col_idx]),
                    widths[col_idx],
                    table
                        .alignments
                        .get(col_idx)
                        .copied()
                        .unwrap_or(Alignment::None),
                );
                let cell_style = if row_idx < table.header_rows {
                    Style::default()
                        .fg(self.theme.primary.to_color())
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(self.theme.text.to_color())
                };
                line_spans.push(Span::raw(" "));
                line_spans.push(Span::styled(padded, cell_style));
                line_spans.push(Span::raw(" "));
                line_spans.push(Span::styled("│", border_style));
            }
            self.lines.push(Line::from(line_spans));

            if row_idx + 1 == table.header_rows && row_idx + 1 < rows.len() {
                self.lines.push(prefix_line(
                    &table.prefix,
                    vec![Span::styled(mid.clone(), border_style)],
                ));
            }
        }

        self.lines.push(prefix_line(
            &table.prefix,
            vec![Span::styled(bottom, border_style)],
        ));
    }

    fn render_rule(&mut self) {
        self.finish_text_block();
        self.finish_code_block();
        self.finish_table();
        self.begin_block(EmittedBlockKind::Rule);
        let prefix = self.quote_prefix();
        let width = self.width.saturating_sub(prefix.width).max(8);
        self.lines.push(prefix_line(
            &prefix,
            vec![Span::styled(
                "─".repeat(width),
                Style::default().fg(self.theme.text_muted.to_color()),
            )],
        ));
    }

    fn begin_block(&mut self, kind: EmittedBlockKind) {
        if !self.lines.is_empty()
            && !matches!(self.lines.last(), Some(line) if line.spans.is_empty())
            && !(self.last_block_kind == Some(EmittedBlockKind::ListItem)
                && kind == EmittedBlockKind::ListItem)
        {
            self.lines.push(Line::raw(""));
        }
        self.last_block_kind = Some(kind);
    }

    fn quote_prefix(&self) -> Prefix {
        let gutter_style = Style::default().fg(self.theme.secondary.to_color());
        let mut spans = Vec::new();
        for _ in 0..self.blockquote_depth {
            spans.push(Span::styled("│ ", gutter_style));
        }
        Prefix {
            width: self.blockquote_depth * 2,
            spans,
        }
    }

    fn continuation_prefix(&self) -> Prefix {
        if let Some((_, rest)) = &self.current_item_prefix {
            return combine_prefixes(&self.quote_prefix(), rest);
        }
        self.quote_prefix()
    }

    fn current_block_prefixes(&self) -> (Prefix, Prefix, bool) {
        if let Some((first, rest)) = &self.current_item_prefix {
            return (
                combine_prefixes(&self.quote_prefix(), first),
                combine_prefixes(&self.quote_prefix(), rest),
                true,
            );
        }
        let prefix = self.quote_prefix();
        (prefix.clone(), prefix, false)
    }

    fn next_item_prefixes(&mut self) -> (Prefix, Prefix) {
        let indent = "  ".repeat(self.list_stack.len().saturating_sub(1));
        let marker = match self.list_stack.last_mut() {
            Some(ListKind::Ordered(next)) => {
                let current = *next;
                *next += 1;
                format!("{current}. ")
            }
            _ => "• ".to_string(),
        };
        let marker_width = display_width(&marker);
        let bullet_style = Style::default()
            .fg(self.theme.warning.to_color())
            .add_modifier(Modifier::BOLD);
        let base_indent = raw_prefix(indent.clone());
        let first = combine_prefixes(&base_indent, &styled_prefix(marker, bullet_style));
        let rest = combine_prefixes(&base_indent, &raw_prefix(" ".repeat(marker_width)));
        (first, rest)
    }
}

fn wrap_inline_nodes(
    nodes: &[InlineNode],
    width: usize,
    first_prefix: &Prefix,
    rest_prefix: &Prefix,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut current = first_prefix.spans.clone();
    let mut current_width = first_prefix.width;
    let mut line_has_content = false;
    let mut pending_space = false;
    let mut active_prefix = first_prefix.clone();

    let flush_line = |lines: &mut Vec<Line<'static>>,
                      current: &mut Vec<Span<'static>>,
                      current_width: &mut usize,
                      active_prefix: &mut Prefix,
                      line_has_content: &mut bool,
                      pending_space: &mut bool| {
        lines.push(Line::from(std::mem::take(current)));
        *active_prefix = rest_prefix.clone();
        *current = active_prefix.spans.clone();
        *current_width = active_prefix.width;
        *line_has_content = false;
        *pending_space = false;
    };

    for node in nodes {
        match node {
            InlineNode::Break => {
                flush_line(
                    &mut lines,
                    &mut current,
                    &mut current_width,
                    &mut active_prefix,
                    &mut line_has_content,
                    &mut pending_space,
                );
            }
            InlineNode::Text {
                text,
                style,
                atomic,
            } => {
                if *atomic {
                    push_wrapped_token(
                        text,
                        *style,
                        width,
                        &mut lines,
                        &mut current,
                        &mut current_width,
                        &mut active_prefix,
                        &mut line_has_content,
                        &mut pending_space,
                        rest_prefix,
                    );
                    continue;
                }

                for token in tokenize_text(text) {
                    match token {
                        WrapToken::Space => {
                            pending_space = line_has_content;
                        }
                        WrapToken::Word(word) => {
                            push_wrapped_token(
                                &word,
                                *style,
                                width,
                                &mut lines,
                                &mut current,
                                &mut current_width,
                                &mut active_prefix,
                                &mut line_has_content,
                                &mut pending_space,
                                rest_prefix,
                            );
                        }
                    }
                }
            }
        }
    }

    if line_has_content || lines.is_empty() {
        lines.push(Line::from(current));
    }

    lines
}

fn push_wrapped_token(
    token: &str,
    style: Style,
    width: usize,
    lines: &mut Vec<Line<'static>>,
    current: &mut Vec<Span<'static>>,
    current_width: &mut usize,
    active_prefix: &mut Prefix,
    line_has_content: &mut bool,
    pending_space: &mut bool,
    rest_prefix: &Prefix,
) {
    let mut remaining = token.to_string();
    while !remaining.is_empty() {
        let available_width = width.saturating_sub(*current_width).max(1);
        let leading_space = if *pending_space && *line_has_content {
            1
        } else {
            0
        };
        let token_width = display_width(&remaining);

        if *line_has_content && leading_space + token_width > available_width {
            lines.push(Line::from(std::mem::take(current)));
            *active_prefix = rest_prefix.clone();
            *current = active_prefix.spans.clone();
            *current_width = active_prefix.width;
            *line_has_content = false;
            *pending_space = false;
            continue;
        }

        if *pending_space && *line_has_content {
            current.push(Span::raw(" "));
            *current_width += 1;
        }

        let available_width = width.saturating_sub(*current_width).max(1);
        if token_width <= available_width {
            current.push(Span::styled(remaining.clone(), style));
            *current_width += token_width;
            *line_has_content = true;
            *pending_space = false;
            break;
        }

        let split_at = split_at_width(&remaining, available_width);
        let head = remaining[..split_at].to_string();
        let tail = remaining[split_at..].to_string();

        current.push(Span::styled(head, style));
        *current_width = width;
        *line_has_content = true;
        *pending_space = false;
        lines.push(Line::from(std::mem::take(current)));
        *active_prefix = rest_prefix.clone();
        *current = active_prefix.spans.clone();
        *current_width = active_prefix.width;
        *line_has_content = false;
        remaining = tail;
    }
}

enum WrapToken {
    Space,
    Word(String),
}

fn tokenize_text(text: &str) -> Vec<WrapToken> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_space = false;

    for ch in text.chars() {
        if ch.is_whitespace() {
            if !current.is_empty() {
                if in_space {
                    tokens.push(WrapToken::Space);
                } else {
                    tokens.push(WrapToken::Word(std::mem::take(&mut current)));
                }
            }
            current.clear();
            in_space = true;
            current.push(ch);
        } else {
            if !current.is_empty() && in_space {
                tokens.push(WrapToken::Space);
                current.clear();
            }
            in_space = false;
            current.push(ch);
        }
    }

    if !current.is_empty() {
        if in_space {
            tokens.push(WrapToken::Space);
        } else {
            tokens.push(WrapToken::Word(current));
        }
    }

    tokens
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

fn heading_style(level: u8, theme: &Theme) -> Style {
    match level {
        1 => Style::default()
            .fg(theme.primary.to_color())
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        2 => Style::default()
            .fg(theme.primary.to_color())
            .add_modifier(Modifier::BOLD),
        3 => Style::default()
            .fg(theme.secondary.to_color())
            .add_modifier(Modifier::BOLD),
        _ => Style::default()
            .fg(theme.text.to_color())
            .add_modifier(Modifier::BOLD),
    }
}

fn format_box_bar(left: &str, label: &str, width: usize, fill: char) -> String {
    let label_text = if label.is_empty() {
        String::new()
    } else {
        format!(" {label} ")
    };
    let label_width = display_width(left) + display_width(&label_text);
    let fill_width = width.saturating_sub(label_width);
    format!("{left}{label_text}{}", fill.to_string().repeat(fill_width))
}

fn table_border(left: char, mid: char, right: char, widths: &[usize]) -> String {
    let mut out = String::new();
    out.push(left);
    for (idx, width) in widths.iter().enumerate() {
        out.push_str(&"─".repeat(*width + 2));
        out.push(if idx + 1 == widths.len() { right } else { mid });
    }
    out
}

fn pad_aligned(text: &str, width: usize, alignment: Alignment) -> String {
    let content_width = display_width(text);
    if content_width >= width {
        return text.to_string();
    }
    let padding = width - content_width;
    match alignment {
        Alignment::Center => {
            let left = padding / 2;
            let right = padding - left;
            format!("{}{}{}", " ".repeat(left), text, " ".repeat(right))
        }
        Alignment::Right => format!("{}{}", " ".repeat(padding), text),
        Alignment::Left | Alignment::None => format!("{}{}", text, " ".repeat(padding)),
    }
}

fn truncate_to_width(text: &str, width: usize) -> String {
    if display_width(text) <= width {
        return text.to_string();
    }
    if width <= 1 {
        return "…".to_string();
    }
    let split = split_at_width(text, width - 1);
    format!("{}…", &text[..split])
}

fn split_at_width(text: &str, max_width: usize) -> usize {
    if max_width == 0 {
        return text.chars().next().map_or(0, char::len_utf8);
    }

    let mut width = 0;
    let mut split = 0;
    for (idx, ch) in text.char_indices() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + ch_width > max_width {
            return if split == 0 {
                idx + ch.len_utf8()
            } else {
                split
            };
        }
        width += ch_width;
        split = idx + ch.len_utf8();
    }
    text.len()
}

fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

fn plain_inline_width(nodes: &[InlineNode]) -> usize {
    nodes
        .iter()
        .map(|node| match node {
            InlineNode::Text { text, .. } => display_width(text),
            InlineNode::Break => 1,
        })
        .sum()
}

fn raw_prefix(text: String) -> Prefix {
    let width = display_width(&text);
    Prefix {
        spans: if text.is_empty() {
            Vec::new()
        } else {
            vec![Span::raw(text)]
        },
        width,
    }
}

fn styled_prefix(text: String, style: Style) -> Prefix {
    let width = display_width(&text);
    Prefix {
        spans: if text.is_empty() {
            Vec::new()
        } else {
            vec![Span::styled(text, style)]
        },
        width,
    }
}

fn combine_prefixes(left: &Prefix, right: &Prefix) -> Prefix {
    let mut spans = left.spans.clone();
    spans.extend(right.spans.clone());
    Prefix {
        spans,
        width: left.width + right.width,
    }
}

fn prefix_line(prefix: &Prefix, mut spans: Vec<Span<'static>>) -> Line<'static> {
    let mut line_spans = prefix.spans.clone();
    line_spans.append(&mut spans);
    Line::from(line_spans)
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

        let found = collect_markdown_view_paths(dir.path(), None);
        assert_eq!(found.first(), Some(&claude_dir.join("plan.md")));
    }

    #[test]
    fn markdown_path_collection_includes_other_markdown_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let claude_dir = dir.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::write(claude_dir.join("notes.md"), "# Notes").unwrap();
        std::fs::write(dir.path().join("RETRO.md"), "# Retro").unwrap();
        std::fs::create_dir_all(dir.path().join("docs")).unwrap();
        std::fs::write(dir.path().join("docs").join("guide.md"), "# Guide").unwrap();

        let found = collect_markdown_view_paths(dir.path(), None);
        assert!(found.contains(&claude_dir.join("notes.md")));
        assert!(found.contains(&dir.path().join("RETRO.md")));
        assert!(found.contains(&dir.path().join("docs").join("guide.md")));
    }

    #[test]
    fn markdown_path_collection_includes_repo_root_for_worktree() {
        let repo = tempfile::TempDir::new().unwrap();
        let worktree = repo.path().join(".worktrees").join("feature-a");
        let worktree_claude = worktree.join(".claude");
        std::fs::create_dir_all(&worktree_claude).unwrap();
        std::fs::write(worktree_claude.join("plan.md"), "# Worktree Plan").unwrap();
        std::fs::write(repo.path().join("RETRO.md"), "# Repo Retro").unwrap();
        std::fs::create_dir_all(worktree.join("docs")).unwrap();
        std::fs::write(
            worktree.join("docs").join("worktree-guide.md"),
            "# Worktree Guide",
        )
        .unwrap();

        let found = collect_markdown_view_paths(&worktree, Some(repo.path()));
        assert!(found.contains(&worktree_claude.join("plan.md")));
        assert!(found.contains(&repo.path().join("RETRO.md")));
        assert!(found.contains(&worktree.join("docs").join("worktree-guide.md")));
    }

    #[test]
    fn markdown_path_collection_skips_hidden_dirs_and_nested_worktrees() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join(".opencode")).unwrap();
        std::fs::write(dir.path().join(".opencode").join("hidden.md"), "# Hidden").unwrap();
        std::fs::create_dir_all(dir.path().join(".worktrees").join("other")).unwrap();
        std::fs::write(
            dir.path()
                .join(".worktrees")
                .join("other")
                .join("nested.md"),
            "# Nested",
        )
        .unwrap();

        let found = collect_markdown_view_paths(dir.path(), None);
        assert!(!found.contains(&dir.path().join(".opencode").join("hidden.md")));
        assert!(
            !found.contains(
                &dir.path()
                    .join(".worktrees")
                    .join("other")
                    .join("nested.md")
            )
        );
    }

    #[test]
    fn markdown_view_label_marks_repo_root_files() {
        let repo = tempfile::TempDir::new().unwrap();
        let worktree = repo.path().join(".worktrees").join("feature-a");
        let repo_path = repo.path().join(".claude").join("plan.md");

        assert_eq!(
            markdown_view_label(&repo_path, &worktree, Some(repo.path())),
            "[repo root] .claude/plan.md"
        );
    }

    #[test]
    fn markdown_view_label_marks_worktree_files_when_repo_root_is_present() {
        let repo = tempfile::TempDir::new().unwrap();
        let worktree = repo.path().join(".worktrees").join("feature-a");
        let worktree_path = worktree.join(".claude").join("plan.md");

        assert_eq!(
            markdown_view_label(&worktree_path, &worktree, Some(repo.path())),
            "[worktree] .claude/plan.md"
        );
    }

    #[test]
    fn markdown_view_scope_distinguishes_worktree_from_repo_root() {
        let repo = tempfile::TempDir::new().unwrap();
        let worktree = repo.path().join(".worktrees").join("feature-a");
        let worktree_path = worktree.join(".claude").join("plan.md");
        let repo_path = repo.path().join(".claude").join("context.md");

        assert_eq!(
            markdown_view_scope(&worktree_path, &worktree, Some(repo.path())),
            MarkdownViewScope::Worktree
        );
        assert_eq!(
            markdown_view_scope(&repo_path, &worktree, Some(repo.path())),
            MarkdownViewScope::RepoRoot
        );
    }

    #[test]
    fn preferred_plan_markdown_path_prefers_worktree_before_repo_root() {
        let repo = tempfile::TempDir::new().unwrap();
        let worktree = repo.path().join(".worktrees").join("feature-a");
        std::fs::create_dir_all(worktree.join(".claude")).unwrap();
        std::fs::create_dir_all(repo.path().join(".claude")).unwrap();
        let worktree_plan = worktree.join(".claude").join("plan.md");
        let repo_plan = repo.path().join(".claude").join("plan.md");
        std::fs::write(&worktree_plan, "# Worktree Plan").unwrap();
        std::fs::write(&repo_plan, "# Repo Plan").unwrap();

        assert_eq!(
            preferred_plan_markdown_path(&worktree, Some(repo.path())),
            Some(worktree_plan)
        );
    }

    #[test]
    fn read_plan_preview_compacts_markdown_content() {
        let dir = tempfile::TempDir::new().unwrap();
        let claude_dir = dir.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::write(
            claude_dir.join("plan.md"),
            "# Plan\n\n1. Inspect reducer\n2. Patch sidebar\n3. Run tests\n",
        )
        .unwrap();

        assert_eq!(
            read_plan_preview(dir.path(), None).as_deref(),
            Some("Plan\n1. Inspect reducer\n2. Patch sidebar\n3. Run tests")
        );
    }

    #[test]
    fn render_markdown_formats_headings_without_literal_hashes() {
        let theme = Theme::default();
        let rendered = render_markdown("# Title\n\nBody text", &theme, 48, None);
        let text = rendered
            .lines
            .iter()
            .map(rendered_line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("Title"));
        assert!(!text.contains("# Title"));
        assert!(text.contains("━━━━"));
    }

    #[test]
    fn render_markdown_wraps_list_items_with_hanging_indent() {
        let theme = Theme::default();
        let rendered = render_markdown(
            "- this is a fairly long list item that should wrap onto another line cleanly",
            &theme,
            28,
            None,
        );
        let strings = rendered
            .lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(strings[0].starts_with("• "), "{strings:#?}");
        assert!(
            strings.iter().skip(1).any(|line| line.starts_with("  ")),
            "{strings:#?}"
        );
    }

    #[test]
    fn render_markdown_formats_code_blocks_without_fences() {
        let theme = Theme::default();
        let rendered = render_markdown("```rust\nfn main() {}\n```", &theme, 40, None);
        let text = rendered
            .lines
            .iter()
            .map(rendered_line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("rust"), "{text}");
        assert!(text.contains("fn main() {}"), "{text}");
        assert!(!text.contains("```"), "{text}");
        assert!(text.contains("╭─"), "{text}");
        assert!(text.contains("╰─"), "{text}");
    }

    #[test]
    fn render_markdown_formats_tables_as_grid() {
        let theme = Theme::default();
        let rendered = render_markdown(
            "| Name | Status |\n| --- | --- |\n| AMF | Ready |",
            &theme,
            40,
            None,
        );
        let text = rendered
            .lines
            .iter()
            .map(rendered_line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("┌"), "{text}");
        assert!(text.contains("│ AMF "), "{text}");
        assert!(text.contains("└"), "{text}");
    }

    fn rendered_line_text(line: &Line<'static>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }
}
