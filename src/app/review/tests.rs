use super::comments::{
    apply_suggestions_to_file, comment_anchor_label, editor_invocation, editor_target_line,
    reanchor_file_comments,
};
use super::headless::{
    CHECK_OUTPUT_MAX_CHARS, build_pr_review, parse_co_review_output, pr_postable_lines,
    severity_review_event, truncate_check_output, walkthrough_context,
};
use super::preparation::{
    anchor_file_path, archive_review_notes, compose_feedback_log, load_review_notes,
    parse_agent_responses, parse_review_history_rounds, parse_review_notes,
    split_overflow_review_notes, split_overflow_rounds,
};
use super::progression::compute_search_matches;
use std::collections::{HashMap, HashSet};

use crate::app::state::DiffViewerState;
use crate::app::{CommentAnchorContext, FileComment, LineComment, Severity};
use crate::diff::{
    DiffFile, DiffFileStatus, DiffHunk, DiffLine, DiffLineKind, DiffLineLocation,
    parse_unified_diff,
};

fn line_comment(new_line: Option<usize>, old_line: Option<usize>, text: &str) -> LineComment {
    LineComment {
        location: DiffLineLocation { old_line, new_line },
        start: None,
        text: text.to_string(),
        draft: false,
        suggestion: None,
        severity: Severity::default(),
        anchor_context: None,
        start_anchor_context: None,
        anchor_lost: false,
        resolved: false,
        carried: false,
    }
}

/// A multi-line comment spanning `start`..`end` on the current (RIGHT) side.
fn ranged_comment(start: usize, end: usize, text: &str) -> LineComment {
    LineComment {
        location: DiffLineLocation {
            old_line: None,
            new_line: Some(end),
        },
        start: Some(DiffLineLocation {
            old_line: None,
            new_line: Some(start),
        }),
        text: text.to_string(),
        draft: false,
        suggestion: None,
        severity: Severity::default(),
        anchor_context: None,
        start_anchor_context: None,
        anchor_lost: false,
        resolved: false,
        carried: false,
    }
}

fn local_apply_file(content: &str) -> DiffFile {
    DiffFile {
        old_path: Some("src/example.rs".to_string()),
        path: "src/example.rs".to_string(),
        status: DiffFileStatus::Modified,
        additions: 1,
        deletions: 1,
        is_binary: false,
        old_content: Some("old\ncontent\n".to_string()),
        new_content: Some(content.to_string()),
        patch: String::new(),
        hunks: vec![DiffHunk {
            header: "@@ -1,3 +1,3 @@".to_string(),
            old_start: 1,
            old_lines: 3,
            new_start: 1,
            new_lines: 3,
            lines: vec![
                DiffLine {
                    kind: DiffLineKind::Context,
                    text: " one".to_string(),
                },
                DiffLine {
                    kind: DiffLineKind::Added,
                    text: "+two".to_string(),
                },
                DiffLine {
                    kind: DiffLineKind::Context,
                    text: " three".to_string(),
                },
            ],
        }],
    }
}

#[test]
fn local_suggestion_replaces_range_and_preserves_crlf() {
    let workdir = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(workdir.path().join("src")).unwrap();
    let content = "one\r\ntwo\r\nthree\r\n";
    std::fs::write(workdir.path().join("src/example.rs"), content).unwrap();
    let file = local_apply_file(content);
    let locations = file.addressable_lines();
    let mut comment = line_comment(Some(3), Some(3), "replace both");
    comment.start = Some(locations[1]);
    comment.location = locations[2];
    comment.suggestion = Some("TWO\nTHREE".to_string());

    let report = apply_suggestions_to_file(workdir.path(), &file, &[(0, comment)]);

    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(report.applied.len(), 1);
    assert_eq!(
        std::fs::read_to_string(workdir.path().join("src/example.rs")).unwrap(),
        "one\r\nTWO\r\nTHREE\r\n"
    );
}

#[test]
fn local_suggestion_refuses_a_file_changed_since_diff_load() {
    let workdir = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(workdir.path().join("src")).unwrap();
    let reviewed = "one\ntwo\nthree\n";
    let live = "one\nchanged elsewhere\nthree\n";
    std::fs::write(workdir.path().join("src/example.rs"), live).unwrap();
    let file = local_apply_file(reviewed);
    let mut comment = line_comment(Some(2), None, "replace it");
    comment.suggestion = Some("TWO".to_string());

    let report = apply_suggestions_to_file(workdir.path(), &file, &[(0, comment)]);

    assert!(report.applied.is_empty());
    assert!(report.failures[0].contains("changed since the diff was loaded"));
    assert_eq!(
        std::fs::read_to_string(workdir.path().join("src/example.rs")).unwrap(),
        live
    );
}

#[test]
fn local_suggestion_refuses_deletion_side_anchor() {
    let workdir = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(workdir.path().join("src")).unwrap();
    std::fs::write(workdir.path().join("src/example.rs"), "one\n").unwrap();
    let mut file = local_apply_file("one\n");
    file.hunks = vec![DiffHunk {
        header: "@@ -1,2 +1 @@".to_string(),
        old_start: 1,
        old_lines: 2,
        new_start: 1,
        new_lines: 1,
        lines: vec![DiffLine {
            kind: DiffLineKind::Removed,
            text: "-gone".to_string(),
        }],
    }];
    let mut comment = line_comment(None, Some(1), "replace deletion");
    comment.suggestion = Some("replacement".to_string());

    let report = apply_suggestions_to_file(workdir.path(), &file, &[(0, comment)]);

    assert!(report.applied.is_empty());
    assert!(report.failures[0].contains("deletion-only line"));
    assert_eq!(
        std::fs::read_to_string(workdir.path().join("src/example.rs")).unwrap(),
        "one\n"
    );
}

#[test]
fn local_suggestion_batch_applies_bottom_up_when_line_counts_change() {
    let workdir = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(workdir.path().join("src")).unwrap();
    let content = "one\ntwo\nthree\n";
    std::fs::write(workdir.path().join("src/example.rs"), content).unwrap();
    let file = local_apply_file(content);
    let locations = file.addressable_lines();
    let mut second = line_comment(Some(2), None, "expand two");
    second.location = locations[1];
    second.suggestion = Some("TWO-A\nTWO-B".to_string());
    let mut third = line_comment(Some(3), Some(3), "replace three");
    third.location = locations[2];
    third.suggestion = Some("THREE".to_string());

    let report = apply_suggestions_to_file(workdir.path(), &file, &[(0, second), (1, third)]);

    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(report.applied.len(), 2);
    assert_eq!(
        std::fs::read_to_string(workdir.path().join("src/example.rs")).unwrap(),
        "one\nTWO-A\nTWO-B\nTHREE\n"
    );
}

#[test]
fn co_review_output_parses_findings_onto_addressable_lines() {
    let locs = vec![
        DiffLineLocation {
            old_line: Some(10),
            new_line: Some(10),
        },
        DiffLineLocation {
            old_line: None,
            new_line: Some(11),
        },
        DiffLineLocation {
            old_line: Some(12),
            new_line: None,
        },
    ];
    let output = "11|This add looks risky.\n\
             10 | context concern\n\
             - 11|tolerate a bullet prefix\n\
             999|no such line, dropped\n\
             not a finding line\n\
             11|\n";
    let drafts = parse_co_review_output(output, &locs);

    // The 999 finding has no matching new_line and the empty-text and
    // non-matching lines are skipped; the rest become draft comments.
    assert_eq!(drafts.len(), 3);
    assert!(drafts.iter().all(|c| c.draft && c.start.is_none()));
    assert_eq!(drafts[0].location.new_line, Some(11));
    assert_eq!(drafts[0].text, "This add looks risky.");
    // ` 10 | …` (spaces around the pipe) still resolves to new_line 10.
    assert_eq!(drafts[1].location.new_line, Some(10));
    assert_eq!(drafts[1].text, "context concern");
    // The leading-bullet finding still anchors to line 11.
    assert_eq!(drafts[2].location.new_line, Some(11));
}

#[test]
fn pr_review_maps_lines_inline_and_files_to_file_comments() {
    let rejected = vec![
        (
            "src/a.rs".to_string(),
            "tighten this up".to_string(),
            Severity::Suggestion,
        ),
        ("src/b.rs".to_string(), String::new(), Severity::Blocker),
    ];
    let line_comments = vec![(
        "src/c.rs".to_string(),
        vec![
            line_comment(Some(42), None, "off-by-one"),
            line_comment(None, Some(7), "why delete this?"),
        ],
    )];
    let (body, comments, file_comments) = build_pr_review(
        &rejected,
        &[],
        &line_comments,
        "overall LGTM-ish",
        &HashMap::new(),
    );

    // Inline comments: an added/current line posts on RIGHT, a deletion-only
    // line on LEFT.
    assert_eq!(comments.len(), 2);
    assert_eq!(comments[0].path, "src/c.rs");
    assert_eq!(comments[0].line, 42);
    assert_eq!(comments[0].side, "RIGHT");
    assert_eq!(comments[1].line, 7);
    assert_eq!(comments[1].side, "LEFT");

    // Body carries only the general feedback now — whole-file rejections
    // post as their own file-level comments instead of being dumped here.
    assert_eq!(body, "overall LGTM-ish");
    assert!(!body.contains("Files needing revision"));

    // Whole-file rejections: one `PrFileComment` per file, tagged with its
    // severity, with a filler line when no feedback text was given.
    assert_eq!(file_comments.len(), 2);
    assert_eq!(file_comments[0].path, "src/a.rs");
    assert_eq!(file_comments[0].body, "**[suggestion]** tighten this up");
    assert_eq!(file_comments[1].path, "src/b.rs");
    assert_eq!(file_comments[1].body, "**[blocker]** Needs revision.");
}

#[test]
fn pr_review_body_empty_when_only_line_comments() {
    let line_comments = vec![(
        "src/c.rs".to_string(),
        vec![line_comment(Some(3), None, "nit")],
    )];
    let (body, comments, file_comments) =
        build_pr_review(&[], &[], &line_comments, "", &HashMap::new());
    assert!(body.is_empty());
    assert_eq!(comments.len(), 1);
    assert!(file_comments.is_empty());
}

#[test]
fn pr_review_skips_a_comment_on_a_line_outside_the_prs_diff() {
    // Expanding the rendered context lets the reviewer comment on a line
    // git's own diff never emitted. GitHub rejects an inline comment there,
    // and `create_review` posts the batch atomically — so it must be
    // dropped rather than allowed to sink the whole review.
    let line_comments = vec![(
        "src/c.rs".to_string(),
        vec![
            line_comment(Some(3), None, "in the diff"),
            line_comment(Some(90), None, "only visible after expanding"),
        ],
    )];
    let postable: HashMap<String, HashSet<crate::diff::DiffLineLocation>> = [(
        "src/c.rs".to_string(),
        HashSet::from([crate::diff::DiffLineLocation {
            old_line: None,
            new_line: Some(3),
        }]),
    )]
    .into_iter()
    .collect();

    let (_, comments, _) = build_pr_review(&[], &[], &line_comments, "", &postable);

    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].line, 3);
    // With no entry for the path the map imposes no restriction, which is
    // the pre-expansion behaviour.
    let (_, unrestricted, _) = build_pr_review(&[], &[], &line_comments, "", &HashMap::new());
    assert_eq!(unrestricted.len(), 2);
}

#[test]
fn pr_review_drops_a_range_whose_start_is_outside_the_prs_diff() {
    let line_comments = vec![(
        "src/c.rs".to_string(),
        vec![ranged_comment(10, 14, "this whole block")],
    )];
    // The end is in the diff but the start was only revealed by expansion:
    // post it as a single-line comment rather than an invalid span.
    let postable: HashMap<String, HashSet<crate::diff::DiffLineLocation>> = [(
        "src/c.rs".to_string(),
        HashSet::from([crate::diff::DiffLineLocation {
            old_line: None,
            new_line: Some(14),
        }]),
    )]
    .into_iter()
    .collect();

    let (_, comments, _) = build_pr_review(&[], &[], &line_comments, "", &postable);

    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].line, 14);
    assert_eq!(comments[0].start_line, None);
    assert_eq!(comments[0].start_side, None);
}

#[test]
fn pr_postable_lines_tracks_the_original_patch_not_the_expanded_hunks() {
    let old: String = (1..=30).map(|i| format!("l{i}\n")).collect();
    let new = old.replace("l15\n", "l15 changed\n");
    let mut file = crate::diff::parse_unified_diff(
        "\
diff --git a/a.rs b/a.rs
index 1111111..2222222 100644
--- a/a.rs
+++ b/a.rs
@@ -12,7 +12,7 @@
 l12
 l13
 l14
-l15
+l15 changed
 l16
 l17
 l18
",
    )
    .unwrap()
    .pop()
    .unwrap();
    file.old_content = Some(old);
    file.new_content = Some(new);

    // Expansion rewrites `hunks` but deliberately leaves `patch` alone, so
    // the postable set still describes git's real diff.
    file.hunks = file.hunks_with_context(usize::MAX).unwrap();
    assert_eq!(file.addressable_lines().len(), 31);

    let postable = pr_postable_lines(std::slice::from_ref(&file));
    let allowed = &postable["a.rs"];
    assert_eq!(allowed.len(), 8);
    assert!(allowed.contains(&crate::diff::DiffLineLocation {
        old_line: Some(12),
        new_line: Some(12)
    }));
    assert!(!allowed.contains(&crate::diff::DiffLineLocation {
        old_line: Some(1),
        new_line: Some(1)
    }));
}

#[test]
fn pr_review_emits_start_line_for_a_ranged_comment() {
    let line_comments = vec![(
        "src/c.rs".to_string(),
        vec![ranged_comment(10, 14, "this whole block")],
    )];
    let (_, comments, _) = build_pr_review(&[], &[], &line_comments, "", &HashMap::new());
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].line, 14);
    assert_eq!(comments[0].side, "RIGHT");
    assert_eq!(comments[0].start_line, Some(10));
    assert_eq!(comments[0].start_side, Some("RIGHT"));
}

#[test]
fn single_line_comment_has_no_start_line() {
    let line_comments = vec![(
        "src/c.rs".to_string(),
        vec![line_comment(Some(5), None, "nit")],
    )];
    let (_, comments, _) = build_pr_review(&[], &[], &line_comments, "", &HashMap::new());
    assert_eq!(comments[0].start_line, None);
    assert_eq!(comments[0].start_side, None);
}

#[test]
fn pr_review_appends_suggestion_block_to_comment_body() {
    let mut comment = line_comment(Some(5), None, "use a guard");
    comment.suggestion = Some("let x = y?;".to_string());
    let line_comments = vec![("src/c.rs".to_string(), vec![comment])];
    let (_, comments, _) = build_pr_review(&[], &[], &line_comments, "", &HashMap::new());
    assert_eq!(comments.len(), 1);
    // The comment body leads with the conventional-comments severity tag.
    assert_eq!(
        comments[0].body,
        "**[suggestion]** use a guard\n\n```suggestion\nlet x = y?;\n```"
    );
}

#[test]
fn pr_review_suggestion_only_comment_is_just_the_block() {
    let mut comment = line_comment(Some(5), None, "");
    comment.suggestion = Some("let x = y?;".to_string());
    let line_comments = vec![("src/c.rs".to_string(), vec![comment])];
    let (_, comments, _) = build_pr_review(&[], &[], &line_comments, "", &HashMap::new());
    // Even a suggestion-only comment carries its severity tag.
    assert_eq!(
        comments[0].body,
        "**[suggestion]**\n\n```suggestion\nlet x = y?;\n```"
    );
}

fn blocker_comment(new_line: usize, text: &str) -> LineComment {
    let mut c = line_comment(Some(new_line), None, text);
    c.severity = Severity::Blocker;
    c
}

#[test]
fn severity_event_requests_changes_on_a_blocker() {
    // A blocker rejection escalates.
    let rejected = vec![("a.rs".to_string(), "no".to_string(), Severity::Blocker)];
    assert_eq!(
        severity_review_event(&rejected, &[], &[]),
        "REQUEST_CHANGES"
    );
    // A blocker line comment escalates even with no rejection.
    let sections = vec![("a.rs".to_string(), vec![blocker_comment(3, "must fix")])];
    assert_eq!(
        severity_review_event(&[], &[], &sections),
        "REQUEST_CHANGES"
    );
}

#[test]
fn severity_event_approves_with_no_rejections_else_comments() {
    // No rejection, only non-blocking notes → an approving review.
    let sections = vec![("a.rs".to_string(), vec![line_comment(Some(3), None, "nit")])];
    assert_eq!(severity_review_event(&[], &[], &sections), "APPROVE");
    assert_eq!(severity_review_event(&[], &[], &[]), "APPROVE");
    // A non-blocking rejection is a plain comment review.
    let rejected = vec![("a.rs".to_string(), "meh".to_string(), Severity::Suggestion)];
    assert_eq!(severity_review_event(&rejected, &[], &[]), "COMMENT");
}

#[test]
fn feedback_file_comment_tags_whole_file_rejection_with_severity() {
    // A whole-file rejection's file-level comment carries its severity tag.
    let rejected = vec![("a.rs".to_string(), "fix".to_string(), Severity::Blocker)];
    let (_, _, file_comments) = build_pr_review(&rejected, &[], &[], "", &HashMap::new());
    assert_eq!(file_comments.len(), 1);
    assert_eq!(file_comments[0].path, "a.rs");
    assert_eq!(file_comments[0].body, "**[blocker]** fix");
}

#[test]
fn verdict_free_file_comment_posts_and_can_escalate() {
    let comments = vec![(
        "src/module.rs".to_string(),
        FileComment {
            text: "This module should be split.".to_string(),
            severity: Severity::Blocker,
            resolved: false,
            carried: false,
        },
    )];
    let (body, inline, file_comments) = build_pr_review(&[], &comments, &[], "", &HashMap::new());
    assert!(body.is_empty());
    assert!(inline.is_empty());
    assert_eq!(file_comments.len(), 1);
    assert_eq!(file_comments[0].path, "src/module.rs");
    assert_eq!(
        file_comments[0].body,
        "**[blocker]** This module should be split."
    );
    assert_eq!(
        severity_review_event(&[], &comments, &[]),
        "REQUEST_CHANGES"
    );
}

#[test]
fn anchor_label_renders_single_and_range_and_base() {
    use super::comments::comment_anchor_label;
    assert_eq!(
        comment_anchor_label("src/a.rs", &line_comment(Some(42), None, "x")),
        "src/a.rs:42"
    );
    assert_eq!(
        comment_anchor_label("src/a.rs", &ranged_comment(10, 14, "x")),
        "src/a.rs:10-14"
    );
    assert_eq!(
        comment_anchor_label("src/a.rs", &line_comment(None, Some(7), "x")),
        "src/a.rs:7 (base)"
    );
}

#[test]
fn first_round_writes_title_then_round() {
    let round = "## Review — 2026-06-25T00:00:00Z\n\nbody.\n\n";
    let out = compose_feedback_log(None, round);
    assert_eq!(
        out,
        "# Final Review Feedback\n\n## Review — 2026-06-25T00:00:00Z\n\nbody.\n\n"
    );
}

#[test]
fn later_round_is_prepended_above_prior_rounds() {
    let existing = "# Final Review Feedback\n\n## Review — 2026-06-24T00:00:00Z\n\nold.\n\n";
    let round = "## Review — 2026-06-25T00:00:00Z\n\nnew.\n\n";
    let out = compose_feedback_log(Some(existing), round);
    // Single title, newest round first, prior round retained after it.
    assert_eq!(out.matches("# Final Review Feedback").count(), 1);
    let new_at = out.find("new.").unwrap();
    let old_at = out.find("old.").unwrap();
    assert!(new_at < old_at, "newest round should come first");
    assert!(out.contains("## Review — 2026-06-24T00:00:00Z"));
}

#[test]
fn tolerates_prior_file_without_title() {
    // A legacy / hand-edited file that doesn't start with the title is kept
    // verbatim below the new round rather than dropped.
    let existing = "## Review — 2026-06-24T00:00:00Z\n\nold.\n";
    let out = compose_feedback_log(Some(existing), "## Review — x\n\nnew.\n\n");
    assert!(out.starts_with("# Final Review Feedback\n\n## Review — x"));
    assert!(out.contains("old."));
}

#[test]
fn split_overflow_rounds_keeps_everything_under_the_cap() {
    let content = compose_feedback_log(
        Some("# Final Review Feedback\n\n## Review — r1\n\nold.\n\n"),
        "## Review — r2\n\nnew.\n\n",
    );
    let (live, overflow) = split_overflow_rounds(&content, 2);
    assert_eq!(live, content);
    assert!(overflow.is_none());
}

#[test]
fn split_overflow_rounds_moves_rounds_past_the_cap_to_the_archive() {
    let existing = compose_feedback_log(
        Some("# Final Review Feedback\n\n## Review — r1\n\noldest.\n\n"),
        "## Review — r2\n\nmiddle.\n\n",
    );
    let content = compose_feedback_log(Some(&existing), "## Review — r3\n\nnewest.\n\n");
    let (live, overflow) = split_overflow_rounds(&content, 2);

    // The two newest rounds stay live, still under a single title.
    assert!(live.starts_with("# Final Review Feedback\n\n## Review — r3"));
    assert!(live.contains("newest."));
    assert!(live.contains("## Review — r2"));
    assert!(live.contains("middle."));
    assert!(!live.contains("oldest."));

    // The oldest round is pushed out for the caller to archive.
    let overflow = overflow.expect("oldest round should overflow");
    assert!(overflow.contains("## Review — r1"));
    assert!(overflow.contains("oldest."));
}

#[test]
fn history_round_parser_preserves_markdown_and_carried_thread_count() {
    let content = "\
# Final Review Feedback

## Review — 2026-07-24T12:00:00Z

**Files reviewed:** 2

#### src/a.rs:7 — [blocker] (unresolved from a previous round)

Fix this.

**Agent:** Fixed in src/a.rs.

## Review — 2026-07-23T12:00:00Z

**Check:** `cargo test` — passed
";
    let rounds = parse_review_history_rounds(content, super::preparation::FEEDBACK_TITLE);
    assert_eq!(rounds.len(), 2);
    assert_eq!(rounds[0].title, "Review — 2026-07-24T12:00:00Z");
    assert_eq!(rounds[0].carried_unresolved, 1);
    assert!(rounds[0].markdown.contains("**Agent:** Fixed"));
    assert!(rounds[1].markdown.contains("cargo test"));
}

#[test]
fn truncate_check_output_passes_short_output_through() {
    assert_eq!(truncate_check_output("all good"), "all good");
}

#[test]
fn truncate_check_output_caps_long_output() {
    let long = "x".repeat(CHECK_OUTPUT_MAX_CHARS + 500);
    let out = truncate_check_output(&long);
    assert!(out.starts_with(&"x".repeat(CHECK_OUTPUT_MAX_CHARS)));
    assert!(out.ends_with("… (truncated)"));
    assert_eq!(
        out.chars().count(),
        CHECK_OUTPUT_MAX_CHARS + "\n… (truncated)".chars().count()
    );
}

#[test]
fn anchor_file_path_strips_line_suffix() {
    assert_eq!(anchor_file_path("src/foo.rs"), "src/foo.rs");
    assert_eq!(anchor_file_path("src/foo.rs:42"), "src/foo.rs");
    assert_eq!(anchor_file_path("src/foo.rs:42-48"), "src/foo.rs");
    assert_eq!(anchor_file_path("src/foo.rs:42 (base)"), "src/foo.rs");
    // A colon not followed by a digit is part of the path, not a suffix.
    assert_eq!(anchor_file_path("weird:name.rs"), "weird:name.rs");
}

#[test]
fn parse_agent_responses_groups_replies_by_file() {
    let feedback = "\
# Final Review Feedback

## Review — 2026-07-02T00:00:00Z

### Files Needing Revision

#### src/foo.rs — [blocker]

Needs error handling.

**Agent:** fixed in src/foo.rs — added a match on the Result.

### Line Comments

#### src/foo.rs:42 — [suggestion]

Rename this variable.

**Agent:** done, renamed to `count`.

#### src/bar.rs:10-12 — [question]

Why the loop?

**Agent:** disagree — the loop is needed for the retry.
";
    let out = parse_agent_responses(feedback);
    let foo = out.get("src/foo.rs").expect("foo replies");
    assert_eq!(foo.len(), 2);
    assert_eq!(foo[0].anchor, "src/foo.rs");
    assert!(foo[0].response.contains("added a match"));
    assert_eq!(foo[1].anchor, "src/foo.rs:42");
    assert!(foo[1].response.contains("renamed to"));
    let bar = out.get("src/bar.rs").expect("bar replies");
    assert_eq!(bar[0].anchor, "src/bar.rs:10-12");
    assert!(bar[0].response.contains("disagree"));
}

#[test]
fn parse_agent_responses_only_reads_latest_round() {
    // Older rounds (below the first `## Review`) must not leak in.
    let feedback = "\
# Final Review Feedback

## Review — 2026-07-02T00:00:00Z

### Line Comments

#### src/foo.rs:5 — [nit]

New item.

## Review — 2026-07-01T00:00:00Z

### Line Comments

#### src/old.rs:9 — [blocker]

Old item.

**Agent:** addressed last round.
";
    let out = parse_agent_responses(feedback);
    // The newest round's item has no reply; the old round's reply is ignored.
    assert!(out.is_empty(), "only the latest round is parsed, {out:?}");
}

#[test]
fn parse_agent_responses_skips_item_text_without_reply() {
    let feedback = "\
# Final Review Feedback

## Review — 2026-07-02T00:00:00Z

### Line Comments

#### src/foo.rs:5 — [nit]

Just a comment, no agent reply yet.
";
    assert!(parse_agent_responses(feedback).is_empty());
}

#[test]
fn parse_agent_responses_captures_multiline_reply() {
    let feedback = "\
# Final Review Feedback

## Review — 2026-07-02T00:00:00Z

### Line Comments

#### src/foo.rs:5 — [suggestion]

Do the thing.

**Agent:** first line of reply.
second line of reply.
";
    let out = parse_agent_responses(feedback);
    let reply = &out.get("src/foo.rs").unwrap()[0].response;
    assert!(reply.contains("first line"));
    assert!(reply.contains("second line"));
}

#[test]
fn walkthrough_prompt_includes_path_and_patch() {
    let file = crate::diff::DiffFile {
        old_path: Some("a.rs".into()),
        path: "a.rs".into(),
        status: crate::diff::DiffFileStatus::Modified,
        additions: 1,
        deletions: 1,
        is_binary: false,
        old_content: None,
        new_content: None,
        patch: "@@ -1 +1 @@\n-old line\n+new line".into(),
        hunks: vec![],
    };
    let prompt = crate::prompts::render_template(
        crate::prompts::PromptId::ReviewWalkthrough
            .spec()
            .default_template,
        &walkthrough_context(&file),
    );
    assert!(prompt.contains("File: a.rs"));
    assert!(prompt.contains("+new line"));
    assert!(prompt.contains("```diff"));
}

#[test]
fn walkthrough_prompt_truncates_huge_patches() {
    let file = crate::diff::DiffFile {
        old_path: Some("big.rs".into()),
        path: "big.rs".into(),
        status: crate::diff::DiffFileStatus::Modified,
        additions: 1,
        deletions: 0,
        is_binary: false,
        old_content: None,
        new_content: None,
        patch: "+x\n".repeat(10_000),
        hunks: vec![],
    };
    let prompt = crate::prompts::render_template(
        crate::prompts::PromptId::ReviewWalkthrough
            .spec()
            .default_template,
        &walkthrough_context(&file),
    );
    assert!(prompt.contains("(diff truncated)"));
}

#[test]
fn parses_documented_and_grouped_note_formats() {
    let content = "\
## src/app/state.rs — add fields

Added the review fields.
Second line.

---

### src/handlers/diff.rs — wire keys

Wired the keys.

## Overview heading not a path

ignored body
";
    let notes = parse_review_notes(content);
    assert_eq!(
        notes.get("src/app/state.rs").map(String::as_str),
        Some("Added the review fields.\nSecond line.")
    );
    assert_eq!(
        notes.get("src/handlers/diff.rs").map(String::as_str),
        Some("Wired the keys.")
    );
    // The non-path overview heading is stored under its text but never
    // matches a real file path.
    assert!(!notes.contains_key("src/app/review.rs"));
}

#[test]
fn bare_path_heading_without_title_is_parsed() {
    let notes = parse_review_notes("## src/main.rs\n\nDid a thing.\n");
    assert_eq!(
        notes.get("src/main.rs").map(String::as_str),
        Some("Did a thing.")
    );
}

#[test]
fn review_notes_cap_keeps_latest_unique_files_and_archives_superseded_notes() {
    let content = "\
# Optional preamble

## src/old.rs — first

Old note.

---

## src/keep.rs — first

Superseded note.

---

## src/new.rs — current

Newest file.

---

## src/keep.rs — updated

Current note.

---
";
    let (live, overflow) = split_overflow_review_notes(content, 2);

    assert!(live.starts_with("# Optional preamble"));
    assert!(live.contains("## src/new.rs — current"));
    assert!(live.contains("## src/keep.rs — updated"));
    assert!(!live.contains("Superseded note."));
    assert!(!live.contains("Old note."));

    let overflow = overflow.expect("old and superseded notes should overflow");
    assert!(!overflow.contains("# Optional preamble"));
    assert!(overflow.contains("## src/old.rs — first"));
    assert!(overflow.contains("## src/keep.rs — first"));
    assert!(!overflow.contains("## src/keep.rs — updated"));
}

#[test]
fn review_notes_cap_is_noop_when_preamble_present_but_unique_notes_fit() {
    let content = "# Optional preamble\n\n## src/a.rs — a\n\nA.\n\n---\n\n## src/b.rs — b\n\nB.\n";
    let (live, overflow) = split_overflow_review_notes(content, 2);
    assert_eq!(live, content);
    assert!(overflow.is_none());
}

#[test]
fn review_notes_cap_is_noop_when_unique_notes_fit() {
    let content = "## src/a.rs — a\n\nA.\n\n---\n\n## src/b.rs — b\n\nB.\n";
    let (live, overflow) = split_overflow_review_notes(content, 2);
    assert_eq!(live, content);
    assert!(overflow.is_none());
}

#[test]
fn blind_appended_duplicates_collapse_below_the_cap() {
    // Option F: the agent appends notes without reading the file, so within
    // one session it can append several sections for the same file. The
    // per-turn archive pass must collapse them to the newest even when the
    // unique-file count is nowhere near MAX_LIVE_REVIEW_NOTE_FILES — i.e.
    // with no cap pressure at all, only supersession.
    let content = "\
## src/main.rs — first pass

Sketched the entry point.

---

## src/lib.rs — helper

Added a helper.

---

## src/main.rs — second pass

Wired the entry point to the helper.

---
";
    let (live, overflow) = split_overflow_review_notes(content, 50);

    assert!(live.contains("## src/lib.rs — helper"));
    assert!(live.contains("## src/main.rs — second pass"));
    assert!(!live.contains("Sketched the entry point."));

    let overflow = overflow.expect("the superseded src/main.rs note should archive");
    assert!(overflow.contains("Sketched the entry point."));
    assert!(!overflow.contains("second pass"));
}

#[test]
fn archived_review_notes_remain_visible_but_live_note_wins() {
    let dir = tempfile::TempDir::new().unwrap();
    let claude = dir.path().join(".claude");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::write(
        claude.join("review-notes-archive.md"),
        "# Review Notes Archive\n\n\
             ## src/archived.rs — old\n\nArchived context.\n\n---\n\n\
             ## src/live.rs — stale\n\nStale context.\n",
    )
    .unwrap();
    std::fs::write(
        claude.join("review-notes.md"),
        "## src/live.rs — current\n\nCurrent context.\n",
    )
    .unwrap();

    let notes = load_review_notes(dir.path());
    assert_eq!(
        notes.get("src/archived.rs").map(String::as_str),
        Some("Archived context.")
    );
    assert_eq!(
        notes.get("src/live.rs").map(String::as_str),
        Some("Current context.")
    );
}

#[test]
fn archive_review_notes_moves_overflow_and_is_idempotent() {
    let dir = tempfile::TempDir::new().unwrap();
    let claude = dir.path().join(".claude");
    std::fs::create_dir_all(&claude).unwrap();
    let mut content = String::new();
    for index in 0..=super::preparation::MAX_LIVE_REVIEW_NOTE_FILES {
        content.push_str(&format!(
            "## src/file-{index}.rs — note\n\nNote {index}.\n\n---\n\n"
        ));
    }
    std::fs::write(claude.join("review-notes.md"), content).unwrap();

    assert_eq!(archive_review_notes(dir.path()).unwrap(), 1);
    assert_eq!(archive_review_notes(dir.path()).unwrap(), 0);

    let live = std::fs::read_to_string(claude.join("review-notes.md")).unwrap();
    assert!(!live.contains("## src/file-0.rs"));
    assert!(live.contains(&format!(
        "## src/file-{}.rs",
        super::preparation::MAX_LIVE_REVIEW_NOTE_FILES
    )));
    let archive = std::fs::read_to_string(claude.join("review-notes-archive.md")).unwrap();
    assert_eq!(archive.matches("# Review Notes Archive").count(), 1);
    assert!(archive.contains("## src/file-0.rs"));
}

#[test]
fn archive_review_notes_collapses_blind_appended_duplicates_on_disk() {
    // End-to-end P0 for Option F: a blind-append turn that re-notes the same
    // file must leave only the newest section live, with the stale copy in
    // the archive and load_review_notes returning the current note. No cap
    // is hit here — two sections, one path.
    let dir = tempfile::TempDir::new().unwrap();
    let claude = dir.path().join(".claude");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::write(
        claude.join("review-notes.md"),
        "## src/a.rs — first\n\nEarly note.\n\n---\n\n\
             ## src/a.rs — refined\n\nLater, fuller note.\n\n---\n",
    )
    .unwrap();

    assert_eq!(archive_review_notes(dir.path()).unwrap(), 1);
    assert_eq!(archive_review_notes(dir.path()).unwrap(), 0);

    let live = std::fs::read_to_string(claude.join("review-notes.md")).unwrap();
    assert!(live.contains("Later, fuller note."));
    assert!(!live.contains("Early note."));

    let notes = load_review_notes(dir.path());
    assert_eq!(
        notes.get("src/a.rs").map(String::as_str),
        Some("Later, fuller note.")
    );
}

const SEARCH_PATCH: &str = "\
diff --git a/a.rs b/a.rs
index 1111111..2222222 100644
--- a/a.rs
+++ b/a.rs
@@ -1,3 +1,4 @@
 fn alpha() {}
-fn beta() {}
+fn beta_two() {}
+fn gamma_alpha() {}
 fn delta() {}
";

#[test]
fn compute_search_matches_finds_case_insensitive_substrings() {
    let files = parse_unified_diff(SEARCH_PATCH).unwrap();
    let file = &files[0];
    // Addressable lines: 0 alpha (ctx), 1 beta (del), 2 beta_two (add),
    // 3 gamma_alpha (add), 4 delta (ctx).
    assert_eq!(compute_search_matches(file, "alpha"), vec![0, 3]);
    assert_eq!(compute_search_matches(file, "beta"), vec![1, 2]);
    // Case-insensitive.
    assert_eq!(compute_search_matches(file, "ALPHA"), vec![0, 3]);
}

#[test]
fn compute_search_matches_empty_query_or_no_hit_is_empty() {
    let files = parse_unified_diff(SEARCH_PATCH).unwrap();
    let file = &files[0];
    assert!(compute_search_matches(file, "").is_empty());
    assert!(compute_search_matches(file, "   ").is_empty());
    assert!(compute_search_matches(file, "nonexistent").is_empty());
}

#[test]
fn on_file_changed_clears_active_search() {
    let mut state = DiffViewerState::new(
        crate::app::state::ViewState::new(
            "p".into(),
            "f".into(),
            "s".into(),
            "w".into(),
            "Claude".into(),
            crate::project::SessionKind::Claude,
            crate::project::VibeMode::Vibeless,
            true,
        ),
        std::path::PathBuf::from("/tmp"),
    );
    state.search_query = "alpha".into();
    state.search_matches = vec![0, 3];
    state.search_match_pos = Some(1);
    state.editing_search = true;

    state.on_file_changed();

    assert!(state.search_query.is_empty());
    assert!(state.search_matches.is_empty());
    assert_eq!(state.search_match_pos, None);
    assert!(!state.editing_search);
}

// ---- re-anchoring line comments across edits -------------------------

fn texts(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|s| s.to_string()).collect()
}

/// The same file, before and after an edit shifted its line numbers. The
/// hunk contents are identical; only the hunk header moves, so every
/// comment's exact `DiffLineLocation` goes stale while its context snippet
/// still matches uniquely.
fn shifted_diff_pair() -> (DiffFile, DiffFile) {
    let body = "\
 fn main() {
-    let x = 1;
+    let x = 2;
+    let y = 3;
 }
";
    let build = |header: &str| {
        let raw = format!(
            "diff --git a/a.rs b/a.rs\n\
                 index 1111111..2222222 100644\n\
                 --- a/a.rs\n\
                 +++ b/a.rs\n\
                 {header}\n{body}"
        );
        parse_unified_diff(&raw).unwrap().remove(0)
    };
    (build("@@ -1,3 +1,4 @@"), build("@@ -10,3 +20,4 @@"))
}

/// A single-line comment anchored to `idx`, with its context snippet captured
/// from `file` exactly as `recapture_anchor_contexts` would.
fn anchored_comment(file: &DiffFile, idx: usize, text: &str) -> LineComment {
    LineComment {
        location: file.addressable_lines()[idx],
        start: None,
        text: text.to_string(),
        draft: false,
        suggestion: None,
        severity: Severity::default(),
        anchor_context: CommentAnchorContext::capture(&file.addressable_line_texts(), idx),
        start_anchor_context: None,
        anchor_lost: false,
        resolved: false,
        carried: false,
    }
}

#[test]
fn anchor_context_capture_bounds_at_file_edges() {
    let lines = texts(&["l0", "l1", "l2", "l3", "l4"]);

    let mid = CommentAnchorContext::capture(&lines, 2).unwrap();
    assert_eq!(mid.line, "l2");
    assert_eq!(mid.before, texts(&["l0", "l1"]));
    assert_eq!(mid.after, texts(&["l3", "l4"]));

    let first = CommentAnchorContext::capture(&lines, 0).unwrap();
    assert!(first.before.is_empty());
    assert_eq!(first.after, texts(&["l1", "l2"]));

    let last = CommentAnchorContext::capture(&lines, 4).unwrap();
    assert_eq!(last.before, texts(&["l2", "l3"]));
    assert!(last.after.is_empty());

    assert!(CommentAnchorContext::capture(&lines, 5).is_none());
}

#[test]
fn anchor_context_best_match_disambiguates_repeated_lines_by_neighbours() {
    let ctx = CommentAnchorContext {
        line: "    x += 1;".to_string(),
        before: texts(&["fn a() {"]),
        after: texts(&["}"]),
    };
    // "    x += 1;" appears twice; only the second sits between fn a() and }.
    let haystack = texts(&[
        "fn b() {",
        "    x += 1;",
        "    more();",
        "fn a() {",
        "    x += 1;",
        "}",
    ]);
    assert_eq!(ctx.best_match(&haystack), Some(4));
}

#[test]
fn anchor_context_best_match_tolerates_reindentation() {
    let ctx = CommentAnchorContext {
        line: "let x = 1;".to_string(),
        before: texts(&["fn a() {"]),
        after: Vec::new(),
    };
    let haystack = texts(&["fn a() {", "        let x = 1;"]);
    assert_eq!(ctx.best_match(&haystack), Some(1));
}

#[test]
fn anchor_context_best_match_gives_up_when_ambiguous_or_absent() {
    // Two identical candidates with identical neighbours: a tie, so refuse to
    // guess rather than re-anchor onto the wrong one.
    let ambiguous = CommentAnchorContext {
        line: "x".to_string(),
        before: texts(&["a"]),
        after: texts(&["b"]),
    };
    let tie = texts(&["a", "x", "b", "a", "x", "b"]);
    assert_eq!(ambiguous.best_match(&tie), None);

    // The line is simply gone.
    assert_eq!(ambiguous.best_match(&texts(&["p", "q"])), None);

    // A blank anchor line carries no signal.
    let blank = CommentAnchorContext {
        line: "   ".to_string(),
        before: texts(&["a"]),
        after: texts(&["b"]),
    };
    assert_eq!(blank.best_match(&texts(&["a", "", "b"])), None);
}

#[test]
fn reanchor_leaves_still_resolving_comments_untouched() {
    let (before, _) = shifted_diff_pair();
    let mut comments = vec![anchored_comment(&before, 2, "why 2?")];
    let original = comments[0].location;

    let (moved, lost) = reanchor_file_comments(&before, &mut comments);
    assert_eq!((moved, lost), (0, 0));
    assert_eq!(comments[0].location, original);
    assert!(!comments[0].anchor_lost);
}

#[test]
fn reanchor_relocates_a_comment_whose_line_moved() {
    let (before, after) = shifted_diff_pair();
    let mut comments = vec![anchored_comment(&before, 2, "why 2?")];
    // The added `let x = 2;` was new_line 2; after the shift it is new_line 21.
    assert_eq!(comments[0].location.new_line, Some(2));

    let (moved, lost) = reanchor_file_comments(&after, &mut comments);
    assert_eq!((moved, lost), (1, 0));
    assert_eq!(comments[0].location, after.addressable_lines()[2]);
    assert_eq!(comments[0].location.new_line, Some(21));
    assert!(!comments[0].anchor_lost);
}

#[test]
fn reanchor_does_not_trust_a_reused_line_number_with_different_text() {
    let before = local_apply_file("one\ntwo\nthree\n");
    let mut after = local_apply_file("one\ninserted\ntwo\nthree\n");
    after.hunks[0].new_lines = 4;
    after.hunks[0].lines = vec![
        DiffLine {
            kind: DiffLineKind::Context,
            text: " one".to_string(),
        },
        DiffLine {
            kind: DiffLineKind::Added,
            text: "+inserted".to_string(),
        },
        DiffLine {
            kind: DiffLineKind::Added,
            text: "+two".to_string(),
        },
        DiffLine {
            kind: DiffLineKind::Context,
            text: " three".to_string(),
        },
    ];
    let mut comment = line_comment(Some(2), None, "track two");
    comment.location = before.addressable_lines()[1];
    comment.anchor_context = CommentAnchorContext::capture(&before.addressable_line_texts(), 1);

    let (moved, lost) = reanchor_file_comments(&after, std::slice::from_mut(&mut comment));

    assert_eq!((moved, lost), (1, 0));
    assert_eq!(comment.location.new_line, Some(3));
}

#[test]
fn reanchor_flags_a_comment_whose_line_is_gone() {
    let (_, after) = shifted_diff_pair();
    let mut comment = LineComment {
        location: DiffLineLocation {
            old_line: None,
            new_line: Some(2),
        },
        start: None,
        text: "stale".to_string(),
        draft: false,
        suggestion: None,
        severity: Severity::default(),
        anchor_context: Some(CommentAnchorContext {
            line: "    let deleted = 9;".to_string(),
            before: Vec::new(),
            after: Vec::new(),
        }),
        start_anchor_context: None,
        anchor_lost: false,
        resolved: false,
        carried: false,
    };
    let (moved, lost) = reanchor_file_comments(&after, std::slice::from_mut(&mut comment));
    assert_eq!((moved, lost), (0, 1));
    assert!(comment.anchor_lost);

    // Already-lost comments are not re-counted on a subsequent refresh.
    let (moved, lost) = reanchor_file_comments(&after, std::slice::from_mut(&mut comment));
    assert_eq!((moved, lost), (0, 0));
    assert!(comment.anchor_lost);
}

#[test]
fn reanchor_degrades_a_range_whose_start_cannot_be_found() {
    let (before, after) = shifted_diff_pair();
    let mut comment = anchored_comment(&before, 3, "this pair");
    // A range starting at idx 2, but with a start snippet that no longer
    // matches anything in the refreshed diff.
    comment.start = Some(before.addressable_lines()[2]);
    comment.start_anchor_context = Some(CommentAnchorContext {
        line: "    let vanished = 0;".to_string(),
        before: Vec::new(),
        after: Vec::new(),
    });

    let (moved, lost) = reanchor_file_comments(&after, std::slice::from_mut(&mut comment));
    assert_eq!((moved, lost), (1, 0));
    // End re-anchored; the span collapsed rather than inverting or guessing.
    assert_eq!(comment.location, after.addressable_lines()[3]);
    assert_eq!(comment.start, None);
    assert!(!comment.anchor_lost);
}

#[test]
fn reanchor_relocates_both_ends_of_a_range() {
    let (before, after) = shifted_diff_pair();
    let mut comment = anchored_comment(&before, 3, "this pair");
    comment.start = Some(before.addressable_lines()[2]);
    comment.start_anchor_context =
        CommentAnchorContext::capture(&before.addressable_line_texts(), 2);

    let (moved, lost) = reanchor_file_comments(&after, std::slice::from_mut(&mut comment));
    assert_eq!((moved, lost), (1, 0));
    assert_eq!(comment.location, after.addressable_lines()[3]);
    assert_eq!(comment.start, Some(after.addressable_lines()[2]));
}

#[test]
fn reanchor_cannot_relocate_a_comment_with_no_context_snippet() {
    // Comments loaded from a progress file written before re-anchoring existed.
    let (_, after) = shifted_diff_pair();
    let mut comment = LineComment {
        location: DiffLineLocation {
            old_line: None,
            new_line: Some(2),
        },
        start: None,
        text: "legacy".to_string(),
        draft: false,
        suggestion: None,
        severity: Severity::default(),
        anchor_context: None,
        start_anchor_context: None,
        anchor_lost: false,
        resolved: false,
        carried: false,
    };
    let (moved, lost) = reanchor_file_comments(&after, std::slice::from_mut(&mut comment));
    assert_eq!((moved, lost), (0, 1));
    assert!(comment.anchor_lost);
}

#[test]
fn progress_files_written_before_reanchoring_still_load() {
    // A minimal old-shape progress file: none of the anchor fields, and none
    // of the other fields added since the first version of the format.
    let json = r#"{
            "line_comments": {
                "a.rs": [{ "location": { "old_line": null, "new_line": 2 }, "text": "old" }]
            }
        }"#;
    let progress: super::preparation::ReviewProgress = serde_json::from_str(json).unwrap();
    let comment = &progress.line_comments["a.rs"][0];

    assert_eq!(comment.text, "old");
    assert_eq!(comment.location.new_line, Some(2));
    assert_eq!(comment.anchor_context, None);
    assert_eq!(comment.start_anchor_context, None);
    assert!(!comment.anchor_lost);
}

#[test]
fn lost_anchor_comment_labels_by_file_and_skips_inline_pr_posting() {
    let mut comment = line_comment(Some(42), None, "still worth a look");
    comment.anchor_lost = true;

    assert_eq!(
        comment_anchor_label("src/foo.rs", &comment),
        "src/foo.rs (anchor lost — possibly addressed)"
    );
    // …and that heading still resolves back to its file for agent replies.
    assert_eq!(
        anchor_file_path("src/foo.rs (anchor lost — possibly addressed)"),
        "src/foo.rs"
    );

    // A stale line number must never be posted inline on the PR.
    let sections = vec![("src/foo.rs".to_string(), vec![comment])];
    let (body, comments, _) = build_pr_review(&[], &[], &sections, "", &HashMap::new());
    assert!(comments.is_empty(), "lost anchor must not post inline");
    assert!(!body.contains("src/foo.rs:42"));
}

/// Addressable lines: 0 context(new 1), 1 removed(new None), 2 added(new 2),
/// 3 added(new 3), 4 context(new 4).
fn editor_line_file() -> DiffFile {
    parse_unified_diff(
        "\
diff --git a/a.rs b/a.rs
index 1111111..2222222 100644
--- a/a.rs
+++ b/a.rs
@@ -1,3 +1,4 @@
 fn alpha() {}
-fn beta() {}
+fn beta_two() {}
+fn gamma_alpha() {}
 fn delta() {}
",
    )
    .unwrap()
    .remove(0)
}

#[test]
fn editor_target_line_uses_the_cursored_lines_current_side_number() {
    let file = editor_line_file();
    assert_eq!(editor_target_line(&file, Some(2)), Some(2));
    assert_eq!(editor_target_line(&file, Some(3)), Some(3));
    assert_eq!(editor_target_line(&file, Some(4)), Some(4));
}

#[test]
fn editor_target_line_falls_back_to_the_nearest_surviving_line() {
    let file = editor_line_file();
    // Index 1 is a removal: it has no line in the file on disk, so the
    // editor should land on the surviving line just above it.
    assert_eq!(editor_target_line(&file, Some(1)), Some(1));
}

#[test]
fn editor_target_line_without_a_cursor_lands_on_the_first_hunk() {
    let file = editor_line_file();
    assert_eq!(editor_target_line(&file, None), Some(1));
}

#[test]
fn editor_target_line_is_none_when_there_is_nothing_addressable() {
    let mut file = editor_line_file();
    file.hunks.clear();
    assert_eq!(editor_target_line(&file, None), None);
    assert_eq!(editor_target_line(&file, Some(3)), None);
}

#[test]
fn editor_invocation_uses_the_plus_flag_for_vi_style_editors() {
    let (program, args) = editor_invocation("nvim", "src/a.rs", Some(42)).unwrap();
    assert_eq!(program, "nvim");
    assert_eq!(args, vec!["+42".to_string(), "src/a.rs".to_string()]);
}

#[test]
fn editor_invocation_keeps_flags_baked_into_the_env_var() {
    let (program, args) = editor_invocation("emacsclient -nw", "src/a.rs", Some(7)).unwrap();
    assert_eq!(program, "emacsclient");
    assert_eq!(args, vec!["-nw", "+7", "src/a.rs"]);
}

#[test]
fn editor_invocation_makes_vscode_block_and_goto() {
    let (program, args) = editor_invocation("code", "src/a.rs", Some(42)).unwrap();
    assert_eq!(program, "code");
    assert_eq!(args, vec!["--wait", "--goto", "src/a.rs:42"]);

    // An explicit --wait must not be duplicated.
    let (_, args) = editor_invocation("code --wait", "src/a.rs", Some(42)).unwrap();
    assert_eq!(args, vec!["--wait", "--goto", "src/a.rs:42"]);
}

#[test]
fn editor_invocation_uses_path_suffix_syntax_where_that_is_the_convention() {
    let (_, args) = editor_invocation("hx", "src/a.rs", Some(42)).unwrap();
    assert_eq!(args, vec!["src/a.rs:42"]);
}

#[test]
fn editor_invocation_opens_an_unknown_editor_at_the_top_rather_than_guessing() {
    // A flag an editor doesn't understand would be read as a second
    // filename, silently opening an empty buffer called `+42`.
    let (program, args) = editor_invocation("my-editor", "src/a.rs", Some(42)).unwrap();
    assert_eq!(program, "my-editor");
    assert_eq!(args, vec!["src/a.rs"]);
}

#[test]
fn editor_invocation_handles_an_absolute_path_and_no_line() {
    let (program, args) = editor_invocation("/usr/bin/vim", "src/a.rs", None).unwrap();
    assert_eq!(program, "/usr/bin/vim");
    assert_eq!(args, vec!["src/a.rs"]);

    assert!(editor_invocation("   ", "src/a.rs", Some(1)).is_none());
}
