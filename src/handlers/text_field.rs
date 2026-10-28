//! Keys shared by the single-line text fields in AMF's forms (New Project,
//! New Feature, batch creation, rename prompts), so every one of them clears
//! and ignores chords the same way.
//!
//! Multi-line editors (compose, prompt editor) use the `TextEditor` keymap
//! instead, where `Ctrl+L` clears the whole buffer.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// `Ctrl+U` clears the focused field: the shell's "kill line", and the key
/// PR review's session-name field already used.
pub(crate) fn is_clear_key(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('u') | KeyCode::Char('U'))
}

/// A `Ctrl+<letter>` chord the form does not bind. A text field must ignore
/// it rather than type the letter (`Ctrl+U` used to insert a literal `u`).
pub(crate) fn is_unbound_ctrl_chord(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char(_)) && key.modifiers.contains(KeyModifiers::CONTROL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_u_is_the_clear_key_and_plain_u_is_text() {
        assert!(is_clear_key(&KeyEvent::new(
            KeyCode::Char('u'),
            KeyModifiers::CONTROL
        )));
        assert!(!is_clear_key(&KeyEvent::new(
            KeyCode::Char('u'),
            KeyModifiers::NONE
        )));
        assert!(is_unbound_ctrl_chord(&KeyEvent::new(
            KeyCode::Char('x'),
            KeyModifiers::CONTROL
        )));
        assert!(!is_unbound_ctrl_chord(&KeyEvent::new(
            KeyCode::Char('X'),
            KeyModifiers::SHIFT
        )));
    }
}
