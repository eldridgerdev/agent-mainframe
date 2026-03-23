use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorKeymap {
    Plain,
    Vim,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VimMode {
    Insert,
    Normal,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EditorOutcome {
    pub handled: bool,
    pub text_changed: bool,
    pub cursor_moved: bool,
    pub mode_changed: bool,
}

impl EditorOutcome {
    fn handled() -> Self {
        Self {
            handled: true,
            ..Self::default()
        }
    }

    fn changed() -> Self {
        Self {
            handled: true,
            text_changed: true,
            ..Self::default()
        }
    }

    fn moved() -> Self {
        Self {
            handled: true,
            cursor_moved: true,
            ..Self::default()
        }
    }

    fn mode_changed() -> Self {
        Self {
            handled: true,
            mode_changed: true,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone)]
pub struct TextEditor {
    text: String,
    cursor: usize,
    preferred_col: Option<usize>,
    keymap: EditorKeymap,
    vim_mode: VimMode,
}

impl TextEditor {
    pub fn new(text: String) -> Self {
        let cursor = text.len();
        Self {
            text,
            cursor,
            preferred_col: None,
            keymap: EditorKeymap::Plain,
            vim_mode: VimMode::Insert,
        }
    }

    pub fn with_vim(text: String) -> Self {
        let mut editor = Self::new(text);
        editor.keymap = EditorKeymap::Vim;
        editor
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn keymap(&self) -> EditorKeymap {
        self.keymap
    }

    pub fn vim_mode(&self) -> Option<VimMode> {
        match self.keymap {
            EditorKeymap::Plain => None,
            EditorKeymap::Vim => Some(self.vim_mode),
        }
    }

    pub fn toggle_vim(&mut self) -> EditorOutcome {
        match self.keymap {
            EditorKeymap::Plain => {
                self.keymap = EditorKeymap::Vim;
                self.vim_mode = VimMode::Insert;
            }
            EditorKeymap::Vim => {
                self.keymap = EditorKeymap::Plain;
                self.vim_mode = VimMode::Insert;
            }
        }
        self.preferred_col = None;
        EditorOutcome::mode_changed()
    }

    pub fn insert_str(&mut self, text: &str) -> EditorOutcome {
        if text.is_empty() {
            return EditorOutcome::default();
        }

        self.text.insert_str(self.cursor, text);
        self.cursor += text.len();
        self.preferred_col = None;
        EditorOutcome::changed()
    }

    pub fn cursor_row_col(&self) -> (usize, usize) {
        let row = self.text[..self.cursor]
            .chars()
            .filter(|&ch| ch == '\n')
            .count();
        let line_start = self.line_start(self.cursor);
        let col = self.text[line_start..self.cursor].chars().count();
        (row, col)
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> EditorOutcome {
        match self.keymap {
            EditorKeymap::Plain => self.handle_plain_key(key),
            EditorKeymap::Vim => self.handle_vim_key(key),
        }
    }

    fn handle_plain_key(&mut self, key: KeyEvent) -> EditorOutcome {
        match key.code {
            KeyCode::Enter => self.insert_str("\n"),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Left => self.move_left(),
            KeyCode::Right => self.move_right(),
            KeyCode::Up => self.move_up(),
            KeyCode::Down => self.move_down(),
            KeyCode::Home => self.move_home(),
            KeyCode::End => self.move_end(),
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                let mut text = String::new();
                text.push(c);
                self.insert_str(&text)
            }
            _ => EditorOutcome::default(),
        }
    }

    fn handle_vim_key(&mut self, key: KeyEvent) -> EditorOutcome {
        match self.vim_mode {
            VimMode::Insert => self.handle_vim_insert_key(key),
            VimMode::Normal => self.handle_vim_normal_key(key),
        }
    }

    fn handle_vim_insert_key(&mut self, key: KeyEvent) -> EditorOutcome {
        match key.code {
            KeyCode::Esc => {
                self.vim_mode = VimMode::Normal;
                self.preferred_col = None;
                EditorOutcome::mode_changed()
            }
            _ => self.handle_plain_key(key),
        }
    }

    fn handle_vim_normal_key(&mut self, key: KeyEvent) -> EditorOutcome {
        match key.code {
            KeyCode::Esc => EditorOutcome::handled(),
            KeyCode::Char('i') if key.modifiers.is_empty() => {
                self.vim_mode = VimMode::Insert;
                EditorOutcome::mode_changed()
            }
            KeyCode::Char('a') if key.modifiers.is_empty() => {
                let moved = self.move_right();
                self.vim_mode = VimMode::Insert;
                let mut outcome = EditorOutcome::mode_changed();
                outcome.cursor_moved = moved.cursor_moved;
                outcome
            }
            KeyCode::Char('A') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                let moved = self.move_end();
                self.vim_mode = VimMode::Insert;
                let mut outcome = EditorOutcome::mode_changed();
                outcome.cursor_moved = moved.cursor_moved;
                outcome
            }
            KeyCode::Char('I') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                let moved = self.move_first_non_whitespace();
                self.vim_mode = VimMode::Insert;
                let mut outcome = EditorOutcome::mode_changed();
                outcome.cursor_moved = moved.cursor_moved;
                outcome
            }
            KeyCode::Char('h') if key.modifiers.is_empty() => self.move_left(),
            KeyCode::Left => self.move_left(),
            KeyCode::Char('l') if key.modifiers.is_empty() => self.move_right(),
            KeyCode::Right => self.move_right(),
            KeyCode::Char('j') if key.modifiers.is_empty() => self.move_down(),
            KeyCode::Down => self.move_down(),
            KeyCode::Char('k') if key.modifiers.is_empty() => self.move_up(),
            KeyCode::Up => self.move_up(),
            KeyCode::Char('0') if key.modifiers.is_empty() => self.move_home(),
            KeyCode::Home => self.move_home(),
            KeyCode::Char('$') if key.modifiers.contains(KeyModifiers::SHIFT) => self.move_end(),
            KeyCode::End => self.move_end(),
            KeyCode::Char('w') if key.modifiers.is_empty() => self.move_word_forward(),
            KeyCode::Char('b') if key.modifiers.is_empty() => self.move_word_backward(),
            KeyCode::Char('x') if key.modifiers.is_empty() => self.delete(),
            KeyCode::Char('o') if key.modifiers.is_empty() => self.open_below(),
            KeyCode::Char('O') if key.modifiers.contains(KeyModifiers::SHIFT) => self.open_above(),
            _ => EditorOutcome::default(),
        }
    }

    fn backspace(&mut self) -> EditorOutcome {
        if self.cursor == 0 {
            return EditorOutcome::default();
        }

        let prev = self.prev_boundary(self.cursor);
        self.text.drain(prev..self.cursor);
        self.cursor = prev;
        self.preferred_col = None;
        EditorOutcome::changed()
    }

    fn delete(&mut self) -> EditorOutcome {
        if self.cursor >= self.text.len() {
            return EditorOutcome::default();
        }

        let next = self.next_boundary(self.cursor);
        self.text.drain(self.cursor..next);
        self.preferred_col = None;
        EditorOutcome::changed()
    }

    fn move_left(&mut self) -> EditorOutcome {
        if self.cursor == 0 {
            return EditorOutcome::default();
        }

        self.cursor = self.prev_boundary(self.cursor);
        self.preferred_col = None;
        EditorOutcome::moved()
    }

    fn move_right(&mut self) -> EditorOutcome {
        if self.cursor >= self.text.len() {
            return EditorOutcome::default();
        }

        self.cursor = self.next_boundary(self.cursor);
        self.preferred_col = None;
        EditorOutcome::moved()
    }

    fn move_up(&mut self) -> EditorOutcome {
        let current_start = self.line_start(self.cursor);
        if current_start == 0 {
            return EditorOutcome::default();
        }

        let desired_col = self.preferred_col.unwrap_or_else(|| self.current_col());
        let prev_end = current_start.saturating_sub(1);
        let prev_start = self.line_start(prev_end);
        self.cursor = self.line_col_to_index(prev_start, desired_col);
        self.preferred_col = Some(desired_col);
        EditorOutcome::moved()
    }

    fn move_down(&mut self) -> EditorOutcome {
        let current_end = self.line_end(self.cursor);
        if current_end >= self.text.len() {
            return EditorOutcome::default();
        }

        let desired_col = self.preferred_col.unwrap_or_else(|| self.current_col());
        let next_start = current_end + 1;
        self.cursor = self.line_col_to_index(next_start, desired_col);
        self.preferred_col = Some(desired_col);
        EditorOutcome::moved()
    }

    fn move_home(&mut self) -> EditorOutcome {
        let next = self.line_start(self.cursor);
        if next == self.cursor {
            return EditorOutcome::default();
        }
        self.cursor = next;
        self.preferred_col = None;
        EditorOutcome::moved()
    }

    fn move_first_non_whitespace(&mut self) -> EditorOutcome {
        let line_start = self.line_start(self.cursor);
        let line_end = self.line_end(self.cursor);
        let line = &self.text[line_start..line_end];
        let offset = line
            .char_indices()
            .find(|(_, ch)| !ch.is_whitespace())
            .map(|(idx, _)| idx)
            .unwrap_or(0);
        let next = line_start + offset;
        if next == self.cursor {
            return EditorOutcome::default();
        }
        self.cursor = next;
        self.preferred_col = None;
        EditorOutcome::moved()
    }

    fn move_end(&mut self) -> EditorOutcome {
        let next = self.line_end(self.cursor);
        if next == self.cursor {
            return EditorOutcome::default();
        }
        self.cursor = next;
        self.preferred_col = None;
        EditorOutcome::moved()
    }

    fn move_word_forward(&mut self) -> EditorOutcome {
        let mut idx = self.cursor;
        while idx < self.text.len() {
            let Some(ch) = self.char_at(idx) else {
                break;
            };
            if Self::is_word_char(ch) {
                idx = self.next_boundary(idx);
            } else {
                break;
            }
        }

        while idx < self.text.len() {
            let Some(ch) = self.char_at(idx) else {
                break;
            };
            if Self::is_word_char(ch) {
                break;
            }
            idx = self.next_boundary(idx);
        }

        if idx == self.cursor {
            return EditorOutcome::default();
        }

        self.cursor = idx;
        self.preferred_col = None;
        EditorOutcome::moved()
    }

    fn move_word_backward(&mut self) -> EditorOutcome {
        if self.cursor == 0 {
            return EditorOutcome::default();
        }

        let mut idx = self.prev_boundary(self.cursor);
        while idx > 0 {
            let Some(ch) = self.char_at(idx) else {
                break;
            };
            if Self::is_word_char(ch) {
                break;
            }
            idx = self.prev_boundary(idx);
        }

        while idx > 0 {
            let prev = self.prev_boundary(idx);
            let Some(ch) = self.char_at(prev) else {
                break;
            };
            if !Self::is_word_char(ch) {
                break;
            }
            idx = prev;
        }

        if idx == self.cursor {
            return EditorOutcome::default();
        }

        self.cursor = idx;
        self.preferred_col = None;
        EditorOutcome::moved()
    }

    fn open_below(&mut self) -> EditorOutcome {
        let line_end = self.line_end(self.cursor);
        let has_next_line = line_end < self.text.len();
        let insert_at = if has_next_line {
            line_end + 1
        } else {
            line_end
        };
        self.text.insert(insert_at, '\n');
        self.cursor = if has_next_line {
            insert_at
        } else {
            insert_at + 1
        };
        self.vim_mode = VimMode::Insert;
        self.preferred_col = None;
        EditorOutcome {
            handled: true,
            text_changed: true,
            cursor_moved: true,
            mode_changed: true,
        }
    }

    fn open_above(&mut self) -> EditorOutcome {
        let insert_at = self.line_start(self.cursor);
        self.text.insert(insert_at, '\n');
        self.cursor = insert_at;
        self.vim_mode = VimMode::Insert;
        self.preferred_col = None;
        EditorOutcome {
            handled: true,
            text_changed: true,
            cursor_moved: true,
            mode_changed: true,
        }
    }

    fn current_col(&self) -> usize {
        let line_start = self.line_start(self.cursor);
        self.text[line_start..self.cursor].chars().count()
    }

    fn line_start(&self, idx: usize) -> usize {
        self.text[..idx].rfind('\n').map(|pos| pos + 1).unwrap_or(0)
    }

    fn line_end(&self, idx: usize) -> usize {
        self.text[idx..]
            .find('\n')
            .map(|offset| idx + offset)
            .unwrap_or(self.text.len())
    }

    fn line_col_to_index(&self, line_start: usize, col: usize) -> usize {
        let line_end = self.line_end(line_start);
        let mut idx = line_start;
        let mut remaining = col;
        while idx < line_end && remaining > 0 {
            idx = self.next_boundary(idx);
            remaining -= 1;
        }
        idx
    }

    fn prev_boundary(&self, idx: usize) -> usize {
        self.text[..idx]
            .char_indices()
            .last()
            .map(|(offset, _)| offset)
            .unwrap_or(0)
    }

    fn next_boundary(&self, idx: usize) -> usize {
        if idx >= self.text.len() {
            return self.text.len();
        }

        self.text[idx..]
            .char_indices()
            .nth(1)
            .map(|(offset, _)| idx + offset)
            .unwrap_or(self.text.len())
    }

    fn char_at(&self, idx: usize) -> Option<char> {
        self.text[idx..].chars().next()
    }

    fn is_word_char(ch: char) -> bool {
        ch.is_alphanumeric() || ch == '_'
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn shift(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::SHIFT)
    }

    #[test]
    fn vim_insert_mode_switches_to_normal_on_escape() {
        let mut editor = TextEditor::with_vim("hello".to_string());

        assert_eq!(editor.vim_mode(), Some(VimMode::Insert));

        let outcome = editor.handle_key(key(KeyCode::Esc));

        assert!(outcome.mode_changed);
        assert_eq!(editor.vim_mode(), Some(VimMode::Normal));
        assert_eq!(editor.text(), "hello");
    }

    #[test]
    fn vim_normal_mode_supports_navigation_and_insert() {
        let mut editor = TextEditor::with_vim("alpha beta".to_string());
        editor.handle_key(key(KeyCode::Esc));

        editor.handle_key(key(KeyCode::Char('0')));
        editor.handle_key(key(KeyCode::Char('w')));
        editor.handle_key(key(KeyCode::Char('a')));
        editor.handle_key(key(KeyCode::Char('!')));

        assert_eq!(editor.vim_mode(), Some(VimMode::Insert));
        assert_eq!(editor.text(), "alpha b!eta");
    }

    #[test]
    fn vim_normal_mode_can_delete_and_open_lines() {
        let mut editor = TextEditor::with_vim("one\ntwo".to_string());
        editor.handle_key(key(KeyCode::Esc));
        editor.handle_key(key(KeyCode::Home));
        editor.handle_key(key(KeyCode::Up));
        editor.handle_key(key(KeyCode::Char('x')));
        assert_eq!(editor.text(), "ne\ntwo");

        editor.handle_key(key(KeyCode::Char('o')));
        assert_eq!(editor.vim_mode(), Some(VimMode::Insert));
        assert_eq!(editor.text(), "ne\n\ntwo");
    }

    #[test]
    fn cursor_row_col_tracks_multiline_positions() {
        let mut editor = TextEditor::with_vim("one\ntwo".to_string());
        editor.handle_key(key(KeyCode::Home));
        editor.handle_key(key(KeyCode::Down));
        editor.handle_key(key(KeyCode::Right));
        editor.handle_key(key(KeyCode::Right));

        assert_eq!(editor.cursor_row_col(), (1, 2));
    }

    #[test]
    fn shift_shortcuts_work_in_normal_mode() {
        let mut editor = TextEditor::with_vim("  hello\nworld".to_string());
        editor.handle_key(key(KeyCode::Esc));
        editor.handle_key(key(KeyCode::Home));
        editor.handle_key(key(KeyCode::Up));
        editor.handle_key(shift(KeyCode::Char('I')));
        editor.handle_key(key(KeyCode::Char('>')));
        editor.handle_key(key(KeyCode::Esc));
        editor.handle_key(shift(KeyCode::Char('A')));
        editor.handle_key(key(KeyCode::Char('!')));

        assert_eq!(editor.text(), "  >hello!\nworld");
    }

    #[test]
    fn toggle_vim_switches_between_plain_and_vim_insert() {
        let mut editor = TextEditor::new("hello".to_string());

        let outcome = editor.toggle_vim();
        assert!(outcome.mode_changed);
        assert_eq!(editor.keymap(), EditorKeymap::Vim);
        assert_eq!(editor.vim_mode(), Some(VimMode::Insert));

        let outcome = editor.toggle_vim();
        assert!(outcome.mode_changed);
        assert_eq!(editor.keymap(), EditorKeymap::Plain);
        assert_eq!(editor.vim_mode(), None);
    }
}
