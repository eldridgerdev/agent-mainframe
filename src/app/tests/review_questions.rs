use crate::app::review_questions::DraftDestination;
use crate::app::review_questions::test_support as worker;
use crate::app::review_questions::test_support::{
    Completion, Job, RunInput, Task, git, prepare, review_context, validate_anchor,
};
use crate::app::{App, AppMode};
use crate::app::{DiffScope, DiffViewerState, PrDiffTarget, ViewState};
use crate::editor::TextEditor;
use crate::project::{AgentKind, ProjectStore, SessionKind, VibeMode};
use crate::prompts::PromptId;
use crate::traits::{MockTmuxOps, MockWorktreeOps};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::Path;
use std::sync::{Arc, atomic::AtomicBool, mpsc};
use std::time::{Duration, Instant};
use tempfile::TempDir;

struct Fixture {
    dir: TempDir,
    base: String,
    head: String,
}
impl Fixture {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        git(dir.path(), &["init", "-b", "main"]).unwrap();
        std::fs::write(dir.path().join("caller.rs"), "fn caller() -> u32 { 1 }\n").unwrap();
        std::fs::write(
            dir.path().join("helper.rs"),
            "pub fn reusable() -> u32 { 1 }\n",
        )
        .unwrap();
        commit(dir.path());
        let base = git(dir.path(), &["rev-parse", "HEAD"]).unwrap();
        std::fs::write(
            dir.path().join("caller.rs"),
            "fn caller() -> u32 { 1 + 0 }\n",
        )
        .unwrap();
        commit(dir.path());
        let head = git(dir.path(), &["rev-parse", "HEAD"]).unwrap();
        Self { dir, base, head }
    }
    fn app(&self) -> App {
        let mut worktree = MockWorktreeOps::new();
        worktree
            .expect_repo_root()
            .returning(|path| Ok(path.to_path_buf()));
        let mut app = App::new_for_test(
            ProjectStore::empty(),
            Box::new(MockTmuxOps::new()),
            Box::new(worktree),
        );
        let mut viewer = DiffViewerState::new(
            ViewState::new(
                "project".into(),
                "feature".into(),
                "session".into(),
                "claude".into(),
                "Claude".into(),
                SessionKind::Claude,
                VibeMode::Vibe,
                false,
            ),
            self.dir.path().to_path_buf(),
        );
        let snapshot =
            crate::diff::load_snapshot(self.dir.path(), Some(&self.base), false).unwrap();
        viewer.review = true;
        viewer.base_commit = snapshot.base_commit;
        viewer.files = snapshot.files;
        viewer.comment_cursor = Some(1);
        viewer.patch_scroll = 7;
        app.mode = AppMode::DiffViewer(viewer);
        app.review_question_work.runner = inspect_helper;
        app.review_question_work.ai_diff = local_ai_diff;
        app
    }
    fn pr(&self) -> crate::github::PrRef {
        crate::github::PrRef {
            number: 1,
            head_sha: self.head.clone(),
            url: "https://github.com/o/r/pull/1".into(),
            owner: "o".into(),
            repo: "r".into(),
            head_ref: "main".into(),
        }
    }
    fn manual(&self, app: &mut App) {
        let AppMode::DiffViewer(s) = &mut app.mode else {
            panic!("review");
        };
        let pr = serde_json::from_value::<crate::github::ReviewablePr>(serde_json::json!({
            "number": 1, "baseRefName": "main", "headRefName": "main", "headRefOid": self.head,
        }))
        .unwrap();
        s.scope = DiffScope::PullRequest(Box::new(PrDiffTarget {
            repo: "github.com/o/r".into(),
            pr,
            merge_base_oid: self.base.clone(),
        }));
    }
    fn ai(&self, app: &mut App) {
        app.open_ai_review_for_pr(self.dir.path().to_path_buf(), self.pr());
        if let AppMode::AiReview(s) = &mut app.mode {
            s.findings.push(crate::app::ai_review::AiReviewFinding {
                path: Some("caller.rs".into()),
                line: Some(1),
                side: Some(crate::diff::DiffSide::New),
                body: "Could this reuse a helper?".into(),
                diff_hunk: None,
                skipped: false,
                published: false,
            });
        }
    }
}
fn commit(path: &Path) {
    git(path, &["add", "."]).unwrap();
    git(
        path,
        &[
            "-c",
            "user.name=AMF",
            "-c",
            "user.email=amf@example.com",
            "commit",
            "-m",
            "fixture",
        ],
    )
    .unwrap();
}
fn local_ai_diff(
    path: &Path,
    _: &crate::github::PrRef,
) -> anyhow::Result<Vec<crate::diff::DiffFile>> {
    Ok(crate::diff::load_commit_snapshot(path, "HEAD", false)?.files)
}
fn inspect_helper(input: &RunInput) -> anyhow::Result<String> {
    assert!(input.discovery);
    assert!(input.prompt.contains("caller.rs"));
    assert!(input.prompt.contains("beyond the selected"));
    assert!(
        input
            .prompt
            .contains(&input.context.workdir.display().to_string())
    );
    assert!(!input.prompt.contains("{{"));
    let helper = std::fs::read_to_string(input.context.workdir.join("helper.rs"))?;
    assert!(helper.contains("fn reusable"));
    Ok("Use `helper.rs:1`, `reusable()`, instead of the expression in `caller.rs:1` (new side). I inspected the unchanged helper.".into())
}
fn draft(input: &RunInput) -> anyhow::Result<String> {
    assert!(!input.discovery);
    assert!(input.prompt.contains("helper.rs:1"));
    assert!(!input.prompt.contains("{{"));
    Ok("Could we call reusable() from helper.rs here?".into())
}
fn drain(app: &mut App) {
    let until = Instant::now() + Duration::from_secs(5);
    while app.review_question_work.job.is_some() && Instant::now() < until {
        app.poll_review_questions();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        app.review_question_work.job.is_none(),
        "worker did not finish"
    );
}
fn ask(app: &mut App) {
    app.open_review_questions();
    app.review_questions_mut().unwrap().editor =
        TextEditor::new("Can we reuse an existing helper?".into());
    app.submit_review_question();
    drain(app);
}

#[test]
fn unchanged_helper_discovery_on_all_surfaces_and_harnesses_preserves_review_position() {
    let fixture = Fixture::new();
    for surface in 0..3 {
        for harness in [
            AgentKind::Claude,
            AgentKind::Codex,
            AgentKind::Opencode,
            AgentKind::Pi,
        ] {
            let mut app = fixture.app();
            if surface == 1 {
                fixture.manual(&mut app);
            }
            if surface == 2 {
                fixture.ai(&mut app);
            }
            app.open_review_questions();
            app.review_questions_mut().unwrap().harness = harness;
            ask(&mut app);
            let q = app.review_questions().unwrap();
            assert!(q.error.is_none(), "{:?}", q.error);
            assert!(
                q.turns[0]
                    .answer
                    .as_deref()
                    .unwrap()
                    .contains("helper.rs:1")
            );
            assert!(q.turns[0].context.anchor.is_some());
            app.close_review_questions();
            if let AppMode::DiffViewer(s) = &app.mode {
                assert_eq!(s.comment_cursor, Some(1));
                assert_eq!(s.patch_scroll, 7);
            }
            if let AppMode::AiReview(s) = &app.mode {
                assert_eq!(s.selected, 0);
                assert_eq!(s.findings.len(), 1);
            }
            assert_eq!(app.review_questions().unwrap().turns.len(), 1);
            app.open_review_questions();
            assert_eq!(app.review_questions().unwrap().turns.len(), 1);
        }
    }
}

#[test]
fn question_without_line_selection_can_search_the_repository() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    if let AppMode::DiffViewer(s) = &mut app.mode {
        s.comment_cursor = None;
    }
    ask(&mut app);
    assert!(app.review_questions().unwrap().turns[0].answer.is_some());
    assert!(
        app.review_questions().unwrap().turns[0]
            .context
            .anchor
            .is_none()
    );
}

#[test]
fn mismatch_dirty_checkout_and_changed_worktree_retain_question_and_review() {
    for case in 0..3 {
        let fixture = Fixture::new();
        let mut app = fixture.app();
        if case < 2 {
            fixture.manual(&mut app);
        }
        if case == 0 {
            git(fixture.dir.path(), &["checkout", "--detach", &fixture.base]).unwrap();
        } else {
            std::fs::write(fixture.dir.path().join("helper.rs"), "changed\n").unwrap();
        }
        ask(&mut app);
        let q = app.review_questions().unwrap();
        assert!(q.error.is_some());
        assert_eq!(q.editor.text(), "Can we reuse an existing helper?");
        assert!(q.turns[0].answer.is_none());
        assert!(matches!(app.mode, AppMode::DiffViewer(_)));
    }
}

#[test]
fn follow_ups_receive_same_context_history_but_closing_review_clears_it() {
    fn follow_up(input: &RunInput) -> anyhow::Result<String> {
        assert!(input.prompt.contains("Earlier conversation"));
        assert!(input.prompt.contains("I inspected the unchanged helper"));
        Ok("The helper has the same return type.".into())
    }
    let fixture = Fixture::new();
    let mut app = fixture.app();
    ask(&mut app);
    app.review_question_work.runner = follow_up;
    app.review_questions_mut().unwrap().editor =
        TextEditor::new("Is the return type compatible?".into());
    app.submit_review_question();
    drain(&mut app);
    assert_eq!(app.review_questions().unwrap().turns.len(), 2);
    let owner = app.review_questions().unwrap().owner.clone();
    app.mode = AppMode::Normal;
    app.poll_review_questions();
    let reopened = fixture.app();
    assert!(reopened.review_questions().unwrap().turns.is_empty());
    assert_ne!(reopened.review_questions().unwrap().owner, owner);
}

fn inject(app: &mut App) -> mpsc::Sender<Completion> {
    let context = review_context(app).unwrap();
    let q = app.review_questions_mut().unwrap();
    q.next_request += 1;
    q.request = Some(q.next_request);
    let (sender, receiver) = mpsc::channel();
    app.review_question_work.job = Some(Job {
        owner: q.owner.clone(),
        request: q.next_request,
        turn: q.selected,
        context,
        task: Task::Answer {
            question: "question".into(),
            earlier: String::new(),
        },
        started: Instant::now(),
        cancelled: Arc::new(AtomicBool::new(false)),
        receiver,
    });
    sender
}

#[test]
fn cancellation_retry_switching_and_obsolete_completion_are_isolated() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    ask(&mut app);
    let sender = inject(&mut app);
    let job = app.review_question_work.job.as_ref().unwrap();
    let token = job.cancelled.clone();
    let old = (job.owner.clone(), job.request, job.context.version.clone());
    app.cancel_review_question();
    assert!(token.load(std::sync::atomic::Ordering::Relaxed));
    assert!(
        sender
            .send(Completion {
                owner: old.0,
                request: old.1,
                version: old.2,
                result: Err("late".into())
            })
            .is_err()
    );
    app.retry_review_question();
    drain(&mut app);
    assert_eq!(app.review_questions().unwrap().turns.len(), 2);
    assert!(app.review_questions().unwrap().turns[1].answer.is_some());
    let sender = inject(&mut app);
    app.mode = fixture.app().mode;
    assert!(!app.poll_review_questions());
    assert!(
        sender
            .send(Completion {
                owner: "old".into(),
                request: 1,
                version: "old".into(),
                result: Err("old".into())
            })
            .is_err()
    );
    assert!(app.review_questions().unwrap().turns.is_empty());
}

#[test]
fn empty_failure_and_disconnection_are_recoverable() {
    fn empty(_: &RunInput) -> anyhow::Result<String> {
        Ok(String::new())
    }
    fn failed(_: &RunInput) -> anyhow::Result<String> {
        anyhow::bail!("mock harness failed")
    }
    let fixture = Fixture::new();
    for runner in [empty as worker::Runner, failed as worker::Runner] {
        let mut app = fixture.app();
        app.review_question_work.runner = runner;
        ask(&mut app);
        assert!(app.review_questions().unwrap().error.is_some());
        assert!(!app.review_questions().unwrap().editor.text().is_empty());
        app.review_question_work.runner = inspect_helper;
        app.retry_review_question();
        drain(&mut app);
        assert!(
            app.review_questions()
                .unwrap()
                .turns
                .last()
                .unwrap()
                .answer
                .is_some()
        );
    }
    let mut app = fixture.app();
    drop(inject(&mut app));
    assert!(app.poll_review_questions());
    assert!(
        app.review_questions()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .contains("worker stopped")
    );
}

#[test]
fn inline_and_general_drafts_are_editable_and_preserve_existing_drafts() {
    for surface in 0..3 {
        for destination in [DraftDestination::Inline, DraftDestination::General] {
            let fixture = Fixture::new();
            let mut app = fixture.app();
            if surface == 1 {
                fixture.manual(&mut app);
            }
            if surface == 2 {
                fixture.ai(&mut app);
            }
            if let AppMode::DiffViewer(s) = &mut app.mode {
                s.general_feedback = "Existing feedback".into();
            }
            ask(&mut app);
            app.review_question_work.runner = draft;
            app.draft_review_question(destination);
            drain(&mut app);
            let q = app.review_questions_mut().unwrap();
            assert!(q.draft.is_some(), "{:?}", q.error);
            q.draft.as_mut().unwrap().1 = TextEditor::new("Edited comment".into());
            app.transfer_review_question_draft();
            drain(&mut app);
            match &app.mode {
                AppMode::DiffViewer(s) => {
                    assert!(!s.questions.open);
                    assert!(s.feedback_editor.text().contains("Edited comment"));
                    if destination == DraftDestination::General {
                        assert!(s.feedback_editor.text().contains("Existing feedback"));
                        assert!(s.editing_general);
                    } else {
                        assert!(s.editing_line_comment);
                        assert!(s.line_comments.is_empty());
                    }
                }
                AppMode::AiReview(s) => {
                    assert_eq!(s.findings.len(), 2);
                    assert!(!s.findings[1].published);
                    assert_eq!(s.finding_editor.as_ref().unwrap().text(), "Edited comment");
                    assert_eq!(
                        s.findings[1].path.is_some(),
                        destination == DraftDestination::Inline
                    );
                    assert_eq!(
                        s.findings[1].diff_hunk.is_some(),
                        destination == DraftDestination::Inline
                    );
                    assert!(s.post_confirm.is_none());
                }
                _ => panic!("review was lost"),
            }
            if surface == 2 {
                app.ai_review_stop_edit_finding();
                app.ai_review_open_post_confirm();
                let AppMode::AiReview(s) = &app.mode else {
                    panic!("AI review was lost");
                };
                let post = s.post_confirm.as_ref().unwrap();
                if destination == DraftDestination::Inline {
                    assert_eq!(post.inline.len(), 1);
                    assert_eq!(post.inline[0].path, "caller.rs");
                    assert_eq!(post.inline[0].line, 1);
                    assert_eq!(post.inline[0].side, "RIGHT");
                    assert!(post.inline[0].body.contains("Edited comment"));
                } else {
                    assert!(post.inline.is_empty());
                    assert!(post.editor.text().contains("Edited comment"));
                }
                assert!(s.findings.iter().all(|f| !f.published));
            }
        }
    }
}

#[test]
fn deleted_lines_use_old_side_and_repository_references_are_not_comment_anchors() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    if let AppMode::DiffViewer(s) = &mut app.mode {
        s.comment_cursor = Some(0);
    }
    let context = review_context(&app).unwrap();
    let prepared = prepare(&context, local_ai_diff).unwrap();
    let mut anchor = context.anchor.unwrap();
    assert_eq!(anchor.side, crate::diff::DiffSide::Old);
    validate_anchor(&anchor, &prepared.files).unwrap();
    anchor.path = "helper.rs".into();
    assert!(validate_anchor(&anchor, &prepared.files).is_err());
    let files = crate::diff::parse_unified_diff(
        "diff --git a/caller.rs b/caller.rs\n--- a/caller.rs\n+++ b/caller.rs\n@@ -1 +1 @@\n-old\n+new\n@@ -20 +20 @@\n-old\n+new\n",
    )
    .unwrap();
    anchor.path = "caller.rs".into();
    anchor.side = crate::diff::DiffSide::New;
    anchor.start = files[0].resolve_source_line(anchor.side, 1).unwrap();
    anchor.end = files[0].resolve_source_line(anchor.side, 20).unwrap();
    assert!(
        validate_anchor(&anchor, &files)
            .unwrap_err()
            .to_string()
            .contains("separate diff hunks")
    );
    anchor.end = anchor.start;
    validate_anchor(&anchor, &files).unwrap();
}

#[test]
fn ai_inline_drafts_keep_context_and_deleted_line_sides_in_the_posting_payload() {
    use crate::diff::DiffSide;
    for side in [DiffSide::New, DiffSide::Old] {
        let mut fixture = Fixture::new();
        std::fs::write(
            fixture.dir.path().join("caller.rs"),
            "fn caller() -> u32 { 1 }\nfn another() -> u32 { 2 }\n",
        )
        .unwrap();
        commit(fixture.dir.path());
        fixture.base = git(fixture.dir.path(), &["rev-parse", "HEAD"]).unwrap();
        let reviewed = if side == DiffSide::New {
            "fn caller() -> u32 { 1 }\nfn another() -> u32 { 2 + 0 }\n"
        } else {
            "fn caller() -> u32 { 1 }\n"
        };
        std::fs::write(fixture.dir.path().join("caller.rs"), reviewed).unwrap();
        commit(fixture.dir.path());
        fixture.head = git(fixture.dir.path(), &["rev-parse", "HEAD"]).unwrap();
        let mut app = fixture.app();
        fixture.ai(&mut app);
        let line = if side == DiffSide::New { 1 } else { 2 };
        if let AppMode::AiReview(s) = &mut app.mode {
            s.findings[0].side = Some(side);
            s.findings[0].line = Some(line);
        }
        ask(&mut app);
        app.review_question_work.runner = draft;
        app.draft_review_question(DraftDestination::Inline);
        drain(&mut app);
        app.transfer_review_question_draft();
        drain(&mut app);
        let AppMode::AiReview(s) = &app.mode else {
            panic!("review");
        };
        assert!(s.findings[1].diff_hunk.is_some());
        app.ai_review_stop_edit_finding();
        app.ai_review_open_post_confirm();
        let AppMode::AiReview(s) = &app.mode else {
            panic!("review");
        };
        let post = s.post_confirm.as_ref().unwrap();
        assert_eq!(post.inline.len(), 1);
        assert_eq!(post.inline[0].line, line);
        assert_eq!(
            post.inline[0].side,
            if side == DiffSide::New {
                "RIGHT"
            } else {
                "LEFT"
            }
        );
        assert!(s.findings.iter().all(|f| !f.published));
    }
}

#[test]
fn changed_context_rejects_drafts_and_escape_preserves_comment_text() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    ask(&mut app);
    app.review_question_work.runner = draft;
    app.draft_review_question(DraftDestination::General);
    drain(&mut app);
    std::fs::write(fixture.dir.path().join("helper.rs"), "changed\n").unwrap();
    app.transfer_review_question_draft();
    drain(&mut app);
    let q = app.review_questions().unwrap();
    assert!(q.error.is_some());
    assert!(q.draft.is_some());
    crate::handlers::handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        20,
    )
    .unwrap();
    assert!(app.review_questions().unwrap().draft.is_none());
    app.close_review_questions();
    if let AppMode::DiffViewer(s) = &app.mode {
        assert!(!s.editing_general);
        assert!(s.general_feedback.is_empty());
    }
}

#[test]
fn question_key_and_overlay_do_not_move_or_edit_the_review() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    if let AppMode::DiffViewer(s) = &mut app.mode {
        s.feedback_editor = TextEditor::new("Unsent comment".into());
    }
    crate::handlers::handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('Q'), KeyModifiers::NONE),
        20,
    )
    .unwrap();
    assert!(app.review_questions().unwrap().open);
    crate::handlers::handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
        20,
    )
    .unwrap();
    let backend = ratatui::backend::TestBackend::new(100, 30);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
    crate::handlers::handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        20,
    )
    .unwrap();
    if let AppMode::DiffViewer(s) = &app.mode {
        assert_eq!(s.feedback_editor.text(), "Unsent comment");
        assert_eq!(s.comment_cursor, Some(1));
        assert_eq!(s.patch_scroll, 7);
    }
    assert_eq!(app.review_questions().unwrap().editor.text(), "a");
}

#[test]
fn complete_prompt_context_and_feature_project_global_builtin_precedence() {
    use crate::db::prompt_overrides::OverrideScope;
    use crate::prompts::PromptSource;
    let fixture = Fixture::new();
    let mut app = fixture.app();
    let context = review_context(&app).unwrap();
    let prepared = prepare(&context, local_ai_diff).unwrap();
    let tokens = context.tokens(&prepared, "question", "earlier", "answer");
    let db_dir = TempDir::new().unwrap();
    app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());
    for id in [PromptId::ReviewQuestion, PromptId::ReviewQuestionDraft] {
        let spec = crate::prompts::spec(id);
        for placeholder in spec.placeholders {
            assert!(tokens.get(placeholder).is_some(), "{placeholder}");
        }
        let rendered = app.resolve_headless_prompt(
            id,
            &AgentKind::Claude,
            fixture.dir.path(),
            fixture.dir.path(),
            &tokens,
        );
        assert!(!rendered.contains("{{"));
        assert_eq!(
            app.resolve_headless_template(
                id,
                &AgentKind::Claude,
                fixture.dir.path(),
                fixture.dir.path()
            )
            .1,
            PromptSource::BuiltIn
        );
        app.db
            .as_ref()
            .unwrap()
            .upsert_prompt_override(
                id.as_str(),
                &OverrideScope::Global,
                None,
                "global {{question}}",
            )
            .unwrap();
        assert_eq!(
            app.resolve_headless_prompt(
                id,
                &AgentKind::Claude,
                fixture.dir.path(),
                fixture.dir.path(),
                &tokens
            ),
            "global question"
        );
        std::fs::write(
            fixture.dir.path().join("amf.json"),
            serde_json::json!({"prompt_overrides": {id.as_str(): {"template": "project {{question}}"}}})
                .to_string(),
        )
        .unwrap();
        assert_eq!(
            app.resolve_headless_template(
                id,
                &AgentKind::Claude,
                fixture.dir.path(),
                fixture.dir.path()
            )
            .1,
            PromptSource::Project
        );
        app.db
            .as_ref()
            .unwrap()
            .upsert_prompt_override(
                id.as_str(),
                &OverrideScope::Feature {
                    workdir: fixture.dir.path().display().to_string(),
                },
                None,
                "feature {{question}} {{unknown}}",
            )
            .unwrap();
        assert_eq!(
            app.resolve_headless_prompt(
                id,
                &AgentKind::Claude,
                fixture.dir.path(),
                fixture.dir.path(),
                &tokens
            ),
            "feature question {{unknown}}"
        );
        assert_eq!(
            app.resolve_headless_template(
                id,
                &AgentKind::Claude,
                fixture.dir.path(),
                fixture.dir.path()
            )
            .1,
            PromptSource::Feature
        );
    }
}

#[test]
fn changed_review_revision_and_stale_completion_do_not_replace_current_answers() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    ask(&mut app);
    let sender = inject(&mut app);
    let job = app.review_question_work.job.as_ref().unwrap();
    sender
        .send(Completion {
            owner: job.owner.clone(),
            request: job.request - 1,
            version: job.context.version.clone(),
            result: Err("obsolete".into()),
        })
        .unwrap();
    assert!(!app.poll_review_questions());
    assert!(app.review_question_work.job.is_some());
    assert!(app.review_questions().unwrap().turns[0].answer.is_some());
    if let AppMode::DiffViewer(s) = &mut app.mode {
        s.base_commit = fixture.head.clone();
    }
    assert!(app.poll_review_questions());
    assert!(
        app.review_questions()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .contains("context changed")
    );
    app.open_review_questions();
    let q = app.review_questions().unwrap();
    assert_ne!(q.current_version, q.turns[0].context.version);
}

#[test]
fn overlay_paste_and_draft_cancel_preserve_unsent_review_text() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    if let AppMode::DiffViewer(s) = &mut app.mode {
        s.feedback_editor = TextEditor::new("Unsent".into());
        s.editing_general = true;
    }
    app.open_review_questions();
    crate::handlers::handle_paste(&mut app, "Can I reuse this?\nWhere?").unwrap();
    app.close_review_questions();
    if let AppMode::DiffViewer(s) = &app.mode {
        assert_eq!(s.feedback_editor.text(), "Unsent");
    }
    assert_eq!(
        app.review_questions().unwrap().editor.text(),
        "Can I reuse this?\nWhere?"
    );
}

#[test]
fn completion_while_overlay_is_closed_keeps_original_review_ownership() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    ask(&mut app);
    let sender = inject(&mut app);
    let job = app.review_question_work.job.as_ref().unwrap();
    let completion = Completion {
        owner: job.owner.clone(),
        request: job.request,
        version: job.context.version.clone(),
        result: Ok(worker::Outcome::Answer(
            prepare(&job.context, local_ai_diff).unwrap(),
            "Arrived while reviewing".into(),
        )),
    };
    app.close_review_questions();
    sender.send(completion).unwrap();
    assert!(app.poll_review_questions());
    let q = app.review_questions().unwrap();
    assert!(!q.open);
    assert_eq!(
        q.turns[0].answer.as_deref(),
        Some("Arrived while reviewing")
    );
    if let AppMode::DiffViewer(s) = &app.mode {
        assert_eq!(s.patch_scroll, 7);
        assert_eq!(s.comment_cursor, Some(1));
    }
}

#[test]
fn changes_during_execution_reject_the_answer_and_refresh_recovers() {
    fn changed(input: &RunInput) -> anyhow::Result<String> {
        std::fs::write(
            input.context.workdir.join("helper.rs"),
            "pub fn reusable() -> u32 { 2 }\n",
        )?;
        Ok("This answer concerns the old helper".into())
    }
    let fixture = Fixture::new();
    let mut app = fixture.app();
    app.review_question_work.runner = changed;
    ask(&mut app);
    assert!(app.review_questions().unwrap().turns[0].answer.is_none());
    assert!(
        app.review_questions()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .contains("Repository changed")
    );
    let snapshot =
        crate::diff::load_snapshot(fixture.dir.path(), Some(&fixture.base), false).unwrap();
    if let AppMode::DiffViewer(s) = &mut app.mode {
        s.files = snapshot.files;
    }
    app.review_question_work.runner = inspect_helper;
    app.retry_review_question();
    drain(&mut app);
    assert!(app.review_questions().unwrap().turns[1].answer.is_some());
}

#[test]
fn closing_ai_review_clears_history_without_cancelling_its_independent_review_run() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    fixture.ai(&mut app);
    ask(&mut app);
    let AppMode::AiReview(state) = &app.mode else {
        panic!("AI review");
    };
    let (sender, receiver) = mpsc::channel();
    app.ai_review_run.begin(receiver, state.clone());
    let _question = inject(&mut app);
    let token = app
        .review_question_work
        .job
        .as_ref()
        .unwrap()
        .cancelled
        .clone();
    app.close_ai_review();
    assert!(token.load(std::sync::atomic::Ordering::Relaxed));
    assert!(app.ai_review_run.is_pending());
    assert!(
        app.ai_review_run
            .origin()
            .as_ref()
            .unwrap()
            .questions
            .turns
            .is_empty()
    );
    drop(sender);
}

#[test]
fn changed_inline_selection_is_rejected_instead_of_moving_the_anchor() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    ask(&mut app);
    app.review_question_work.runner = draft;
    app.draft_review_question(DraftDestination::Inline);
    drain(&mut app);
    if let AppMode::DiffViewer(s) = &mut app.mode {
        s.comment_cursor = Some(0);
    }
    app.transfer_review_question_draft();
    drain(&mut app);
    let q = app.review_questions().unwrap();
    assert!(q.draft.is_some());
    assert!(
        q.error
            .as_ref()
            .unwrap()
            .contains("original selected diff line")
    );
    if let AppMode::DiffViewer(s) = &app.mode {
        assert!(s.line_comments.is_empty());
        assert!(!s.editing_line_comment);
    }
}

#[test]
fn pending_request_timeout_retains_question_and_allows_retry() {
    let fixture = Fixture::new();
    let mut app = fixture.app();
    ask(&mut app);
    let _sender = inject(&mut app);
    app.review_question_work.job.as_mut().unwrap().started =
        Instant::now() - Duration::from_secs(181);
    assert!(app.poll_review_questions());
    assert!(
        app.review_questions()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .contains("timed out")
    );
    assert_eq!(
        app.review_questions().unwrap().editor.text(),
        "Can we reuse an existing helper?"
    );
    app.retry_review_question();
    drain(&mut app);
    assert!(
        app.review_questions()
            .unwrap()
            .turns
            .last()
            .unwrap()
            .answer
            .is_some()
    );
}
