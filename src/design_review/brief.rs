//! The refinement brief (plan section 5): what the reviewer sends the
//! developer when a review round finishes with changes requested. Pure: the
//! room gathers the items and their reviewer turns, this module assigns
//! labels, renders the text and decides whether it goes inline or through a
//! file.

use crate::token_cost::ledger::estimate_tokens_from_bytes;

use super::agenda::ItemKind;

/// A brief of at least this many estimated tokens is written to a file and
/// the paste points at it, so a long brief does not flood the agent's input.
pub const INLINE_TOKEN_LIMIT: u64 = 2_000;

/// Where a large brief is written, relative to the feature's workdir.
pub fn brief_relative_path(round: i64) -> String {
    format!(".amf/design-review/round-{round}.md")
}

/// Whether an item is a requested change (`C1`) or an open question (`Q1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestKind {
    Change,
    Question,
}

impl RequestKind {
    fn prefix(self) -> char {
        match self {
            RequestKind::Change => 'C',
            RequestKind::Question => 'Q',
        }
    }
}

/// One item for the brief, in agenda order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BriefInput {
    pub item_id: String,
    pub kind: RequestKind,
    /// A label the item already carries (a second finish of the same round
    /// keeps `C1` on the item that had it).
    pub existing_label: Option<String>,
    /// The bracketed part of its line, see [`item_tag`].
    pub tag: String,
    /// The reviewer's request or question, from the item's newest reviewer
    /// turn. Whitespace is collapsed so each item is one line.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Brief {
    pub round: i64,
    /// `(item id, label)` for every input, to persist on the items.
    pub labels: Vec<(String, String)>,
    /// The full text, every item and the reply instructions.
    pub text: String,
}

impl Brief {
    pub fn estimated_tokens(&self) -> u64 {
        estimate_tokens_from_bytes(self.text.len())
    }
}

/// A review label: `C<n>` or `Q<n>`, case-insensitive, `n` at least 1.
/// Returns the upper-cased label and its kind.
pub fn parse_label(text: &str) -> Option<(String, RequestKind)> {
    let text = text.trim();
    let mut chars = text.chars();
    let kind = match chars.next()? {
        'C' | 'c' => RequestKind::Change,
        'Q' | 'q' => RequestKind::Question,
        _ => return None,
    };
    let digits = chars.as_str();
    // Bounded, so `C99999999999999999999` is a typo, not an overflow.
    if digits.is_empty()
        || digits.len() > 6
        || !digits.bytes().all(|b| b.is_ascii_digit())
        || digits.bytes().all(|b| b == b'0')
    {
        return None;
    }
    let number: u32 = digits.parse().ok()?;
    Some((format!("{}{number}", kind.prefix()), kind))
}

/// Whether `label` is a label this module assigns (as opposed to the agenda's
/// own `d4` labels).
pub fn is_review_label(label: &str) -> bool {
    parse_label(label).is_some_and(|(canonical, _)| canonical == label)
}

fn number_of(label: &str) -> u32 {
    label[1..].parse().unwrap_or(0)
}

/// The bracketed part of an item's line: `d4 <title>` for a decision,
/// `unexplained: <summary>` for an undeclared change, `d7` for a question on
/// a decision or its ask, and the item's own title for a packet section.
/// `decision` is the item's decision, `(seq, title)`.
pub fn item_tag(
    kind: Option<ItemKind>,
    decision: Option<(i64, &str)>,
    item_title: &str,
    request: RequestKind,
) -> String {
    match (kind, decision) {
        (Some(ItemKind::Decision), Some((seq, title))) => match request {
            RequestKind::Change => format!("d{seq} {title}"),
            RequestKind::Question => format!("d{seq}"),
        },
        (Some(ItemKind::Ask), Some((seq, _))) => format!("d{seq}"),
        (Some(ItemKind::Unexplained | ItemKind::UnexplainedHit), _) => {
            format!("unexplained: {item_title}")
        }
        _ => item_title.to_string(),
    }
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn counted(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Label every input and render the brief. Changes come first, then
/// questions, each in the order given. An input keeps its existing label
/// when that is a valid label of its kind and unused; the rest take the
/// lowest free number. `reserved` holds the labels of the round's other
/// items, the ones not in this brief, which stay theirs, so a reply's label
/// names one item.
pub fn build_brief(round: i64, inputs: &[BriefInput], reserved: &[String]) -> Brief {
    let mut labelled: Vec<(&BriefInput, String)> = Vec::new();
    for kind in [RequestKind::Change, RequestKind::Question] {
        let group: Vec<&BriefInput> = inputs.iter().filter(|i| i.kind == kind).collect();
        let mut taken: Vec<u32> = reserved
            .iter()
            .filter_map(|label| parse_label(label))
            .filter(|(_, k)| *k == kind)
            .map(|(label, _)| number_of(&label))
            .collect();
        let mut keep: Vec<Option<u32>> = Vec::new();
        for input in &group {
            let existing = input
                .existing_label
                .as_deref()
                .and_then(parse_label)
                .filter(|(_, k)| *k == kind)
                .map(|(label, _)| number_of(&label))
                .filter(|n| !taken.contains(n));
            if let Some(n) = existing {
                taken.push(n);
            }
            keep.push(existing);
        }
        let mut next = 1;
        for (input, kept) in group.into_iter().zip(keep) {
            let n = kept.unwrap_or_else(|| {
                while taken.contains(&next) {
                    next += 1;
                }
                taken.push(next);
                next
            });
            labelled.push((input, format!("{}{n}", kind.prefix())));
        }
    }

    let changes: Vec<&(&BriefInput, String)> = labelled
        .iter()
        .filter(|(i, _)| i.kind == RequestKind::Change)
        .collect();
    let questions: Vec<&(&BriefInput, String)> = labelled
        .iter()
        .filter(|(i, _)| i.kind == RequestKind::Question)
        .collect();

    let mut text = format!(
        "Design review round {round}: {}, {}.\n\n",
        counted(changes.len(), "change", "changes"),
        counted(questions.len(), "question", "questions"),
    );
    for (input, label) in changes.iter().chain(questions.iter()).map(|p| (p.0, &p.1)) {
        text.push_str(&format!(
            "{label} [{}] {}\n",
            collapse(&input.tag),
            collapse(&input.text)
        ));
    }
    text.push_str("\nReply once per item, one line:\n");
    if let Some((_, first)) = changes.first() {
        let second = changes.get(1).map_or(first, |(_, label)| label);
        text.push_str(&format!(
            "  amf review reply {first} --done \"<what changed>\"\n"
        ));
        text.push_str(&format!(
            "  amf review reply {second} --pushback \"<why not>\"\n"
        ));
    }
    if let Some((_, first)) = questions.first() {
        text.push_str(&format!(
            "  amf review reply {first} --answer \"<answer>\"\n"
        ));
    }
    text.push_str("If a change replaces a recorded decision, add one with --supersedes dN.\n");

    let labels = labelled
        .iter()
        .map(|(input, label)| (input.item_id.clone(), label.clone()))
        .collect();
    Brief {
        round,
        labels,
        text,
    }
}

/// What gets pasted, and the file it points at when the brief is large.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    pub inline: String,
    /// `(path relative to the workdir, contents)`, written before the paste.
    pub file: Option<(String, String)>,
}

/// Inline under [`INLINE_TOKEN_LIMIT`] estimated tokens; otherwise the full
/// text goes to [`brief_relative_path`] and the paste is a short pointer at
/// `pointer_path` (the file's path as the agent should open it).
pub fn deliver(brief: &Brief, pointer_path: &str) -> Delivery {
    if brief.estimated_tokens() < INLINE_TOKEN_LIMIT {
        return Delivery {
            inline: brief.text.clone(),
            file: None,
        };
    }
    let headline = brief.text.lines().next().unwrap_or_default();
    Delivery {
        inline: format!(
            "{headline}\nRead {pointer_path} for every item and how to reply, then answer \
             each one with `amf review reply <label> ...`."
        ),
        file: Some((brief_relative_path(brief.round), brief.text.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(id: &str, kind: RequestKind, tag: &str, text: &str) -> BriefInput {
        BriefInput {
            item_id: id.into(),
            kind,
            existing_label: None,
            tag: tag.into(),
            text: text.into(),
        }
    }

    fn fixture() -> Vec<BriefInput> {
        vec![
            input(
                "i1",
                RequestKind::Change,
                "d4 Outbox file",
                "Ingest on a timer as well as at turn end.",
            ),
            input(
                "i2",
                RequestKind::Question,
                "d7",
                "Why not one table per review?",
            ),
            input(
                "i3",
                RequestKind::Change,
                "unexplained: new dependency `ulid`",
                "Justify\nor remove.",
            ),
        ]
    }

    #[test]
    fn renders_the_plan_shape_exactly() {
        let brief = build_brief(1, &fixture(), &[]);
        assert_eq!(
            brief.text,
            "Design review round 1: 2 changes, 1 question.\n\
             \n\
             C1 [d4 Outbox file] Ingest on a timer as well as at turn end.\n\
             C2 [unexplained: new dependency `ulid`] Justify or remove.\n\
             Q1 [d7] Why not one table per review?\n\
             \n\
             Reply once per item, one line:\n\
             \x20 amf review reply C1 --done \"<what changed>\"\n\
             \x20 amf review reply C2 --pushback \"<why not>\"\n\
             \x20 amf review reply Q1 --answer \"<answer>\"\n\
             If a change replaces a recorded decision, add one with --supersedes dN.\n"
        );
        assert_eq!(
            brief.labels,
            [
                ("i1".to_string(), "C1".to_string()),
                ("i3".to_string(), "C2".to_string()),
                ("i2".to_string(), "Q1".to_string()),
            ]
        );
    }

    #[test]
    fn a_single_change_uses_its_label_for_both_examples() {
        let brief = build_brief(2, &fixture()[..1], &[]);
        assert!(
            brief
                .text
                .starts_with("Design review round 2: 1 change, 0 questions.\n")
        );
        assert!(brief.text.contains("amf review reply C1 --done"));
        assert!(brief.text.contains("amf review reply C1 --pushback"));
        assert!(!brief.text.contains("--answer"));
    }

    #[test]
    fn questions_only_skip_the_change_examples() {
        let brief = build_brief(1, &fixture()[1..2], &[]);
        assert!(brief.text.contains("0 changes, 1 question."));
        assert!(!brief.text.contains("--done"));
        assert!(brief.text.contains("amf review reply Q1 --answer"));
    }

    #[test]
    fn existing_labels_are_kept_and_new_ones_take_the_free_numbers() {
        let mut inputs = fixture();
        inputs[2].existing_label = Some("C1".into());
        inputs[0].existing_label = Some("d4".into());
        inputs[1].existing_label = Some("C1".into()); // wrong kind
        let brief = build_brief(1, &inputs, &[]);
        let label = |id: &str| {
            brief
                .labels
                .iter()
                .find(|(i, _)| i == id)
                .map(|(_, l)| l.as_str())
                .unwrap()
        };
        assert_eq!(label("i3"), "C1");
        assert_eq!(label("i1"), "C2");
        assert_eq!(label("i2"), "Q1");
    }

    #[test]
    fn labels_held_by_the_rounds_other_items_are_not_reused() {
        // C1 was accepted and Q1 answered; C2 is change-requested again.
        let mut inputs = fixture();
        inputs[2].existing_label = Some("C2".into());
        let reserved = ["C1".to_string(), "Q1".to_string()];
        let brief = build_brief(2, &inputs, &reserved);
        assert_eq!(
            brief.labels,
            [
                ("i1".to_string(), "C3".to_string()),
                ("i3".to_string(), "C2".to_string()),
                ("i2".to_string(), "Q2".to_string()),
            ]
        );
    }

    #[test]
    fn parse_label_accepts_c_and_q_numbers_only() {
        assert_eq!(parse_label("c3"), Some(("C3".into(), RequestKind::Change)));
        assert_eq!(
            parse_label(" Q12 "),
            Some(("Q12".into(), RequestKind::Question))
        );
        for bad in [
            "", "C", "C0", "D4", "d4", "C1x", "C-1", "1", "CC1", "C1234567",
        ] {
            assert_eq!(parse_label(bad), None, "{bad:?}");
        }
        assert!(is_review_label("C1"));
        assert!(!is_review_label("c1"));
        assert!(!is_review_label("d4"));
    }

    #[test]
    fn tags_name_the_decision_or_the_unexplained_change() {
        use RequestKind::*;
        assert_eq!(
            item_tag(
                Some(ItemKind::Decision),
                Some((4, "Outbox file")),
                "d4 Outbox file",
                Change
            ),
            "d4 Outbox file"
        );
        assert_eq!(
            item_tag(Some(ItemKind::Decision), Some((7, "T")), "d7 T", Question),
            "d7"
        );
        assert_eq!(
            item_tag(Some(ItemKind::Ask), Some((2, "T")), "d2? why", Change),
            "d2"
        );
        assert_eq!(
            item_tag(
                Some(ItemKind::UnexplainedHit),
                None,
                "new dependency `ulid`",
                Change
            ),
            "unexplained: new dependency `ulid`"
        );
        assert_eq!(item_tag(Some(ItemKind::Fit), None, "Fit", Change), "Fit");
    }

    fn padded(extra_bytes: usize) -> Brief {
        let base = build_brief(3, &fixture()[..1], &[]);
        let mut inputs = fixture()[..1].to_vec();
        inputs[0].text.push(' ');
        inputs[0].text.push_str(&"x".repeat(extra_bytes - 1));
        let brief = build_brief(3, &inputs, &[]);
        assert_eq!(brief.text.len(), base.text.len() + extra_bytes);
        brief
    }

    #[test]
    fn under_two_thousand_tokens_is_inline() {
        let base = build_brief(3, &fixture()[..1], &[]).text.len();
        // 4 bytes per token: 7_996 bytes is 1_999 tokens.
        let brief = padded(7_996 - base);
        assert_eq!(brief.estimated_tokens(), 1_999);
        let delivery = deliver(&brief, "/w/.amf/design-review/round-3.md");
        assert_eq!(delivery.inline, brief.text);
        assert_eq!(delivery.file, None);
    }

    #[test]
    fn two_thousand_tokens_goes_to_a_file_and_the_paste_points_at_it() {
        let base = build_brief(3, &fixture()[..1], &[]).text.len();
        let brief = padded(8_000 - base);
        assert_eq!(brief.estimated_tokens(), 2_000);
        let delivery = deliver(&brief, "/w/.amf/design-review/round-3.md");
        assert_eq!(
            delivery.file,
            Some((
                ".amf/design-review/round-3.md".to_string(),
                brief.text.clone()
            ))
        );
        assert_eq!(
            delivery.inline,
            "Design review round 3: 1 change, 0 questions.\n\
             Read /w/.amf/design-review/round-3.md for every item and how to reply, then \
             answer each one with `amf review reply <label> ...`."
        );
        assert!(delivery.inline.len() < 400);
    }
}
