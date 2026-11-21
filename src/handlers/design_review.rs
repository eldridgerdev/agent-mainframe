//! Key handling for the Design Review room. Layers swallow keys in the order
//! help, text prompt, then the active tab, so a stray `q` cannot close the
//! room while a note is half typed.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::design_review::room::{InputKind, RoomTab};
use crate::app::{App, AppMode};

/// Lines the paging keys move the content pane.
const PAGE_STEP: isize = 10;

pub fn handle_design_review_key(app: &mut App, key: KeyEvent) -> Result<()> {
    let (help_open, typing, tab, drilled) = match &app.mode {
        AppMode::DesignReview(room) => (
            room.help_open,
            room.input.is_some(),
            room.tab,
            room.usage.drill.is_some(),
        ),
        _ => return Ok(()),
    };

    let finishing = matches!(&app.mode, AppMode::DesignReview(room) if room.finish.is_some());
    if finishing && !help_open {
        handle_finish_key(app, key);
        return Ok(());
    }

    if help_open {
        if matches!(
            key.code,
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?')
        ) {
            app.design_review_toggle_help();
        }
        return Ok(());
    }

    if typing {
        handle_input_key(app, key);
        return Ok(());
    }

    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match (tab, key.code) {
        (_, KeyCode::Char('?')) => app.design_review_toggle_help(),
        (_, KeyCode::Tab) => app.design_review_set_tab(match tab {
            RoomTab::Agenda => RoomTab::Usage,
            RoomTab::Usage => RoomTab::Agenda,
        }),
        (_, KeyCode::Char('u')) if !ctrl => app.design_review_set_tab(RoomTab::Usage),

        (RoomTab::Agenda, KeyCode::Esc | KeyCode::Char('q')) => app.close_design_review(),
        (RoomTab::Agenda, KeyCode::Char('j') | KeyCode::Down) => app.design_review_move(1),
        (RoomTab::Agenda, KeyCode::Char('k') | KeyCode::Up) => app.design_review_move(-1),
        (RoomTab::Agenda, KeyCode::PageDown) => app.design_review_scroll_content(PAGE_STEP),
        (RoomTab::Agenda, KeyCode::PageUp) => app.design_review_scroll_content(-PAGE_STEP),
        (RoomTab::Agenda, KeyCode::Char('d')) if ctrl => {
            app.design_review_scroll_content(PAGE_STEP)
        }
        (RoomTab::Agenda, KeyCode::Char('u')) if ctrl => {
            app.design_review_scroll_content(-PAGE_STEP)
        }
        (RoomTab::Agenda, KeyCode::Char(']')) => app.design_review_scroll_thread(-3),
        (RoomTab::Agenda, KeyCode::Char('[')) => app.design_review_scroll_thread(3),
        (RoomTab::Agenda, KeyCode::Left | KeyCode::Right | KeyCode::Char('H' | 'L')) => {
            app.design_review_pan(key);
        }
        (RoomTab::Agenda, KeyCode::Char('m')) => app.design_review_toggle_diagram_source(),
        (RoomTab::Agenda, KeyCode::Char('a')) => app.design_review_accept(),
        (RoomTab::Agenda, KeyCode::Char('n')) => app.design_review_begin_input(InputKind::Note),
        (RoomTab::Agenda, KeyCode::Char('c')) => {
            app.design_review_begin_input(InputKind::RequestChange)
        }
        (RoomTab::Agenda, KeyCode::Char('s')) => app.design_review_begin_input(InputKind::Ask),
        (RoomTab::Agenda, KeyCode::Char('f')) => app.design_review_begin_finish(),
        (RoomTab::Agenda, KeyCode::Char('o') | KeyCode::Enter) => app.design_review_open_files(),

        (RoomTab::Usage, KeyCode::Esc) => {
            if !app.design_review_usage_back() {
                app.design_review_set_tab(RoomTab::Agenda);
            }
        }
        (RoomTab::Usage, KeyCode::Char('q')) => {
            if drilled {
                app.design_review_usage_back();
            } else {
                app.close_design_review();
            }
        }
        (RoomTab::Usage, KeyCode::Char('g')) => app.design_review_cycle_grouping(),
        (RoomTab::Usage, KeyCode::Char('j') | KeyCode::Down) => app.design_review_usage_move(1),
        (RoomTab::Usage, KeyCode::Char('k') | KeyCode::Up) => app.design_review_usage_move(-1),
        (RoomTab::Usage, KeyCode::Enter) => app.design_review_usage_drill(),
        (RoomTab::Usage, KeyCode::Char('y')) => app.design_review_usage_copy(),
        (RoomTab::Usage, KeyCode::Char('r')) => app.design_review_reload_usage(),
        _ => {}
    }
    Ok(())
}

/// Keys while the finish flow (destination, confirmation) is open.
fn handle_finish_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.design_review_finish_cancel(),
        KeyCode::Down | KeyCode::Char('j') => app.design_review_finish_move(1),
        KeyCode::Up | KeyCode::Char('k') => app.design_review_finish_move(-1),
        KeyCode::Enter => app.design_review_finish_confirm(),
        _ => {}
    }
}

fn handle_input_key(app: &mut App, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc => app.design_review_input_cancel(),
        KeyCode::Enter => app.design_review_input_submit(),
        KeyCode::Backspace => app.design_review_input_backspace(),
        KeyCode::Char('u') if ctrl => app.design_review_input_clear(),
        KeyCode::Char(c) if !ctrl => app.design_review_input_char(c),
        _ => {}
    }
}
