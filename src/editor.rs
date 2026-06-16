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

/// A pending vim operator awaiting a motion or text object (the `d` in
/// `dw`). The framework is operator-generic; change still to follow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operator {
    Delete,
    Yank,
}

/// A resolved operator motion: either a charwise byte range or a
/// linewise span identified by byte positions on its first/last lines.
#[derive(Debug, Clone, Copy)]
enum OpTarget {
    Char(usize, usize),
    Line(usize, usize),
}

/// The unnamed register holding the most recent yank or delete. Named
/// registers are a later (Tier 3) feature.
#[derive(Debug, Clone, Default)]
struct Register {
    text: String,
    /// True when the contents represent whole lines (yanked/deleted
    /// linewise), which changes how `p`/`P` paste them.
    linewise: bool,
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

/// A point-in-time copy of the buffer used for undo/redo.
#[derive(Debug, Clone)]
struct Snapshot {
    text: String,
    cursor: usize,
}

/// Cap on history depth so pathological editing can't grow memory
/// without bound. Generous for prompt-sized buffers.
const MAX_HISTORY: usize = 1000;

#[derive(Debug, Clone)]
pub struct TextEditor {
    text: String,
    cursor: usize,
    preferred_col: Option<usize>,
    keymap: EditorKeymap,
    vim_mode: VimMode,
    /// States to restore on `u`, oldest first.
    undo_stack: Vec<Snapshot>,
    /// States to restore on `Ctrl-r`, in reverse-undo order.
    redo_stack: Vec<Snapshot>,
    /// Snapshot staged when an insert session begins; flushed into the
    /// undo stack by the first mutation so the whole session is one step.
    pending: Option<Snapshot>,
    /// Operator awaiting a motion (e.g. `d` pressed, waiting for `w`).
    pending_op: Option<Operator>,
    /// Unnamed register for yank/delete/paste.
    register: Register,
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
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            pending: None,
            pending_op: None,
            register: Register::default(),
        }
    }

    pub fn with_vim(text: String) -> Self {
        let mut editor = Self::new(text);
        editor.keymap = EditorKeymap::Vim;
        // The editor opens in insert mode; stage the initial state so the
        // first edits can be undone back to the original text.
        editor.arm_undo();
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
                // Entering vim lands in insert mode; stage the current
                // state so the session is undoable.
                self.arm_undo();
            }
            EditorKeymap::Vim => {
                self.keymap = EditorKeymap::Plain;
                self.vim_mode = VimMode::Insert;
                self.pending = None;
                self.pending_op = None;
            }
        }
        self.preferred_col = None;
        EditorOutcome::mode_changed()
    }

    fn current_snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.text.clone(),
            cursor: self.cursor,
        }
    }

    fn push_history(stack: &mut Vec<Snapshot>, snapshot: Snapshot) {
        if stack.len() >= MAX_HISTORY {
            stack.remove(0);
        }
        stack.push(snapshot);
    }

    /// Record the current state as an immediate, atomic undo step and
    /// invalidate the redo history. Used by one-shot normal-mode edits.
    fn push_undo_state(&mut self) {
        self.pending = None;
        let snapshot = self.current_snapshot();
        Self::push_history(&mut self.undo_stack, snapshot);
        self.redo_stack.clear();
    }

    /// Stage a snapshot for an upcoming insert session. The first
    /// mutation flushes it, so an entire session collapses to one undo
    /// step; if no mutation happens, the snapshot is discarded.
    fn arm_undo(&mut self) {
        self.pending = Some(self.current_snapshot());
    }

    /// Flush a staged insert-session snapshot. No-op when nothing is
    /// staged (plain mode, or a session past its first edit).
    fn commit_pending(&mut self) {
        if let Some(snapshot) = self.pending.take() {
            Self::push_history(&mut self.undo_stack, snapshot);
            self.redo_stack.clear();
        }
    }

    fn undo(&mut self) -> EditorOutcome {
        let Some(prev) = self.undo_stack.pop() else {
            return EditorOutcome::handled();
        };
        let snapshot = self.current_snapshot();
        Self::push_history(&mut self.redo_stack, snapshot);
        self.text = prev.text;
        self.cursor = prev.cursor.min(self.text.len());
        self.pending = None;
        self.preferred_col = None;
        EditorOutcome {
            handled: true,
            text_changed: true,
            cursor_moved: true,
            mode_changed: false,
        }
    }

    fn redo(&mut self) -> EditorOutcome {
        let Some(next) = self.redo_stack.pop() else {
            return EditorOutcome::handled();
        };
        let snapshot = self.current_snapshot();
        Self::push_history(&mut self.undo_stack, snapshot);
        self.text = next.text;
        self.cursor = next.cursor.min(self.text.len());
        self.pending = None;
        self.preferred_col = None;
        EditorOutcome {
            handled: true,
            text_changed: true,
            cursor_moved: true,
            mode_changed: false,
        }
    }

    pub fn insert_str(&mut self, text: &str) -> EditorOutcome {
        if text.is_empty() {
            return EditorOutcome::default();
        }

        self.commit_pending();
        self.text.insert_str(self.cursor, text);
        self.cursor += text.len();
        self.preferred_col = None;
        EditorOutcome::changed()
    }

    pub fn clear(&mut self) -> EditorOutcome {
        if self.text.is_empty() {
            return EditorOutcome::default();
        }

        self.push_undo_state();
        self.text.clear();
        self.cursor = 0;
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
                // Discard an unused insert-session snapshot so entering
                // insert and leaving without editing records no undo step.
                self.pending = None;
                EditorOutcome::mode_changed()
            }
            _ => self.handle_plain_key(key),
        }
    }

    fn handle_vim_normal_key(&mut self, key: KeyEvent) -> EditorOutcome {
        // A pending operator (e.g. `d`) consumes the next key as its
        // motion/text-object before any normal-mode binding runs.
        if let Some(op) = self.pending_op {
            return self.apply_pending_operator(op, key);
        }

        match key.code {
            KeyCode::Esc => EditorOutcome::handled(),
            KeyCode::Char('i') if key.modifiers.is_empty() => {
                self.arm_undo();
                self.vim_mode = VimMode::Insert;
                EditorOutcome::mode_changed()
            }
            KeyCode::Char('a') if key.modifiers.is_empty() => {
                self.arm_undo();
                let moved = self.move_right();
                self.vim_mode = VimMode::Insert;
                let mut outcome = EditorOutcome::mode_changed();
                outcome.cursor_moved = moved.cursor_moved;
                outcome
            }
            KeyCode::Char('A') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.arm_undo();
                let moved = self.move_end();
                self.vim_mode = VimMode::Insert;
                let mut outcome = EditorOutcome::mode_changed();
                outcome.cursor_moved = moved.cursor_moved;
                outcome
            }
            KeyCode::Char('I') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.arm_undo();
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
            KeyCode::Char('e') if key.modifiers.is_empty() => self.move_word_end(),
            KeyCode::Char('d') if key.modifiers.is_empty() => {
                self.pending_op = Some(Operator::Delete);
                EditorOutcome::handled()
            }
            KeyCode::Char('y') if key.modifiers.is_empty() => {
                self.pending_op = Some(Operator::Yank);
                EditorOutcome::handled()
            }
            KeyCode::Char('D') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                let end = self.line_end(self.cursor);
                self.apply_operator(Operator::Delete, OpTarget::Char(self.cursor, end))
            }
            KeyCode::Char('Y') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.apply_operator(Operator::Yank, OpTarget::Line(self.cursor, self.cursor))
            }
            KeyCode::Char('x') if key.modifiers.is_empty() => {
                // `x` is `dl`: delete the character under the cursor,
                // populating the register, as a single undo step.
                let end = self.next_boundary(self.cursor);
                self.apply_operator(Operator::Delete, OpTarget::Char(self.cursor, end))
            }
            KeyCode::Char('p') if key.modifiers.is_empty() => self.paste_after(),
            KeyCode::Char('P') if key.modifiers.contains(KeyModifiers::SHIFT) => self.paste_before(),
            KeyCode::Char('u') if key.modifiers.is_empty() => self.undo(),
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => self.redo(),
            KeyCode::Char('o') if key.modifiers.is_empty() => self.open_below(),
            KeyCode::Char('O') if key.modifiers.contains(KeyModifiers::SHIFT) => self.open_above(),
            _ => EditorOutcome::default(),
        }
    }

    fn backspace(&mut self) -> EditorOutcome {
        if self.cursor == 0 {
            return EditorOutcome::default();
        }

        self.commit_pending();
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

        self.commit_pending();
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
        let next = self.first_non_blank_of_line(self.cursor);
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
        let idx = self.word_forward_index(self.cursor);
        if idx == self.cursor {
            return EditorOutcome::default();
        }
        self.cursor = idx;
        self.preferred_col = None;
        EditorOutcome::moved()
    }

    fn move_word_backward(&mut self) -> EditorOutcome {
        let idx = self.word_backward_index(self.cursor);
        if idx == self.cursor {
            return EditorOutcome::default();
        }
        self.cursor = idx;
        self.preferred_col = None;
        EditorOutcome::moved()
    }

    fn move_word_end(&mut self) -> EditorOutcome {
        let idx = self.word_end_index(self.cursor);
        if idx == self.cursor {
            return EditorOutcome::default();
        }
        self.cursor = idx;
        self.preferred_col = None;
        EditorOutcome::moved()
    }

    /// Index of the start of the next word at or after `from`.
    fn word_forward_index(&self, from: usize) -> usize {
        let mut idx = from;
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
        idx
    }

    /// Index of the start of the word at or before `from`.
    fn word_backward_index(&self, from: usize) -> usize {
        if from == 0 {
            return 0;
        }
        let mut idx = self.prev_boundary(from);
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
        idx
    }

    /// Index of the last character of the next word after `from`
    /// (the `e` motion target — points *at* that character).
    fn word_end_index(&self, from: usize) -> usize {
        let len = self.text.len();
        if from >= len {
            return from;
        }
        let mut idx = self.next_boundary(from);
        while idx < len {
            let Some(ch) = self.char_at(idx) else {
                break;
            };
            if Self::is_word_char(ch) {
                break;
            }
            idx = self.next_boundary(idx);
        }
        while idx < len {
            let next = self.next_boundary(idx);
            match self.char_at(next) {
                Some(ch) if next < len && Self::is_word_char(ch) => idx = next,
                _ => break,
            }
        }
        idx
    }

    /// Index of the first non-whitespace char on the line containing
    /// `idx`, or the line start if the line is blank.
    fn first_non_blank_of_line(&self, idx: usize) -> usize {
        let line_start = self.line_start(idx);
        let line_end = self.line_end(idx);
        let line = &self.text[line_start..line_end];
        let offset = line
            .char_indices()
            .find(|(_, ch)| !ch.is_whitespace())
            .map(|(i, _)| i)
            .unwrap_or(0);
        line_start + offset
    }

    /// Full linewise byte range covering the lines at `a` and `b`,
    /// including one trailing newline (or the leading one on the last
    /// line) so the lines are removed cleanly.
    fn linewise_range(&self, a: usize, b: usize) -> (usize, usize) {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        let start = self.line_start(lo);
        let end = self.line_end(hi);
        if end < self.text.len() {
            (start, end + 1)
        } else if start > 0 {
            (start - 1, end)
        } else {
            (start, end)
        }
    }

    /// Resolve a charwise operator target for `key`, returning the byte
    /// range `[start, end)` to operate on, or None when `key` is not a
    /// supported charwise motion.
    fn charwise_op_range(&self, key: KeyEvent) -> Option<(usize, usize)> {
        let c = self.cursor;
        match key.code {
            KeyCode::Char('w') if key.modifiers.is_empty() => Some((c, self.word_forward_index(c))),
            KeyCode::Char('b') if key.modifiers.is_empty() => Some((self.word_backward_index(c), c)),
            KeyCode::Char('e') if key.modifiers.is_empty() => {
                // Inclusive: extend past the end-of-word character.
                Some((c, self.next_boundary(self.word_end_index(c))))
            }
            KeyCode::Char('$') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                Some((c, self.line_end(c)))
            }
            KeyCode::Char('0') if key.modifiers.is_empty() => Some((self.line_start(c), c)),
            KeyCode::Char('h') if key.modifiers.is_empty() => Some((self.prev_boundary(c), c)),
            KeyCode::Left => Some((self.prev_boundary(c), c)),
            KeyCode::Char('l') if key.modifiers.is_empty() => Some((c, self.next_boundary(c))),
            KeyCode::Right => Some((c, self.next_boundary(c))),
            _ => None,
        }
    }

    /// Resolve and apply an operator's motion (the second key of `dw`,
    /// `dd`, etc.). Esc or an unsupported key cancels the operator.
    fn apply_pending_operator(&mut self, op: Operator, key: KeyEvent) -> EditorOutcome {
        if key.code == KeyCode::Esc {
            self.pending_op = None;
            return EditorOutcome::handled();
        }
        self.pending_op = None;

        // A doubled operator (`dd`, `yy`) acts linewise on the current
        // line.
        let doubled_key = match op {
            Operator::Delete => KeyCode::Char('d'),
            Operator::Yank => KeyCode::Char('y'),
        };
        let doubled = key.code == doubled_key && key.modifiers.is_empty();

        let down = matches!(key.code, KeyCode::Down)
            || (key.code == KeyCode::Char('j') && key.modifiers.is_empty());
        let up = matches!(key.code, KeyCode::Up)
            || (key.code == KeyCode::Char('k') && key.modifiers.is_empty());

        let target = if doubled {
            Some(OpTarget::Line(self.cursor, self.cursor))
        } else if down {
            let line_end = self.line_end(self.cursor);
            (line_end < self.text.len()).then(|| OpTarget::Line(self.cursor, line_end + 1))
        } else if up {
            let line_start = self.line_start(self.cursor);
            (line_start > 0).then(|| OpTarget::Line(line_start - 1, self.cursor))
        } else {
            self.charwise_op_range(key)
                .map(|(start, end)| OpTarget::Char(start, end))
        };

        match target {
            Some(target) => self.apply_operator(op, target),
            // Unsupported motion: consume the key, no change.
            None => EditorOutcome::handled(),
        }
    }

    /// Apply an operator to a resolved target: copy the spanned text to
    /// the register, then delete it (Delete) or leave it (Yank).
    fn apply_operator(&mut self, op: Operator, target: OpTarget) -> EditorOutcome {
        let (del_start, del_end, content, linewise, cursor_at) = match target {
            OpTarget::Char(start, end) => {
                if start >= end {
                    // Empty/no-op motion: don't touch the register, undo,
                    // or redo history.
                    return EditorOutcome::handled();
                }
                (start, end, self.text[start..end].to_string(), false, start)
            }
            OpTarget::Line(a, b) => {
                let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
                let content_start = self.line_start(lo);
                let content_end = self.line_end(hi);
                // Normalize to whole lines with a single trailing newline,
                // independent of how the deletion range treats newlines.
                let mut content = self.text[content_start..content_end].to_string();
                content.push('\n');
                let (del_start, del_end) = self.linewise_range(lo, hi);
                (del_start, del_end, content, true, del_start)
            }
        };

        self.register = Register {
            text: content,
            linewise,
        };

        match op {
            Operator::Yank => {
                // Yank leaves the buffer unchanged. A charwise yank moves
                // the cursor to the start of the range (matching vim for
                // backward motions); a linewise yank keeps the cursor.
                if linewise {
                    return EditorOutcome::handled();
                }
                self.cursor = cursor_at.min(self.text.len());
                self.preferred_col = None;
                EditorOutcome::moved()
            }
            Operator::Delete => {
                self.push_undo_state();
                self.text.drain(del_start..del_end);
                self.cursor = cursor_at.min(self.text.len());
                if linewise {
                    self.cursor = self.first_non_blank_of_line(self.cursor);
                }
                self.preferred_col = None;
                EditorOutcome {
                    handled: true,
                    text_changed: true,
                    cursor_moved: true,
                    mode_changed: false,
                }
            }
        }
    }

    fn paste_after(&mut self) -> EditorOutcome {
        if self.register.text.is_empty() {
            return EditorOutcome::handled();
        }
        if self.register.linewise {
            self.paste_linewise(false)
        } else {
            let insert_at = if self.cursor < self.text.len() {
                self.next_boundary(self.cursor)
            } else {
                self.cursor
            };
            self.paste_charwise(insert_at)
        }
    }

    fn paste_before(&mut self) -> EditorOutcome {
        if self.register.text.is_empty() {
            return EditorOutcome::handled();
        }
        if self.register.linewise {
            self.paste_linewise(true)
        } else {
            self.paste_charwise(self.cursor)
        }
    }

    fn paste_charwise(&mut self, insert_at: usize) -> EditorOutcome {
        self.push_undo_state();
        let text = self.register.text.clone();
        let end = insert_at + text.len();
        self.text.insert_str(insert_at, &text);
        // Cursor on the last character of the pasted text (vim).
        self.cursor = self.prev_boundary(end);
        self.preferred_col = None;
        EditorOutcome {
            handled: true,
            text_changed: true,
            cursor_moved: true,
            mode_changed: false,
        }
    }

    fn paste_linewise(&mut self, before: bool) -> EditorOutcome {
        self.push_undo_state();
        let content = self.register.text.clone(); // ends with '\n'
        let line_start = if before {
            let insert_at = self.line_start(self.cursor);
            self.text.insert_str(insert_at, &content);
            insert_at
        } else {
            let line_end = self.line_end(self.cursor);
            if line_end < self.text.len() {
                let insert_at = line_end + 1;
                self.text.insert_str(insert_at, &content);
                insert_at
            } else {
                // Last line has no trailing newline: add one, then the
                // content without its own trailing newline.
                let trimmed = content.trim_end_matches('\n');
                let insert_at = self.text.len();
                self.text.insert(insert_at, '\n');
                self.text.insert_str(insert_at + 1, trimmed);
                insert_at + 1
            }
        };
        self.cursor = self.first_non_blank_of_line(line_start);
        self.preferred_col = None;
        EditorOutcome {
            handled: true,
            text_changed: true,
            cursor_moved: true,
            mode_changed: false,
        }
    }

    fn open_below(&mut self) -> EditorOutcome {
        self.push_undo_state();
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
        self.push_undo_state();
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

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
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
    fn undo_and_redo_restore_delete() {
        let mut editor = TextEditor::with_vim("hello".to_string());
        editor.handle_key(key(KeyCode::Esc));
        editor.handle_key(key(KeyCode::Home));
        editor.handle_key(key(KeyCode::Char('x')));
        assert_eq!(editor.text(), "ello");

        let outcome = editor.handle_key(key(KeyCode::Char('u')));
        assert!(outcome.text_changed);
        assert_eq!(editor.text(), "hello");

        let outcome = editor.handle_key(ctrl(KeyCode::Char('r')));
        assert!(outcome.text_changed);
        assert_eq!(editor.text(), "ello");
    }

    #[test]
    fn undo_collapses_a_whole_insert_session() {
        let mut editor = TextEditor::with_vim("ab".to_string());
        editor.handle_key(key(KeyCode::Esc));
        // Append " cd" via an insert session entered with `a`.
        editor.handle_key(key(KeyCode::Char('a')));
        editor.handle_key(key(KeyCode::Char(' ')));
        editor.handle_key(key(KeyCode::Char('c')));
        editor.handle_key(key(KeyCode::Char('d')));
        editor.handle_key(key(KeyCode::Esc));
        assert_eq!(editor.text(), "ab cd");

        editor.handle_key(key(KeyCode::Char('u')));
        assert_eq!(editor.text(), "ab");
    }

    #[test]
    fn undo_reverts_open_below_and_its_typed_text() {
        let mut editor = TextEditor::with_vim("one".to_string());
        editor.handle_key(key(KeyCode::Esc));
        editor.handle_key(key(KeyCode::Char('o')));
        editor.handle_key(key(KeyCode::Char('t')));
        editor.handle_key(key(KeyCode::Char('w')));
        editor.handle_key(key(KeyCode::Esc));
        assert_eq!(editor.text(), "one\ntw");

        editor.handle_key(key(KeyCode::Char('u')));
        assert_eq!(editor.text(), "one");
    }

    #[test]
    fn entering_insert_without_editing_records_no_undo_step() {
        let mut editor = TextEditor::with_vim("hi".to_string());
        editor.handle_key(key(KeyCode::Esc));
        editor.handle_key(key(KeyCode::Home));
        // Make one real edit so there is a baseline undo step.
        editor.handle_key(key(KeyCode::Char('x')));
        assert_eq!(editor.text(), "i");
        // Enter and leave insert without typing.
        editor.handle_key(key(KeyCode::Char('i')));
        editor.handle_key(key(KeyCode::Esc));
        // A single undo should jump past the empty session to the edit.
        editor.handle_key(key(KeyCode::Char('u')));
        assert_eq!(editor.text(), "hi");
    }

    #[test]
    fn new_edit_clears_redo_history() {
        let mut editor = TextEditor::with_vim("hello".to_string());
        editor.handle_key(key(KeyCode::Esc));
        editor.handle_key(key(KeyCode::Home));
        editor.handle_key(key(KeyCode::Char('x')));
        editor.handle_key(key(KeyCode::Char('u')));
        assert_eq!(editor.text(), "hello");

        // A fresh edit invalidates the redo we just created.
        editor.handle_key(key(KeyCode::Char('x')));
        assert_eq!(editor.text(), "ello");
        let outcome = editor.handle_key(ctrl(KeyCode::Char('r')));
        assert!(!outcome.text_changed);
        assert_eq!(editor.text(), "ello");
    }

    /// Put a fresh vim editor into normal mode with the cursor at the
    /// very start of the buffer. (`with_vim` opens with the cursor at the
    /// end, so we walk up to the top line before going home.)
    fn normal_at_start(text: &str) -> TextEditor {
        let mut editor = TextEditor::with_vim(text.to_string());
        editor.handle_key(key(KeyCode::Esc));
        for _ in 0..text.lines().count().max(1) {
            editor.handle_key(key(KeyCode::Up));
        }
        editor.handle_key(key(KeyCode::Home));
        editor
    }

    #[test]
    fn delete_word_with_dw() {
        let mut editor = normal_at_start("hello world");
        editor.handle_key(key(KeyCode::Char('d')));
        let outcome = editor.handle_key(key(KeyCode::Char('w')));
        assert!(outcome.text_changed);
        assert_eq!(editor.text(), "world");
        assert_eq!(editor.cursor(), 0);
    }

    #[test]
    fn delete_to_end_of_word_with_de_is_inclusive() {
        let mut editor = normal_at_start("hello world");
        editor.handle_key(key(KeyCode::Char('d')));
        editor.handle_key(key(KeyCode::Char('e')));
        assert_eq!(editor.text(), " world");
    }

    #[test]
    fn delete_back_with_db() {
        let mut editor = normal_at_start("hello world");
        // Move to the start of "world" (index 6).
        editor.handle_key(key(KeyCode::Char('w')));
        editor.handle_key(key(KeyCode::Char('d')));
        editor.handle_key(key(KeyCode::Char('b')));
        assert_eq!(editor.text(), "world");
    }

    #[test]
    fn delete_to_line_end_with_d_dollar_and_shift_d() {
        let mut editor = normal_at_start("hello world");
        editor.handle_key(key(KeyCode::Char('w'))); // cursor at "world"
        editor.handle_key(key(KeyCode::Char('d')));
        editor.handle_key(shift(KeyCode::Char('$')));
        assert_eq!(editor.text(), "hello ");

        let mut editor = normal_at_start("hello world");
        editor.handle_key(shift(KeyCode::Char('D')));
        assert_eq!(editor.text(), "");
    }

    #[test]
    fn delete_line_with_dd() {
        let mut editor = normal_at_start("one\ntwo\nthree");
        editor.handle_key(key(KeyCode::Down)); // cursor on "two"
        editor.handle_key(key(KeyCode::Char('d')));
        editor.handle_key(key(KeyCode::Char('d')));
        assert_eq!(editor.text(), "one\nthree");
    }

    #[test]
    fn delete_two_lines_with_dj() {
        let mut editor = normal_at_start("a\nb\nc");
        editor.handle_key(key(KeyCode::Char('d')));
        editor.handle_key(key(KeyCode::Char('j')));
        assert_eq!(editor.text(), "c");
    }

    #[test]
    fn operator_is_cancelled_by_escape() {
        let mut editor = normal_at_start("hello world");
        editor.handle_key(key(KeyCode::Char('d')));
        editor.handle_key(key(KeyCode::Esc));
        // The pending `d` is gone; a plain `w` now just moves.
        editor.handle_key(key(KeyCode::Char('w')));
        assert_eq!(editor.text(), "hello world");
        assert_eq!(editor.cursor(), 6);
    }

    #[test]
    fn delete_can_be_undone_as_one_step() {
        let mut editor = normal_at_start("hello world");
        editor.handle_key(key(KeyCode::Char('d')));
        editor.handle_key(key(KeyCode::Char('w')));
        assert_eq!(editor.text(), "world");
        editor.handle_key(key(KeyCode::Char('u')));
        assert_eq!(editor.text(), "hello world");
    }

    #[test]
    fn e_moves_to_end_of_word() {
        let mut editor = normal_at_start("hello world");
        let outcome = editor.handle_key(key(KeyCode::Char('e')));
        assert!(outcome.cursor_moved);
        assert_eq!(editor.cursor(), 4); // the second 'l'... 'o' of hello
    }

    #[test]
    fn yank_word_and_paste_after() {
        let mut editor = normal_at_start("hello world");
        editor.handle_key(key(KeyCode::Char('y')));
        editor.handle_key(key(KeyCode::Char('w'))); // yank "hello "
        editor.handle_key(key(KeyCode::Char('p'))); // paste after cursor
        assert_eq!(editor.text(), "hhello ello world");
    }

    #[test]
    fn paste_before_with_capital_p() {
        let mut editor = normal_at_start("ab");
        editor.handle_key(key(KeyCode::Char('y')));
        editor.handle_key(key(KeyCode::Char('l'))); // yank "a"
        editor.handle_key(shift(KeyCode::Char('P'))); // paste before cursor
        assert_eq!(editor.text(), "aab");
    }

    #[test]
    fn delete_then_paste_charwise() {
        let mut editor = normal_at_start("hello world");
        editor.handle_key(key(KeyCode::Char('x'))); // delete 'h' into register
        assert_eq!(editor.text(), "ello world");
        editor.handle_key(key(KeyCode::Char('p'))); // paste after cursor
        assert_eq!(editor.text(), "ehllo world");
    }

    #[test]
    fn yank_line_and_paste_below() {
        let mut editor = normal_at_start("one\ntwo");
        editor.handle_key(key(KeyCode::Char('y')));
        editor.handle_key(key(KeyCode::Char('y'))); // yank "one\n"
        editor.handle_key(key(KeyCode::Char('p'))); // paste on the line below
        assert_eq!(editor.text(), "one\none\ntwo");
    }

    #[test]
    fn capital_y_yanks_line_and_capital_p_pastes_above() {
        let mut editor = normal_at_start("one\ntwo");
        editor.handle_key(shift(KeyCode::Char('Y'))); // yank current line
        editor.handle_key(shift(KeyCode::Char('P'))); // paste above
        assert_eq!(editor.text(), "one\none\ntwo");
    }

    #[test]
    fn delete_line_then_paste_below() {
        let mut editor = normal_at_start("one\ntwo\nthree");
        editor.handle_key(key(KeyCode::Char('d')));
        editor.handle_key(key(KeyCode::Char('d'))); // delete "one" line
        assert_eq!(editor.text(), "two\nthree");
        editor.handle_key(key(KeyCode::Char('p'))); // paste it below "two"
        assert_eq!(editor.text(), "two\none\nthree");
    }

    #[test]
    fn linewise_paste_below_on_last_line_adds_newline() {
        let mut editor = normal_at_start("one\ntwo");
        editor.handle_key(key(KeyCode::Char('y')));
        editor.handle_key(key(KeyCode::Char('y'))); // yank "one\n"
        editor.handle_key(key(KeyCode::Down)); // move to last line "two"
        editor.handle_key(key(KeyCode::Char('p'))); // paste below last line
        assert_eq!(editor.text(), "one\ntwo\none");
    }

    #[test]
    fn paste_with_empty_register_is_a_noop() {
        let mut editor = normal_at_start("abc");
        let outcome = editor.handle_key(key(KeyCode::Char('p')));
        assert!(!outcome.text_changed);
        assert_eq!(editor.text(), "abc");
    }

    #[test]
    fn undo_on_empty_history_is_a_noop() {
        let mut editor = TextEditor::with_vim("hello".to_string());
        editor.handle_key(key(KeyCode::Esc));
        let outcome = editor.handle_key(key(KeyCode::Char('u')));
        assert!(!outcome.text_changed);
        assert_eq!(editor.text(), "hello");
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
