//! Key handling for the Learning Mode overlay
//! (`docs/backlog/learning-mode-plan.md`).
//!
//! Dispatch order matters: the modal layers (help, pickers, the question
//! prompt) each swallow every key while they're open, so a stray `q` can't
//! close the overlay out from under someone who is mid-question.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, AppMode, LearningFocus, LearningQaIntent};

/// How many lines the paging keys move.
const PAGE_STEP: usize = 10;

pub fn handle_learning_key(app: &mut App, key: KeyEvent) -> Result<()> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    let (
        help_open,
        question_open,
        harness_picker_open,
        starter_picker_open,
        action_open,
        answer_open,
    ) = match &app.mode {
        AppMode::Learning(state) => (
            state.help_open,
            state.question.is_some(),
            state.harness_picker.is_some(),
            state.starter_picker.is_some(),
            state.action_editor.is_some(),
            state.answer_open,
        ),
        _ => return Ok(()),
    };

    // The help overlay is first: it opens itself on a project's first visit,
    // so it has to be dismissible before anything else reads a key.
    if help_open {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => app.learning_close_help(),
            KeyCode::Char('j') | KeyCode::Down => app.learning_help_scroll(1),
            KeyCode::Char('k') | KeyCode::Up => app.learning_help_scroll(-1),
            KeyCode::PageDown => app.learning_help_scroll(PAGE_STEP as isize),
            KeyCode::PageUp => app.learning_help_scroll(-(PAGE_STEP as isize)),
            _ => {}
        }
        return Ok(());
    }

    if starter_picker_open {
        match key.code {
            KeyCode::Esc => app.learning_close_starter_picker(),
            KeyCode::Char('j') | KeyCode::Down => app.learning_starter_picker_move(1),
            KeyCode::Char('k') | KeyCode::Up => app.learning_starter_picker_move(-1),
            KeyCode::Enter => app.learning_starter_picker_confirm(),
            _ => {}
        }
        return Ok(());
    }

    if harness_picker_open {
        match key.code {
            KeyCode::Esc => app.learning_close_harness_picker(),
            KeyCode::Char('j') | KeyCode::Down => app.learning_harness_picker_move(1),
            KeyCode::Char('k') | KeyCode::Up => app.learning_harness_picker_move(-1),
            KeyCode::Enter => app.learning_harness_picker_confirm(),
            _ => {}
        }
        return Ok(());
    }

    // Before the answer pane, which is where this dialog is usually opened
    // from: it has to swallow the keys typed into its title.
    if action_open {
        return handle_action_key(app, key);
    }

    if question_open {
        return handle_question_key(app, key, ctrl);
    }

    if answer_open {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => app.learning_close_answer(),
            KeyCode::Char('j') | KeyCode::Down => app.learning_answer_scroll(1),
            KeyCode::Char('k') | KeyCode::Up => app.learning_answer_scroll(-1),
            KeyCode::PageDown | KeyCode::Char(' ') => {
                app.learning_answer_scroll(PAGE_STEP as isize)
            }
            KeyCode::PageUp => app.learning_answer_scroll(-(PAGE_STEP as isize)),
            KeyCode::Char('g') => app.learning_answer_scroll_to_top(),
            KeyCode::Char('G') => app.learning_answer_scroll_to_bottom(),
            // The most likely next move while reading an answer: ask about
            // something in it. Closes the pane and opens the prompt.
            KeyCode::Char('F') => app.learning_open_follow_up(),
            // The next most likely: doubt it. Re-asks with the repo readable.
            KeyCode::Char('D') => {
                app.learning_deep_dive();
            }
            // Keep what you're reading as something to come back to.
            KeyCode::Char('a') => app.learning_make_actionable(),
            // Take what you're reading to an agent that can act on it. This
            // leaves the overlay, so it is the last thing in the list.
            KeyCode::Char('S') => {
                app.learning_escalate();
            }
            // Re-file what you're reading: an explanation that turned out to
            // be a problem belongs under "change".
            KeyCode::Char('i') => {
                app.learning_relabel_intent();
            }
            KeyCode::Char('?') => app.learning_open_help(),
            _ => {}
        }
        return Ok(());
    }

    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.close_learning_mode(),
        KeyCode::Char('?') => app.learning_open_help(),
        KeyCode::Tab => app.learning_cycle_focus(),

        // Navigation, routed to whichever pane has focus.
        KeyCode::Char('j') | KeyCode::Down => learning_move(app, 1),
        KeyCode::Char('k') | KeyCode::Up => learning_move(app, -1),
        KeyCode::PageDown => learning_move(app, PAGE_STEP as isize),
        KeyCode::PageUp => learning_move(app, -(PAGE_STEP as isize)),
        KeyCode::Enter => app.learning_activate_selection(),

        // Moving around the tree. `l`/`Right` opens, `h`/`Left` closes or
        // steps out — the shape every file tree uses, so it needs no
        // explaining to anyone who has met one, and `Enter` covers anyone who
        // hasn't.
        KeyCode::Char('l') | KeyCode::Right => {
            app.learning_expand_or_open();
        }
        KeyCode::Char('h') | KeyCode::Left => app.learning_collapse_or_parent(),
        KeyCode::Char('Z') => app.learning_toggle_expand_all(),

        // What the next question is about.
        KeyCode::Char('v') => app.learning_start_range(),
        KeyCode::Char('V') => app.learning_clear_range(),
        KeyCode::Char('f') => app.learning_select_whole_file(),
        KeyCode::Char('P') => app.learning_select_project(),
        KeyCode::Char('x') => app.learning_select_hunk(),

        // Asking.
        KeyCode::Char('e') => app.learning_open_question(LearningQaIntent::Explain, None),
        KeyCode::Char('c') => app.learning_open_question(LearningQaIntent::Action, None),
        KeyCode::Char('t') => app.learning_open_starter_picker(),
        KeyCode::Char('F') => app.learning_open_follow_up(),
        KeyCode::Char('D') => {
            app.learning_deep_dive();
        }
        KeyCode::Char('i') => {
            app.learning_relabel_intent();
        }
        KeyCode::Char('a') => app.learning_make_actionable(),
        KeyCode::Char('S') => {
            app.learning_escalate();
        }

        // Settings and view.
        KeyCode::Char('s') => app.learning_toggle_scope(),
        KeyCode::Char('L') => app.learning_toggle_level(),
        KeyCode::Char('m') => app.learning_open_harness_picker(),
        KeyCode::Char('z') => app.learning_toggle_start_here(),
        _ => {}
    }
    Ok(())
}

/// Keys while the question prompt is open. Tab submits (matching the review
/// feedback editor), Esc cancels, and everything else is typing.
fn handle_question_key(app: &mut App, key: KeyEvent, ctrl: bool) -> Result<()> {
    if ctrl && key.code == KeyCode::Char('t') {
        if let AppMode::Learning(state) = &mut app.mode
            && let Some(q) = &mut state.question
        {
            q.editor.toggle_vim();
        }
        return Ok(());
    }
    // Ctrl+E flips explain ⇄ change before submitting, so a mis-started
    // question doesn't have to be retyped.
    if ctrl && key.code == KeyCode::Char('e') {
        app.learning_question_toggle_intent();
        return Ok(());
    }
    if ctrl && key.code == KeyCode::Char('p') {
        app.learning_open_starter_picker();
        return Ok(());
    }

    match key.code {
        KeyCode::Tab => {
            app.learning_submit_question();
        }
        KeyCode::Esc
            if matches!(
                &app.mode,
                AppMode::Learning(state)
                    if state.question.as_ref().is_some_and(|q| q.editor.vim_mode().is_none())
            ) =>
        {
            app.learning_cancel_question();
        }
        _ => {
            if let AppMode::Learning(state) = &mut app.mode
                && let Some(q) = &mut state.question
            {
                let outcome = q.editor.handle_key(key);
                if outcome.text_changed || outcome.cursor_moved {
                    q.sync_to_cursor = true;
                }
            }
        }
    }
    Ok(())
}

/// Keys while the "keep this on the TODO list" confirmation is open.
///
/// Enter writes it; Tab does too, because the question prompt next door submits
/// with Tab and someone arriving from it will reach for the same key. Esc walks
/// away without writing anything, which is the whole point of the dialog. Every
/// other key types into the title, including `q` — nothing here may close the
/// overlay out from under an edit.
fn handle_action_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Enter | KeyCode::Tab => {
            app.learning_confirm_action();
        }
        KeyCode::Esc => app.learning_cancel_action(),
        _ => {
            if let AppMode::Learning(state) = &mut app.mode
                && let Some(editor) = &mut state.action_editor
            {
                let outcome = editor.title.handle_key(key);
                if outcome.text_changed || outcome.cursor_moved {
                    editor.sync_to_cursor = true;
                }
                if outcome.text_changed {
                    // The refusal was about the title being empty, and it no
                    // longer is — or it is again, and pressing Enter will say so
                    // afresh.
                    editor.error = None;
                }
            }
        }
    }
    Ok(())
}

/// Move the cursor in the focused pane.
fn learning_move(app: &mut App, delta: isize) {
    let focus = match &app.mode {
        AppMode::Learning(state) => state.focus,
        _ => return,
    };
    match focus {
        LearningFocus::FileList => {
            for _ in 0..delta.unsigned_abs() {
                if delta > 0 {
                    app.learning_select_next_entry();
                } else {
                    app.learning_select_prev_entry();
                }
            }
        }
        LearningFocus::Content => app.learning_cursor_move(delta),
        LearningFocus::Qa => app.learning_select_qa(delta),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::LearningFocus;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn learning(app: &App) -> &crate::app::LearningViewState {
        match &app.mode {
            AppMode::Learning(state) => state,
            _ => panic!("expected Learning mode"),
        }
    }

    fn opened() -> (tempfile::TempDir, App) {
        let (repo, mut app) = crate::app::learning::tests::opened_app_for_handlers();
        // Focus the content pane so line keys mean something.
        app.learning_cycle_focus();
        (repo, app)
    }

    // ── the K entry key ──────────────────────────────────────

    /// `K` goes through `handle_normal_key`, not this module's handler — the
    /// overlay has to be reachable from the dashboard for any of the rest to
    /// matter.
    fn press_k(app: &mut App) {
        crate::handlers::handle_normal_key(app, key(KeyCode::Char('K'))).unwrap();
    }

    #[test]
    fn k_opens_learning_mode_on_the_selected_feature() {
        let (_repo, mut app) = crate::app::learning::tests::dashboard_app_for_handlers();
        app.selection = crate::app::Selection::Feature(0, 0);

        press_k(&mut app);

        let state = learning(&app);
        assert_eq!(state.pi, 0);
        assert_eq!(state.fi, 0);
        assert_eq!(state.feature_name, "my-feat");
        assert!(!state.entries.is_empty(), "the file list loaded");
    }

    #[test]
    fn k_on_a_project_row_opens_its_first_feature() {
        let (_repo, mut app) = crate::app::learning::tests::dashboard_app_for_handlers();
        app.selection = crate::app::Selection::Project(0);

        press_k(&mut app);

        assert_eq!(learning(&app).fi, 0);
    }

    #[test]
    fn k_explains_itself_when_the_project_has_no_features() {
        let (_repo, mut app) = crate::app::learning::tests::featureless_app_for_handlers();
        app.selection = crate::app::Selection::Project(0);

        press_k(&mut app);

        assert!(
            matches!(app.mode, AppMode::Normal),
            "nothing to read, so the overlay must not open"
        );
        assert!(
            app.message
                .as_deref()
                .is_some_and(|m| m.contains("feature")),
            "the keypress has to say why nothing happened, got {:?}",
            app.message
        );
    }

    /// The whole chain in one test: the key sets the mode, the dashboard's
    /// `draw` dispatches on it, and the overlay paints. Each half is covered
    /// elsewhere; what this catches is the two not being connected.
    #[test]
    fn k_makes_the_overlay_actually_render() {
        use ratatui::{Terminal, backend::TestBackend};

        let (_repo, mut app) = crate::app::learning::tests::dashboard_app_for_handlers();
        app.selection = crate::app::Selection::Feature(0, 0);
        press_k(&mut app);

        let backend = TestBackend::new(140, 44);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(
            rendered.contains("Learning Mode"),
            "the overlay is on screen"
        );
        assert!(rendered.contains("read-only"));
    }

    #[test]
    fn closing_the_overlay_returns_to_the_dashboard() {
        let (_repo, mut app) = crate::app::learning::tests::dashboard_app_for_handlers();
        app.selection = crate::app::Selection::Feature(0, 0);
        press_k(&mut app);

        handle_learning_key(&mut app, key(KeyCode::Char('q'))).unwrap();

        assert!(matches!(app.mode, AppMode::Normal));
        assert!(matches!(
            app.selection,
            crate::app::Selection::Feature(0, 0)
        ));
    }

    #[test]
    fn f_asks_a_follow_up_from_the_answer_you_are_reading() {
        let (_repo, mut app) = opened();

        // Ask, and hand the row an answer without running a real CLI.
        let parent = app
            .learning_ask("What is this?", LearningQaIntent::Explain, None)
            .unwrap();
        app.learning_answer_tx
            .send(crate::app::learning::LearningAnswer {
                qa_id: parent.clone(),
                result: Ok("It is the entry point.".to_string()),
            })
            .unwrap();
        assert!(app.poll_learning_answers_bg());

        handle_learning_key(&mut app, key(KeyCode::Char('F'))).unwrap();
        let question = learning(&app).question.as_ref().unwrap();
        assert_eq!(question.parent_qa_id.as_deref(), Some(parent.as_str()));
        assert!(!learning(&app).answer_open, "the answer pane steps aside");

        for c in "what is that?".chars() {
            handle_learning_key(&mut app, key(KeyCode::Char(c))).unwrap();
        }
        handle_learning_key(&mut app, key(KeyCode::Tab)).unwrap();

        let state = learning(&app);
        assert_eq!(state.qa.len(), 2);
        assert_eq!(state.qa[1].question, "what is that?");
        assert_eq!(state.qa[1].parent_qa_id.as_deref(), Some(parent.as_str()));
    }

    #[test]
    fn d_sends_the_answer_you_are_reading_back_with_the_repo_open() {
        let (_repo, mut app) = opened();

        let origin = app
            .learning_ask("What is this?", LearningQaIntent::Explain, None)
            .unwrap();
        app.learning_answer_tx
            .send(crate::app::learning::LearningAnswer {
                qa_id: origin.clone(),
                result: Ok("It is the entry point.".to_string()),
            })
            .unwrap();
        assert!(app.poll_learning_answers_bg());

        // From the answer pane, where doubting what you just read happens.
        handle_learning_key(&mut app, key(KeyCode::Tab)).unwrap();
        assert_eq!(learning(&app).focus, LearningFocus::Qa);
        handle_learning_key(&mut app, key(KeyCode::Enter)).unwrap();
        assert!(learning(&app).answer_open);
        handle_learning_key(&mut app, key(KeyCode::Char('D'))).unwrap();

        let state = learning(&app);
        assert!(!state.answer_open, "the pane steps aside for the new run");
        assert_eq!(state.qa.len(), 2);
        assert_eq!(state.qa[1].parent_qa_id.as_deref(), Some(origin.as_str()));
        assert_eq!(
            state.qa[1].run_mode,
            crate::app::LearningRunMode::DeepDive,
            "this one gets to read the repo"
        );
    }

    /// Re-filing is offered where an answer is read, not only from the list —
    /// realising "that's a bug, not a fact" happens while reading it.
    #[test]
    fn i_re_files_the_answer_you_are_reading() {
        let (_repo, mut app) = opened();

        let id = app
            .learning_ask("What does this do?", LearningQaIntent::Explain, None)
            .unwrap();
        app.learning_answer_tx
            .send(crate::app::learning::LearningAnswer {
                qa_id: id.clone(),
                result: Ok("It retries forever.".to_string()),
            })
            .unwrap();
        assert!(app.poll_learning_answers_bg());

        handle_learning_key(&mut app, key(KeyCode::Tab)).unwrap();
        handle_learning_key(&mut app, key(KeyCode::Enter)).unwrap();
        assert!(learning(&app).answer_open);

        handle_learning_key(&mut app, key(KeyCode::Char('i'))).unwrap();

        let state = learning(&app);
        assert_eq!(state.qa[0].intent, LearningQaIntent::Action);
        assert!(
            state.answer_open,
            "you are still reading it — re-filing doesn't close the pane"
        );
        assert!(state.notice.is_some(), "and it says what it did");
    }

    /// Keeping an answer is offered where the answer is read: deciding "I
    /// should come back to this" happens while reading it, not from the list.
    /// A DB-backed overlay with an answer in it, past the first-open help
    /// overlay — which a real database means is showing, since this is the
    /// project's first visit.
    fn answered_with_db() -> (tempfile::TempDir, tempfile::TempDir, App) {
        let (repo, db, mut app) = crate::app::learning::tests::opened_app_with_db_for_handlers();
        assert!(learning(&app).help_open, "the intro shows on first open");
        handle_learning_key(&mut app, key(KeyCode::Esc)).unwrap();

        let id = app
            .learning_ask("What does this do?", LearningQaIntent::Explain, None)
            .unwrap();
        app.learning_answer_tx
            .send(crate::app::learning::LearningAnswer {
                qa_id: id,
                result: Ok("It runs the program.".to_string()),
            })
            .unwrap();
        assert!(app.poll_learning_answers_bg());
        (repo, db, app)
    }

    #[test]
    fn a_keeps_the_answer_you_are_reading() {
        let (_repo, _db, mut app) = answered_with_db();

        // From the answer pane.
        handle_learning_key(&mut app, key(KeyCode::Tab)).unwrap();
        handle_learning_key(&mut app, key(KeyCode::Tab)).unwrap();
        assert_eq!(learning(&app).focus, LearningFocus::Qa);
        handle_learning_key(&mut app, key(KeyCode::Enter)).unwrap();
        assert!(learning(&app).answer_open);
        handle_learning_key(&mut app, key(KeyCode::Char('a'))).unwrap();
        assert!(learning(&app).action_editor.is_some());

        // `q` types rather than closing anything, and Enter is what writes.
        handle_learning_key(&mut app, key(KeyCode::Char('q'))).unwrap();
        assert!(
            matches!(app.mode, AppMode::Learning(_)),
            "a stray q must not close the overlay out from under an edit"
        );
        handle_learning_key(&mut app, key(KeyCode::Enter)).unwrap();

        assert!(learning(&app).action_editor.is_none());
        assert!(
            learning(&app).answer_open,
            "you are still reading it — keeping it doesn't close the pane"
        );
        assert!(
            learning(&app).qa[0].todo_id.is_some(),
            "the entry now points at the item it produced"
        );
        let db = app.db.as_ref().unwrap();
        let list = db
            .todo_list(&crate::db::todos::TodoScope::Project {
                project_id: "proj-1".to_string(),
            })
            .unwrap()
            .unwrap();
        let todos = db.todos(&list.id).unwrap();
        assert_eq!(todos.len(), 1);
        assert!(
            todos[0].title.ends_with('q'),
            "the keystrokes landed in the title: {}",
            todos[0].title
        );
    }

    #[test]
    fn esc_walks_away_from_the_confirmation_without_writing() {
        let (_repo, _db, mut app) = answered_with_db();

        handle_learning_key(&mut app, key(KeyCode::Char('a'))).unwrap();
        assert!(learning(&app).action_editor.is_some());
        handle_learning_key(&mut app, key(KeyCode::Esc)).unwrap();

        assert!(learning(&app).action_editor.is_none());
        assert!(learning(&app).qa[0].todo_id.is_none());
        let db = app.db.as_ref().unwrap();
        assert!(
            db.todo_list(&crate::db::todos::TodoScope::Project {
                project_id: "proj-1".to_string(),
            })
            .unwrap()
            .is_none(),
            "not even a list was created"
        );
    }

    /// The one key that leaves the overlay, pressed where it is reached for:
    /// from the answer that made the user want a real agent.
    #[test]
    fn s_hands_the_answer_you_are_reading_to_a_live_agent() {
        let (_repo, _db, mut app) = crate::app::learning::tests::launchable_app_for_handlers();
        // A real database means the intro is showing on this first visit.
        handle_learning_key(&mut app, key(KeyCode::Esc)).unwrap();

        let id = app
            .learning_ask("What does this do?", LearningQaIntent::Explain, None)
            .unwrap();
        app.learning_answer_tx
            .send(crate::app::learning::LearningAnswer {
                qa_id: id,
                result: Ok("It is the entry point.".to_string()),
            })
            .unwrap();
        assert!(app.poll_learning_answers_bg());

        // From the answer pane.
        handle_learning_key(&mut app, key(KeyCode::Tab)).unwrap();
        handle_learning_key(&mut app, key(KeyCode::Tab)).unwrap();
        assert_eq!(learning(&app).focus, LearningFocus::Qa);
        handle_learning_key(&mut app, key(KeyCode::Enter)).unwrap();
        assert!(learning(&app).answer_open);

        handle_learning_key(&mut app, key(KeyCode::Char('S'))).unwrap();

        assert_eq!(app.store.projects[0].features[0].sessions.len(), 1);
        match &app.mode {
            AppMode::Compose(state) => assert!(
                state.editor.text().contains("What does this do?"),
                "the composer is pre-filled: {}",
                state.editor.text()
            ),
            other => panic!(
                "expected the composer, got {:?}",
                std::mem::discriminant(other)
            ),
        }
    }

    #[test]
    fn the_help_overlay_opens_closes_and_swallows_keys() {
        let (_repo, mut app) = opened();

        handle_learning_key(&mut app, key(KeyCode::Char('?'))).unwrap();
        assert!(learning(&app).help_open);

        // While it's open, nothing else acts on a key — `s` would otherwise
        // toggle the browse scope.
        let scope_before = learning(&app).scope;
        handle_learning_key(&mut app, key(KeyCode::Char('s'))).unwrap();
        assert!(learning(&app).help_open);
        assert_eq!(learning(&app).scope, scope_before);

        handle_learning_key(&mut app, key(KeyCode::Char('j'))).unwrap();
        assert_eq!(learning(&app).help_scroll, 1);

        handle_learning_key(&mut app, key(KeyCode::Esc)).unwrap();
        assert!(!learning(&app).help_open);
        assert!(
            matches!(app.mode, AppMode::Learning(_)),
            "still in the mode"
        );
    }

    #[test]
    fn q_closes_the_overlay_but_not_while_a_question_is_being_typed() {
        let (_repo, mut app) = opened();

        handle_learning_key(&mut app, key(KeyCode::Char('e'))).unwrap();
        assert!(learning(&app).question.is_some());

        // `q` is typing here, not a close.
        handle_learning_key(&mut app, key(KeyCode::Char('q'))).unwrap();
        assert!(matches!(app.mode, AppMode::Learning(_)));
        assert_eq!(learning(&app).question.as_ref().unwrap().editor.text(), "q");

        handle_learning_key(&mut app, key(KeyCode::Esc)).unwrap();
        assert!(learning(&app).question.is_none());

        handle_learning_key(&mut app, key(KeyCode::Char('q'))).unwrap();
        assert!(matches!(app.mode, AppMode::Normal));
    }

    #[test]
    fn tab_submits_the_typed_question() {
        let (_repo, mut app) = opened();

        handle_learning_key(&mut app, key(KeyCode::Char('e'))).unwrap();
        for c in "what is this".chars() {
            handle_learning_key(&mut app, key(KeyCode::Char(c))).unwrap();
        }
        handle_learning_key(&mut app, key(KeyCode::Tab)).unwrap();

        let state = learning(&app);
        assert!(state.question.is_none());
        assert_eq!(state.qa.len(), 1);
        assert_eq!(state.qa[0].question, "what is this");
        assert_eq!(state.qa[0].intent, LearningQaIntent::Explain);
    }

    #[test]
    fn ctrl_e_flips_intent_without_losing_the_text() {
        let (_repo, mut app) = opened();
        handle_learning_key(&mut app, key(KeyCode::Char('e'))).unwrap();
        for c in "split this".chars() {
            handle_learning_key(&mut app, key(KeyCode::Char(c))).unwrap();
        }
        handle_learning_key(&mut app, ctrl(KeyCode::Char('e'))).unwrap();

        let q = learning(&app).question.as_ref().unwrap();
        assert_eq!(q.intent, LearningQaIntent::Action);
        assert_eq!(q.editor.text(), "split this");
    }

    #[test]
    fn the_starter_picker_fills_the_prompt_without_asking() {
        let (_repo, mut app) = opened();

        handle_learning_key(&mut app, key(KeyCode::Char('t'))).unwrap();
        assert!(learning(&app).starter_picker.is_some());
        assert!(
            learning(&app).question.is_some(),
            "the picker opens the prompt it fills"
        );

        handle_learning_key(&mut app, key(KeyCode::Enter)).unwrap();
        let state = learning(&app);
        assert!(state.starter_picker.is_none());
        assert!(!state.question.as_ref().unwrap().editor.text().is_empty());
        assert!(state.qa.is_empty(), "picking a preset asks nothing yet");
    }

    #[test]
    fn the_two_ask_keys_choose_the_intent() {
        let (_repo, mut app) = opened();

        handle_learning_key(&mut app, key(KeyCode::Char('e'))).unwrap();
        assert_eq!(
            learning(&app).question.as_ref().unwrap().intent,
            LearningQaIntent::Explain
        );
        handle_learning_key(&mut app, key(KeyCode::Esc)).unwrap();

        handle_learning_key(&mut app, key(KeyCode::Char('c'))).unwrap();
        assert_eq!(
            learning(&app).question.as_ref().unwrap().intent,
            LearningQaIntent::Action
        );
    }

    #[test]
    fn tab_cycles_focus_between_the_three_panes() {
        let (_repo, mut app) = opened();
        // `opened` already moved focus once, off the file list.
        assert_eq!(learning(&app).focus, LearningFocus::Content);
        handle_learning_key(&mut app, key(KeyCode::Tab)).unwrap();
        assert_eq!(learning(&app).focus, LearningFocus::Qa);
        handle_learning_key(&mut app, key(KeyCode::Tab)).unwrap();
        assert_eq!(learning(&app).focus, LearningFocus::FileList);
    }

    /// `l` and `h` walk the tree, and `Enter` does the same job for anyone who
    /// never finds them. All three go through the file-list focus, which is
    /// where `opened` does *not* leave the focus, so this also proves the keys
    /// are routed by pane rather than swallowed globally.
    #[test]
    fn the_tree_keys_open_and_close_folders() {
        let (_repo, mut app) = opened();
        handle_learning_key(&mut app, key(KeyCode::Tab)).unwrap();
        handle_learning_key(&mut app, key(KeyCode::Tab)).unwrap();
        assert_eq!(learning(&app).focus, LearningFocus::FileList);

        let src = learning(&app)
            .entries
            .iter()
            .position(|e| e.dir_path() == Some("src"))
            .expect("the tree should have a src folder");
        if let crate::app::AppMode::Learning(state) = &mut app.mode {
            state.selected_entry = src;
        }

        // `src` opens by default (it holds a Start here candidate), so `h`
        // closes it and `l` opens it again.
        assert!(learning(&app).expanded_dirs.contains("src"));
        handle_learning_key(&mut app, key(KeyCode::Char('h'))).unwrap();
        assert!(!learning(&app).expanded_dirs.contains("src"));
        handle_learning_key(&mut app, key(KeyCode::Char('l'))).unwrap();
        assert!(learning(&app).expanded_dirs.contains("src"));

        // Enter toggles it too, and leaves the cursor on the folder rather
        // than shifting focus to the content pane the way it does for a file.
        handle_learning_key(&mut app, key(KeyCode::Enter)).unwrap();
        assert!(!learning(&app).expanded_dirs.contains("src"));
        assert_eq!(learning(&app).focus, LearningFocus::FileList);
        assert_eq!(
            learning(&app).selected_entry().and_then(|e| e.dir_path()),
            Some("src")
        );

        handle_learning_key(&mut app, key(KeyCode::Char('Z'))).unwrap();
        assert!(learning(&app).expanded_dirs.contains("src"));
    }

    #[test]
    fn selection_keys_change_what_the_question_is_about() {
        let (_repo, mut app) = opened();

        handle_learning_key(&mut app, key(KeyCode::Char('f'))).unwrap();
        assert_eq!(learning(&app).anchor, crate::app::LearningAnchor::File);

        handle_learning_key(&mut app, key(KeyCode::Char('P'))).unwrap();
        assert_eq!(learning(&app).anchor, crate::app::LearningAnchor::Project);

        handle_learning_key(&mut app, key(KeyCode::Char('j'))).unwrap();
        // Moving the cursor off the project anchor is deliberate: the project
        // anchor is only left by choosing another one.
        assert_eq!(learning(&app).anchor, crate::app::LearningAnchor::Project);

        handle_learning_key(&mut app, key(KeyCode::Char('v'))).unwrap();
        assert!(matches!(
            learning(&app).anchor,
            crate::app::LearningAnchor::Lines { .. }
        ));
    }

    #[test]
    fn the_level_and_scope_keys_toggle_their_settings() {
        let (_repo, mut app) = opened();
        let scope = learning(&app).scope;

        handle_learning_key(&mut app, key(KeyCode::Char('L'))).unwrap();
        assert_eq!(learning(&app).level, crate::app::LearningLevel::Familiar);

        handle_learning_key(&mut app, key(KeyCode::Char('s'))).unwrap();
        assert_ne!(learning(&app).scope, scope);
    }

    #[test]
    fn the_harness_picker_swallows_keys_while_open() {
        let (_repo, mut app) = opened();
        handle_learning_key(&mut app, key(KeyCode::Char('m'))).unwrap();
        assert!(learning(&app).harness_picker.is_some());

        let level = learning(&app).level;
        handle_learning_key(&mut app, key(KeyCode::Char('L'))).unwrap();
        assert_eq!(learning(&app).level, level, "L must not reach the overlay");

        handle_learning_key(&mut app, key(KeyCode::Esc)).unwrap();
        assert!(learning(&app).harness_picker.is_none());
    }
}
