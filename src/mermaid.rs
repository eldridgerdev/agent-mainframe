//! Bounded terminal Mermaid rendering. App schedules work; renderers only read artifacts.

use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;

use selkie::diagrams::{Diagram, DiagramType};
use selkie::layout::{NodeShape, NodeSizeConfig, SizeEstimator, ToLayoutGraph};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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
    pub source: String,
    /// Content width, excluding Markdown prefixes and the diagram frame.
    pub width: usize,
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
    requests: Vec<DiagramSpec>,
    ready: Artifacts,
    pending: Option<Pending>,
}

impl DiagramDocument {
    /// Called with pure parser output; never starts work during a draw.
    pub fn set_requests(&mut self, requests: Vec<DiagramSpec>) {
        self.requests = requests;
    }

    pub fn result(&self, source: &str, width: usize) -> Option<&DiagramResult> {
        self.ready
            .iter()
            .find(|(spec, _)| spec.width == width && spec.source == source)
            .map(|(_, result)| result)
    }

    pub fn toggle_source(&mut self) {
        self.show_source = !self.show_source;
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
                .any(|spec| self.result(&spec.source, spec.width).is_none())
    }

    /// App calls this outside drawing. A completed result invalidates the host's
    /// line cache even if terminal width has not changed.
    pub fn poll(&mut self, worker: &mut DiagramWorker) -> bool {
        self.ready.retain(|(spec, _)| self.requests.contains(spec));
        let missing: Vec<_> = self
            .requests
            .iter()
            .filter(|spec| self.result(&spec.source, spec.width).is_none())
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
    if spec.width == 0 {
        return Err("Widen the window to display this diagram".into());
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
            let labels: Vec<_> = data
                .vertices
                .values()
                .map(|vertex| vertex.text.as_deref().unwrap_or(&vertex.id))
                .chain(data.edges.iter().map(|edge| edge.text.as_str()))
                .chain(data.subgraphs.iter().map(|group| group.title.as_str()))
                .filter(|label| !label.is_empty())
                .map(str::to_owned)
                .collect();
            check_labels(&labels)?;
            let graph = db
                .to_layout_graph(&TerminalSizeEstimator)
                .map_err(|e| diagnostic(&e.to_string()))?;
            let graph = selkie::layout::layout(graph).map_err(|e| diagnostic(&e.to_string()))?;
            check_cancelled(control)?;
            let scale = selkie::render::ascii::scale::CellScale::default();
            let cols = scale.to_col(graph.width.unwrap_or(400.0)).saturating_add(8);
            let rows = scale
                .to_row(graph.height.unwrap_or(300.0))
                .saturating_add(4);
            if cols.saturating_mul(rows) > 32_768 {
                return Err("Diagram exceeds the terminal canvas limit".into());
            }
            // max_width would silently truncate. Render uncompressed and refuse
            // overflow ourselves so relationships and labels cannot be clipped.
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
            let labels: Vec<_> = db
                .get_actors()
                .values()
                .map(|actor| actor.description.as_str())
                .chain(
                    db.get_messages()
                        .iter()
                        .map(|message| message.message.as_str()),
                )
                .filter(|label| !label.is_empty())
                .map(str::to_owned)
                .collect();
            check_labels(&labels)?;
            // Conservative upper bound on the upstream actor/gap canvas: each
            // gap fits the longest label, and each event needs two rows.
            let max_label = labels
                .iter()
                .map(|label| label.chars().count())
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
    let width = text.lines().map(UnicodeWidthStr::width).max().unwrap_or(0);
    if width > spec.width {
        return Err(format!("Diagram needs {width} columns; widen the window"));
    }
    if text.trim().is_empty() || labels.iter().any(|label| !text.contains(label.as_str())) {
        return Err("This diagram cannot preserve every label; showing source".into());
    }
    Ok(text)
}

fn check_cancelled(control: &RenderControl) -> Result<(), String> {
    if control.is_cancelled() {
        Err("Diagram rendering cancelled".into())
    } else {
        Ok(())
    }
}

fn check_labels(labels: &[String]) -> Result<(), String> {
    if labels.iter().any(|label| {
        label.chars().count() > 120
            || label.chars().any(char::is_control)
            || label.contains(['<', '>'])
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

    const FLOW: &str = "flowchart TD\n A[Start] --> B{Ready?}\n B -->|Yes| C[Deploy]\n B -->|No| D[Fix]\n D --> B\n";
    const SEQUENCE: &str = "sequenceDiagram\n participant User\n participant AMF\n User->>AMF: Open plan\n alt Valid\n AMF-->>User: Diagram\n else Invalid\n AMF-->>User: Source\n end\n";

    fn spec(source: &str, width: usize) -> DiagramSpec {
        DiagramSpec {
            source: source.into(),
            width,
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
        let DiagramResult::Rendered(lines) =
            render_diagram(&spec(FLOW, 80), &RenderControl::default())
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
            render_diagram(&spec(source, 120), &RenderControl::default()),
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
                100,
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
            render_diagram(&spec(SEQUENCE, 80), &RenderControl::default())
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
    fn narrow_diagrams_refuse_overflow_instead_of_clipping() {
        for width in [0, 1, 10, 20] {
            assert!(
                matches!(render_diagram(&spec(SEQUENCE, width), &RenderControl::default()), DiagramResult::Fallback(reason) if reason.contains("widen") || reason.contains("Widen"))
            );
        }
        for width in [60, 80, 120] {
            let DiagramResult::Rendered(lines) =
                render_diagram(&spec(SEQUENCE, width), &RenderControl::default())
            else {
                panic!("expected sequence at {width}")
            };
            assert!(
                lines
                    .iter()
                    .all(|line| UnicodeWidthStr::width(line.as_str()) <= width)
            );
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
                matches!(render_diagram(&spec(&source, 80), &RenderControl::default()), DiagramResult::Fallback(reason) if reason.contains("limit"))
            );
        }
        let control = RenderControl::default();
        control.cancel();
        assert_eq!(
            render_diagram(&spec(FLOW, 80), &control),
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
            let Some(DiagramResult::Rendered(lines)) = doc.result(&source.source, source.width)
            else {
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
        assert!(narrow.contains("widen") && narrow.contains("sequenceDiagram"));
        assert!(prepare(&markdown, 80, &mut doc, &mut worker).contains("mermaid · diagram"));
    }

    #[test]
    fn stale_replies_are_discarded_after_source_or_width_change() {
        for new in [spec("flowchart TD\n A --> Changed", 80), spec(FLOW, 40)] {
            let old = spec(FLOW, 80);
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
}
