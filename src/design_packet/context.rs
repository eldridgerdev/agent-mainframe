//! Budgeted, pure prompt-context builders: one per packet section.
//!
//! Each builder fills its role's input cap (estimated tokens) with the
//! highest-priority content first, so under pressure hunks go before commit
//! subjects and those before decision titles. What did not fit is named in a
//! trailer line ("… 12 files omitted") instead of vanishing silently. No
//! builder runs git or reads a file.

use sha2::{Digest, Sha256};

use crate::diff::{DiffFile, DiffFileStatus};
use crate::token_cost::ledger::estimate_tokens_from_bytes;

use super::detectors::{DetectorHit, paths_intersect, uncovered};
use super::staff::{StaffModel, StaffRole};

/// A decision record reduced to what a prompt needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionSummary {
    pub seq: u32,
    pub kind: String,
    pub title: String,
    pub paths: Vec<String>,
}

/// Everything the builders may draw on. All borrowed, all optional in effect:
/// an empty slice simply contributes nothing.
#[derive(Debug, Clone, Copy, Default)]
pub struct PacketInputs<'a> {
    pub files: &'a [DiffFile],
    pub commit_subjects: &'a [String],
    pub changed_modules: &'a [String],
    pub decisions: &'a [DecisionSummary],
    pub plan_excerpt: Option<&'a str>,
    /// The generated brief, for the plan-drift comparison.
    pub brief: Option<&'a str>,
    /// Every hit from `detectors::detect`, covered or not. `fit_context`
    /// shows them all; `unexplained_context` keeps only the uncovered ones.
    pub detector_hits: &'a [DetectorHit],
}

/// Largest single item (one hunk block, one line) before it is clipped.
const MAX_ITEM_BYTES: usize = 3_000;
/// Bytes held back for the omission trailers.
const TRAILER_RESERVE_BYTES: usize = 400;
/// Tokens of each role's cap held back for the template prose (and, for the
/// diagram prompts, the renderer limits) that wraps the context, so the
/// prompt actually sent stays within `StaffRole::input_cap`. A test pins
/// every design-review template under it.
pub const PROMPT_RESERVE_TOKENS: u64 = 2_000;
/// A file with at least this many changed lines is "large" for the
/// unexplained-change section.
pub const LARGE_CHANGE_LINES: usize = 80;

struct Budget {
    limit_bytes: usize,
    body: String,
    trailers: Vec<String>,
}

impl Budget {
    /// A budget for the context of a prompt capped at `cap_tokens`: the cap
    /// less the template reserve and the trailer reserve.
    fn new(cap_tokens: u64) -> Self {
        let context_tokens = cap_tokens.saturating_sub(PROMPT_RESERVE_TOKENS);
        Self {
            limit_bytes: (context_tokens as usize * 4).saturating_sub(TRAILER_RESERVE_BYTES),
            body: String::new(),
            trailers: Vec::new(),
        }
    }

    fn has_room(&self, extra: usize) -> bool {
        self.body.len() + extra <= self.limit_bytes
    }

    /// Add a headed list. Items past the budget are dropped, in order, and
    /// counted as `noun` in a trailer; a heading with no item under it is
    /// not kept.
    fn section(&mut self, heading: &str, items: Vec<String>, noun: &str) {
        if items.is_empty() {
            return;
        }
        let total = items.len();
        let mark = self.body.len();
        let head = format!("## {heading}\n");
        let mut kept = 0;
        if self.has_room(head.len() + 1) {
            self.body.push_str(&head);
            for item in items {
                let line = format!("{}\n", clip(&item, MAX_ITEM_BYTES));
                if !self.has_room(line.len() + 1) {
                    break;
                }
                self.body.push_str(&line);
                kept += 1;
            }
            if kept == 0 {
                self.body.truncate(mark);
            } else {
                self.body.push('\n');
            }
        }
        if kept < total {
            self.trailers
                .push(format!("… {} {noun} omitted", total - kept));
        }
    }

    fn finish(mut self) -> (String, u64) {
        for trailer in &self.trailers {
            self.body.push_str(trailer);
            self.body.push('\n');
        }
        let tokens = estimate_tokens_from_bytes(self.body.len());
        (self.body, tokens)
    }
}

/// Cut `text` to at most `max` bytes on a char boundary, marking the cut.
fn clip(text: &str, max: usize) -> String {
    let text = text.trim_end();
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max.saturating_sub(4);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{} …", &text[..end])
}

fn status_word(status: &DiffFileStatus) -> &'static str {
    match status {
        DiffFileStatus::Added | DiffFileStatus::Untracked => "added",
        DiffFileStatus::Modified => "modified",
        DiffFileStatus::Deleted => "deleted",
        DiffFileStatus::Renamed => "renamed",
        DiffFileStatus::Copied => "copied",
        DiffFileStatus::TypeChanged => "type-changed",
    }
}

fn churn(file: &DiffFile) -> usize {
    file.additions + file.deletions
}

fn decision_lines(decisions: &[&DecisionSummary], with_paths: bool) -> Vec<String> {
    decisions
        .iter()
        .map(|d| {
            let mut line = format!("d{} [{}] {}", d.seq, d.kind, d.title);
            if with_paths && !d.paths.is_empty() {
                line.push_str(&format!(" ({})", d.paths.join(", ")));
            }
            line
        })
        .collect()
}

fn all_decisions<'a>(inputs: &PacketInputs<'a>) -> Vec<&'a DecisionSummary> {
    inputs.decisions.iter().collect()
}

fn stat_lines(files: &[DiffFile]) -> Vec<String> {
    files
        .iter()
        .map(|f| {
            format!(
                "{} {} (+{} -{})",
                status_word(&f.status),
                f.path,
                f.additions,
                f.deletions
            )
        })
        .collect()
}

/// Hunk text per text file, smallest diffs first so the most files fit.
fn hunk_items(files: &[DiffFile]) -> Vec<String> {
    let mut with_patch: Vec<&DiffFile> = files
        .iter()
        .filter(|f| !f.is_binary && !f.patch.trim().is_empty())
        .collect();
    with_patch.sort_by_key(|f| churn(f));
    with_patch
        .into_iter()
        .map(|f| format!("### {}\n{}", f.path, f.patch.trim_end()))
        .collect()
}

fn module_items(inputs: &PacketInputs) -> Vec<String> {
    inputs.changed_modules.to_vec()
}

fn lines(text: &str) -> Vec<String> {
    text.lines().map(str::to_owned).collect()
}

/// Scribe: brief. Priority: decisions, commits, modules, file stat, hunks.
pub fn brief_context(inputs: &PacketInputs) -> (String, u64) {
    let mut b = Budget::new(StaffRole::Scribe.input_cap());
    b.section(
        "Decisions",
        decision_lines(&all_decisions(inputs), false),
        "decisions",
    );
    b.section("Commits", inputs.commit_subjects.to_vec(), "commits");
    b.section("Changed modules", module_items(inputs), "modules");
    b.section("Files changed", stat_lines(inputs.files), "files");
    b.section("Diff", hunk_items(inputs.files), "file diffs");
    b.finish()
}

/// Cartographer seed: architecture. Architecture and data decisions lead.
pub fn architecture_context(inputs: &PacketInputs) -> (String, u64) {
    let mut b = Budget::new(StaffRole::Cartographer.input_cap());
    let (core, other): (Vec<_>, Vec<_>) = inputs
        .decisions
        .iter()
        .partition(|d| matches!(d.kind.as_str(), "architecture" | "data"));
    b.section(
        "Architecture and data decisions",
        decision_lines(&core, true),
        "decisions",
    );
    b.section("Changed modules", module_items(inputs), "modules");
    b.section(
        "Other decisions",
        decision_lines(&other, false),
        "decisions",
    );
    b.section("Files changed", stat_lines(inputs.files), "files");
    b.finish()
}

/// Cartographer seed: the main new path as a sequence.
pub fn key_flow_context(inputs: &PacketInputs) -> (String, u64) {
    let mut b = Budget::new(StaffRole::Cartographer.input_cap());
    b.section(
        "Decisions",
        decision_lines(&all_decisions(inputs), true),
        "decisions",
    );
    b.section("Commits", inputs.commit_subjects.to_vec(), "commits");
    b.section("Changed modules", module_items(inputs), "modules");
    let added: Vec<DiffFile> = inputs
        .files
        .iter()
        .filter(|f| matches!(f.status, DiffFileStatus::Added | DiffFileStatus::Untracked))
        .cloned()
        .collect();
    b.section("New files", stat_lines(&added), "new files");
    b.section("Files changed", stat_lines(inputs.files), "files");
    b.section("Diff", hunk_items(inputs.files), "file diffs");
    b.finish()
}

/// Cartographer seed: modules touched versus added, plus prior decisions on
/// the same paths (the caller passes them as `decisions`) and every detector
/// hit, covered or not.
pub fn fit_context(inputs: &PacketInputs) -> (String, u64) {
    let mut b = Budget::new(StaffRole::Cartographer.input_cap());
    b.section("Modules touched", module_items(inputs), "modules");
    let added: Vec<String> = inputs
        .files
        .iter()
        .filter(|f| matches!(f.status, DiffFileStatus::Added | DiffFileStatus::Untracked))
        .map(|f| f.path.clone())
        .collect();
    b.section("Files added", added, "added files");
    b.section(
        "Decisions",
        decision_lines(&all_decisions(inputs), true),
        "decisions",
    );
    let hits = inputs
        .detector_hits
        .iter()
        .map(|h| h.summary.clone())
        .collect();
    b.section("Mechanical findings", hits, "findings");
    b.section("Files changed", stat_lines(inputs.files), "files");
    b.finish()
}

fn covered_by_decision(path: &str, decisions: &[DecisionSummary]) -> bool {
    decisions
        .iter()
        .any(|d| d.paths.iter().any(|p| paths_intersect(p, path)))
}

/// Scribe: large changes that no decision covers. Detector hits lead; then
/// the decisions (so the model can see what is explained), then large files
/// outside every decision's paths.
pub fn unexplained_context(inputs: &PacketInputs) -> (String, u64) {
    let mut b = Budget::new(StaffRole::Scribe.input_cap());
    // Filtered here, so callers pass every hit (as `fit_context` expects).
    let hits = uncovered(inputs.detector_hits, inputs.decisions)
        .into_iter()
        .map(|h| h.summary)
        .collect();
    b.section("Mechanical findings with no decision", hits, "findings");
    let mut large: Vec<&DiffFile> = inputs
        .files
        .iter()
        .filter(|f| {
            churn(f) >= LARGE_CHANGE_LINES && !covered_by_decision(&f.path, inputs.decisions)
        })
        .collect();
    large.sort_by_key(|f| std::cmp::Reverse(churn(f)));
    let large: Vec<String> = stat_lines(&large.into_iter().cloned().collect::<Vec<_>>());
    b.section(
        "Large changes outside every decision's paths",
        large,
        "files",
    );
    b.section(
        "Recorded decisions",
        decision_lines(&all_decisions(inputs), true),
        "decisions",
    );
    b.finish()
}

/// Scribe: the plan excerpt against the brief.
pub fn plan_drift_context(inputs: &PacketInputs) -> (String, u64) {
    let mut b = Budget::new(StaffRole::Scribe.input_cap());
    // Decisions are short and carry the recorded deviations: keep them first.
    b.section(
        "Decisions",
        decision_lines(&all_decisions(inputs), false),
        "decisions",
    );
    b.section("Brief", lines(inputs.brief.unwrap_or("")), "brief lines");
    b.section(
        "Plan excerpt (AMF_PLAN.md)",
        lines(inputs.plan_excerpt.unwrap_or("")),
        "plan lines",
    );
    b.finish()
}

/// Stable cache key for one staff output: SHA-256 over the section name, the
/// built context, the template text actually used (so an edited override
/// invalidates it), the rendered renderer limits (empty for prompts without
/// them) and the resolved staff harness, model and reasoning level (so
/// switching the scribe from Haiku to Sonnet invalidates it), each
/// length-framed. Lowercase hex.
pub fn input_hash(
    section: &str,
    context: &str,
    template_text: &str,
    renderer_limits: &str,
    staff: &StaffModel,
) -> String {
    let mut hasher = Sha256::new();
    let reasoning = staff.reasoning.map(|r| r.slug()).unwrap_or("");
    for part in [
        section,
        context,
        template_text,
        renderer_limits,
        staff.harness.slug(),
        staff.model.as_str(),
        reasoning,
    ] {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part.as_bytes());
    }
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design_packet::detectors::DetectorKind;
    use crate::design_packet::staff::{CARTOGRAPHER_SEED_CAP, SCRIBE_CAP};
    use crate::headless::ReasoningLevel;
    use crate::project::AgentKind;
    use crate::prompts::PromptId;

    fn file(path: &str, status: DiffFileStatus, additions: usize, patch: &str) -> DiffFile {
        DiffFile {
            old_path: None,
            path: path.to_owned(),
            status,
            additions,
            deletions: 0,
            is_binary: false,
            old_content: None,
            new_content: None,
            patch: patch.to_owned(),
            hunks: Vec::new(),
        }
    }

    fn decision(seq: u32, kind: &str, title: &str, paths: &[&str]) -> DecisionSummary {
        DecisionSummary {
            seq,
            kind: kind.into(),
            title: title.into(),
            paths: paths.iter().map(|p| p.to_string()).collect(),
        }
    }

    struct Big {
        files: Vec<DiffFile>,
        commits: Vec<String>,
        modules: Vec<String>,
        decisions: Vec<DecisionSummary>,
        plan: String,
        brief: String,
        hits: Vec<DetectorHit>,
    }

    fn big() -> Big {
        Big {
            files: (0..400)
                .map(|i| {
                    file(
                        &format!("src/m{}/f{i}.rs", i % 20),
                        if i % 3 == 0 {
                            DiffFileStatus::Added
                        } else {
                            DiffFileStatus::Modified
                        },
                        100 + i,
                        &format!("diff --git a/x b/x\n{}", "+let line = 1;\n".repeat(2_000)),
                    )
                })
                .collect(),
            commits: (0..3_000)
                .map(|i| format!("commit subject number {i} with words"))
                .collect(),
            modules: (0..2_000).map(|i| format!("src/module_{i}")).collect(),
            decisions: (0..2_000)
                .map(|i| {
                    decision(
                        i,
                        if i % 2 == 0 {
                            "architecture"
                        } else {
                            "tradeoff"
                        },
                        "a decision title",
                        &["src/x"],
                    )
                })
                .collect(),
            plan: "plan line about the work\n".repeat(20_000),
            brief: "brief line about the work\n".repeat(20_000),
            hits: (0..2_000)
                .map(|i| DetectorHit {
                    kind: DetectorKind::BuildConfig,
                    paths: vec![format!("p{i}")],
                    summary: format!("Build config changed: p{i}"),
                })
                .collect(),
        }
    }

    impl Big {
        fn inputs(&self) -> PacketInputs<'_> {
            PacketInputs {
                files: &self.files,
                commit_subjects: &self.commits,
                changed_modules: &self.modules,
                decisions: &self.decisions,
                plan_excerpt: Some(&self.plan),
                brief: Some(&self.brief),
                detector_hits: &self.hits,
            }
        }
    }

    #[test]
    fn every_builder_stays_under_its_cap_and_reports_omissions() {
        let b = big();
        let i = b.inputs();
        let cases: [(&str, (String, u64), u64); 6] = [
            ("brief", brief_context(&i), SCRIBE_CAP),
            (
                "architecture",
                architecture_context(&i),
                CARTOGRAPHER_SEED_CAP,
            ),
            ("key_flow", key_flow_context(&i), CARTOGRAPHER_SEED_CAP),
            ("fit", fit_context(&i), CARTOGRAPHER_SEED_CAP),
            ("unexplained", unexplained_context(&i), SCRIBE_CAP),
            ("plan_drift", plan_drift_context(&i), SCRIBE_CAP),
        ];
        for (name, (text, tokens), cap) in cases {
            assert!(tokens <= cap, "{name}: {tokens} > {cap}");
            assert_eq!(tokens, estimate_tokens_from_bytes(text.len()), "{name}");
            assert!(
                text.contains(" omitted"),
                "{name} reports omissions:\n{}",
                &text[text.len().saturating_sub(300)..]
            );
            assert!(text.contains("… "), "{name}");
        }
    }

    #[test]
    fn small_input_is_kept_whole_with_no_trailer() {
        let files = [file(
            "src/a.rs",
            DiffFileStatus::Added,
            3,
            "diff --git a/a b/a\n+x\n",
        )];
        let decisions = [decision(1, "architecture", "Use a queue", &["src/a.rs"])];
        let commits = ["add queue".to_owned()];
        let inputs = PacketInputs {
            files: &files,
            commit_subjects: &commits,
            decisions: &decisions,
            ..Default::default()
        };
        let (text, tokens) = brief_context(&inputs);
        assert!(text.contains("d1 [architecture] Use a queue"));
        assert!(text.contains("add queue"));
        assert!(text.contains("added src/a.rs (+3 -0)"));
        assert!(text.contains("### src/a.rs"));
        assert!(!text.contains("omitted"));
        assert!(tokens > 0);
    }

    #[test]
    fn hunks_are_dropped_before_decisions_and_commits() {
        let files: Vec<DiffFile> = (0..50)
            .map(|i| {
                file(
                    &format!("f{i}.rs"),
                    DiffFileStatus::Modified,
                    10,
                    &"+x\n".repeat(2_500),
                )
            })
            .collect();
        let decisions = [decision(1, "tradeoff", "Keep titles", &[])];
        let commits = ["keep commits".to_owned()];
        let inputs = PacketInputs {
            files: &files,
            commit_subjects: &commits,
            decisions: &decisions,
            ..Default::default()
        };
        let (text, _) = brief_context(&inputs);
        assert!(text.contains("Keep titles") && text.contains("keep commits"));
        assert!(text.contains("file diffs omitted"));
        assert!(
            !text.contains("files omitted"),
            "stat for all 50 files fits"
        );
    }

    #[test]
    fn architecture_leads_with_architecture_and_data_decisions() {
        let decisions = [
            decision(1, "tradeoff", "Other", &[]),
            decision(2, "data", "Schema shape", &["src/db"]),
        ];
        let inputs = PacketInputs {
            decisions: &decisions,
            ..Default::default()
        };
        let (text, _) = architecture_context(&inputs);
        assert!(text.find("Schema shape").unwrap() < text.find("Other").unwrap());
        assert!(text.contains("(src/db)"));
    }

    #[test]
    fn unexplained_lists_only_large_uncovered_files() {
        let files = [
            file(
                "src/big.rs",
                DiffFileStatus::Modified,
                LARGE_CHANGE_LINES,
                "",
            ),
            file("src/covered.rs", DiffFileStatus::Modified, 500, ""),
            file("src/small.rs", DiffFileStatus::Modified, 3, ""),
        ];
        let decisions = [decision(
            1,
            "architecture",
            "Covers one",
            &["src/covered.rs"],
        )];
        let inputs = PacketInputs {
            files: &files,
            decisions: &decisions,
            ..Default::default()
        };
        let (text, _) = unexplained_context(&inputs);
        let large = text.split("## Recorded decisions").next().unwrap();
        assert!(large.contains("src/big.rs"));
        assert!(!large.contains("src/covered.rs") && !large.contains("src/small.rs"));
    }

    #[test]
    fn plan_drift_carries_both_texts() {
        let inputs = PacketInputs {
            plan_excerpt: Some("Plan: ship a queue"),
            brief: Some("Built a stack"),
            ..Default::default()
        };
        let (text, _) = plan_drift_context(&inputs);
        assert!(text.contains("Plan: ship a queue") && text.contains("Built a stack"));
    }

    #[test]
    fn clip_respects_char_boundaries() {
        let clipped = clip(&"é".repeat(100), 51);
        assert!(clipped.len() <= 51 && clipped.ends_with('…'));
        assert_eq!(clip("short  ", 51), "short");
    }

    fn staff(model: &str, reasoning: Option<ReasoningLevel>) -> StaffModel {
        StaffModel {
            harness: AgentKind::Claude,
            model: model.into(),
            reasoning,
        }
    }

    #[test]
    fn hash_is_stable_distinct_and_length_framed() {
        let haiku = staff("haiku", None);
        let h = input_hash("brief", "ctx", "tpl", "", &haiku);
        assert_eq!(h, input_hash("brief", "ctx", "tpl", "", &haiku));
        assert_eq!(h.len(), 64);
        // Pinned: a changed algorithm would silently invalidate every cache.
        assert_eq!(
            h,
            "00d8fc1411677e99f8920cb59ebf6dd05687a5def8e7ce4dcae9e9a31de81f3a"
        );
        assert_ne!(h, input_hash("fit", "ctx", "tpl", "", &haiku));
        assert_ne!(h, input_hash("brief", "ctx2", "tpl", "", &haiku));
        assert_ne!(h, input_hash("brief", "ctx", "tpl2", "", &haiku));
        assert_ne!(h, input_hash("brief", "ctx", "tpl", "limits", &haiku));
        let sonnet = staff("sonnet", None);
        assert_ne!(h, input_hash("brief", "ctx", "tpl", "", &sonnet));
        let low = staff("haiku", Some(ReasoningLevel::Low));
        assert_ne!(h, input_hash("brief", "ctx", "tpl", "", &low));
        let codex = StaffModel {
            harness: AgentKind::Codex,
            ..haiku.clone()
        };
        assert_ne!(h, input_hash("brief", "ctx", "tpl", "", &codex));
        assert_ne!(
            input_hash("a", "bc", "", "", &haiku),
            input_hash("ab", "c", "", "", &haiku)
        );
    }

    #[test]
    fn prompt_reserve_covers_every_design_template() {
        let limits = crate::design_packet::diagram::renderer_limits();
        for id in [
            PromptId::DesignBrief,
            PromptId::DesignArchitecture,
            PromptId::DesignKeyFlow,
            PromptId::DesignFit,
            PromptId::DesignUnexplained,
            PromptId::DesignPlanDrift,
        ] {
            for harness in [
                AgentKind::Claude,
                AgentKind::Codex,
                AgentKind::Opencode,
                AgentKind::Pi,
            ] {
                let bytes = id.spec().default_template_for(&harness).len() + limits.len();
                assert!(
                    estimate_tokens_from_bytes(bytes) <= PROMPT_RESERVE_TOKENS,
                    "{} ({bytes} bytes)",
                    id.as_str()
                );
            }
        }
    }

    #[test]
    fn unexplained_keeps_only_uncovered_hits_and_fit_keeps_all() {
        let hits = [
            DetectorHit {
                kind: DetectorKind::Dependency,
                paths: vec!["Cargo.toml".into()],
                summary: "Cargo.toml gained 1 dependency line(s)".into(),
            },
            DetectorHit {
                kind: DetectorKind::BuildConfig,
                paths: vec!["build.rs".into()],
                summary: "Build or CI configuration changed: build.rs".into(),
            },
        ];
        let decisions = [decision(1, "dependency", "Add regex", &["Cargo.toml"])];
        let inputs = PacketInputs {
            decisions: &decisions,
            detector_hits: &hits,
            ..Default::default()
        };
        let (text, _) = unexplained_context(&inputs);
        assert!(!text.contains("gained 1 dependency"));
        assert!(text.contains("configuration changed: build.rs"));
        let (fit, _) = fit_context(&inputs);
        assert!(fit.contains("gained 1 dependency") && fit.contains("build.rs"));
    }
}
