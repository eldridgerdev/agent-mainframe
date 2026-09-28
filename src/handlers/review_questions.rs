use crate::app::{App, review_questions::DraftDestination};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub(crate) fn handle(app: &mut App, key: KeyEvent) -> Result<()> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let Some(q) = app.review_questions() else {
        return Ok(());
    };
    if key.code == KeyCode::Esc {
        if q.draft.is_some() {
            app.cancel_review_question();
            app.review_questions_mut().expect("review").draft = None;
        } else {
            app.close_review_questions();
        }
        return Ok(());
    }
    if ctrl && key.code == KeyCode::Char('x') {
        app.cancel_review_question();
        return Ok(());
    }
    if q.draft.is_some() {
        if q.request.is_some() {
            return Ok(());
        }
        if ctrl && key.code == KeyCode::Enter {
            app.transfer_review_question_draft();
        } else if let Some((_, editor)) = &mut app.review_questions_mut().expect("review").draft {
            editor.handle_key(key);
        }
        return Ok(());
    }
    if ctrl && key.code == KeyCode::Char('h') {
        app.cycle_review_question_harness();
    } else if ctrl && key.code == KeyCode::Enter {
        app.submit_review_question();
    } else if q.editing {
        app.review_questions_mut()
            .expect("review")
            .editor
            .handle_key(key);
    } else {
        match key.code {
            KeyCode::Char('e') => {
                let q = app.review_questions_mut().expect("review");
                if q.turns.get(q.selected).is_some_and(|t| t.answer.is_some()) {
                    q.editor = crate::editor::TextEditor::new(String::new());
                }
                q.editing = true;
            }
            KeyCode::Char('r') => app.retry_review_question(),
            KeyCode::Char('i') => app.draft_review_question(DraftDestination::Inline),
            KeyCode::Char('g') => app.draft_review_question(DraftDestination::General),
            KeyCode::Char('x') => app.cancel_review_question(),
            KeyCode::Down | KeyCode::Char('j') => {
                app.review_questions_mut().expect("review").scroll += 1
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let q = app.review_questions_mut().expect("review");
                q.scroll = q.scroll.saturating_sub(1);
            }
            KeyCode::Char('[') | KeyCode::Char(']') => {
                let q = app.review_questions_mut().expect("review");
                if key.code == KeyCode::Char('[') {
                    q.selected = q.selected.saturating_sub(1);
                } else {
                    q.selected = (q.selected + 1).min(q.turns.len().saturating_sub(1));
                }
                q.scroll = 0;
            }
            _ => {}
        }
    }
    Ok(())
}
