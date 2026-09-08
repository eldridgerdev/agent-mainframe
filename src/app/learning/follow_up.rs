use crate::app::{
    App, AppMode, LearningAnchor, LearningAnchorDrift, LearningLevel, LearningQa, LearningQaIntent,
    Selection, StartIntent,
};

// ── making an answer actionable ──────────────────────────────

/// Longest seeded TODO title, in characters. Long enough for a real imperative
/// sentence, short enough that the TODOs list still reads as a list.
pub(super) const MAX_TODO_TITLE: usize = 80;

/// Lines of the answer carried into the TODO body. The whole answer would bury
/// the item it is attached to; this is enough to recognise what it was about,
/// and the answer itself stays in Learning Mode either way.
pub(super) const MAX_TODO_ANSWER_LINES: usize = 12;

impl App {
    /// Offer to turn the selected answer into an item on the project's TODO
    /// list.
    ///
    /// Nothing is written by this key — it opens a confirmation carrying an
    /// editable title and the note that would be added. That is deliberate:
    /// the seeded title is a guess (see [`todo_title_seed`]), and a mode whose
    /// whole promise is "this changes nothing" cannot start writing on a
    /// single keypress.
    ///
    /// An entry that has already been made actionable jumps to the item it
    /// produced rather than making a second one — except when that item has
    /// since been deleted from the TODOs overlay, where the dead link is
    /// cleared and a fresh note offered instead of jumping into an empty list.
    pub fn learning_make_actionable(&mut self) {
        let Some(qa) = (match &self.mode {
            AppMode::Learning(state) => state.qa.get(state.selected_qa).cloned(),
            _ => return,
        }) else {
            self.learning_error(
                "Ask something first — this keeps an answer you already have as a to-do note.",
            );
            return;
        };

        // The note *is* the answer, so there has to be one.
        if qa.answer.is_none() {
            self.learning_error(match qa.status {
                crate::app::LearningQaStatus::Failed => {
                    "That question never got an answer to keep. Ask it again first."
                }
                _ => "That answer is still generating — you can keep it once it arrives.",
            });
            return;
        }

        let mut replacing_deleted = false;
        if let Some(todo_id) = qa.todo_id.clone() {
            if self.learning_jump_to_todo(&todo_id) {
                return;
            }
            // The item is gone from the list, so the marker on this row is a
            // promise the TODOs overlay can no longer keep. Drop it and let the
            // user write a new one, saying which of the two happened.
            if let AppMode::Learning(state) = &mut self.mode
                && let Some(row) = state.qa.iter_mut().find(|r| r.id == qa.id)
            {
                row.todo_id = None;
                row.updated_at = crate::db::learning::now_timestamp();
            }
            let _ = self.persist_learning_qa_by_id(&qa.id);
            replacing_deleted = true;
        }

        // Without a database there is no TODO list to add to — and unlike the
        // Q&A history, an in-memory item would not even be visible from the
        // dashboard, so pretending would be worse than refusing.
        if self.db.is_none() {
            self.learning_error(
                "AMF can't reach its database, so there's no TODO list to add to. Your questions and answers still work.",
            );
            return;
        }

        let drift = match &self.mode {
            AppMode::Learning(state) => state.drift_for(&qa.id),
            _ => None,
        };
        if let AppMode::Learning(state) = &mut self.mode {
            state.error = None;
            state.clear_notice();
            // The answer pane stays open behind the dialog, which draws over
            // it. Keeping an answer is not the start of something else the way
            // a follow-up or a deep dive is — you are still reading it, and the
            // confirmation banner lands inside the pane where you are.
            state.action_editor = Some(crate::app::LearningActionEditor {
                qa_id: qa.id.clone(),
                title: crate::editor::TextEditor::new(todo_title_seed(&qa)),
                body: todo_body(&qa, drift),
                error: replacing_deleted
                    .then(|| "The item this was on has been deleted — this adds a new one.".into()),
                scroll: 0,
                sync_to_cursor: true,
            });
        }
    }
}

impl App {
    /// Open the TODOs overlay with `todo_id` under the cursor. `false` when the
    /// item can't be found, which is the caller's cue that the link is stale.
    pub(super) fn learning_jump_to_todo(&mut self, todo_id: &str) -> bool {
        let (pi, fi) = match &self.mode {
            AppMode::Learning(state) => (state.pi, state.fi),
            _ => return false,
        };
        if self.db.is_none() {
            return false;
        }

        // Open the editor first and look for the item across every pane it
        // loaded, rather than guessing which scope the note was kept in: an
        // older keep landed in the project list, a new one in the worktree
        // list, and a moved one could be in either.
        let restore = std::mem::replace(&mut self.mode, AppMode::Normal);
        if let Err(e) = self.open_todos_view(pi, fi) {
            self.log_warn("learning", format!("couldn't open the TODO list: {e}"));
            self.mode = restore;
            return false;
        }
        let found = match &self.mode {
            AppMode::Todos(state) => state.panes.iter().enumerate().find_map(|(p, pane)| {
                pane.todos
                    .iter()
                    .position(|t| t.id == todo_id)
                    .map(|i| (p, i, pane.scope.clone()))
            }),
            _ => None,
        };
        let Some((pane_index, index, scope)) = found else {
            // The link is stale: put the reader back where they were rather
            // than dropping them into a list that does not hold the item.
            self.mode = restore;
            return false;
        };
        // Following a direct link to a hidden scope makes that scope visible
        // process-wide so the selected TODO can actually be shown.
        self.set_todo_scope_visibility(&scope, true);
        if let AppMode::Todos(state) = &mut self.mode {
            state.focus = Some(pane_index);
            if let Some(pane) = state.panes.get_mut(pane_index) {
                pane.selected = index;
            }
        }
        // The screen has just changed out from under a keypress that looked
        // like it would add something, so it has to say why it didn't.
        self.push_toast_info("You already kept that one — here it is on the TODO list.");
        true
    }
}

impl App {
    /// Close the confirmation without writing anything.
    pub fn learning_cancel_action(&mut self) {
        if let AppMode::Learning(state) = &mut self.mode {
            state.action_editor = None;
        }
    }
}

impl App {
    /// Write the confirmed note to the project's TODO list and link it back to
    /// the answer it came from. Returns the new `todos.id`.
    ///
    /// The list is reached the way quick-capture reaches it — by ensuring the
    /// project has a TODOs *session* as well as a list. A list with no session
    /// row is invisible from the dashboard, so a note written into one would be
    /// a note the user can never find again.
    pub fn learning_confirm_action(&mut self) -> Option<String> {
        let (qa_id, title, body) = match &self.mode {
            AppMode::Learning(state) => {
                let editor = state.action_editor.as_ref()?;
                (
                    editor.qa_id.clone(),
                    editor.title.text().trim().to_string(),
                    editor.body.clone(),
                )
            }
            _ => return None,
        };
        if title.is_empty() {
            // Refused inside the dialog: it covers the overlay's banner line,
            // so a refusal raised out there would be invisible from in here.
            if let AppMode::Learning(state) = &mut self.mode
                && let Some(editor) = &mut state.action_editor
            {
                editor.error =
                    Some("Give it a title first — this is what you'll see later.".into());
            }
            return None;
        }

        let (pi, fi) = match &self.mode {
            AppMode::Learning(state) => (state.pi, state.fi),
            _ => return None,
        };
        let has_session = self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
            .is_some_and(|f| f.has_todos_session());
        if !has_session && let Err(e) = self.add_todos_session_for_picker(pi, fi, None) {
            self.log_error("learning", format!("couldn't create a TODOs session: {e}"));
            self.learning_cancel_action();
            self.learning_error(format!(
                "Couldn't start a TODO list — nothing was written: {e}"
            ));
            return None;
        }

        // The same target quick-capture uses: the feature's own worktree list,
        // or the project's when it sits on the repo root. A note kept while
        // reading a checkout belongs to that checkout's work.
        let scope = self.default_todo_scope(pi, fi)?;
        let project = self.store.projects.get(pi)?;
        let feature_id = project.features.get(fi).map(|f| f.id.clone());

        let written = self.db.as_ref().map(|db| {
            db.load_or_create_todo_list(&scope, feature_id.as_deref())
                .and_then(|list| {
                    db.add_todo(
                        &list.id,
                        &title,
                        Some(&body),
                        crate::db::todos::TodoPriority::Med,
                    )
                })
        });
        let todo = match written {
            Some(Ok(todo)) => todo,
            Some(Err(e)) => {
                self.log_error("learning", format!("couldn't add the TODO: {e}"));
                self.learning_cancel_action();
                self.learning_error(format!(
                    "Couldn't add it to the TODO list — nothing was written: {e}"
                ));
                return None;
            }
            // Refused up front in `learning_make_actionable`; belt and braces.
            None => return None,
        };

        self.learning_cancel_action();
        if let AppMode::Learning(state) = &mut self.mode
            && let Some(row) = state.qa.iter_mut().find(|r| r.id == qa_id)
        {
            row.todo_id = Some(todo.id.clone());
            row.updated_at = crate::db::learning::now_timestamp();
        }
        self.log_info(
            "learning",
            format!("kept an answer as TODO {} ({title})", todo.id),
        );

        // The item exists either way, so a failed link is reported as the
        // partial success it is rather than rolled back — undoing it would mean
        // deleting a note the user just watched being added.
        match self.persist_learning_qa_by_id(&qa_id) {
            Ok(()) => self.learning_notice_for_qa(
                &qa_id,
                "Kept on this project's TODO list — a note about your code, not a change to it.",
            ),
            Err(e) => self.learning_error(format!(
                "Added to the TODO list, but the link back to this answer wasn't saved: {e}"
            )),
        }
        Some(todo.id)
    }
}

/// The title a new TODO is seeded with.
///
/// An `Action` answer is written to lead with a one-line imperative summary
/// (see `intent_instructions`), so its first line is the title. An `Explain`
/// answer has no such line, and the plan accepts that the seed there is a
/// truncation the user is expected to fix — which is why nothing is written
/// until they confirm. An answer that opens with nothing usable falls back to
/// the question, which at least names the subject.
pub fn todo_title_seed(qa: &LearningQa) -> String {
    let seed = qa
        .answer
        .as_deref()
        .and_then(first_meaningful_line)
        .or_else(|| first_meaningful_line(&qa.question))
        .unwrap_or_else(|| "Learning Mode note".to_string());
    truncate_title(&seed, MAX_TODO_TITLE)
}

/// The first line of `text` that carries words, stripped of the markdown
/// decoration an answer's opening line usually wears.
///
/// Answers are rendered as markdown in the answer pane, but a TODO title is
/// shown raw — so a heading's `##` or a bullet's `-` would be read as part of
/// the sentence.
pub(super) fn first_meaningful_line(text: &str) -> Option<String> {
    text.lines()
        .map(strip_markdown_decoration)
        .find(|line| !line.is_empty())
}

/// Strip leading heading/quote/bullet markers and surrounding emphasis from one
/// line, leaving the sentence inside.
pub(super) fn strip_markdown_decoration(line: &str) -> String {
    let mut rest = line.trim();
    // A fenced block's delimiter is decoration with nothing behind it.
    if rest.starts_with("```") || rest.starts_with("~~~") {
        return String::new();
    }
    loop {
        let before = rest;
        rest = rest.trim_start_matches(['#', '>']).trim_start();
        // A bullet marker is only a marker when a space follows it — otherwise
        // `**Split this**` loses its emphasis to the list rule and then no
        // longer looks like a matched pair.
        if let Some(tail) = rest
            .strip_prefix(['-', '*', '+'])
            .filter(|tail| tail.starts_with(char::is_whitespace))
        {
            rest = tail.trim_start();
        }
        // An ordered-list marker: digits, then `.` or `)`, then a space — the
        // space is what separates `1. Do this` from a sentence opening with a
        // decimal number like `12.5 seconds is the default`.
        if let Some(tail) = rest
            .split_once(['.', ')'])
            .filter(|(head, tail)| {
                !head.is_empty()
                    && head.chars().all(|c| c.is_ascii_digit())
                    && (tail.is_empty() || tail.starts_with(char::is_whitespace))
            })
            .map(|(_, tail)| tail.trim_start())
        {
            rest = tail;
        }
        // Emphasis wraps the sentence rather than leading it, so it comes off
        // both ends at once — `**Split this function**` is a title, while
        // `**bold** start` is a sentence that happens to begin with emphasis.
        for marker in ["**", "__", "*", "_", "`"] {
            if let Some(inner) = rest
                .strip_prefix(marker)
                .and_then(|r| r.strip_suffix(marker))
                && !inner.is_empty()
            {
                rest = inner.trim();
                break;
            }
        }
        if rest == before {
            break;
        }
    }
    rest.trim_end_matches(':').trim().to_string()
}

/// Cut a seeded title to `max` characters at a word boundary where there is
/// one, marking that it was cut.
pub(super) fn truncate_title(title: &str, max: usize) -> String {
    if title.chars().count() <= max {
        return title.to_string();
    }
    let head: String = title.chars().take(max).collect();
    let cut = match head.rsplit_once(' ') {
        // Only worth backing up to a word boundary if most of the line survives.
        Some((head, _)) if head.chars().count() >= max / 2 => head,
        _ => head.as_str(),
    };
    format!("{}…", cut.trim_end())
}

/// The note body: where the question was anchored, what was asked, and enough
/// of the answer to recognise it. This is what a spawned agent receives
/// verbatim (`App::todo_spawn_prompt`), so it has to stand on its own.
pub fn todo_body(qa: &LearningQa, drift: Option<LearningAnchorDrift>) -> String {
    let mut body = format!("From Learning Mode — {}\n", anchor_locator(qa));
    // The locator above is where the question was asked, which is a historical
    // fact and stays as it is. If the code has moved since, saying so here is
    // the difference between a note that leads somewhere and one that quietly
    // points at whatever now occupies those lines — and this body is handed to
    // an agent verbatim by `todo_spawn_prompt`, so a silent stale locator would
    // send it to read the wrong code.
    if let Some(drift) = drift {
        body.push_str(&format!(
            "\n{}\n",
            drift.describe(qa.anchor.line_range_for_display())
        ));
    }
    body.push_str(&format!("\nAsked: {}\n", qa.question.trim()));

    let answer = qa.answer.as_deref().unwrap_or("").trim();
    if !answer.is_empty() {
        let lines: Vec<&str> = answer.lines().collect();
        let shown = lines.len().min(MAX_TODO_ANSWER_LINES);
        body.push_str(if lines.len() > shown {
            "\nThe agent's answer began:\n"
        } else {
            "\nThe agent answered:\n"
        });
        for line in &lines[..shown] {
            body.push_str(line);
            body.push('\n');
        }
        if lines.len() > shown {
            body.push_str("…\n");
        }
    }
    body
}

/// A greppable `path:start-end` locator for the anchor, for the body's first
/// line. The prose form lives in `LearningAnchor::describe`; this one is meant
/// to be pasted into an editor.
pub fn anchor_locator(qa: &LearningQa) -> String {
    let path = qa.file_path.as_deref();
    match (qa.anchor, path) {
        (LearningAnchor::Project, _) | (_, None) => "the whole project".to_string(),
        (LearningAnchor::File, Some(path)) => path.to_string(),
        (LearningAnchor::Hunk { index }, Some(path)) => format!("{path} (change #{})", index + 1),
        (LearningAnchor::Lines { start, end }, Some(path)) if start == end => {
            format!("{path}:{start}")
        }
        (LearningAnchor::Lines { start, end }, Some(path)) => format!("{path}:{start}-{end}"),
    }
}

// ── escalating to a live session ─────────────────────────────

/// Lines of the answer carried into the composer seed.
///
/// More generous than the TODO body's cap: a live agent is being asked to
/// continue from this answer, not merely to recognise which one it was. Still
/// capped, because the seed lands in an editable composer and a two-hundred-line
/// paste is not something anyone reviews before sending.
pub(super) const MAX_SEED_ANSWER_LINES: usize = 40;

/// Lines of the anchored selection carried into the seed. Shorter than the
/// answer cap on purpose: the seed names the file and line range, and unlike the
/// headless run, the session receiving it can open the file itself.
pub(super) const MAX_SEED_SELECTION_LINES: usize = 30;

impl App {
    /// Hand the selected Q&A to a live agent session on this feature.
    ///
    /// This is the one door out of Learning Mode's read-only promise, so it is
    /// built to be crossed knowingly: the session is created, the composer is
    /// opened **pre-filled and unsent**, and a toast says that this session can
    /// do what Learning Mode could not. Nothing reaches the agent until the user
    /// presses Enter on a prompt they have read.
    ///
    /// The seed carries where the question was anchored, the question, and the
    /// answer — see [`escalation_seed`] — so the live agent starts where the
    /// reading left off instead of from nothing.
    ///
    /// A row that already opened a session jumps back to it rather than starting
    /// a second, and does *not* re-seed: that conversation already has this
    /// context. A link whose session has since been removed is dropped and a
    /// fresh one started, saying which of the two happened.
    ///
    /// Returns the `FeatureSession.id` the row is now linked to.
    pub fn learning_escalate(&mut self) -> Option<String> {
        let (qa, pi, fi) = match &self.mode {
            AppMode::Learning(state) => {
                (state.qa.get(state.selected_qa).cloned(), state.pi, state.fi)
            }
            _ => return None,
        };
        let Some(qa) = qa else {
            self.learning_error(
                "Ask something first — this hands a question you already asked to a live agent.",
            );
            return None;
        };
        // A failed row is *not* refused: a headless run that never came back is
        // exactly when handing the question to a live agent is worth doing, and
        // the seed says the first attempt failed instead of quoting an answer
        // that does not exist. An in-flight one is refused, because escalating
        // it would set two agents on the same question at once.
        if qa.status.is_in_flight() {
            self.learning_error(
                "That answer is still generating — you can hand it to a live agent once it arrives.",
            );
            return None;
        }

        let mut replacing_deleted = false;
        if let Some(session_id) = qa.spawned_session_id.clone() {
            let feature = self.store.projects.get(pi).and_then(|p| p.features.get(fi));
            let existing = feature.and_then(|f| {
                f.sessions
                    .iter()
                    .position(|s| s.id == session_id && self.learning_session_is_reusable(f, s))
            });
            match existing {
                Some(si) => {
                    self.selection = Selection::Session(pi, fi, si);
                    if let Err(e) = self.enter_view_without_auto_compose() {
                        self.log_error("learning", format!("couldn't open the session: {e}"));
                        self.learning_error(format!("Couldn't open that session: {e}"));
                        return None;
                    }
                    // The screen has just changed out from under a keypress that
                    // looked like it would start something, so it has to say why
                    // it didn't. No re-seed: that conversation already has this.
                    self.push_toast_info("You already opened a session for that one — here it is.");
                    return Some(session_id);
                }
                // The session is gone — removed, or its window is no longer
                // running — so the `→ session` marker is a promise nothing can
                // keep. Drop it and start a fresh one.
                None => {
                    self.learning_clear_spawned_session(&qa.id);
                    replacing_deleted = true;
                }
            }
        }

        // The feature's own agent, not the harness that answered in here: the
        // live session is work on this feature, and every other session in it
        // runs that agent. Continuity costs nothing, because the seed carries
        // the answer verbatim rather than relying on the agent remembering it.
        let harness = self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
            .map(|f| f.agent.clone());
        let label = learning_session_label(&qa);
        // The link back to the answer is recorded from inside this overlay,
        // which the resource confirmation dialog would replace, so this start
        // warns and goes ahead instead of parking.
        let si = match self.create_agent_session_labeled(
            pi,
            fi,
            &label,
            harness,
            StartIntent::Warn("the agent for this question"),
        ) {
            Ok(si) => si,
            Err(e) => {
                self.log_error("learning", format!("couldn't start a session: {e}"));
                self.learning_error(format!(
                    "Couldn't start an agent session — nothing was changed: {e}"
                ));
                return None;
            }
        };
        let session_id = self.store.projects[pi].features[fi].sessions[si].id.clone();

        // Recorded while the overlay is still the mode, the way the TODO spawn
        // does it: once `enter_view` lands there is no `state.qa` to write to.
        if let AppMode::Learning(state) = &mut self.mode
            && let Some(row) = state.qa.iter_mut().find(|r| r.id == qa.id)
        {
            row.spawned_session_id = Some(session_id.clone());
            row.updated_at = crate::db::learning::now_timestamp();
        }
        let link = self.persist_learning_qa_by_id(&qa.id);
        self.log_info(
            "learning",
            format!("escalated a question to session {session_id} ({label})"),
        );

        // Read before `enter_view` changes the mode: the overlay's drift map
        // goes with it, and this seed is the last thing that can carry the
        // warning across.
        let drift = match &self.mode {
            AppMode::Learning(state) => state.drift_for(&qa.id),
            _ => None,
        };
        let seed = escalation_seed(&qa, drift);
        self.selection = Selection::Session(pi, fi, si);
        if let Err(e) = self.enter_view_without_auto_compose() {
            self.log_error("learning", format!("couldn't open the new session: {e}"));
            self.push_toast_error(format!(
                "The session started, but AMF couldn't open it: {e}"
            ));
            return Some(session_id);
        }
        if let Err(e) = self.open_compose_seeded(seed) {
            self.log_error("learning", format!("couldn't seed the composer: {e}"));
            self.push_toast_error(format!(
                "The session started, but the prompt wasn't loaded: {e}"
            ));
            return Some(session_id);
        }

        // Anything still worth saying goes through `message`, not a toast: the
        // composer is now the mode, and `ui::dashboard` draws it and returns
        // *before* the shared toast pass, so a toast raised here would never
        // appear. `promote_message_to_toast` picks this up the moment the user
        // steps back to the pane. The boundary this key crosses is said in the
        // seed itself, which is the thing they are looking at right now.
        //
        // The session exists either way, so a failed link is reported as the
        // partial success it is rather than rolled back — and it outranks the
        // stale-link notice, which is only bookkeeping.
        if let Err(e) = link {
            self.message = Some(format!(
                "Error: the session started, but the link back to this answer wasn't saved: {e}"
            ));
        } else if replacing_deleted {
            self.message =
                Some("The session that answer opened is gone — this is a new one.".to_string());
        }
        Some(session_id)
    }
}

impl App {
    /// Whether a linked session is one `S` can hand the user back to.
    ///
    /// A surviving record is not enough: the agent can have exited, or its
    /// window been killed, while the rest of the feature runs on — and opening
    /// a dead pane is not the conversation the marker promised. A *stopped*
    /// feature is not dead in that sense: nothing of it is running, and
    /// entering the session starts it and recreates every saved window, so the
    /// linked conversation comes back with it. Only a live tmux session missing
    /// this window counts as gone.
    pub(super) fn learning_session_is_reusable(
        &self,
        feature: &crate::project::Feature,
        session: &crate::project::FeatureSession,
    ) -> bool {
        !session.kind.is_tmux_backed()
            || !self.tmux.session_exists(&feature.tmux_session)
            || self
                .tmux
                .window_exists(&feature.tmux_session, &session.tmux_window)
    }
}

impl App {
    /// Drop a `spawned_session_id` whose session no longer exists, in memory and
    /// (with a DB) on disk.
    pub(super) fn learning_clear_spawned_session(&mut self, qa_id: &str) {
        if let AppMode::Learning(state) = &mut self.mode
            && let Some(row) = state.qa.iter_mut().find(|r| r.id == qa_id)
        {
            row.spawned_session_id = None;
            row.updated_at = crate::db::learning::now_timestamp();
        }
        let _ = self.persist_learning_qa_by_id(qa_id);
    }
}

/// A short session label naming what the session was opened about.
///
/// The anchor rather than the question: the session list is scanned for "which
/// bit of code was that", and a truncated question reads the same as every other
/// truncated question.
pub fn learning_session_label(qa: &LearningQa) -> String {
    const MAX: usize = 24;
    let locator = anchor_locator(qa);
    if locator.chars().count() > MAX {
        // From the left: a path's tail is what identifies it.
        let tail: String = locator
            .chars()
            .skip(locator.chars().count() - MAX)
            .collect();
        format!("Learning: …{tail}")
    } else {
        format!("Learning: {locator}")
    }
}

/// The composer seed for an escalated Q&A.
///
/// Built as something a user would be willing to send unedited: where they were
/// reading, what they asked, what they were told, and what they want next. It is
/// never auto-submitted, so it is written to be *read* first — which is also why
/// it says plainly how much the earlier answer is worth. A no-tools answer could
/// only see the excerpt, and telling the live agent that is what stops a
/// fabricated file path being carried forward as an established fact.
///
/// At `Newcomer` level it also asks the live agent to narrate what it is doing,
/// since the user escalating is the one least able to read a silent diff.
///
/// The **closing** ask names the boundary this seed crosses — Learning Mode
/// could not change files, this session can. That belongs in the prompt rather
/// than in a toast (the composer draws over the pane and returns before the
/// shared toast pass, so a toast raised on arrival is never painted), and it
/// belongs at the *end* rather than the top: the composer opens with the cursor
/// after the last line, so the tail is what is on screen when the user arrives.
/// It is also true and useful to the agent reading it.
pub fn escalation_seed(qa: &LearningQa, drift: Option<LearningAnchorDrift>) -> String {
    let mut seed = String::from(match qa.intent {
        LearningQaIntent::Explain => {
            "I've been reading this code in AMF's Learning Mode and want to keep going with you.\n"
        }
        LearningQaIntent::Action => {
            "I've been reading this code in AMF's Learning Mode, and there's a change I'd like made.\n"
        }
    });
    seed.push_str(&format!("\nWhere I was reading: {}\n", anchor_locator(qa)));
    // The live agent is about to go and read that location. If the file has
    // moved on since the question was asked, sending it there without saying so
    // is how a stale anchor turns into a confidently wrong answer — the one
    // failure this mode is least able to afford, because the excerpt below
    // still shows the code the user remembers.
    if let Some(drift) = drift {
        seed.push_str(&format!(
            "{}\n",
            drift.describe(qa.anchor.line_range_for_display())
        ));
    }

    if !qa.selection_text.trim().is_empty() {
        seed.push_str(if qa.selection_is_diff {
            "\nThe change I was looking at (unified diff):\n\n```diff\n"
        } else {
            "\nThe code I was looking at:\n\n```\n"
        });
        seed.push_str(&seed_excerpt(&qa.selection_text, MAX_SEED_SELECTION_LINES));
        seed.push_str("```\n");
    }

    seed.push_str(&format!("\nWhat I asked: {}\n", qa.question.trim()));

    match qa
        .answer
        .as_deref()
        .map(str::trim)
        .filter(|answer| !answer.is_empty())
    {
        Some(answer) => {
            seed.push_str(match qa.run_mode {
                crate::app::LearningRunMode::NoTools => {
                    "\nWhat I was told. This came from a one-shot run that could only see the \
                     excerpt above — not the rest of the repository — so check it against the \
                     real code before relying on it:\n\n"
                }
                crate::app::LearningRunMode::DeepDive => {
                    "\nWhat I was told, by an agent with read-only access to this repository:\n\n"
                }
            });
            seed.push_str(&seed_excerpt(answer, MAX_SEED_ANSWER_LINES));
        }
        None => seed.push_str(
            "\nThat question never got an answer — the run failed before it came back.\n",
        ),
    }

    if qa.level == LearningLevel::Newcomer {
        seed.push_str(
            "\nI'm new to this codebase, so explain what you're doing as you go and define any \
             terms you use.\n",
        );
    }

    seed.push_str(match qa.intent {
        LearningQaIntent::Explain => {
            "\nPlease carry on from there. Start by checking that answer against the real code, \
             and tell me anything it got wrong. Unlike the run that produced it, you can change \
             files here — so ask me before you change anything.\n"
        }
        LearningQaIntent::Action => {
            "\nPlease make that change. Check the real code first — the answer above may be \
             wrong about it. Unlike the run that produced it, you can change files here, so \
             tell me what you're going to do before you do it.\n"
        }
    });
    seed
}

/// `text` capped at `max_lines`, with the cut marked so nothing reads as the
/// whole of something it isn't.
pub(super) fn seed_excerpt(text: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let shown = lines.len().min(max_lines);
    let mut out: String = lines[..shown]
        .iter()
        .map(|line| format!("{line}\n"))
        .collect();
    if lines.len() > shown {
        out.push_str(&format!("… {} more lines not shown\n", lines.len() - shown));
    }
    out
}
