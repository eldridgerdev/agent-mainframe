use super::domain::{
    COMMENT_HUNK_CONTEXT_LINES, LEGACY_REVIEW_SESSION_LABEL, WHOLE_FILE_HUNK_LINES,
    append_amf_attribution, reply_posted_via_amf,
};
use super::fetch::{make_snippet, normalize, truncate_chars};
use super::investigation::{
    InvestigationChangedFile, InvestigationFollowUp, InvestigationPromptContext,
    build_investigation_prompt, investigation_failure_message, upsert_investigation_in_memory,
};
use super::memory::{bootstrap_pr_text, bootstrap_prompt};
use super::reply::{
    commit_after_fix_request, commit_for_done_reply, commit_touching_file, commit_touching_line,
};
use std::path::Path;

use chrono::Local;

use crate::app::{AgentKind, ReplyState};
use crate::editor::TextEditor;
use crate::github::{PrRef, Review, ReviewComment, ReviewThread};

use super::*;
use crate::github::GhUser;

fn pr() -> PrRef {
    PrRef {
        number: 1,
        head_sha: "sha".into(),
        url: "https://github.com/o/r/pull/1".into(),
        owner: "o".into(),
        repo: "r".into(),
        head_ref: "main".into(),
    }
}

fn user(login: &str, kind: &str) -> GhUser {
    GhUser {
        login: login.into(),
        kind: kind.into(),
    }
}

fn sample_comment(id: u64, author: &str, is_bot: bool) -> PrComment {
    PrComment {
        id,
        kind: CommentKind::Inline,
        author: author.to_string(),
        is_bot,
        path: Some("src/lib.rs".to_string()),
        line: Some(10),
        side: None,
        outdated: false,
        file_level: false,
        diff_hunk: None,
        body: "example".to_string(),
        snippet: "example".to_string(),
        in_reply_to: None,
        thread_id: None,
        is_resolved: false,
        triage: TriageState::default(),
        local_note: None,
        batch_id: None,
        github_id: None,
        github_review_id: None,
    }
}

#[test]
fn strips_details_comments_and_images() {
    let body = "Real point here.\n\n<details>\n<summary>Prompt for AI agents</summary>\n\
            lots of tokens\n</details>\n<!-- internal note -->\n![badge](http://x/y.png)";
    let out = strip_bot_boilerplate(body);
    assert_eq!(out, "Real point here.");
}

#[test]
fn strips_quoted_diff_fence() {
    let body = "This can race with the poller.\n\n```diff\n@@ -40,6 +40,7 @@\n-old\n+new\n```\n\nGuard it behind the lock.";
    let out = strip_bot_boilerplate(body);
    assert_eq!(
        out,
        "This can race with the poller.\n\nGuard it behind the lock."
    );
}

#[test]
fn strips_quoted_suggestion_fence() {
    let body = "Consider this:\n\n```suggestion\nlet x = 1;\n```\n\nSaves a line.";
    let out = strip_bot_boilerplate(body);
    assert_eq!(out, "Consider this:\n\nSaves a line.");
}

#[test]
fn strips_leading_quoted_diff_lines() {
    let body = "> -old line\n> +new line\n\nActual comment text.";
    let out = strip_bot_boilerplate(body);
    assert_eq!(out, "Actual comment text.");
}

#[test]
fn leaves_non_diff_fences_untouched() {
    let body = "Use this instead:\n\n```rust\nlet x = 1;\n```\n\nCleaner.";
    let out = strip_bot_boilerplate(body);
    assert_eq!(out, body);
}

/// Init a throwaway git repo at `dir`, writing `contents` for `rel_path`
/// across one commit per entry in `contents` (so later entries are more
/// recent history). Returns the short sha of each commit, oldest first.
fn git_repo_with_history(dir: &Path, rel_path: &str, contents: &[&str]) -> Vec<String> {
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git command");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Test"]);
    let file = dir.join(rel_path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let mut shas = Vec::new();
    for (i, body) in contents.iter().enumerate() {
        std::fs::write(&file, body).unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", &format!("commit {i}")]);
        shas.push(git(&["rev-parse", "--short", "HEAD"]));
    }
    shas
}

#[test]
fn commit_touching_line_finds_the_commit_that_last_changed_it() {
    let repo = tempfile::TempDir::new().unwrap();
    let shas = git_repo_with_history(
        repo.path(),
        "src/file.rs",
        &["line1\nline2\nline3\n", "line1\nCHANGED\nline3\n"],
    );

    // Line 2 was touched by the second commit only.
    assert_eq!(
        commit_touching_line(repo.path(), "src/file.rs", 2),
        Some(shas[1].clone())
    );
    // Line 1 has never changed since the first commit.
    assert_eq!(
        commit_touching_line(repo.path(), "src/file.rs", 1),
        Some(shas[0].clone())
    );
}

#[test]
fn commit_touching_line_is_none_for_a_line_outside_the_file() {
    let repo = tempfile::TempDir::new().unwrap();
    git_repo_with_history(repo.path(), "src/file.rs", &["one line\n"]);

    assert_eq!(commit_touching_line(repo.path(), "src/file.rs", 999), None);
}

#[test]
fn commit_touching_file_returns_the_latest_commit_on_that_path() {
    let repo = tempfile::TempDir::new().unwrap();
    let shas = git_repo_with_history(repo.path(), "src/file.rs", &["a\n", "b\n", "c\n"]);

    assert_eq!(
        commit_touching_file(repo.path(), "src/file.rs"),
        Some(shas[2].clone())
    );
}

#[test]
fn commit_touching_file_is_none_for_an_untracked_path() {
    let repo = tempfile::TempDir::new().unwrap();
    git_repo_with_history(repo.path(), "src/file.rs", &["a\n"]);

    assert_eq!(commit_touching_file(repo.path(), "src/other.rs"), None);
}

#[test]
fn commit_for_done_reply_prefers_line_history_when_the_line_is_current() {
    let repo = tempfile::TempDir::new().unwrap();
    let shas = git_repo_with_history(
        repo.path(),
        "src/file.rs",
        &["line1\nline2\n", "line1\nfixed\n"],
    );
    let mut comment = inline_comment("needs a fix", false);
    comment.path = Some("src/file.rs".into());
    comment.line = Some(2);
    comment.outdated = false;

    assert_eq!(
        commit_for_done_reply(repo.path(), &comment),
        (Some(shas[1].clone()), true)
    );
}

#[test]
fn commit_for_done_reply_skips_line_search_for_an_outdated_anchor() {
    let repo = tempfile::TempDir::new().unwrap();
    // The comment's remembered line (2) hasn't changed since the first
    // commit; only the file as a whole was touched again afterward.
    let shas = git_repo_with_history(
        repo.path(),
        "src/file.rs",
        &["line1\nline2\n", "line1\nline2\nline3\n"],
    );
    let mut comment = inline_comment("stale anchor", false);
    comment.path = Some("src/file.rs".into());
    comment.line = Some(2);
    comment.outdated = true;

    // Falls straight to file history (the most recent commit) rather
    // than trusting the outdated line number.
    assert_eq!(
        commit_for_done_reply(repo.path(), &comment),
        (Some(shas[1].clone()), true)
    );
}

#[test]
fn commit_for_done_reply_falls_back_to_head_with_a_caveat() {
    let repo = tempfile::TempDir::new().unwrap();
    let shas = git_repo_with_history(repo.path(), "src/other.rs", &["x\n"]);
    let mut comment = inline_comment("unrelated file", false);
    comment.path = Some("src/not-tracked.rs".into());
    comment.line = Some(1);
    comment.outdated = false;

    // Neither line nor file history exists for this path; falls back to
    // bare HEAD, flagged as an unconfident match.
    assert_eq!(
        commit_for_done_reply(repo.path(), &comment),
        (Some(shas[0].clone()), false)
    );
}

#[test]
fn commit_for_done_reply_none_outside_a_git_repo() {
    let dir = tempfile::TempDir::new().unwrap();
    let comment = inline_comment("no repo here", false);

    assert_eq!(commit_for_done_reply(dir.path(), &comment), (None, false));
}

#[test]
fn commit_after_fix_request_finds_a_commit_made_since_the_base() {
    let repo = tempfile::TempDir::new().unwrap();
    let shas = git_repo_with_history(repo.path(), "src/file.rs", &["line1\n", "line1\nline2\n"]);
    let mut comment = inline_comment("needs a fix", false);
    comment.path = Some("src/file.rs".into());

    assert_eq!(
        commit_after_fix_request(repo.path(), &comment, &shas[0]),
        Some(shas[1].clone())
    );
}

#[test]
fn commit_after_fix_request_none_for_a_conversation_level_comment() {
    let repo = tempfile::TempDir::new().unwrap();
    let shas = git_repo_with_history(repo.path(), "src/file.rs", &["a\n", "b\n"]);
    let mut comment = inline_comment("no path here", false);
    comment.path = None;

    // No file to check "touched-ness" against, so any commit after the
    // base would be an unverified guess — decline rather than cite one.
    assert_eq!(
        commit_after_fix_request(repo.path(), &comment, &shas[0]),
        None
    );
}

#[test]
fn commit_after_fix_request_none_when_head_is_not_a_descendant_of_base() {
    let repo = tempfile::TempDir::new().unwrap();
    let shas = git_repo_with_history(repo.path(), "src/file.rs", &["a\n"]);
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(repo.path())
            .output()
            .unwrap();
    };
    // Move to an unrelated branch history so HEAD no longer descends from
    // the recorded base — e.g. a force-push or rebase moved the branch.
    git(&["checkout", "-q", "--orphan", "other"]);
    std::fs::write(repo.path().join("src/file.rs"), "unrelated\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "unrelated history"]);
    let mut comment = inline_comment("needs a fix", false);
    comment.path = Some("src/file.rs".into());

    assert_eq!(
        commit_after_fix_request(repo.path(), &comment, &shas[0]),
        None
    );
}

#[test]
fn snippet_truncates_with_ellipsis() {
    let long = "x".repeat(200);
    let s = truncate_chars(&long, SNIPPET_LEN);
    assert_eq!(s.chars().count(), SNIPPET_LEN);
    assert!(s.ends_with('…'));
}

#[test]
fn snippet_skips_leading_blank_lines() {
    assert_eq!(make_snippet("\n\n  hello world  \n", false), "hello world");
}

#[test]
fn normalize_attaches_resolution_and_outdated() {
    let comments = vec![
        ReviewComment {
            id: 11,
            path: Some("a.rs".into()),
            line: None, // outdated
            original_line: Some(7),
            side: Some("RIGHT".into()),
            diff_hunk: Some("@@".into()),
            subject_type: None,
            body: "race condition".into(),
            user: user("alice", "User"),
            in_reply_to_id: None,
            pull_request_review_id: Some(99),
        },
        ReviewComment {
            id: 12,
            path: Some("b.rs".into()),
            line: Some(3),
            original_line: Some(3),
            side: Some("RIGHT".into()),
            diff_hunk: None,
            subject_type: None,
            body: "nit".into(),
            user: user("coderabbitai", "Bot"),
            in_reply_to_id: None,
            pull_request_review_id: None,
        },
    ];
    let threads = vec![ReviewThread {
        id: "T1".into(),
        is_resolved: true,
        comment_ids: vec![11],
    }];

    let review = normalize(pr(), comments, vec![], vec![], threads);
    assert_eq!(review.comments.len(), 2);

    let c11 = &review.comments[0];
    assert_eq!(c11.line, Some(7)); // fell back to original_line
    assert!(c11.outdated);
    assert!(c11.is_resolved);
    assert_eq!(c11.thread_id.as_deref(), Some("T1"));
    assert!(!c11.is_bot);

    let c12 = &review.comments[1];
    assert!(!c12.outdated);
    assert!(!c12.is_resolved);
    assert!(c12.is_bot);

    assert_eq!(review.open_count(), 1); // only c12 is unresolved
}

#[test]
fn normalize_marks_file_level_comments_and_not_as_outdated() {
    // A file-level comment has no `line` by definition — that must not be
    // mistaken for an outdated line comment.
    let comments = vec![ReviewComment {
        id: 21,
        path: Some("src/big.rs".into()),
        line: None,
        original_line: None,
        side: None,
        diff_hunk: Some("@@ -1,400 +1,420 @@\n+ enormous".into()),
        subject_type: Some("file".into()),
        body: "This module does too much.".into(),
        user: user("alice", "User"),
        in_reply_to_id: None,
        pull_request_review_id: Some(99),
    }];

    let review = normalize(pr(), comments, vec![], vec![], vec![]);
    let c = &review.comments[0];
    assert!(c.file_level);
    assert!(!c.outdated);
    assert_eq!(c.prompt_hunk(), None);
}

#[test]
fn normalize_drops_empty_review_summaries() {
    let reviews = vec![
        Review {
            id: 1,
            body: "".into(),
            state: "APPROVED".into(),
            user: user("bob", "User"),
        },
        Review {
            id: 2,
            body: "Please add a test.".into(),
            state: "CHANGES_REQUESTED".into(),
            user: user("bob", "User"),
        },
    ];
    let review = normalize(pr(), vec![], reviews, vec![], vec![]);
    assert_eq!(review.comments.len(), 1);
    assert_eq!(
        review.comments[0].kind,
        CommentKind::ReviewSummary {
            state: "CHANGES_REQUESTED".into()
        }
    );
}

fn inline_comment(body: &str, is_bot: bool) -> PrComment {
    PrComment {
        id: 1,
        kind: CommentKind::Inline,
        author: "alice".into(),
        is_bot,
        path: Some("src/app/sync.rs".into()),
        line: Some(42),
        side: Some("RIGHT".into()),
        outdated: false,
        file_level: false,
        diff_hunk: Some("@@ -38,4 +38,5 @@\n  poll = 250;\n+ self.sync();".into()),
        body: body.into(),
        snippet: String::new(),
        in_reply_to: None,
        thread_id: None,
        is_resolved: false,
        triage: TriageState::Untriaged,
        local_note: None,
        batch_id: None,
        github_id: None,
        github_review_id: None,
    }
}

#[test]
fn fix_prompt_includes_file_line_comment_and_hunk() {
    let c = inline_comment("Guard this behind the lock.", false);
    let prompt = c.fix_prompt();
    assert!(prompt.starts_with("Address this PR review comment."));
    assert!(prompt.contains("File: src/app/sync.rs:42"));
    assert!(prompt.contains("Comment (@alice): Guard this behind the lock."));
    assert!(prompt.contains("Diff hunk:"));
    assert!(prompt.contains("+ self.sync();"));
    // No file contents are ever injected — only the comment + hunk.
    assert!(!prompt.contains("fn "));
}

#[test]
fn fix_prompt_omits_hunk_for_file_level_comment() {
    let mut c = inline_comment("Split this module up.", false);
    c.file_level = true;
    c.line = None;
    // GitHub hands a file-level comment the entire file diff as its hunk.
    c.diff_hunk = Some("@@ -1,400 +1,420 @@\n+ a\n+ b".into());

    assert_eq!(c.prompt_hunk(), None);
    assert!(c.hunk_suppressed());

    let prompt = c.fix_prompt();
    assert!(prompt.contains("File: src/app/sync.rs  (comment on the whole file)"));
    assert!(!prompt.contains("Diff hunk:"));
    assert!(!prompt.contains("+ a"));
    // The agent is told the hunk was withheld, not that there was none.
    assert!(prompt.contains("Diff hunk omitted"));
}

#[test]
fn whole_file_sized_hunk_is_dropped_but_ordinary_ones_are_kept() {
    let hunk_of = |n: usize| {
        Some(
            std::iter::repeat_n("+ line", n)
                .collect::<Vec<_>>()
                .join("\n"),
        )
    };

    // Real line comments run to ~90 hunk lines; those keep their context.
    let mut c = inline_comment("This block is wrong.", false);
    c.diff_hunk = hunk_of(93);
    assert!(c.prompt_hunk().is_some());
    assert!(!c.hunk_suppressed());
    assert!(c.fix_prompt().contains("Diff hunk:"));

    // Only a pathological, whole-file-sized hunk trips the backstop.
    c.diff_hunk = hunk_of(WHOLE_FILE_HUNK_LINES + 1);
    assert_eq!(c.prompt_hunk(), None);
    let prompt = c.fix_prompt();
    // Still line-anchored, so the pointer keeps its line — only the wall of
    // diff is dropped, and not as a "whole file" comment.
    assert!(prompt.contains("File: src/app/sync.rs:42"));
    assert!(!prompt.contains("(comment on the whole file)"));
    assert!(!prompt.contains("Diff hunk:"));
    assert!(prompt.contains("Diff hunk omitted"));
}

#[test]
fn line_comment_windows_githubs_large_hunk_around_its_anchor() {
    let mut c = inline_comment("Only this line is relevant.", false);
    c.line = Some(20);
    c.outdated = true;
    c.diff_hunk = Some(
        std::iter::once("@@ -1,0 +1,40 @@".to_string())
            .chain((1..=40).map(|i| format!("+line{i}")))
            .collect::<Vec<_>>()
            .join("\n"),
    );

    let hunk = c.prompt_hunk().expect("the anchor is in the hunk");
    let lines: Vec<_> = hunk.lines().collect();
    assert_eq!(lines.len(), COMMENT_HUNK_CONTEXT_LINES * 2 + 2);
    assert!(hunk.contains("+line20"));
    assert!(!hunk.contains("+line1\n"));
    assert!(!hunk.contains("+line40"));
}

#[test]
fn combined_fix_prompt_drops_whole_file_hunks() {
    let ordinary = inline_comment("Guard this behind the lock.", false);
    let mut file_level = inline_comment("Split this module up.", false);
    file_level.path = Some("src/big.rs".into());
    file_level.file_level = true;
    file_level.line = None;
    file_level.diff_hunk = Some("@@ -1,400 +1,420 @@\n+ enormous".into());

    let prompt = combined_fix_prompt(&[&ordinary, &file_level]);

    // The line-anchored comment keeps its (small) hunk...
    assert!(prompt.contains("+ self.sync();"));
    // ...while the whole-file hunk never lands in the shared prompt, where
    // several of them would otherwise compound.
    assert!(!prompt.contains("+ enormous"));
    assert!(prompt.contains("File: src/big.rs  (comment on the whole file)"));
}

#[test]
fn fix_prompt_strips_bot_boilerplate() {
    let c = inline_comment("<details>noise</details>Real point.", true);
    let prompt = c.fix_prompt();
    assert!(prompt.contains("Comment (@alice): Real point."));
    assert!(!prompt.contains("<details>"));
}

#[test]
fn combined_fix_prompt_numbers_comments_under_one_preamble() {
    let mut a = inline_comment("Guard this behind the lock.", false);
    a.path = Some("src/a.rs".into());
    a.line = Some(10);
    let mut b = inline_comment("Rename this field.", false);
    b.path = Some("src/b.rs".into());
    b.line = Some(20);

    let prompt = combined_fix_prompt(&[&a, &b]);

    // One shared preamble, not repeated per comment.
    assert!(prompt.starts_with("Address these PR review comments."));
    assert!(!prompt.contains("Address this PR review comment."));
    assert_eq!(prompt.matches("Address these").count(), 1);

    // Each comment appears as a numbered entry with its own file:line + text.
    assert!(prompt.contains("Comment 1:"));
    assert!(prompt.contains("Comment 2:"));
    assert!(prompt.contains("File: src/a.rs:10"));
    assert!(prompt.contains("Guard this behind the lock."));
    assert!(prompt.contains("File: src/b.rs:20"));
    assert!(prompt.contains("Rename this field."));

    // Still no file contents — only the comment text + diff hunks.
    assert!(!prompt.contains("fn "));
}

fn changed_files() -> Vec<InvestigationChangedFile> {
    vec![
        InvestigationChangedFile {
            path: "src/app/sync.rs".into(),
            additions: 12,
            deletions: 3,
        },
        InvestigationChangedFile {
            path: "src/app/mod.rs".into(),
            additions: 1,
            deletions: 0,
        },
    ]
}

#[test]
fn investigation_prompt_carries_only_comment_pr_meta_and_changed_files() {
    let comment = inline_comment("This guard is wrong when x is negative.", false);
    let files = changed_files();
    let ctx = InvestigationPromptContext {
        comment: &comment,
        pr_number: 321,
        pr_title: "Rework the sync poll loop",
        pr_description: "Speeds up status reconciliation.",
        changed_files: &files,
        follow_up: None,
        user_context: None,
    };
    let prompt = build_investigation_prompt(&ctx);

    assert!(prompt.starts_with("Investigate this PR review comment."));
    assert!(prompt.contains("PR #321: Rework the sync poll loop"));
    assert!(prompt.contains("PR description:\nSpeeds up status reconciliation."));
    assert!(prompt.contains("Files changed by this PR"));
    assert!(prompt.contains("  src/app/sync.rs  (+12 -3)"));
    assert!(prompt.contains("  src/app/mod.rs  (+1 -0)"));
    // The comment context comes through the shared minimal block.
    assert!(prompt.contains("--- The review comment ---"));
    assert!(prompt.contains("File: src/app/sync.rs:42"));
    assert!(prompt.contains("Comment (@alice): This guard is wrong when x is negative."));
    assert!(prompt.contains("Diff hunk:"));
    // Strictly read-only, and it must not turn into a fix.
    assert!(prompt.contains("read-only access"));
    assert!(prompt.contains("do not modify anything"));
    assert!(prompt.contains("without making the change"));
    // No file contents, and no follow-up scaffolding on the initial run.
    assert!(!prompt.contains("fn "));
    assert!(!prompt.contains("The investigation so far"));
    assert!(!prompt.contains("follow-up question"));
}

#[test]
fn investigation_prompt_is_byte_identical_when_user_context_is_absent_or_blank() {
    let comment = inline_comment("This guard is wrong when x is negative.", false);
    let files = changed_files();
    let base = InvestigationPromptContext {
        comment: &comment,
        pr_number: 321,
        pr_title: "Rework the sync poll loop",
        pr_description: "Speeds up status reconciliation.",
        changed_files: &files,
        follow_up: None,
        user_context: None,
    };
    let without = build_investigation_prompt(&base);

    // An explicit note that is only whitespace renders exactly nothing —
    // the empty-input path is unchanged from before the feature.
    let blank = InvestigationPromptContext {
        user_context: Some("   \n  "),
        ..base.clone()
    };
    assert_eq!(build_investigation_prompt(&blank), without);
    assert!(!without.contains("What the person triaging suspects"));
}

#[test]
fn investigation_prompt_frames_user_context_as_a_hypothesis_to_verify() {
    let comment = inline_comment("This guard is wrong when x is negative.", false);
    let files = changed_files();
    let ctx = InvestigationPromptContext {
        comment: &comment,
        pr_number: 321,
        pr_title: "Rework the sync poll loop",
        pr_description: "Speeds up status reconciliation.",
        changed_files: &files,
        follow_up: None,
        user_context: Some("  I think src/app/sync.rs already clamps x — double-check.  "),
    };
    let prompt = build_investigation_prompt(&ctx);

    assert!(prompt.contains("--- What the person triaging suspects ---"));
    assert!(prompt.contains("hypothesis to verify, not an established fact"));
    // Trimmed, and placed after the review comment but before the
    // read-only instruction block.
    assert!(prompt.contains("I think src/app/sync.rs already clamps x — double-check."));
    assert!(!prompt.contains("  I think src/app/sync.rs"));
    let note_at = prompt.find("What the person triaging suspects").unwrap();
    assert!(note_at > prompt.find("--- The review comment ---").unwrap());
    assert!(note_at < prompt.find("read-only access").unwrap());
}

#[test]
fn investigation_prompt_handles_missing_title_description_and_files() {
    let comment = inline_comment("nit", false);
    let ctx = InvestigationPromptContext {
        comment: &comment,
        pr_number: 7,
        pr_title: "   ",
        pr_description: "",
        changed_files: &[],
        follow_up: None,
        user_context: None,
    };
    let prompt = build_investigation_prompt(&ctx);
    assert!(prompt.contains("PR #7\n"));
    assert!(!prompt.contains("PR #7:"));
    assert!(prompt.contains("PR description:\n(none)"));
    assert!(!prompt.contains("Files changed by this PR"));
}

#[test]
fn investigation_prompt_truncates_a_large_changed_file_list() {
    let comment = inline_comment("check this", false);
    let files: Vec<InvestigationChangedFile> = (0..50)
        .map(|i| InvestigationChangedFile {
            path: format!("src/f{i}.rs"),
            additions: 1,
            deletions: 1,
        })
        .collect();
    let ctx = InvestigationPromptContext {
        comment: &comment,
        pr_number: 1,
        pr_title: "big",
        pr_description: "x",
        changed_files: &files,
        follow_up: None,
        user_context: None,
    };
    let prompt = build_investigation_prompt(&ctx);
    assert!(prompt.contains("src/f0.rs"));
    assert!(prompt.contains("src/f39.rs"));
    assert!(!prompt.contains("src/f40.rs"));
    assert!(prompt.contains("…and 10 more"));
}

#[test]
fn investigation_follow_up_prompt_includes_prior_turns_and_the_new_question() {
    let comment = inline_comment("Does this handle the empty case?", false);
    let files = changed_files();
    let prior = vec![
        PrInvestigationTurn {
            question: "q1".into(),
            answer: "a1".into(),
            harness: AgentKind::Claude,
            created_at: "t1".into(),
        },
        PrInvestigationTurn {
            question: "q2".into(),
            answer: "a2".into(),
            harness: AgentKind::Claude,
            created_at: "t2".into(),
        },
        PrInvestigationTurn {
            question: "q3".into(),
            answer: "a3".into(),
            harness: AgentKind::Claude,
            created_at: "t3".into(),
        },
        PrInvestigationTurn {
            question: "q4".into(),
            answer: "a4".into(),
            harness: AgentKind::Claude,
            created_at: "t4".into(),
        },
    ];
    let ctx = InvestigationPromptContext {
        comment: &comment,
        pr_number: 9,
        pr_title: "t",
        pr_description: "d",
        changed_files: &files,
        follow_up: Some(InvestigationFollowUp {
            initial_answer: "The empty case is unhandled at sync.rs:44.",
            prior_turns: &prior,
            question: "Would a guard clause be enough?",
        }),
        user_context: None,
    };
    let prompt = build_investigation_prompt(&ctx);

    assert!(prompt.contains("--- The investigation so far ---"));
    assert!(prompt.contains("Initial finding: The empty case is unhandled at sync.rs:44."));
    // Oldest turn (q1) is trimmed; the last three are kept.
    assert!(!prompt.contains("They then asked: q1"));
    assert!(prompt.contains("They then asked: q2"));
    assert!(prompt.contains("They then asked: q4"));
    assert!(prompt.contains("--- Their follow-up question ---"));
    assert!(prompt.contains("Would a guard clause be enough?"));
    assert!(prompt.contains("Answer their follow-up question in Markdown"));
    assert!(prompt.contains("read-only access"));
}

#[test]
fn reply_target_inline_uses_thread_root() {
    // A reply (in_reply_to set) targets the thread root, not its own id.
    let mut leaf = inline_comment("thanks", false);
    leaf.id = 55;
    leaf.in_reply_to = Some(40);
    assert_eq!(
        leaf.reply_target(),
        ReplyTarget::InlineThread {
            root_comment_id: 40
        }
    );

    // A root inline comment (no in_reply_to) replies to itself.
    let root = inline_comment("nit", false);
    assert_eq!(
        root.reply_target(),
        ReplyTarget::InlineThread { root_comment_id: 1 }
    );
}

#[test]
fn reply_target_conversation_and_summary_post_issue_comment() {
    let mut conv = inline_comment("hi", false);
    conv.kind = CommentKind::Conversation;
    assert_eq!(conv.reply_target(), ReplyTarget::Conversation);

    let mut summary = inline_comment("changes", false);
    summary.kind = CommentKind::ReviewSummary {
        state: "CHANGES_REQUESTED".into(),
    };
    assert_eq!(summary.reply_target(), ReplyTarget::Conversation);
}

#[test]
fn replies_in_finds_only_comments_targeting_this_ones_id() {
    let root = sample_comment(1, "alice", false);
    let mut reply = sample_comment(2, "bob", false);
    reply.in_reply_to = Some(1);
    let mut unrelated = sample_comment(3, "carol", false);
    unrelated.in_reply_to = Some(99);
    let all = vec![root.clone(), reply.clone(), unrelated];

    let found = root.replies_in(&all);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, 2);
    assert_eq!(found[0].author, "bob");

    // A comment with no replies finds none.
    assert!(reply.replies_in(&all).is_empty());
}

#[test]
fn reply_posted_via_amf_detects_the_channel_disclosure_footer() {
    let mut reply = sample_comment(2, "amf-user", false);
    reply.body = format!("Done in `abc123`.\n\n{}", AMF_ATTRIBUTION_FOOTER);
    assert!(reply_posted_via_amf(&reply));

    reply.body = format!("Fixed the guard.\n\n{}", AI_ATTRIBUTION_FOOTER);
    assert!(reply_posted_via_amf(&reply));

    // A reply posted through some other channel (a headless agent using
    // `gh` directly, a human on GitHub) has no such footer.
    reply.body = "Done in `abc123`.".to_string();
    assert!(!reply_posted_via_amf(&reply));
}

#[test]
fn normalize_classifies_exact_amf_footers_without_hiding_human_feedback() {
    let comments = vec![
        ReviewComment {
            id: 1,
            path: Some("src/lib.rs".into()),
            line: Some(10),
            original_line: Some(10),
            side: Some("RIGHT".into()),
            diff_hunk: Some("@@".into()),
            subject_type: None,
            body: "Please guard this write.".into(),
            user: user("reviewer", "User"),
            in_reply_to_id: None,
            pull_request_review_id: Some(90),
        },
        ReviewComment {
            id: 2,
            path: Some("src/lib.rs".into()),
            line: Some(10),
            original_line: Some(10),
            side: Some("RIGHT".into()),
            diff_hunk: Some("@@".into()),
            subject_type: None,
            body: format!("Done in `abc123`.\n\n{AMF_ATTRIBUTION_FOOTER}"),
            user: user("author", "User"),
            in_reply_to_id: Some(1),
            pull_request_review_id: Some(91),
        },
        ReviewComment {
            id: 3,
            path: Some("src/lib.rs".into()),
            line: Some(10),
            original_line: Some(10),
            side: Some("RIGHT".into()),
            diff_hunk: Some("@@".into()),
            subject_type: None,
            body: "I saw the posted via AMF note; this still needs a test.".into(),
            user: user("reviewer", "User"),
            in_reply_to_id: Some(1),
            pull_request_review_id: Some(90),
        },
        ReviewComment {
            id: 4,
            path: Some("src/other.rs".into()),
            line: Some(4),
            original_line: Some(4),
            side: Some("RIGHT".into()),
            diff_hunk: Some("@@".into()),
            subject_type: None,
            body: format!("AI finding.\n\n{AI_REVIEW_ATTRIBUTION_FOOTER}"),
            user: user("author", "User"),
            in_reply_to_id: None,
            pull_request_review_id: Some(92),
        },
    ];

    // This is the same normalize pass a manual refresh runs after an `R`
    // reply has appeared on GitHub.
    let review = normalize(pr(), comments, vec![], vec![], vec![]);
    assert!(review.comments[1].is_amf_authored());
    assert!(review.is_collated_amf_reply(&review.comments[1]));
    assert!(review.comments[3].is_amf_authored());
    assert!(!review.is_collated_amf_reply(&review.comments[3]));
    assert!(review.comments[3].is_actionable());

    // Merely mentioning AMF is not an attribution marker, so the unrelated
    // human reply remains incoming, actionable work.
    assert!(!review.comments[2].is_amf_authored());
    assert!(review.comments[2].is_actionable());
    assert_eq!(review.open_count(), 3);
}

#[test]
fn orphaned_amf_reply_is_not_collated_away() {
    let mut orphan = sample_comment(2, "author", false);
    orphan.in_reply_to = Some(999);
    orphan.body = format!("Done.\n\n{AI_ATTRIBUTION_FOOTER}");
    let review = PrReview {
        pr: pr(),
        comments: vec![orphan.clone()],
        fetched_at: Local::now(),
    };

    assert!(orphan.is_amf_authored());
    assert!(!orphan.is_actionable());
    assert!(!review.is_collated_amf_reply(&orphan));
    assert_eq!(review.open_count(), 0);
}

#[test]
fn fix_prompt_marks_outdated_and_omits_missing_pieces() {
    let mut c = inline_comment("Still relevant?", false);
    c.outdated = true;
    c.diff_hunk = None;
    let prompt = c.fix_prompt();
    assert!(prompt.contains("(comment is on a line that has since changed)"));
    assert!(!prompt.contains("Diff hunk:"));

    // Conversation/summary comments have no path: no File line at all.
    c.path = None;
    c.line = None;
    c.outdated = false;
    let prompt = c.fix_prompt();
    assert!(!prompt.contains("File:"));
    assert!(prompt.contains("Comment (@alice): Still relevant?"));
}

#[test]
fn estimate_tokens_rounds_up() {
    assert_eq!(estimate_tokens(""), 0);
    assert_eq!(estimate_tokens("abc"), 1);
    assert_eq!(estimate_tokens("abcd"), 1);
    assert_eq!(estimate_tokens("abcde"), 2);
}

#[test]
fn triage_state_db_str_roundtrips() {
    for state in [
        TriageState::Untriaged,
        TriageState::Fixing,
        TriageState::Done,
        TriageState::Skipped,
        TriageState::Replied,
    ] {
        assert_eq!(TriageState::from_db_str(state.as_db_str()), state);
    }
    // Unknown tokens degrade to untriaged rather than erroring.
    assert_eq!(TriageState::from_db_str("garbage"), TriageState::Untriaged);
}

#[test]
fn triage_state_labels_and_markers() {
    assert_eq!(TriageState::Untriaged.label(), None);
    assert_eq!(TriageState::Untriaged.marker(), ' ');
    assert_eq!(TriageState::Done.label(), Some("done"));
    assert_eq!(TriageState::Done.marker(), 'x');
    assert_eq!(TriageState::Skipped.marker(), '-');
    assert_eq!(TriageState::Fixing.marker(), '~');
}

#[test]
fn upsert_investigation_in_memory_replaces_by_comment_id() {
    use crate::db::pr_investigations::PrInvestigation;
    let mk = |comment_id: u64, answer: Option<&str>| {
        let mut row =
            PrInvestigation::new_running("p", 1, comment_id, "sha", AgentKind::Claude, "");
        row.answer = answer.map(str::to_string);
        row
    };
    let mut list = vec![mk(10, None), mk(11, None)];
    // Same comment id updates in place, not appends.
    upsert_investigation_in_memory(&mut list, mk(10, Some("done")));
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].comment_id, 10);
    assert_eq!(list[0].answer.as_deref(), Some("done"));
    // A new comment id appends.
    upsert_investigation_in_memory(&mut list, mk(12, None));
    assert_eq!(list.len(), 3);
    assert_eq!(list[2].comment_id, 12);
}

#[test]
fn investigation_failure_message_is_one_line_and_names_the_harness() {
    let err = anyhow::anyhow!("claude exited 1\nstderr noise\nmore noise");
    let msg = investigation_failure_message(&AgentKind::Claude, &err);
    assert!(!msg.contains('\n'));
    assert!(msg.contains("Claude"));
    assert!(msg.contains("claude exited 1"));
    assert!(msg.contains("pick another harness"));
}

#[test]
fn fix_target_defaults_to_dedicated_and_has_tags() {
    assert_eq!(FixTarget::default(), FixTarget::DedicatedReview);
    assert_eq!(FixTarget::DedicatedReview.tag(), "dedicated");
    assert_eq!(FixTarget::ExistingLive.tag(), "live");
}

#[test]
fn fix_target_pick_row_labels_existing_live_and_dedicated() {
    assert_eq!(
        FixTargetPickRow::ExistingLive(None).label(),
        "Existing live session"
    );
    assert_eq!(
        FixTargetPickRow::ExistingLive(Some("Claude 2".to_string())).label(),
        "Existing live session (Claude 2)"
    );
    assert_eq!(
        FixTargetPickRow::Dedicated(AgentKind::Claude).label(),
        "Dedicated triage session (Claude)"
    );
}

#[test]
fn reply_kind_menu_labels_are_distinct() {
    assert_eq!(ReplyKind::ALL.len(), 2);
    assert_eq!(
        ReplyKind::Done.menu_label(),
        "Done — report a completed fix"
    );
    assert_eq!(
        ReplyKind::NotNeeded.menu_label(),
        "Not needed — explain why"
    );
}

#[test]
fn mark_action_menu_label_reflects_current_state() {
    let mut comment = sample_comment(1, "alice", false);
    comment.triage = TriageState::Untriaged;
    comment.is_resolved = false;

    assert_eq!(MarkAction::Done.menu_label(Some(&comment)), "Done (local)");
    assert_eq!(MarkAction::Skip.menu_label(Some(&comment)), "Skip (local)");
    assert_eq!(
        MarkAction::ResolveOnGitHub.menu_label(Some(&comment)),
        "Resolve thread on GitHub"
    );

    comment.triage = TriageState::Done;
    assert_eq!(
        MarkAction::Done.menu_label(Some(&comment)),
        "Done (local) — press to clear"
    );

    comment.triage = TriageState::Skipped;
    assert_eq!(
        MarkAction::Skip.menu_label(Some(&comment)),
        "Skip (local) — press to clear"
    );

    comment.is_resolved = true;
    assert_eq!(
        MarkAction::ResolveOnGitHub.menu_label(Some(&comment)),
        "Reopen thread on GitHub (currently resolved)"
    );

    // No selection: falls back to the untoggled label rather than panicking.
    assert_eq!(MarkAction::Done.menu_label(None), "Done (local)");
}

#[test]
fn fix_session_index_prefers_dedicated_else_creates() {
    use crate::project::{AgentKind, Feature, SessionKind, VibeMode};
    let mut feature = Feature::new(
        "feat".into(),
        "branch".into(),
        std::path::PathBuf::from("/tmp/wd"),
        false,
        VibeMode::Vibeless,
        false,
        false,
        AgentKind::Claude,
        false,
        false,
    );

    // Nothing running yet: both strategies report "must create / nothing to
    // reuse".
    assert_eq!(
        pr_triage_session_index(&feature, FixTarget::DedicatedReview),
        None
    );
    assert_eq!(
        pr_triage_session_index(&feature, FixTarget::ExistingLive),
        None
    );

    // A regular live agent session satisfies existing-live but not dedicated.
    feature.add_session_named(SessionKind::Claude, "Claude".into());
    assert_eq!(
        pr_triage_session_index(&feature, FixTarget::ExistingLive),
        Some(0)
    );
    assert_eq!(
        pr_triage_session_index(&feature, FixTarget::DedicatedReview),
        None
    );

    // An already-running session created before the rename is still reused.
    feature.add_session_named(SessionKind::Claude, LEGACY_REVIEW_SESSION_LABEL.into());
    assert_eq!(
        pr_triage_session_index(&feature, FixTarget::DedicatedReview),
        Some(1)
    );

    // The current label wins when both exist, while existing-live still
    // resolves to the first agent session.
    feature.add_session_named(SessionKind::Claude, TRIAGE_SESSION_LABEL.into());
    assert_eq!(
        pr_triage_session_index(&feature, FixTarget::DedicatedReview),
        Some(2)
    );
    assert_eq!(
        pr_triage_session_index(&feature, FixTarget::ExistingLive),
        Some(0)
    );
}

#[test]
fn named_triage_sessions_resolve_independently() {
    use crate::project::{AgentKind, Feature, SessionKind, VibeMode};
    let mut feature = Feature::new(
        "feature".into(),
        "feature".into(),
        std::path::PathBuf::from("/tmp/feature"),
        false,
        VibeMode::default(),
        false,
        false,
        AgentKind::default(),
        false,
        false,
    );
    feature.add_session_named(SessionKind::Claude, TRIAGE_SESSION_LABEL.into());
    feature.add_session_named(SessionKind::Codex, "PR 321 security".into());
    feature.add_session_named(SessionKind::Claude, "PR 654 docs".into());

    assert_eq!(
        pr_triage_session_index_named(&feature, FixTarget::DedicatedReview, "PR 321 security"),
        Some(1)
    );
    assert_eq!(
        pr_triage_session_index_named(&feature, FixTarget::DedicatedReview, "PR 654 docs"),
        Some(2)
    );
    assert_eq!(
        pr_triage_session_index_named(&feature, FixTarget::DedicatedReview, "missing"),
        None
    );
}

#[test]
fn named_triage_session_rejects_a_different_selected_harness() {
    use crate::project::{AgentKind, Feature, SessionKind, VibeMode};
    let mut feature = Feature::new(
        "feature".into(),
        "feature".into(),
        std::path::PathBuf::from("/tmp/feature"),
        false,
        VibeMode::default(),
        false,
        false,
        AgentKind::default(),
        false,
        false,
    );
    feature.add_session_named(SessionKind::Claude, "PR 321 security".into());

    assert_eq!(
        pr_triage_session_index_named_for_harness(
            &feature,
            FixTarget::DedicatedReview,
            "PR 321 security",
            Some(&AgentKind::Claude),
        )
        .unwrap(),
        Some(0)
    );
    let error = pr_triage_session_index_named_for_harness(
        &feature,
        FixTarget::DedicatedReview,
        "PR 321 security",
        Some(&AgentKind::Codex),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("already runs Claude"), "{error}");
    assert!(error.contains("another session name"), "{error}");
}

#[test]
fn fix_session_index_ignores_non_agent_sessions() {
    use crate::project::{AgentKind, Feature, SessionKind, VibeMode};
    let mut feature = Feature::new(
        "feat".into(),
        "branch".into(),
        std::path::PathBuf::from("/tmp/wd"),
        false,
        VibeMode::Vibeless,
        false,
        false,
        AgentKind::Claude,
        false,
        false,
    );
    // A terminal window is not an agent harness, so it is never a fix target.
    feature.add_session_named(SessionKind::Terminal, "Terminal".into());
    assert_eq!(
        pr_triage_session_index(&feature, FixTarget::ExistingLive),
        None
    );
    assert_eq!(
        pr_triage_session_index(&feature, FixTarget::DedicatedReview),
        None
    );
}

#[test]
fn agent_text_strips_only_for_bots() {
    let human = PrComment {
        id: 1,
        kind: CommentKind::Inline,
        author: "alice".into(),
        is_bot: false,
        path: None,
        line: None,
        side: None,
        outdated: false,
        file_level: false,
        github_id: None,
        github_review_id: None,
        diff_hunk: None,
        body: "<details>keep?</details>plain".into(),
        snippet: String::new(),
        in_reply_to: None,
        thread_id: None,
        is_resolved: false,
        triage: TriageState::Untriaged,
        local_note: None,
        batch_id: None,
    };
    let mut bot = human.clone();
    bot.is_bot = true;
    assert!(human.agent_text().contains("<details>"));
    assert_eq!(bot.agent_text(), "plain");
}

fn review_comment(id: u64, path: Option<&str>, line: Option<u32>, body: &str) -> ReviewComment {
    bot_review_comment(id, path, line, body, false)
}

fn bot_review_comment(
    id: u64,
    path: Option<&str>,
    line: Option<u32>,
    body: &str,
    is_bot: bool,
) -> ReviewComment {
    ReviewComment {
        id,
        path: path.map(String::from),
        line,
        original_line: line,
        side: Some("RIGHT".into()),
        diff_hunk: None,
        subject_type: None,
        body: body.to_string(),
        user: if is_bot {
            user("coderabbitai", "Bot")
        } else {
            user("alice", "User")
        },
        in_reply_to_id: None,
        pull_request_review_id: None,
    }
}

fn review(id: u64, body: &str, is_bot: bool) -> Review {
    Review {
        id,
        body: body.to_string(),
        state: "COMMENTED".into(),
        user: if is_bot {
            user("coderabbitai", "Bot")
        } else {
            user("alice", "User")
        },
    }
}

#[test]
fn bootstrap_depth_default_is_fifty() {
    assert_eq!(BootstrapDepth::default(), BootstrapDepth::Fifty);
    assert_eq!(BootstrapDepth::Fifty.limit(), 50);
    assert_eq!(BootstrapDepth::Twenty.limit(), 20);
    assert_eq!(BootstrapDepth::Hundred.limit(), 100);
    assert!(BootstrapDepth::All.limit() > 100);
}

#[test]
fn bootstrap_pr_text_includes_location_and_review_lines() {
    let comments = vec![review_comment(
        1,
        Some("src/app/sync.rs"),
        Some(42),
        "Guard this behind the lock.",
    )];
    let reviews = vec![review(2, "Looks solid overall.", false)];
    let text = bootstrap_pr_text(&comments, &reviews);
    assert_eq!(
        text,
        "- (src/app/sync.rs:42) Guard this behind the lock.\n- (review) Looks solid overall."
    );
}

#[test]
fn bootstrap_pr_text_strips_bot_boilerplate_and_skips_empty() {
    let comments = vec![
        bot_review_comment(
            1,
            Some("a.rs"),
            None,
            "<details><summary>Prompt for AI agents</summary>noise</details>Real point.",
            true,
        ),
        review_comment(2, None, None, "   "),
    ];
    let text = bootstrap_pr_text(&comments, &[]);
    assert_eq!(text, "- (a.rs) Real point.");
}

#[test]
fn bootstrap_prompt_lists_every_pr_and_instructs_category_format() {
    let bodies = vec![
        (
            1,
            "Fix race".to_string(),
            "- (a.rs:1) Guard the lock".to_string(),
        ),
        (
            2,
            "Add tests".to_string(),
            "- (review) Needs tests".to_string(),
        ),
    ];
    let prompt = bootstrap_prompt(&bodies);
    assert!(prompt.contains("## Category"));
    assert!(prompt.contains("### PR #1: Fix race"));
    assert!(prompt.contains("- (a.rs:1) Guard the lock"));
    assert!(prompt.contains("### PR #2: Add tests"));
    assert!(prompt.contains("- (review) Needs tests"));
}

#[test]
fn append_amf_attribution_uses_a_distinct_channel_disclosure() {
    assert_eq!(
        append_amf_attribution("Done in `abc123`."),
        "Done in `abc123`.\n\n— posted via AMF"
    );
    assert_eq!(
        append_amf_attribution("Trailing newline.\n\n"),
        "Trailing newline.\n\n— posted via AMF"
    );
}

#[test]
fn agent_drafted_replies_use_ai_attribution() {
    let metadata = ReplyGenerationMetadata {
        harness: Some("Codex".to_string()),
        model: Some("gpt-5.5".to_string()),
        estimated_tokens: Some(1_500),
        estimated_cost: Some("$0.04".to_string()),
        combined_batch: None,
    };
    assert_eq!(
        append_reply_attribution("Fixed the guard.", true, Some(&metadata)),
        "Fixed the guard.\n\n_AI generation: harness Codex · model gpt-5.5 · estimated tokens ~1.5k · Fix cost (est.): $0.04_\n\n— drafted by AI via AMF"
    );
    assert_eq!(
        append_reply_attribution("Done in `abc123`.", false, Some(&metadata)),
        "Done in `abc123`.\n\n— posted via AMF"
    );
}

#[test]
fn agent_drafted_reply_in_a_combined_batch_marks_the_shared_cost() {
    let metadata = ReplyGenerationMetadata {
        harness: Some("Codex".to_string()),
        model: Some("gpt-5.5".to_string()),
        estimated_tokens: Some(1_500),
        estimated_cost: Some("$0.04".to_string()),
        combined_batch: Some(crate::app::fix_cost::CombinedBatch { sibling_count: 3 }),
    };
    assert_eq!(
        append_reply_attribution("Fixed the guard.", true, Some(&metadata)),
        "Fixed the guard.\n\n_AI generation: harness Codex · model gpt-5.5 · estimated tokens ~1.5k · Fix cost (est.): $0.04 · combined (3)_\n_Fixed as one of 3 comments handled in a single combined agent run; the fix cost above is that run's total, shared across them._\n\n— drafted by AI via AMF"
    );
}

fn reply_state(agent_drafted: bool, seed: &str, current: &str) -> ReplyState {
    ReplyState {
        comment_id: 1,
        kind: ReplyKind::Done,
        editor: TextEditor::new(current.to_string()),
        agent_drafted,
        generation_metadata: None,
        original_seed: seed.to_string(),
        editing: false,
    }
}

#[test]
fn reply_effective_agent_drafted_holds_for_an_unedited_draft() {
    let reply = reply_state(true, "Fixed the guard.", "Fixed the guard.");
    assert!(reply_effective_agent_drafted(&reply));
}

#[test]
fn reply_effective_agent_drafted_drops_once_the_user_edits_the_draft() {
    // The user rewrote the seeded draft in their own words — no longer
    // purely the agent's, so AI attribution no longer applies.
    let reply = reply_state(true, "Fixed the guard.", "Not needed, already handled.");
    assert!(!reply_effective_agent_drafted(&reply));
}

#[test]
fn reply_effective_agent_drafted_false_without_a_draft() {
    let reply = reply_state(false, "", "Done.");
    assert!(!reply_effective_agent_drafted(&reply));
}
