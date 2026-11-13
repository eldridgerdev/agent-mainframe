//! Bounded terminal Mermaid rendering. App schedules work; renderers only read artifacts.

use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::thread::JoinHandle;

use selkie::diagrams::{Diagram, DiagramType};
use selkie::layout::{NodeShape, NodeSizeConfig, SizeEstimator, ToLayoutGraph};
use unicode_width::UnicodeWidthChar;

pub(crate) const MAX_DIAGRAMS: usize = 16;
pub(crate) const MAX_SOURCE_BYTES: usize = 16 * 1024;

#[derive(Clone, Default)]
struct RenderControl {
    cancelled: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

impl RenderControl {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed) || self.stop.load(Ordering::Relaxed)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DiagramSpec {
    /// Diagrams are laid out at their natural width and clipped when drawn, so
    /// resizing the pane never invalidates a cached result.
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DiagramResult {
    Rendered(Vec<String>),
    Fallback(String),
}

type Artifacts = Vec<(DiagramSpec, DiagramResult)>;

struct Pending {
    specs: Vec<DiagramSpec>,
    control: RenderControl,
    receiver: Receiver<Artifacts>,
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.control.cancel();
    }
}

/// Transient state owned by one viewer. Its private reply channel prevents results
/// for a closed/replaced viewer from being applied to another workflow.
#[derive(Default)]
pub(crate) struct DiagramDocument {
    pub show_source: bool,
    /// Horizontal pan, in cells, applied to every diagram wider than the pane.
    pub pan: usize,
    /// Widest diagram content, the largest per-diagram pan range and the pane
    /// width of the diagram that sets it; written by the Markdown renderer and
    /// read by the footer and the pan handlers.
    widest: AtomicUsize,
    range: AtomicUsize,
    view: AtomicUsize,
    requests: Vec<DiagramSpec>,
    ready: Artifacts,
    pending: Option<Pending>,
}

impl DiagramDocument {
    /// Called with pure parser output; never starts work during a draw.
    pub fn set_requests(&mut self, requests: Vec<DiagramSpec>) {
        self.requests = requests;
    }

    pub fn result(&self, source: &str) -> Option<&DiagramResult> {
        self.ready
            .iter()
            .find(|(spec, _)| spec.source == source)
            .map(|(_, result)| result)
    }

    /// Record a drawn diagram so the footer and pan limits know about it.
    pub(crate) fn note_drawn(&self, content_width: usize, view_width: usize) {
        self.widest.fetch_max(content_width, Ordering::Relaxed);
        // Diagrams can sit in panes of different widths (blockquotes), so the
        // limit is the largest per-diagram range, not widest minus last view.
        let range = content_width.saturating_sub(view_width);
        if range >= self.range.fetch_max(range, Ordering::Relaxed) {
            self.view.store(view_width, Ordering::Relaxed);
        }
    }

    pub(crate) fn begin_draw(&self) {
        self.widest.store(0, Ordering::Relaxed);
        self.range.store(0, Ordering::Relaxed);
        self.view.store(0, Ordering::Relaxed);
    }

    /// Largest useful pan, or 0 when every diagram fits.
    pub fn max_pan(&self) -> usize {
        self.range.load(Ordering::Relaxed)
    }

    /// Visible column range `(first, last, total)`, 1-based, when clipped.
    pub fn pan_status(&self) -> Option<(usize, usize, usize)> {
        let max = self.max_pan();
        if max == 0 || self.show_source {
            return None;
        }
        let view = self.view.load(Ordering::Relaxed);
        let start = self.pan.min(max);
        Some((start + 1, start + view, max + view))
    }

    /// Pan by `delta` cells, clamped to the diagram. Returns whether it moved,
    /// which is when the host must re-render.
    pub fn pan_by(&mut self, delta: isize) -> bool {
        let current = self.pan.min(self.max_pan());
        let next = current.saturating_add_signed(delta).min(self.max_pan());
        let changed = next != self.pan;
        self.pan = next;
        changed
    }

    /// Handle a horizontal pan key. Returns whether the document must be
    /// re-rendered. `h`/`l`/Left/Right pan a few cells; Shift or Ctrl pan a
    /// larger step.
    pub fn handle_pan_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
        use crossterm::event::{KeyCode, KeyModifiers};
        let fast = key
            .modifiers
            .intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL);
        let sign = match key.code {
            KeyCode::Left | KeyCode::Char('h' | 'H') => -1,
            KeyCode::Right | KeyCode::Char('l' | 'L') => 1,
            _ => return false,
        };
        let step = if fast || matches!(key.code, KeyCode::Char('H' | 'L')) {
            PAN_FAST_STEP
        } else {
            PAN_STEP
        };
        self.pan_by(sign * step as isize)
    }

    pub fn toggle_source(&mut self) {
        self.show_source = !self.show_source;
        self.pan = 0;
        self.requests.clear();
        self.pending = None;
    }

    pub fn reset(&mut self) {
        self.requests.clear();
        self.ready.clear();
        self.pending = None;
    }

    pub fn is_pending(&self) -> bool {
        !self.show_source
            && self
                .requests
                .iter()
                .any(|spec| self.result(&spec.source).is_none())
    }

    /// App calls this outside drawing. A completed result invalidates the host's
    /// line cache even if terminal width has not changed.
    pub fn poll(&mut self, worker: &mut DiagramWorker) -> bool {
        self.ready.retain(|(spec, _)| self.requests.contains(spec));
        let missing: Vec<_> = self
            .requests
            .iter()
            .filter(|spec| self.result(&spec.source).is_none())
            .cloned()
            .collect();
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.specs != missing)
        {
            self.pending = None;
        }
        if let Some(pending) = &self.pending {
            match pending.receiver.try_recv() {
                Ok(artifacts) => {
                    self.ready.extend(artifacts);
                    self.pending = None;
                    return true;
                }
                Err(TryRecvError::Disconnected) => {
                    self.ready.extend(missing.into_iter().map(|spec| {
                        (
                            spec,
                            DiagramResult::Fallback(
                                "Diagram worker stopped; refresh to retry".into(),
                            ),
                        )
                    }));
                    self.pending = None;
                    return true;
                }
                Err(TryRecvError::Empty) => return false,
            }
        }
        if !missing.is_empty() && !self.show_source {
            match worker.submit(missing.clone()) {
                Ok(pending) => self.pending = pending,
                Err(()) => {
                    self.ready.extend(missing.into_iter().map(|spec| {
                        (
                            spec,
                            DiagramResult::Fallback(
                                "Could not start diagram worker; refresh to retry".into(),
                            ),
                        )
                    }));
                    return true;
                }
            }
        }
        false
    }
}

pub(crate) const PAN_STEP: usize = 4;
pub(crate) const PAN_FAST_STEP: usize = 16;
pub(crate) const CLIP_LEFT: char = '◂';
pub(crate) const CLIP_RIGHT: char = '▸';

/// Cut display columns `start..start + width` out of `line`. A wide character
/// cut by either edge becomes a space, and combining marks stay with the base
/// character they follow. Edge markers replace the outermost cell of a row that
/// has content beyond that edge.
pub(crate) fn clip_columns(line: &str, start: usize, width: usize) -> String {
    let mut cells = Vec::new();
    let mut col = 0;
    for ch in line.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        cells.push((col, w, ch));
        col += w;
    }
    let end = start + width;
    if start == 0 && col <= width {
        return line.to_owned();
    }
    let beyond = |keep: &dyn Fn(usize) -> bool| {
        cells
            .iter()
            .any(|&(begin, w, ch)| w > 0 && keep(begin) && !ch.is_whitespace())
    };
    let left_marker = width > 2 && beyond(&|begin| begin < start);
    let right_marker = width > 2 && beyond(&|begin| begin >= end);
    let inner_start = start + usize::from(left_marker);
    let inner_end = end - usize::from(right_marker);

    let mut out = String::new();
    if left_marker {
        out.push(CLIP_LEFT);
    }
    let mut written = inner_start;
    let mut base_visible = false;
    for &(begin, w, ch) in &cells {
        if w == 0 {
            if base_visible {
                out.push(ch);
            }
            continue;
        }
        let finish = begin + w;
        if finish <= inner_start || begin >= inner_end {
            base_visible = false;
        } else if begin < inner_start || finish > inner_end {
            let (from, to) = (begin.max(inner_start), finish.min(inner_end));
            out.extend(std::iter::repeat_n(' ', to - from));
            written = to;
            base_visible = false;
        } else {
            out.push(ch);
            written = finish;
            base_visible = true;
        }
    }
    out.extend(std::iter::repeat_n(' ', inner_end.saturating_sub(written)));
    if right_marker {
        out.push(CLIP_RIGHT);
    } else {
        out.truncate(out.trim_end_matches(' ').len());
    }
    out
}

struct Task {
    specs: Vec<DiagramSpec>,
    control: RenderControl,
    reply: SyncSender<Artifacts>,
}

/// One lazily started worker per App, with at most one queued document. Closing
/// a viewer cancels its operation without waiting on the UI thread.
#[derive(Default)]
pub(crate) struct DiagramWorker {
    sender: Option<SyncSender<Task>>,
    thread: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
}

impl DiagramWorker {
    fn submit(&mut self, specs: Vec<DiagramSpec>) -> Result<Option<Pending>, ()> {
        if self.sender.is_none() {
            let (sender, receiver) = mpsc::sync_channel::<Task>(1);
            self.thread = Some(
                std::thread::Builder::new()
                    .name("amf-mermaid".into())
                    .spawn(move || {
                        while let Ok(task) = receiver.recv() {
                            let artifacts = task
                                .specs
                                .into_iter()
                                .map(|spec| {
                                    let result = render_diagram(&spec, &task.control);
                                    (spec, result)
                                })
                                .collect();
                            let _ = task.reply.send(artifacts);
                        }
                    })
                    .map_err(|_| ())?,
            );
            self.sender = Some(sender);
        }
        let control = RenderControl {
            stop: self.stop.clone(),
            ..Default::default()
        };
        let (reply, receiver) = mpsc::sync_channel(1);
        let task = Task {
            specs: specs.clone(),
            control: control.clone(),
            reply,
        };
        match self.sender.as_ref().ok_or(())?.try_send(task) {
            Ok(()) => Ok(Some(Pending {
                specs,
                control,
                receiver,
            })),
            Err(mpsc::TrySendError::Full(_)) => Ok(None),
            Err(mpsc::TrySendError::Disconnected(_)) => Err(()),
        }
    }
}

impl Drop for DiagramWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.sender = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Layout must reserve the cells the text renderer actually draws. The
/// browser-oriented estimator makes long-label diamonds too short, causing
/// their borders to overlap neighbouring nodes and group frames.
struct TerminalSizeEstimator;

impl SizeEstimator for TerminalSizeEstimator {
    fn estimate_text_size(&self, text: &str, _font_size: f64) -> (f64, f64) {
        let scale = selkie::render::ascii::scale::CellScale::default();
        (
            text.chars().count() as f64 * scale.cell_width,
            scale.cell_height,
        )
    }

    fn estimate_node_size(
        &self,
        label: Option<&str>,
        shape: NodeShape,
        _config: &NodeSizeConfig,
    ) -> (f64, f64) {
        let scale = selkie::render::ascii::scale::CellScale::default();
        let rendered =
            selkie::render::ascii::shapes::render_shape(&shape, label.unwrap_or(""), 0, 0);
        (
            rendered.width as f64 * scale.cell_width,
            rendered.height as f64 * scale.cell_height,
        )
    }
}

// Selkie has no interruption checkpoints inside layout. Keep one worker and
// admit only small models before layout, then bound the canvas before allocating
// it. Cancellation is observed between stages/diagrams, not a hard time limit.
fn render_diagram(spec: &DiagramSpec, control: &RenderControl) -> DiagramResult {
    // A renderer defect must not kill the shared worker or leave later
    // documents permanently stuck in their pending state.
    match std::panic::catch_unwind(|| render_text(spec, control)) {
        Ok(Ok(text)) => DiagramResult::Rendered(
            text.lines()
                .map(|line| line.trim_end().to_owned())
                .collect(),
        ),
        Ok(Err(reason)) => DiagramResult::Fallback(reason),
        Err(_) => DiagramResult::Fallback("Diagram renderer failed; showing source".into()),
    }
}

fn render_text(spec: &DiagramSpec, control: &RenderControl) -> Result<String, String> {
    check_cancelled(control)?;
    if spec.source.len() > MAX_SOURCE_BYTES || spec.source.lines().count() > 256 {
        return Err("Diagram exceeds the source limit (16 KiB / 256 lines)".into());
    }
    // Selkie measures labels in characters, not display cells. Do not show a
    // broken topology for wide characters, combining marks or terminal controls.
    if spec
        .source
        .chars()
        .any(|ch| !matches!(ch, '\n' | '\r' | '\t') && UnicodeWidthChar::width(ch) != Some(1))
    {
        return Err("Wide or combining characters need source view in this MVP".into());
    }
    if spec.source.contains("%%{") || spec.source.trim_start().starts_with("---") {
        return Err("Diagram configuration directives need source view in this MVP".into());
    }
    let family =
        selkie::diagrams::detect_type(&spec.source).map_err(|e| diagnostic(&e.to_string()))?;
    if !matches!(family, DiagramType::Flowchart | DiagramType::Sequence) {
        return Err("Supported diagrams: flowchart/graph and sequenceDiagram".into());
    }
    let diagram = selkie::parse(&spec.source).map_err(|e| diagnostic(&e.to_string()))?;
    check_cancelled(control)?;
    let (text, labels) = match &diagram {
        Diagram::Flowchart(db) => {
            use selkie::diagrams::flowchart::EdgeStroke;
            let data = db.get_data();
            if data.vertices.len() > 32
                || data.edges.len() > 64
                || data.subgraphs.len() > 8
                || data.edges.iter().any(|edge| edge.length.unwrap_or(1) > 4)
            {
                return Err(
                    "Diagram exceeds the model limit (32 nodes / 64 edges / 8 groups)".into(),
                );
            }
            // The ASCII renderer always draws a normal line with one target
            // arrow. Other heads, directions and strokes would change meaning.
            if data.edges.iter().any(|edge| {
                edge.edge_type.as_deref() != Some("arrow_point")
                    || edge.stroke != EdgeStroke::Normal
            }) {
                return Err(
                    "Unsupported flowchart arrow types or strokes need source view in this MVP"
                        .into(),
                );
            }
            if data.vertices.values().any(|vertex| {
                vertex.link.is_some()
                    || vertex.have_callback
                    || vertex.icon.is_some()
                    || vertex.img.is_some()
            }) {
                return Err(
                    "Interactive nodes, icons and images need source view in this MVP".into(),
                );
            }
            let node_text = |id: &str| {
                data.vertices
                    .get(id)
                    .and_then(|vertex| vertex.text.as_deref())
                    .unwrap_or(id)
                    .to_owned()
            };
            let labels: Vec<Label> =
                data.vertices
                    .values()
                    .map(|vertex| {
                        let text = vertex.text.as_deref().unwrap_or(&vertex.id);
                        Label::new(text, format!("node \"{text}\""))
                    })
                    .chain(data.edges.iter().map(|edge| {
                        Label::new(
                            &edge.text,
                            format!(
                                "{} -> {}: \"{}\"",
                                node_text(&edge.start),
                                node_text(&edge.end),
                                edge.text
                            ),
                        )
                    }))
                    .chain(data.subgraphs.iter().map(|group| {
                        Label::new(&group.title, format!("group \"{}\"", group.title))
                    }))
                    .filter(|label| !label.text.is_empty())
                    .collect();
            check_labels(&labels)?;
            let graph = db
                .to_layout_graph(&TerminalSizeEstimator)
                .map_err(|e| diagnostic(&e.to_string()))?;
            let graph = selkie::layout::layout(graph).map_err(|e| diagnostic(&e.to_string()))?;
            check_cancelled(control)?;
            let scale = cell_scale();
            let cols = scale.to_col(graph.width.unwrap_or(400.0)).saturating_add(8);
            let rows = scale
                .to_row(graph.height.unwrap_or(300.0))
                .saturating_add(4);
            if cols.saturating_mul(rows) > 32_768 {
                return Err("Diagram exceeds the terminal canvas limit".into());
            }
            // max_width would silently truncate. Render uncompressed and refuse
            // overflow ourselves so relationships and labels cannot be clipped.
            let graph = spread_edge_labels(graph);
            let text = selkie::render::ascii::render_flowchart_ascii(db, &graph)
                .map_err(|e| diagnostic(&e.to_string()))?;
            (text, labels)
        }
        Diagram::Sequence(db) => {
            use selkie::diagrams::sequence::LineType;
            if db.get_actors().len() > 8 || db.get_messages().len() > 64 {
                return Err("Sequence exceeds the model limit (8 participants / 64 events)".into());
            }
            // Lifetime events are stored separately from messages, but the
            // ASCII renderer draws every participant with a complete lifeline.
            if db
                .get_actors()
                .keys()
                .any(|name| db.is_created(name) || db.is_destroyed(name))
            {
                return Err(
                    "Participant creation or destruction needs source view in this MVP".into(),
                );
            }
            // Upstream's text renderer omits these constructs. Preserve source
            // rather than presenting an incomplete explanation as a diagram.
            if !db.get_notes().is_empty()
                || !db.get_boxes().is_empty()
                || db.get_autonumber().is_some()
                || db.get_messages().iter().any(|message| {
                    message.activate
                        || !matches!(
                            message.message_type,
                            LineType::Solid
                                | LineType::Dotted
                                | LineType::SolidOpen
                                | LineType::DottedOpen
                                | LineType::LoopStart
                                | LineType::LoopEnd
                                | LineType::AltStart
                                | LineType::AltElse
                                | LineType::AltEnd
                                | LineType::OptStart
                                | LineType::OptEnd
                                | LineType::ParStart
                                | LineType::ParAnd
                                | LineType::ParEnd
                        )
                })
            {
                return Err(
                    "Notes, boxes, activations or advanced arrows need source view in this MVP"
                        .into(),
                );
            }
            let labels: Vec<Label> = db
                .get_actors()
                .values()
                .map(|actor| {
                    Label::new(
                        &actor.description,
                        format!("participant \"{}\"", actor.description),
                    )
                })
                .chain(db.get_messages().iter().map(|message| {
                    let actor_text = |id: Option<&str>| {
                        id.map(|id| {
                            db.get_actors()
                                .get(id)
                                .map_or(id, |actor| actor.description.as_str())
                        })
                        .unwrap_or("?")
                        .to_owned()
                    };
                    Label::new(
                        &message.message,
                        format!(
                            "{} -> {}: \"{}\"",
                            actor_text(message.from.as_deref()),
                            actor_text(message.to.as_deref()),
                            message.message
                        ),
                    )
                }))
                .filter(|label| !label.text.is_empty())
                .collect();
            check_labels(&labels)?;
            // Conservative upper bound on the upstream actor/gap canvas: each
            // gap fits the longest label, and each event needs two rows.
            let max_label = labels
                .iter()
                .map(|label| label.text.chars().count())
                .max()
                .unwrap_or(0);
            let cols = db
                .get_actors()
                .len()
                .saturating_mul((max_label + 6).max(20) + max_label + 4)
                + 2;
            let rows = db.get_messages().len() * 2 + 8;
            if cols.saturating_mul(rows) > 32_768 {
                return Err("Sequence exceeds the terminal canvas limit".into());
            }
            let text = selkie::render::ascii::render_sequence_ascii(db)
                .map_err(|e| diagnostic(&e.to_string()))?;
            (text, labels)
        }
        _ => return Err("No supported Mermaid diagram found".into()),
    };
    check_cancelled(control)?;
    if text.len() > 128 * 1024 {
        return Err("Diagram exceeds the output limit".into());
    }
    if text.trim().is_empty() {
        return Err("This diagram drew nothing; showing source".into());
    }
    Ok(append_legend(text, &labels))
}

/// A label the diagram must account for, with a description for the legend
/// when the renderer could not draw it.
struct Label {
    text: String,
    describe: String,
}

impl Label {
    fn new(text: &str, describe: String) -> Self {
        Self {
            text: text.to_owned(),
            describe,
        }
    }
}

/// Start columns of whole-word occurrences of `word` in `row`.
fn word_starts(row: &[char], word: &[char]) -> Vec<usize> {
    let boundary = |i: Option<usize>| {
        i.and_then(|i| row.get(i))
            .is_none_or(|c| !c.is_alphanumeric())
    };
    (0..row.len().saturating_sub(word.len().saturating_sub(1)))
        .filter(|&i| {
            row[i..].starts_with(word)
                && boundary(i.checked_sub(1))
                && boundary(Some(i + word.len()))
        })
        .collect()
}

/// True when the words of `label` read as text wrapped inside a box: each next
/// word follows on the same row, or starts on the next row near where the
/// previous word began. Words match whole, so `ask` is not found in `task`.
fn label_drawn(rows: &[&str], label: &str) -> bool {
    if rows.iter().any(|row| row.contains(label)) {
        return true;
    }
    let words: Vec<Vec<char>> = label
        .split_whitespace()
        .map(|w| w.chars().collect())
        .collect();
    let grid: Vec<Vec<char>> = rows.iter().map(|r| r.chars().collect()).collect();
    let Some(first) = words.first() else {
        return false;
    };
    // How far a wrapped line may start from the previous one, in columns.
    const DRIFT: usize = 12;
    for (r0, row) in grid.iter().enumerate() {
        for c0 in word_starts(row, first) {
            let (mut r, mut start, mut end) = (r0, c0, c0 + first.len());
            let mut ok = true;
            for word in &words[1..] {
                let same = grid[r]
                    .get(end..)
                    .and_then(|rest| word_starts(rest, word).first().map(|c| end + c));
                let next = grid.get(r + 1).and_then(|below| {
                    word_starts(below, word)
                        .into_iter()
                        .find(|c| c.abs_diff(start) <= DRIFT)
                });
                if let Some(c) = same {
                    start = c;
                } else if let Some(c) = next {
                    r += 1;
                    start = c;
                } else {
                    ok = false;
                    break;
                }
                end = start + word.len();
            }
            if ok {
                return true;
            }
        }
    }
    false
}

/// selkie's text renderer converts layout pixels to cells with its default
/// scale and offers no way to change it (we render without `max_width`).
fn cell_scale() -> selkie::render::ascii::scale::CellScale {
    selkie::render::ascii::scale::CellScale::default()
}

fn node_cells(
    nodes: &[selkie::layout::LayoutNode],
    scale: &selkie::render::ascii::scale::CellScale,
    offset: (f64, f64),
    out: &mut Vec<(i64, i64, i64, i64)>,
) {
    for node in nodes {
        // Group frames wrap their children; labels may sit inside them.
        if node.children.is_empty() {
            if let (Some(x), Some(y)) = (node.x, node.y) {
                let col = scale.to_col(x - offset.0) as i64;
                let row = scale.to_row(y - offset.1) as i64;
                let w = scale.to_cell_width(node.width) as i64;
                let h = (scale.to_row(node.height) as i64).max(1);
                out.push((row, col, row + h, col + w));
            }
        } else {
            node_cells(&node.children, scale, offset, out);
        }
    }
}

/// Edges between the same nodes get the same label position, so their labels
/// are drawn on top of each other. Move each label that would overlap an
/// earlier label or a node onto a nearby free row, never off the canvas.
/// The move is capped so a label stays next to its own edge.
fn spread_edge_labels(mut graph: selkie::layout::LayoutGraph) -> selkie::layout::LayoutGraph {
    const MAX_SHIFT: i64 = 3;
    let scale = cell_scale();
    let offset = (graph.bounds_x.unwrap_or(0.0), graph.bounds_y.unwrap_or(0.0));
    let mut nodes = Vec::new();
    node_cells(&graph.nodes, &scale, offset, &mut nodes);
    // (row, first column, one past the last column) of labels already placed.
    let mut placed: Vec<(i64, i64, i64)> = Vec::new();
    for edge in &mut graph.edges {
        let (Some(label), Some(pos)) = (edge.label.as_deref(), edge.label_position.as_mut()) else {
            continue;
        };
        let len = label
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .count() as i64;
        let col = ((pos.x - offset.0) / scale.cell_width).round() as i64 - len / 2;
        let row = ((pos.y - offset.1) / scale.cell_height).round() as i64;
        let clear = |r: i64| {
            r >= 0
                && col >= 0
                && placed
                    .iter()
                    .all(|&(pr, pc, pe)| pr != r || col + len < pc || col > pe)
                && nodes
                    .iter()
                    .all(|&(r0, c0, r1, c1)| r < r0 || r >= r1 || col + len <= c0 || col >= c1)
        };
        let shift = (0..=MAX_SHIFT)
            .flat_map(|n| if n == 0 { vec![0] } else { vec![n, -n] })
            .find(|&d| clear(row + d))
            .unwrap_or(0);
        placed.push((row + shift, col, col + len));
        pos.y += shift as f64 * scale.cell_height;
    }
    graph
}

/// Indexes of the labels not drawn in `text`.
fn undrawn(text: &str, labels: &[Label]) -> Vec<usize> {
    let rows: Vec<&str> = text.lines().collect();
    // Edges often share a label ("accept"); one copy on screen must not vouch
    // for the others, so each drawn occurrence is spent on one label. Longest
    // labels claim their text first and blank it out, so "change" cannot
    // borrow a copy from inside "request change".
    let mut grid: Vec<Vec<char>> = rows.iter().map(|r| r.chars().collect()).collect();
    let mut texts: Vec<&str> = labels.iter().map(|l| l.text.as_str()).collect();
    texts.sort_by_key(|t| std::cmp::Reverse(t.chars().count()));
    texts.dedup();
    let mut spare: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for text in texts {
        let needle: Vec<char> = text.chars().collect();
        let mut count = 0;
        for row in &mut grid {
            let mut i = 0;
            while i + needle.len() <= row.len() {
                if row[i..].starts_with(&needle) {
                    row[i..i + needle.len()].fill('\u{0}');
                    count += 1;
                    i += needle.len();
                } else {
                    i += 1;
                }
            }
        }
        spare.insert(text, count);
    }
    let mut missing = Vec::new();
    for (index, label) in labels.iter().enumerate() {
        let count = spare.get_mut(label.text.as_str()).expect("counted above");
        let drawn = if *count > 0 {
            *count -= 1;
            true
        } else {
            // Wrapped inside a box is only possible when no copy is drawn
            // contiguously; otherwise the copies are already spent.
            !rows.iter().any(|row| row.contains(label.text.as_str()))
                && label_drawn(&rows, &label.text)
        };
        if !drawn {
            missing.push(index);
        }
    }
    missing
}

/// Never hide a missing label: draw the diagram and list what is absent.
fn append_legend(text: String, labels: &[Label]) -> String {
    let mut missing: Vec<&str> = Vec::new();
    for index in undrawn(&text, labels) {
        if !missing.contains(&labels[index].describe.as_str()) {
            missing.push(&labels[index].describe);
        }
    }
    if missing.is_empty() {
        return text;
    }
    let mut out = text.trim_end().to_owned();
    out.push_str("\n\nNot shown:");
    for item in missing {
        out.push_str("\n  ");
        out.push_str(item);
    }
    out
}

fn check_cancelled(control: &RenderControl) -> Result<(), String> {
    if control.is_cancelled() {
        Err("Diagram rendering cancelled".into())
    } else {
        Ok(())
    }
}

fn check_labels(labels: &[Label]) -> Result<(), String> {
    if labels.iter().any(|label| {
        label.text.chars().count() > 120
            || label.text.chars().any(char::is_control)
            || label.text.contains(['<', '>'])
    }) {
        Err("Long, multiline or HTML labels need source view in this MVP".into())
    } else {
        Ok(())
    }
}

fn diagnostic(text: &str) -> String {
    text.chars()
        .filter(|ch| !ch.is_control() || *ch == '\n')
        .take(240)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::render_markdown_with_diagrams;
    use crate::theme::Theme;
    use std::time::{Duration, Instant};
    use unicode_width::UnicodeWidthStr;

    const FLOW: &str = "flowchart TD\n A[Start] --> B{Ready?}\n B -->|Yes| C[Deploy]\n B -->|No| D[Fix]\n D --> B\n";
    const SEQUENCE: &str = "sequenceDiagram\n participant User\n participant AMF\n User->>AMF: Open plan\n alt Valid\n AMF-->>User: Diagram\n else Invalid\n AMF-->>User: Source\n end\n";

    fn spec(source: &str) -> DiagramSpec {
        DiagramSpec {
            source: source.into(),
        }
    }

    fn text(rendered: crate::markdown::RenderedMarkdown) -> String {
        rendered
            .lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn prepare(
        source: &str,
        width: usize,
        doc: &mut DiagramDocument,
        worker: &mut DiagramWorker,
    ) -> String {
        let rendered = render_markdown_with_diagrams(source, &Theme::default(), width, None, doc);
        doc.set_requests(rendered.diagram_requests);
        let deadline = Instant::now() + Duration::from_secs(5);
        while doc.is_pending() {
            doc.poll(worker);
            assert!(Instant::now() < deadline, "diagram preparation timed out");
            std::thread::sleep(Duration::from_millis(1));
        }
        text(render_markdown_with_diagrams(
            source,
            &Theme::default(),
            width,
            None,
            doc,
        ))
    }

    #[test]
    fn flowchart_preserves_branches_labels_and_direction() {
        let DiagramResult::Rendered(lines) = render_diagram(&spec(FLOW), &RenderControl::default())
        else {
            panic!("expected flowchart")
        };
        let output = lines.join("\n");
        for label in ["Start", "Ready?", "Yes", "No", "Deploy", "Fix"] {
            assert!(output.contains(label), "{output}");
        }
        assert!(output.contains('▼') && output.contains('▲'), "{output}");
        assert!(output.find("Start").unwrap() < output.find("Ready?").unwrap());
        assert!(output.find("Ready?").unwrap() < output.find("Deploy").unwrap());
    }

    #[test]
    fn unsupported_flowchart_edges_keep_source() {
        for edge in [
            "---", "<-->", "~~~", "--x", "--o", "x--x", "o--o", "-.->", "==>",
        ] {
            let source = format!("flowchart TD\n A {edge} B");
            assert_fallback_preserves_source(
                &source,
                "Unsupported flowchart arrow types or strokes need source view in this MVP",
            );
        }
    }

    fn assert_fallback_preserves_source(source: &str, reason: &str) {
        assert_eq!(
            render_diagram(&spec(source), &RenderControl::default()),
            DiagramResult::Fallback(reason.into()),
            "{source}"
        );
        let markdown = format!("```mermaid\n{source}\n```");
        let output = prepare(
            &markdown,
            120,
            &mut DiagramDocument::default(),
            &mut DiagramWorker::default(),
        );
        assert!(output.contains("Mermaid:"), "{output}");
        assert!(!output.contains("mermaid · diagram"), "{output}");
        for line in source.lines() {
            assert!(output.contains(line), "{output}");
        }
    }

    #[test]
    fn long_decision_borders_do_not_overlap_adjacent_nodes() {
        let DiagramResult::Rendered(lines) = render_diagram(
            &spec(
                "flowchart TD\n Before[Before] --> Decision{Ready for final review?}\n Decision --> After[After]",
            ),
            &RenderControl::default(),
        ) else {
            panic!("expected flowchart")
        };
        let before = lines
            .iter()
            .position(|line| line.contains("Before"))
            .unwrap();
        let after = lines
            .iter()
            .position(|line| line.contains("After"))
            .unwrap();
        let top = lines.iter().position(|line| line.contains("/\\")).unwrap();
        let bottom = lines.iter().rposition(|line| line.contains("\\/")).unwrap();
        assert!(
            before + 1 < top && bottom < after - 1,
            "{}",
            lines.join("\n")
        );
        assert!(lines[before + 1].contains('└'));
        assert!(lines[after - 1].contains('┌'));
    }

    #[test]
    fn advanced_examples_preserve_groups_branches_loops_and_messages() {
        for markdown in [
            include_str!("../docs/mermaid-advanced-flowchart.md"),
            include_str!("../docs/mermaid-advanced-sequence.md"),
        ] {
            let output = prepare(
                markdown,
                100,
                &mut DiagramDocument::default(),
                &mut DiagramWorker::default(),
            );
            assert!(output.contains("mermaid · diagram"), "{output}");
            assert!(!output.contains("Mermaid:"), "{output}");
            assert!(
                output
                    .lines()
                    .all(|line| UnicodeWidthStr::width(line) <= 100)
            );
        }
    }

    #[test]
    fn sequence_preserves_participants_messages_and_alternatives() {
        let DiagramResult::Rendered(lines) =
            render_diagram(&spec(SEQUENCE), &RenderControl::default())
        else {
            panic!("expected sequence")
        };
        let output = lines.join("\n");
        for label in [
            "User",
            "AMF",
            "Open plan",
            "Valid",
            "Diagram",
            "Invalid",
            "Source",
        ] {
            assert!(output.contains(label), "{output}");
        }
        assert!(output.contains('>') && output.contains('<'), "{output}");
        assert!(output.find("Open plan").unwrap() < output.find("Diagram").unwrap());
        assert!(output.find("Diagram").unwrap() < output.find("Source").unwrap());
    }

    #[test]
    fn sequence_participant_lifetime_events_keep_source() {
        for source in [
            "sequenceDiagram\n participant A\n create participant B\n A->>B: Hello",
            "sequenceDiagram\n participant A\n create actor B as Worker\n A->>B: Hello",
            "sequenceDiagram\n A->>B: Hello\n destroy B\n A->>B: Goodbye",
            "sequenceDiagram\n participant A\n create participant B\n A->>B: Hello\n destroy B\n A->>B: Goodbye",
        ] {
            assert_fallback_preserves_source(
                source,
                "Participant creation or destruction needs source view in this MVP",
            );
        }
    }

    #[test]
    fn wide_diagrams_are_clipped_and_panned_instead_of_refused() {
        let markdown = format!("```mermaid\n{SEQUENCE}```");
        let mut doc = DiagramDocument::default();
        let mut worker = DiagramWorker::default();
        let wide = prepare(&markdown, 200, &mut doc, &mut worker);
        let natural = doc.widest.load(Ordering::Relaxed);
        assert!(natural > 40 && !wide.contains('▸'), "{wide}");
        assert_eq!(doc.max_pan(), 0);

        let narrow = prepare(&markdown, 40, &mut doc, &mut worker);
        assert!(narrow.contains("mermaid · diagram") && !narrow.contains("Mermaid:"));
        assert!(
            narrow.contains(CLIP_RIGHT) && !narrow.contains(CLIP_LEFT),
            "{narrow}"
        );
        assert!(narrow.lines().all(|row| UnicodeWidthStr::width(row) <= 40));
        let view = doc.view.load(Ordering::Relaxed);
        assert_eq!(doc.max_pan(), natural - view);
        assert_eq!(doc.pan_status(), Some((1, view, natural)));

        // Panning to the far end shows the left marker and the last column.
        assert!(doc.pan_by(isize::MAX));
        assert_eq!(doc.pan, natural - view);
        let end = text(render_markdown_with_diagrams(
            &markdown,
            &Theme::default(),
            40,
            None,
            &doc,
        ));
        assert!(
            end.contains(CLIP_LEFT) && !end.contains(CLIP_RIGHT),
            "{end}"
        );
        assert!(!doc.pan_by(1), "already at the right end");
        assert!(doc.pan_by(isize::MIN) && doc.pan == 0);
        assert!(!doc.pan_by(-1), "already at the left end");
    }

    #[test]
    fn pan_offset_is_clamped_after_resize() {
        let markdown = format!("```mermaid\n{SEQUENCE}```");
        let mut doc = DiagramDocument::default();
        let mut worker = DiagramWorker::default();
        prepare(&markdown, 40, &mut doc, &mut worker);
        doc.pan_by(isize::MAX);
        let far = doc.pan;
        // The renderer clamps per diagram, so a stale offset never overshoots.
        let wider = text(render_markdown_with_diagrams(
            &markdown,
            &Theme::default(),
            60,
            None,
            &doc,
        ));
        assert!(wider.lines().all(|row| UnicodeWidthStr::width(row) <= 60));
        assert!(doc.max_pan() < far);
        // The draw path clamps the stored offset to the new range.
        doc.pan = doc.pan.min(doc.max_pan());
        assert_eq!(doc.pan, doc.max_pan());
        assert!(!doc.pan_by(0));
        let fits = prepare(&markdown, 200, &mut doc, &mut worker);
        assert_eq!(doc.max_pan(), 0);
        assert!(!fits.contains(CLIP_LEFT) && !fits.contains(CLIP_RIGHT));
    }

    #[test]
    fn pan_limit_uses_each_diagrams_own_pane_width() {
        let doc = DiagramDocument::default();
        doc.begin_draw();
        doc.note_drawn(100, 38); // blockquoted: range 62
        doc.note_drawn(90, 40); // top level: range 50
        assert_eq!(doc.max_pan(), 62);
        assert_eq!(doc.pan_status(), Some((1, 38, 100)));
    }

    #[test]
    fn pan_keys_step_and_report_changes() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut doc = DiagramDocument::default();
        doc.note_drawn(100, 40);
        let press = |code, modifiers| KeyEvent::new(code, modifiers);
        assert!(doc.handle_pan_key(press(KeyCode::Right, KeyModifiers::NONE)));
        assert_eq!(doc.pan, PAN_STEP);
        assert!(doc.handle_pan_key(press(KeyCode::Char('L'), KeyModifiers::SHIFT)));
        assert_eq!(doc.pan, PAN_STEP + PAN_FAST_STEP);
        assert!(doc.handle_pan_key(press(KeyCode::Char('h'), KeyModifiers::NONE)));
        assert!(!doc.handle_pan_key(press(KeyCode::Char('x'), KeyModifiers::NONE)));
    }

    #[test]
    fn clipping_handles_edges_wide_and_combining_characters() {
        assert_eq!(clip_columns("abcdef", 0, 10), "abcdef");
        assert_eq!(clip_columns("abcdefghij", 0, 6), "abcde▸");
        assert_eq!(clip_columns("abcdefghij", 4, 6), "◂fghij");
        assert_eq!(clip_columns("abcdefghij", 2, 6), "◂defg▸");
        // Rows with only blank space beyond the edge carry no marker.
        assert_eq!(clip_columns("abc        ", 0, 6), "abc");
        // A wide character cut by the left or right edge is blanked.
        assert_eq!(clip_columns("a开b开cdefgh", 1, 6), "◂ b开▸");
        assert_eq!(clip_columns("abcd开efghi", 0, 6), "abcd ▸");
        // Combining marks stay with their base and vanish with it.
        assert_eq!(clip_columns("ae\u{301}cdefghi", 0, 6), "ae\u{301}cde▸");
        assert_eq!(clip_columns("abcde\u{301}fghij", 5, 6), "◂ghij");
        for line in ["a开b开cdefgh", "abcd开efghi", "ae\u{301}cdefghi"] {
            for start in 0..8 {
                assert!(
                    UnicodeWidthStr::width(clip_columns(line, start, 5).as_str()) <= 5,
                    "{line:?} at {start}"
                );
            }
        }
    }

    #[test]
    fn invalid_unsupported_and_lossy_diagrams_keep_source() {
        for source in [
            "flowchart TD\n A[unfinished",
            "pie\n \"Rust\" : 10",
            "flowchart TD\n A[开始] --> B[End]",
            "flowchart TD\n A[Tabbed\tlabel] --> B[End]",
            "sequenceDiagram\n A->>B: Hello\n Note over B: Essential context",
        ] {
            let markdown = format!("Before\n\n```mermaid\n{source}\n```\n\nAfter");
            let output = prepare(
                &markdown,
                80,
                &mut DiagramDocument::default(),
                &mut DiagramWorker::default(),
            );
            assert!(output.contains("Mermaid:"), "{output}");
            assert!(output.contains(source.lines().next().unwrap()), "{output}");
            assert!(output.contains("Before") && output.contains("After"));
            assert!(!output.contains("Rendering diagram"));
        }
    }

    #[test]
    fn resource_admission_and_cancelled_work_return_fallback() {
        let oversized = format!("flowchart TD\n A[{}]", "x".repeat(MAX_SOURCE_BYTES));
        let many_nodes = format!(
            "flowchart TD\n{}",
            (0..33)
                .map(|n| format!(" N{n}[Node {n}]\n"))
                .collect::<String>()
        );
        for source in [oversized, many_nodes] {
            assert!(
                matches!(render_diagram(&spec(&source), &RenderControl::default()), DiagramResult::Fallback(reason) if reason.contains("limit"))
            );
        }
        let control = RenderControl::default();
        control.cancel();
        assert_eq!(
            render_diagram(&spec(FLOW), &control),
            DiagramResult::Fallback("Diagram rendering cancelled".into())
        );
    }

    #[test]
    fn fenced_blocks_preserve_prefixes_and_do_not_wrap_connector_rows() {
        for markdown in [
            format!("~~~mermaid\n{FLOW}~~~"),
            format!("````mermaid\n{FLOW}````"),
            format!(
                "> ```mermaid\n{}> ```",
                FLOW.lines()
                    .map(|line| format!("> {line}\n"))
                    .collect::<String>()
            ),
            format!(
                "- Diagram:\n\n  ```mermaid\n{}  ```",
                FLOW.lines()
                    .map(|line| format!("  {line}\n"))
                    .collect::<String>()
            ),
        ] {
            let mut doc = DiagramDocument::default();
            let output = prepare(
                &markdown.replace('\n', "\r\n"),
                60,
                &mut doc,
                &mut DiagramWorker::default(),
            );
            assert!(output.contains("mermaid · diagram"), "{output}");
            assert!(!output.contains("flowchart TD"), "{output}");
            assert!(
                output
                    .lines()
                    .all(|line| UnicodeWidthStr::width(line) <= 60),
                "{output}"
            );
            let source = &doc.requests[0];
            let Some(DiagramResult::Rendered(lines)) = doc.result(&source.source) else {
                panic!("expected prepared diagram")
            };
            for line in lines {
                assert!(
                    output.lines().any(|row| row.ends_with(line)),
                    "connector row changed: {line:?}"
                );
            }
        }
    }

    #[test]
    fn source_toggle_and_resize_do_not_reuse_wrong_artifacts() {
        let markdown = format!("```mermaid\n{SEQUENCE}```");
        let mut doc = DiagramDocument::default();
        let mut worker = DiagramWorker::default();
        assert!(prepare(&markdown, 80, &mut doc, &mut worker).contains("mermaid · diagram"));
        doc.toggle_source();
        let source = prepare(&markdown, 80, &mut doc, &mut worker);
        assert!(source.contains("sequenceDiagram") && !source.contains("mermaid · diagram"));
        doc.toggle_source();
        let narrow = prepare(&markdown, 20, &mut doc, &mut worker);
        assert!(narrow.contains("mermaid · diagram") && !narrow.contains("sequenceDiagram"));
        assert!(prepare(&markdown, 80, &mut doc, &mut worker).contains("mermaid · diagram"));
    }

    #[test]
    fn stale_replies_are_discarded_after_source_or_width_change() {
        for new in [
            spec("flowchart TD\n A --> Changed"),
            spec("flowchart TD\n B --> Other"),
        ] {
            let old = spec(FLOW);
            let control = RenderControl::default();
            let (reply, receiver) = mpsc::sync_channel(1);
            let mut doc = DiagramDocument {
                requests: vec![new],
                pending: Some(Pending {
                    specs: vec![old.clone()],
                    control: control.clone(),
                    receiver,
                }),
                ..Default::default()
            };
            reply
                .send(vec![(old, DiagramResult::Rendered(vec!["STALE".into()]))])
                .unwrap();
            assert!(!doc.poll(&mut DiagramWorker::default()));
            assert!(control.is_cancelled());
            assert!(doc.ready.is_empty());
        }
    }

    #[test]
    fn closing_or_refreshing_cancels_owned_work_and_failures_are_cached() {
        let (reply, receiver) = mpsc::sync_channel(1);
        let control = RenderControl::default();
        let mut doc = DiagramDocument {
            pending: Some(Pending {
                specs: vec![],
                control: control.clone(),
                receiver,
            }),
            ..Default::default()
        };
        doc.reset();
        assert!(control.is_cancelled());
        assert!(reply.send(vec![]).is_err());
        let markdown = "```mermaid\npie\n \"Rust\" : 10\n```";
        let mut worker = DiagramWorker::default();
        prepare(markdown, 80, &mut doc, &mut worker);
        assert!(!doc.is_pending());
        assert!(!doc.poll(&mut worker));
        assert!(doc.pending.is_none());
    }

    #[test]
    fn only_mermaid_fences_schedule_work_and_document_count_is_bounded() {
        let mut doc = DiagramDocument::default();
        let markdown = format!(
            "```rust\n{FLOW}```\n\n    flowchart TD\n\n{}",
            (0..17)
                .map(|n| format!("```mermaid\nflowchart TD\n A --> N{n}\n```\n\n"))
                .collect::<String>()
        );
        let rendered = render_markdown_with_diagrams(&markdown, &Theme::default(), 80, None, &doc);
        assert_eq!(rendered.diagram_requests.len(), MAX_DIAGRAMS);
        doc.set_requests(rendered.diagram_requests.clone());
        let output = text(rendered);
        assert!(output.contains("Diagram limit reached"));
        assert!(output.contains("rust") && output.contains("flowchart TD"));
    }

    #[test]
    fn usage_guide_examples_render_without_fallback() {
        // An 80-column terminal leaves 65 columns after the viewer's 86%
        // modal, borders and scrollbar reservation.
        for width in [65, 80] {
            let mut doc = DiagramDocument::default();
            let output = prepare(
                include_str!("../docs/mermaid-mvp.md"),
                width,
                &mut doc,
                &mut DiagramWorker::default(),
            );
            assert_eq!(doc.requests.len(), 2);
            assert_eq!(output.matches("mermaid · diagram").count(), 2);
            assert!(!output.contains("Mermaid:"), "{output}");
            assert!(output.lines().take(16).any(|line| line.contains("Review")));
        }
    }

    #[test]
    fn wide_example_document_renders_every_diagram_clipped_at_65_columns() {
        // 65 is the content width of an 80-column terminal.
        let mut doc = DiagramDocument::default();
        let output = prepare(
            include_str!("../docs/mermaid-wide-example.md"),
            65,
            &mut doc,
            &mut DiagramWorker::default(),
        );
        assert_eq!(output.matches("mermaid · diagram").count(), 7, "{output}");
        assert!(!output.contains("Mermaid:"), "{output}");
        assert!(output.contains(CLIP_RIGHT));
        assert!(doc.max_pan() > 60);
    }

    const BACKLOG_STATES: &str = "flowchart TD\n    P[Pending] -->|accept| A[Accepted]\n    P -->|ask| Q[Question open]\n    Q -->|answered| P\n    P -->|request change| C[Change requested]\n    C -->|developer replies done| R[Re-review]\n    C -->|developer pushes back| R\n    R -->|accept| A\n    R -->|request change| C\n    P -->|note only| A\n";

    fn rendered(source: &str) -> String {
        match render_diagram(&spec(source), &RenderControl::default()) {
            DiagramResult::Rendered(lines) => lines.join("\n"),
            other => panic!("expected a diagram, got {other:?}"),
        }
    }

    #[test]
    fn dense_flowchart_draws_every_label_without_overdrawing() {
        let output = rendered(BACKLOG_STATES);
        assert!(!output.contains("Not shown"), "{output}");
        for (label, copies) in [
            ("accept", 2),
            ("request change", 2),
            ("developer replies done", 1),
            ("developer pushes back", 1),
        ] {
            assert_eq!(output.matches(label).count(), copies, "{label}\n{output}");
        }
        assert!(
            output.contains("Pending") && output.contains("Re-review"),
            "{output}"
        );
    }

    #[test]
    fn wrapped_label_counts_as_drawn() {
        let rows = ["| Scribe: brief |", "|  and summaries |"];
        assert!(label_drawn(&rows, "Scribe: brief and summaries"));
        assert!(!label_drawn(&rows, "summaries and Scribe:"));
        assert!(!label_drawn(&rows, "Scribe: brief and decisions"));
    }

    #[test]
    fn missing_edge_label_is_listed_with_its_endpoints() {
        let labels = [
            Label::new("Yes", "A -> B: \"Yes\"".into()),
            Label::new("No way", "A -> C: \"No way\"".into()),
        ];
        let out = append_legend("A -Yes-> B".into(), &labels);
        assert!(out.ends_with("Not shown:\n  A -> C: \"No way\""), "{out}");
        assert_eq!(append_legend("Yes No way".into(), &labels), "Yes No way");
    }

    #[test]
    fn one_drawn_copy_does_not_vouch_for_a_duplicate_label() {
        let labels = [
            Label::new("accept", "A -> B: \"accept\"".into()),
            Label::new("accept", "C -> D: \"accept\"".into()),
        ];
        let out = append_legend("A -accept-> B".into(), &labels);
        assert!(out.ends_with("Not shown:\n  C -> D: \"accept\""), "{out}");
    }

    #[test]
    fn clipped_subgraph_title_is_listed() {
        let labels = [Label::new(
            "Long group title",
            "group \"Long group title\"".into(),
        )];
        let out = append_legend("┌─Long gr─┐".into(), &labels);
        assert!(
            out.contains("Not shown:\n  group \"Long group title\""),
            "{out}"
        );
    }

    #[test]
    fn quoted_label_is_found_verbatim() {
        let output = rendered("flowchart TD\n A[\"Say #quot;hi#quot; now\"] --> B[Done]\n");
        assert!(!output.contains("Not shown"), "{output}");
    }

    #[test]
    fn short_label_is_not_vouched_for_by_a_longer_one() {
        let labels = [
            Label::new("request change", "long".into()),
            Label::new("change", "short".into()),
        ];
        assert_eq!(undrawn("| request change |", &labels), vec![1]);
    }

    #[test]
    fn wrapped_words_must_be_adjacent_whole_words() {
        let far = ["note", "", "", "a only"];
        assert!(!label_drawn(&far, "note only"));
        assert!(!label_drawn(&["task", "only"], "ask only"));
    }

    #[test]
    fn spread_labels_keep_the_renderer_scale() {
        let scale = cell_scale();
        assert_eq!((scale.cell_width, scale.cell_height), (8.0, 16.0));
    }

    #[test]
    fn sequence_legend_uses_participant_descriptions() {
        let src = "sequenceDiagram\n participant A as Alice\n participant B as Bob\n A->>B: hi\n";
        let out = rendered(src);
        assert!(!out.contains("A -> B"), "{out}");
    }
}
