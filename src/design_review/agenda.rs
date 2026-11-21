//! The review agenda: which items a review has, in meeting order, and the
//! state machine each item moves through. Pure: the room persists what these
//! functions return (`docs/development/features/design-review.md`).

use crate::db::design_review::DesignDecision;
use crate::design_packet::detectors::{DetectorHit, paths_intersect};

/// Where an agenda item stands (the plan's state diagram).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemState {
    Pending,
    QuestionOpen,
    ChangeRequested,
    ReReview,
    Accepted,
}

impl ItemState {
    pub fn as_str(self) -> &'static str {
        match self {
            ItemState::Pending => "pending",
            ItemState::QuestionOpen => "question-open",
            ItemState::ChangeRequested => "change-requested",
            ItemState::ReReview => "re-review",
            ItemState::Accepted => "accepted",
        }
    }

    /// An unknown stored value reads as `Pending`, the safe state: the item
    /// is shown again rather than silently counted as approved.
    pub fn parse(text: &str) -> Self {
        match text {
            "question-open" => ItemState::QuestionOpen,
            "change-requested" => ItemState::ChangeRequested,
            "re-review" => ItemState::ReReview,
            "accepted" => ItemState::Accepted,
            _ => ItemState::Pending,
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            ItemState::Pending => "○",
            ItemState::QuestionOpen => "?",
            ItemState::ChangeRequested => "✎",
            ItemState::ReReview => "↻",
            ItemState::Accepted => "✓",
        }
    }

    /// Whether the reviewer can act on the item now, which is where the room
    /// resumes. A question or a requested change waits on someone else.
    pub fn awaits_reviewer(self) -> bool {
        matches!(self, ItemState::Pending | ItemState::ReReview)
    }

    pub fn label(self) -> &'static str {
        match self {
            ItemState::Pending => "pending",
            ItemState::QuestionOpen => "question open",
            ItemState::ChangeRequested => "change requested",
            ItemState::ReReview => "re-review",
            ItemState::Accepted => "accepted",
        }
    }
}

/// What the reviewer (or the ladder) did to an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemAction {
    Accept,
    /// Accept with a note.
    Note,
    RequestChange,
    Ask,
    /// The developer replied (`done` or `pushback`) to a requested change.
    DeveloperReplied,
    /// A question was answered, so the item goes back to `back_to`, the
    /// state it had when the question was asked (`pending` or `re-review`).
    /// The caller keeps that state while the question is open.
    Answered {
        back_to: ItemState,
    },
}

/// The state `action` leads to, or why it is refused (worded for a toast).
pub fn transition(state: ItemState, action: ItemAction) -> Result<ItemState, &'static str> {
    use ItemAction::*;
    use ItemState::*;
    match (state, action) {
        (Accepted, _) => Err("Already accepted"),
        (Pending | ReReview, Accept | Note) => Ok(Accepted),
        (Pending | ReReview, RequestChange) => Ok(ChangeRequested),
        (Pending | ReReview, Ask) => Ok(QuestionOpen),
        (QuestionOpen, Answered { back_to }) if back_to.awaits_reviewer() => Ok(back_to),
        (QuestionOpen, Answered { .. }) => {
            Err("A question can only return to pending or re-review")
        }
        (QuestionOpen, _) => Err("A question is still open on this item"),
        (ChangeRequested, DeveloperReplied) => Ok(ReReview),
        (ChangeRequested, _) => {
            Err("Waiting for the developer's reply; the item returns for re-review")
        }
        (_, DeveloperReplied) => Err("No change is waiting on the developer"),
        (_, Answered { .. }) => Err("No question is open on this item"),
    }
}

/// What an agenda row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Brief,
    Architecture,
    KeyFlow,
    Fit,
    Decision,
    /// The cartographer's `unexplained` section.
    Unexplained,
    /// One uncovered detector hit.
    UnexplainedHit,
    /// A decision's `--ask`.
    Ask,
    PlanDrift,
}

impl ItemKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ItemKind::Brief => "brief",
            ItemKind::Architecture => "architecture",
            ItemKind::KeyFlow => "key-flow",
            ItemKind::Fit => "fit",
            ItemKind::Decision => "decision",
            ItemKind::Unexplained => "unexplained",
            ItemKind::UnexplainedHit => "unexplained-hit",
            ItemKind::Ask => "ask",
            ItemKind::PlanDrift => "plan-drift",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        [
            ItemKind::Brief,
            ItemKind::Architecture,
            ItemKind::KeyFlow,
            ItemKind::Fit,
            ItemKind::Decision,
            ItemKind::Unexplained,
            ItemKind::UnexplainedHit,
            ItemKind::Ask,
            ItemKind::PlanDrift,
        ]
        .into_iter()
        .find(|kind| kind.as_str() == text)
    }

    /// The packet section (`design_review_artifacts.section`) an item shows,
    /// for the kinds that are one.
    pub fn section_key(self) -> Option<&'static str> {
        match self {
            ItemKind::Brief
            | ItemKind::Architecture
            | ItemKind::KeyFlow
            | ItemKind::Fit
            | ItemKind::Unexplained
            | ItemKind::PlanDrift => Some(self.as_str()),
            _ => None,
        }
    }
}

/// An item the current inputs call for, before it is matched to a stored one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgendaDraft {
    pub kind: ItemKind,
    /// Stable identity within `kind`: a section key, a decision uid, or a
    /// detector hit's `<detector>:<path>`.
    pub ref_key: String,
    pub label: Option<String>,
    pub title: String,
    pub paths: Vec<String>,
}

/// Everything the agenda is derived from.
pub struct AgendaInputs<'a> {
    pub decisions: &'a [DesignDecision],
    /// Detector hits no decision covers (`detectors::uncovered`).
    pub uncovered_hits: &'a [DetectorHit],
    /// Whether the `unexplained` artifact exists.
    pub has_unexplained_artifact: bool,
}

fn section(kind: ItemKind, title: &str) -> AgendaDraft {
    AgendaDraft {
        kind,
        ref_key: kind.as_str().to_string(),
        label: None,
        title: title.to_string(),
        paths: Vec::new(),
    }
}

/// The agenda in meeting order: brief, architecture, key flow, fit, each
/// non-superseded decision, unexplained changes, the developer's `--ask`
/// questions, plan drift.
pub fn derive_agenda(inputs: &AgendaInputs) -> Vec<AgendaDraft> {
    let live: Vec<&DesignDecision> = inputs
        .decisions
        .iter()
        .filter(|d| d.status != "superseded" && d.source != "review")
        .collect();

    let mut out = vec![
        section(ItemKind::Brief, "Brief"),
        section(ItemKind::Architecture, "Architecture"),
        section(ItemKind::KeyFlow, "Key flow"),
        section(ItemKind::Fit, "Fit"),
    ];
    for d in &live {
        out.push(AgendaDraft {
            kind: ItemKind::Decision,
            ref_key: d.uid.clone(),
            label: Some(format!("d{}", d.seq)),
            title: format!("d{} {}", d.seq, d.title),
            paths: d.paths.clone(),
        });
    }
    if inputs.has_unexplained_artifact {
        out.push(section(ItemKind::Unexplained, "Unexplained changes"));
    }
    for hit in inputs.uncovered_hits {
        out.push(AgendaDraft {
            kind: ItemKind::UnexplainedHit,
            ref_key: format!(
                "{}:{}",
                hit.kind.as_str(),
                hit.paths.first().map_or("", String::as_str)
            ),
            label: None,
            title: hit.summary.clone(),
            paths: hit.paths.clone(),
        });
    }
    for d in &live {
        if let Some(ask) = d.ask.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
            out.push(AgendaDraft {
                kind: ItemKind::Ask,
                ref_key: d.uid.clone(),
                label: Some(format!("d{}?", d.seq)),
                title: format!("d{}? {ask}", d.seq),
                paths: d.paths.clone(),
            });
        }
    }
    out.push(section(ItemKind::PlanDrift, "Plan drift"));
    out
}

/// What an item shows while its packet section has not been written.
pub const NOT_PREPARED: &str = "Not prepared yet";

fn bullet_list(items: &[String], code: bool) -> String {
    items
        .iter()
        .map(|item| {
            if code {
                format!("- `{item}`\n")
            } else {
                format!("- {item}\n")
            }
        })
        .collect()
}

/// The content pane for a decision: kind, why, rejected options, ask, paths.
pub fn decision_markdown(d: &DesignDecision) -> String {
    let mut out = format!("# d{} {}\n\n**Kind:** {}\n\n", d.seq, d.title, d.kind);
    out.push_str(&format!("## Why\n\n{}\n\n", d.why.trim()));
    if !d.rejected.is_empty() {
        out.push_str("## Rejected\n\n");
        out.push_str(&bullet_list(&d.rejected, false));
        out.push('\n');
    }
    if let Some(ask) = d.ask.as_deref().filter(|a| !a.trim().is_empty()) {
        out.push_str(&format!("## Ask\n\n{}\n\n", ask.trim()));
    }
    if !d.paths.is_empty() {
        out.push_str("## Paths\n\n");
        out.push_str(&bullet_list(&d.paths, true));
    }
    out
}

/// The content pane for a decision's `--ask`.
pub fn ask_markdown(d: &DesignDecision) -> String {
    let ask = d.ask.as_deref().unwrap_or("").trim();
    let mut out = format!(
        "# d{}? {}\n\nThe developer asked, about **d{} {}**:\n\n> {}\n\n",
        d.seq, ask, d.seq, d.title, ask
    );
    if !d.paths.is_empty() {
        out.push_str("## Paths\n\n");
        out.push_str(&bullet_list(&d.paths, true));
    }
    out
}

/// The content pane for an uncovered detector hit.
pub fn hit_markdown(summary: &str, paths: &[String]) -> String {
    let mut out = format!(
        "# {summary}\n\nNo decision covers this change. Ask the developer to \
         explain it, or request a change.\n\n"
    );
    if !paths.is_empty() {
        out.push_str("## Paths\n\n");
        out.push_str(&bullet_list(paths, true));
    }
    out
}

/// The ladder's L0 answer: what the stored record already says. It never
/// invents a motive; a record with nothing to quote says so.
pub fn record_answer(d: &DesignDecision) -> String {
    let mut out = format!(
        "Recorded for d{} {}:\n\nWhy: {}",
        d.seq,
        d.title,
        d.why.trim()
    );
    if d.rejected.is_empty() {
        out.push_str("\n\nRejected: none recorded.");
    } else {
        out.push_str("\n\nRejected:\n");
        out.push_str(&bullet_list(&d.rejected, false));
    }
    out
}

/// The L0 answer for an item with no stored record to quote.
pub const NO_RECORD_ANSWER: &str =
    "No decision is recorded for this item, so there is nothing to look up.";

/// A stored item, reduced to what matching needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredItem {
    pub id: String,
    pub kind: String,
    pub ref_key: Option<String>,
    pub label: Option<String>,
    pub title: String,
    pub paths: Vec<String>,
    pub state: ItemState,
}

/// Whether an item still matches the change it reviews. Stored as an integer
/// in `design_review_items.stale` (0, 1, 2); an unknown value reads as
/// `Current`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Staleness {
    #[default]
    Current,
    /// A decision whose recorded paths have all left the diff.
    PossiblyStale,
    /// The inputs no longer produce the item (a superseded decision, a hit
    /// that is now covered, a vanished section). Kept for its thread.
    Retired,
}

impl Staleness {
    pub fn as_i64(self) -> i64 {
        match self {
            Staleness::Current => 0,
            Staleness::PossiblyStale => 1,
            Staleness::Retired => 2,
        }
    }

    pub fn from_i64(value: i64) -> Self {
        match value {
            1 => Staleness::PossiblyStale,
            2 => Staleness::Retired,
            _ => Staleness::Current,
        }
    }

    /// The agenda row's suffix.
    pub fn tag(self) -> &'static str {
        match self {
            Staleness::Current => "",
            Staleness::PossiblyStale => " ⚠ stale?",
            Staleness::Retired => " · off agenda",
        }
    }

    /// The content pane's banner.
    pub fn banner(self) -> Option<&'static str> {
        match self {
            Staleness::Current => None,
            Staleness::PossiblyStale => {
                Some("Possibly stale: none of this decision's files are in the diff any more")
            }
            Staleness::Retired => {
                Some("No longer on the agenda: what produced this item is gone; its thread is kept")
            }
        }
    }
}

/// One row of the merged agenda.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedItem {
    /// The stored item this keeps (and whose state it carries), or `None` for
    /// a new one.
    pub existing_id: Option<String>,
    pub kind: String,
    pub ref_key: Option<String>,
    pub label: Option<String>,
    pub title: String,
    pub paths: Vec<String>,
    pub state: ItemState,
    pub stale: Staleness,
}

/// "Possibly stale": a decision whose recorded paths have all left the
/// current diff. Unknown without a diff, and never for a decision that
/// names no paths.
pub fn possibly_stale(kind: &str, paths: &[String], current_diff: Option<&[String]>) -> bool {
    let Some(current) = current_diff else {
        return false;
    };
    kind == ItemKind::Decision.as_str()
        && !paths.is_empty()
        && !paths
            .iter()
            .any(|p| current.iter().any(|c| paths_intersect(p, c)))
}

/// Match `drafts` to `existing` by kind and `ref_key`. A match keeps the stored
/// item's id and state; its position, title, label and paths follow the new
/// derivation. Drafts with no match are new and `Pending`. A stored item that
/// no draft matches (its decision was superseded, its hit is now covered)
/// stays, after the derived ones and marked `Retired`, because its thread
/// belongs to the review.
pub fn merge_agenda(
    existing: &[StoredItem],
    drafts: &[AgendaDraft],
    current_diff: Option<&[String]>,
) -> Vec<PlannedItem> {
    let mut used = vec![false; existing.len()];
    let mut out = Vec::with_capacity(drafts.len());
    for draft in drafts {
        let hit = existing.iter().enumerate().find(|(i, e)| {
            !used[*i]
                && e.kind == draft.kind.as_str()
                && e.ref_key.as_deref() == Some(draft.ref_key.as_str())
        });
        let (existing_id, state) = match hit {
            Some((i, e)) => {
                used[i] = true;
                (Some(e.id.clone()), e.state)
            }
            None => (None, ItemState::Pending),
        };
        out.push(PlannedItem {
            existing_id,
            kind: draft.kind.as_str().to_string(),
            ref_key: Some(draft.ref_key.clone()),
            // A `C1`/`Q1` label from a brief is the developer's handle for the
            // item, so a re-derivation must not overwrite it with `d4`.
            label: hit
                .and_then(|(_, e)| e.label.clone())
                .filter(|l| super::brief::is_review_label(l))
                .or_else(|| draft.label.clone()),
            title: draft.title.clone(),
            paths: draft.paths.clone(),
            state,
            stale: if possibly_stale(draft.kind.as_str(), &draft.paths, current_diff) {
                Staleness::PossiblyStale
            } else {
                Staleness::Current
            },
        });
    }
    for (i, e) in existing.iter().enumerate() {
        if !used[i] {
            out.push(PlannedItem {
                existing_id: Some(e.id.clone()),
                kind: e.kind.clone(),
                ref_key: e.ref_key.clone(),
                label: e.label.clone(),
                title: e.title.clone(),
                paths: e.paths.clone(),
                state: e.state,
                stale: Staleness::Retired,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design_packet::detectors::DetectorKind;

    fn decision(seq: i64, status: &str, ask: Option<&str>, paths: &[&str]) -> DesignDecision {
        DesignDecision {
            uid: format!("u{seq}"),
            feature_id: "f".into(),
            project_id: "p".into(),
            seq,
            kind: "architecture".into(),
            title: format!("title {seq}"),
            why: "why".into(),
            rejected: vec![],
            ask: ask.map(str::to_string),
            supersedes: None,
            paths: paths.iter().map(|p| p.to_string()).collect(),
            head: None,
            session_id: None,
            harness: "claude".into(),
            source: "agent".into(),
            status: status.into(),
            created_at: "t".into(),
        }
    }

    fn hit(path: &str) -> DetectorHit {
        DetectorHit {
            kind: DetectorKind::Dependency,
            paths: vec![path.into()],
            summary: format!("dependency in {path}"),
        }
    }

    fn titles(drafts: &[AgendaDraft]) -> Vec<String> {
        drafts.iter().map(|d| d.title.clone()).collect()
    }

    #[test]
    fn agenda_follows_meeting_order() {
        let decisions = vec![
            decision(1, "open", Some("sure?"), &["src/a.rs"]),
            decision(2, "superseded", Some("old ask"), &["src/b.rs"]),
            decision(3, "accepted", None, &["src/c.rs"]),
        ];
        let hits = vec![hit("Cargo.toml"), hit("go.mod")];
        let drafts = derive_agenda(&AgendaInputs {
            decisions: &decisions,
            uncovered_hits: &hits,
            has_unexplained_artifact: true,
        });
        assert_eq!(
            titles(&drafts),
            [
                "Brief",
                "Architecture",
                "Key flow",
                "Fit",
                "d1 title 1",
                "d3 title 3",
                "Unexplained changes",
                "dependency in Cargo.toml",
                "dependency in go.mod",
                "d1? sure?",
                "Plan drift",
            ]
        );
        assert_eq!(drafts[4].paths, ["src/a.rs"]);
        assert_eq!(drafts[4].label.as_deref(), Some("d1"));
        assert_eq!(drafts[7].kind, ItemKind::UnexplainedHit);
        assert_eq!(drafts[7].paths, ["Cargo.toml"]);
        assert_eq!(drafts[9].paths, ["src/a.rs"]);

        let bare = derive_agenda(&AgendaInputs {
            decisions: &[],
            uncovered_hits: &[],
            has_unexplained_artifact: false,
        });
        assert_eq!(
            titles(&bare),
            ["Brief", "Architecture", "Key flow", "Fit", "Plan drift"]
        );
    }

    fn stored(from: &[AgendaDraft], accepted: &[&str]) -> Vec<StoredItem> {
        from.iter()
            .enumerate()
            .map(|(i, d)| StoredItem {
                id: format!("id{i}"),
                kind: d.kind.as_str().into(),
                ref_key: Some(d.ref_key.clone()),
                label: d.label.clone(),
                title: d.title.clone(),
                paths: d.paths.clone(),
                state: if accepted.contains(&d.ref_key.as_str()) {
                    ItemState::Accepted
                } else {
                    ItemState::Pending
                },
            })
            .collect()
    }

    #[test]
    fn rederiving_keeps_state_and_adds_new_items() {
        let first = derive_agenda(&AgendaInputs {
            decisions: &[decision(1, "open", None, &["src/a.rs"])],
            uncovered_hits: &[],
            has_unexplained_artifact: false,
        });
        let existing = stored(&first, &["u1", "brief"]);

        let second = derive_agenda(&AgendaInputs {
            decisions: &[
                decision(1, "open", None, &["src/a.rs"]),
                decision(2, "open", None, &["src/b.rs"]),
            ],
            uncovered_hits: &[],
            has_unexplained_artifact: false,
        });
        let merged = merge_agenda(&existing, &second, None);
        assert_eq!(merged.len(), second.len());
        let by_title = |t: &str| merged.iter().find(|m| m.title == t).unwrap();
        assert_eq!(by_title("Brief").state, ItemState::Accepted);
        assert!(by_title("Brief").existing_id.is_some());
        assert_eq!(by_title("d1 title 1").state, ItemState::Accepted);
        assert_eq!(by_title("d2 title 2").state, ItemState::Pending);
        assert_eq!(by_title("d2 title 2").existing_id, None);
        // New order follows the derivation.
        assert_eq!(
            merged.iter().map(|m| m.title.as_str()).collect::<Vec<_>>(),
            titles(&second)
        );
    }

    #[test]
    fn vanished_items_stay_flagged_after_the_derived_ones() {
        let first = derive_agenda(&AgendaInputs {
            decisions: &[decision(1, "open", None, &[])],
            uncovered_hits: &[hit("Cargo.toml")],
            has_unexplained_artifact: false,
        });
        let existing = stored(&first, &[]);
        let second = derive_agenda(&AgendaInputs {
            decisions: &[decision(1, "superseded", None, &[])],
            uncovered_hits: &[],
            has_unexplained_artifact: false,
        });
        let merged = merge_agenda(&existing, &second, None);
        assert_eq!(merged.len(), second.len() + 2);
        let tail: Vec<_> = merged[second.len()..].iter().collect();
        assert!(
            tail.iter()
                .all(|m| m.stale == Staleness::Retired && m.existing_id.is_some())
        );
    }

    #[test]
    fn a_decision_is_possibly_stale_when_all_its_paths_left_the_diff() {
        let current = vec!["src/a.rs".to_string(), "docs/x.md".to_string()];
        let cur = Some(current.as_slice());
        let s = |paths: &[&str]| {
            let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
            possibly_stale("decision", &paths, cur)
        };
        assert!(!s(&["src/a.rs"]));
        assert!(
            !s(&["src/a.rs", "src/gone.rs"]),
            "one path still in the diff"
        );
        assert!(!s(&["src"]), "a directory the diff still touches");
        assert!(s(&["src/gone.rs"]));
        assert!(s(&["src/gone.rs", "src/also-gone.rs"]));
        assert!(!s(&[]), "no paths, nothing to compare");
        assert!(!possibly_stale("decision", &["src/gone.rs".into()], None));
        assert!(!possibly_stale("ask", &["src/gone.rs".into()], cur));

        let drafts = derive_agenda(&AgendaInputs {
            decisions: &[decision(1, "open", None, &["src/gone.rs"])],
            uncovered_hits: &[],
            has_unexplained_artifact: false,
        });
        let merged = merge_agenda(&[], &drafts, cur);
        assert_eq!(
            merged.iter().find(|m| m.kind == "decision").unwrap().stale,
            Staleness::PossiblyStale
        );
        assert!(
            merged
                .iter()
                .filter(|m| m.kind != "decision")
                .all(|m| m.stale == Staleness::Current)
        );
    }

    #[test]
    fn content_markdown_covers_the_record_and_its_lookup() {
        let mut d = decision(4, "open", Some("worth a table?"), &["src/a.rs"]);
        d.rejected = vec!["db write: contention".into()];
        let md = decision_markdown(&d);
        for want in [
            "# d4 title 4",
            "**Kind:** architecture",
            "## Why",
            "## Rejected",
            "- db write: contention",
            "## Ask",
            "worth a table?",
            "- `src/a.rs`",
        ] {
            assert!(md.contains(want), "{want}: {md}");
        }
        let answer = record_answer(&d);
        assert!(answer.contains("Why: why"));
        assert!(answer.contains("- db write: contention"));
        d.rejected.clear();
        assert!(record_answer(&d).contains("none recorded"));
        assert!(ask_markdown(&d).contains("> worth a table?"));
        assert!(hit_markdown("new dependency", &["Cargo.toml".into()]).contains("`Cargo.toml`"));
    }

    #[test]
    fn transitions_follow_the_state_diagram() {
        use ItemAction::*;
        use ItemState::*;
        assert_eq!(transition(Pending, Accept), Ok(Accepted));
        assert_eq!(transition(Pending, Note), Ok(Accepted));
        assert_eq!(transition(Pending, RequestChange), Ok(ChangeRequested));
        assert_eq!(transition(Pending, Ask), Ok(QuestionOpen));
        let answered = |back_to| Answered { back_to };
        assert_eq!(transition(QuestionOpen, answered(Pending)), Ok(Pending));
        assert_eq!(transition(QuestionOpen, answered(ReReview)), Ok(ReReview));
        assert!(transition(QuestionOpen, answered(Accepted)).is_err());
        assert_eq!(transition(ReReview, Accept), Ok(Accepted));
        assert_eq!(transition(ReReview, RequestChange), Ok(ChangeRequested));
        assert!(transition(QuestionOpen, Accept).is_err());
        assert!(transition(ChangeRequested, Accept).is_err());
        assert!(transition(Accepted, Ask).is_err());
        assert!(transition(Pending, answered(Pending)).is_err());
        assert_eq!(transition(ChangeRequested, DeveloperReplied), Ok(ReReview));
        for state in [Pending, QuestionOpen, ReReview, Accepted] {
            assert!(transition(state, DeveloperReplied).is_err(), "{state:?}");
        }
        for state in [Pending, QuestionOpen, ChangeRequested, ReReview, Accepted] {
            assert_eq!(ItemState::parse(state.as_str()), state);
        }
        assert_eq!(ItemState::parse("nonsense"), Pending);
    }
}
