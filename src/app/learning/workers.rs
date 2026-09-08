use super::navigation::{anchor_for_cursor, selection_text};
use crate::app::{App, AppMode, LearningAnchor, LearningLevel, LearningQa, LearningQaIntent};
use crate::project::AgentKind;
use anyhow::Result;
use std::path::PathBuf;

// ── prompt building ──────────────────────────────────────────

/// Lines of surrounding file shown either side of the selection. Enough for
/// the agent to see what the selection sits inside without carrying a whole
/// large file into a no-tools prompt.
pub(super) const CONTEXT_WINDOW_LINES: usize = 80;

/// Hard cap on the surrounding-context block.
pub(super) const MAX_CONTEXT_LINES: usize = 400;

/// Hard cap on the quoted selection. A whole-file anchor on a big file would
/// otherwise blow the prompt up on its own.
pub(super) const MAX_SELECTION_LINES: usize = 400;

/// How many ancestors of a follow-up are carried into its prompt. Deeper
/// ancestors are dropped oldest-first, which bounds prompt growth at the cost
/// of context a later question might have depended on (see the plan's
/// "follow-up threading grows prompts").
pub const MAX_FOLLOW_UP_DEPTH: usize = 3;

/// One earlier turn carried into a follow-up prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParentTurn {
    pub question: String,
    pub answer: String,
}

/// Everything a prompt needs to know about where a question came from. Built
/// from the overlay state, but a plain value so the builders stay pure and
/// testable.
#[derive(Debug, Clone)]
pub struct LearningPromptContext {
    pub project_name: String,
    pub feature_name: String,
    /// Repo-relative path, `None` for the project anchor.
    pub file_path: Option<String>,
    pub anchor: LearningAnchor,
    /// The text the anchor covers.
    pub selection_text: String,
    /// Whether [`selection_text`](Self::selection_text) is a unified-diff
    /// excerpt. The block is presented differently when it is: markers
    /// explained, and no line numbers, since removed lines have none on the
    /// current side.
    pub selection_is_diff: bool,
    /// The whole file the selection came from, for surrounding context.
    pub file_lines: Vec<String>,
    /// 1-based line the selection starts at, when it has one.
    pub selection_start_line: Option<usize>,
    pub question: String,
    pub intent: LearningQaIntent,
    pub level: LearningLevel,
    /// What the run may look at. Set by `learning_enqueue` from the mode that
    /// will actually be dispatched, so the prompt and the row's label can't
    /// disagree about whether the repository was read.
    pub run_mode: crate::app::LearningRunMode,
    /// Oldest first. Trimmed to [`MAX_FOLLOW_UP_DEPTH`] by the builder.
    pub ancestors: Vec<ParentTurn>,
}

/// The `{{token}}` context for
/// [`crate::prompts::PromptId::LearningAnswer`]. Each section is pre-rendered
/// to a string here (empty when it does not apply) so the registry template
/// stays a flat scaffold; the ordering rationale lives on
/// [`crate::prompts::defaults`]'s `LEARNING_ANSWER`.
pub fn learning_prompt_context_tokens(
    ctx: &LearningPromptContext,
) -> crate::prompts::PromptContext {
    let file_line = match &ctx.file_path {
        Some(path) => format!("File: {path}\n"),
        None => String::new(),
    };

    let code_block = if matches!(ctx.anchor, LearningAnchor::Project) {
        "Their question is about the project as a whole, not about one file.\n\n".to_string()
    } else if !ctx.selection_text.trim().is_empty() {
        let mut block = if ctx.selection_is_diff {
            // Unnumbered on purpose: these rows come from a unified diff, where
            // a removed line has no number on the current side and the numbers
            // that do exist are not consecutive.
            let mut b = String::from(
                "--- The change they are asking about (unified diff — lines starting \
                 with '+' were added, '-' were removed, ' ' are unchanged context) ---\n",
            );
            b.push_str(&plain_block(&ctx.selection_text, MAX_SELECTION_LINES));
            b
        } else {
            let mut b = String::from("--- The code they are asking about ---\n");
            b.push_str(&numbered_block(
                &ctx.selection_text,
                ctx.selection_start_line.unwrap_or(1),
                MAX_SELECTION_LINES,
            ));
            b
        };
        block.push_str("\n\n");
        block
    } else {
        String::new()
    };

    let surrounding = match surrounding_context(ctx) {
        Some(context) => format!("{context}\n\n"),
        None => String::new(),
    };

    let ancestors = trimmed_ancestors(&ctx.ancestors);
    let earlier_turns = if ancestors.is_empty() {
        String::new()
    } else {
        let mut e = String::from("--- Earlier in this conversation ---\n");
        for turn in ancestors {
            e.push_str(&format!("They asked: {}\n", turn.question.trim()));
            e.push_str(&format!("You answered: {}\n\n", turn.answer.trim()));
        }
        e
    };

    crate::prompts::PromptContext::new()
        .with("project_name", ctx.project_name.clone())
        .with("feature_name", ctx.feature_name.clone())
        .with("file_line", file_line)
        .with(
            "anchor_description",
            ctx.anchor.describe(ctx.file_path.as_deref()).to_string(),
        )
        .with("code_block", code_block)
        .with("surrounding_context", surrounding)
        .with("earlier_turns", earlier_turns)
        .with("question", ctx.question.trim())
        .with("intent_instructions", intent_instructions(ctx.intent))
        .with("level_instructions", level_instructions(ctx.level))
        .with("run_mode_instructions", run_mode_instructions(ctx.run_mode))
}

/// The full built-in Learning Mode prompt for one question. A thin wrapper
/// over the registry template; overrides go through the resolver at the call
/// site ([`crate::app::App::learning_enqueue`]).
pub fn build_prompt(ctx: &LearningPromptContext) -> String {
    crate::prompts::render_template(
        crate::prompts::PromptId::LearningAnswer
            .spec()
            .default_template,
        &learning_prompt_context_tokens(ctx),
    )
}

/// What the run may look at — and, for a deep dive, what it is obliged to do
/// with that access.
///
/// The row and the answer pane label a deep dive "read the repo", and the whole
/// point of the action is catching a first answer that invented a file or a
/// line number. Read-only tools only make that possible; without being told to,
/// an agent can answer straight from the excerpt and the claim on the row
/// becomes false. So the deep-dive text requires the reading and requires the
/// answer to name what was read, which is also what makes the two answers
/// comparable. The no-tools text is the mirror image: say what you cannot see
/// rather than filling it in.
pub fn run_mode_instructions(mode: crate::app::LearningRunMode) -> &'static str {
    match mode {
        crate::app::LearningRunMode::NoTools => {
            "You are answering from what is quoted above and nothing else — you \
             have no access to the rest of the repository, and you must not \
             claim otherwise. Do not invent file paths, symbols, line numbers, \
             or command output you cannot see here. Where the answer depends on \
             code that is not shown, say so plainly and name the file you would \
             need to read.\n"
        }
        crate::app::LearningRunMode::DeepDive => {
            "You have read-only access to this repository, and this answer is \
             shown to them as one that read it — so read it. Before you answer, \
             open the file above and whatever it depends on: the definitions it \
             calls, the places that call it, and any test that exercises it. \
             Ground every claim in what you actually read, and name the files \
             and symbols you checked so they can follow you. If the code \
             contradicts what you would otherwise have assumed, say so \
             explicitly. If you looked for something and could not find it, say \
             that rather than guessing.\n"
        }
    }
}

/// What the answer is for. This is the only place intent changes anything
/// about the run.
pub fn intent_instructions(intent: LearningQaIntent) -> &'static str {
    match intent {
        LearningQaIntent::Explain => {
            "Explain what this code does and why it is written this way. \
             Answer the question they actually asked. \
             Do not propose changes, rewrites, or improvements — they asked to \
             understand this code, not to change it. If something looks wrong, \
             you may say so in one sentence, but do not turn the answer into a \
             proposal.\n"
        }
        LearningQaIntent::Action => {
            "Propose the smallest concrete change that satisfies their request. \
             Begin your answer with a single line that is an imperative summary \
             of the change, under 80 characters, with no markdown formatting and \
             no trailing period — it is used verbatim as the title of a work \
             item. Then explain what to change, where, and why it is worth \
             changing. Do not make the change yourself; describe it.\n"
        }
    }
}

/// How much the answer may assume. Prompt wording only — it changes no tools,
/// no model, and nothing about which files are visible.
pub fn level_instructions(level: LearningLevel) -> &'static str {
    match level {
        LearningLevel::Newcomer => {
            "Write for someone who has never seen this codebase and may be new \
             to the language. Define every technical term the first time you use \
             it. Prefer short paragraphs and concrete examples over abstraction. \
             Do not assume they know this project's own vocabulary. No question \
             is too basic — answer it plainly rather than commenting on how basic \
             it is. Finish with a section headed \"Where to look next\" listing \
             specific files or symbols and one line on why each is worth \
             reading.\n"
        }
        LearningLevel::Familiar => {
            "Write for someone comfortable in this language who is new only to \
             this codebase. Be dense and skip the basics: no glossary, no \
             definitions of standard language features, and no \"where to look \
             next\" section.\n"
        }
    }
}

/// The most recent [`MAX_FOLLOW_UP_DEPTH`] turns, oldest first.
pub(super) fn trimmed_ancestors(ancestors: &[ParentTurn]) -> &[ParentTurn] {
    let start = ancestors.len().saturating_sub(MAX_FOLLOW_UP_DEPTH);
    &ancestors[start..]
}

/// The file around the selection, line-numbered. Skipped when the anchor is
/// the whole file (the selection *is* the file) or the project.
pub(super) fn surrounding_context(ctx: &LearningPromptContext) -> Option<String> {
    if ctx.file_lines.is_empty() {
        return None;
    }
    if matches!(ctx.anchor, LearningAnchor::Project | LearningAnchor::File) {
        return None;
    }
    let path = ctx.file_path.as_deref().unwrap_or("the file");
    let selection_start = ctx.selection_start_line.unwrap_or(1).max(1);
    let first = selection_start.saturating_sub(CONTEXT_WINDOW_LINES).max(1);
    let selection_lines = ctx.selection_text.lines().count().max(1);
    let last = (selection_start + selection_lines + CONTEXT_WINDOW_LINES)
        .min(ctx.file_lines.len())
        .min(first + MAX_CONTEXT_LINES);
    if last < first {
        return None;
    }
    let block: Vec<String> = ctx.file_lines[first - 1..last].to_vec();
    Some(format!(
        "--- Surrounding context: {path}, lines {first}-{last} ---\n{}",
        numbered_block(&block.join("\n"), first, MAX_CONTEXT_LINES)
    ))
}

/// Text carried through verbatim, under the same truncation rule as
/// [`numbered_block`]. For excerpts whose own leading characters are the
/// point — a diff — where a line-number gutter would only be misleading.
pub(super) fn plain_block(text: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = lines
        .iter()
        .take(max_lines)
        .map(|line| format!("{line}\n"))
        .collect::<String>();
    if lines.len() > max_lines {
        out.push_str(&format!(
            "… {} more lines not shown\n",
            lines.len() - max_lines
        ));
    }
    // Trailing newline is added by the caller's separator.
    out.pop();
    out
}

/// Line-numbered text, truncated with an explicit marker so the model can tell
/// truncation from the real end of a file.
pub(super) fn numbered_block(text: &str, start_line: usize, max_lines: usize) -> String {
    let mut out = String::new();
    let lines: Vec<&str> = text.lines().collect();
    for (i, line) in lines.iter().take(max_lines).enumerate() {
        out.push_str(&format!("{:>6} | {line}\n", start_line + i));
    }
    if lines.len() > max_lines {
        out.push_str(&format!(
            "       … {} more lines not shown\n",
            lines.len() - max_lines
        ));
    }
    // Trailing newline is added by the caller's separator.
    out.pop();
    out
}

/// The place a question is asked about, captured so it can't move under the
/// user. A follow-up reuses its parent's capture verbatim, which is why these
/// three fields are persisted on every row.
#[derive(Debug, Clone)]
pub struct AskAnchor {
    pub anchor: LearningAnchor,
    pub file_path: Option<String>,
    pub selection_text: String,
    /// Captured with the text, not re-read at submit time: browsing away from
    /// branch-changes scope must not turn a quoted diff into numbered source.
    pub selection_is_diff: bool,
}

impl App {
    /// Assemble the prompt context for a question asked right now, against the
    /// overlay's current anchor.
    pub fn learning_prompt_context(
        &self,
        question: &str,
        intent: LearningQaIntent,
        ancestors: Vec<ParentTurn>,
    ) -> Option<LearningPromptContext> {
        self.learning_prompt_context_at(question, intent, ancestors, None)
    }
}

impl App {
    /// As above, but against `captured` when a question inherits its place
    /// from somewhere other than the cursor — a follow-up asked from the
    /// answer pane, where the file list may have moved on since.
    pub fn learning_prompt_context_at(
        &self,
        question: &str,
        intent: LearningQaIntent,
        ancestors: Vec<ParentTurn>,
        captured: Option<&AskAnchor>,
    ) -> Option<LearningPromptContext> {
        let AppMode::Learning(state) = &self.mode else {
            return None;
        };
        let anchor = captured.map(|c| c.anchor).unwrap_or(state.anchor);
        let file_path = match anchor {
            LearningAnchor::Project => None,
            _ => match captured {
                Some(c) => c.file_path.clone(),
                None => state.content_path.clone(),
            },
        };
        let selection_start_line = match anchor {
            LearningAnchor::Lines { start, .. } => Some(start),
            LearningAnchor::Hunk { .. } => match anchor_for_cursor(state) {
                LearningAnchor::Lines { start, .. } => Some(start),
                _ => None,
            },
            _ => Some(1),
        };
        // Surrounding context is only honest while the loaded file is still
        // the one being asked about; a follow-up on a file the user has since
        // browsed away from gets its parent's turn instead of the wrong file.
        let file_lines = if file_path.is_some() && file_path == state.content_path {
            state.content.clone()
        } else if captured.is_some() {
            Vec::new()
        } else {
            state.content.clone()
        };
        Some(LearningPromptContext {
            project_name: state.project_name.clone(),
            feature_name: state.feature_name.clone(),
            file_path,
            anchor,
            selection_text: match captured {
                Some(c) => c.selection_text.clone(),
                None => selection_text(state),
            },
            selection_is_diff: match captured {
                Some(c) => c.selection_is_diff,
                None => state.selection_is_diff(),
            },
            file_lines,
            selection_start_line,
            question: question.to_string(),
            intent,
            level: state.level,
            // Provisional: `learning_enqueue` overwrites it with the mode that
            // is actually dispatched, once `effective_for` has had its say.
            run_mode: crate::app::LearningRunMode::NoTools,
            ancestors,
        })
    }
}

// ── asking (headless, non-blocking) ──────────────────────────

/// Where a new follow-up on `parent_id` belongs: just past the parent and
/// everything already hanging off it, so a thread stays contiguous.
///
/// `None` when the parent isn't in `rows` — a stale id appends rather than
/// disappearing.
pub fn thread_insert_index(rows: &[LearningQa], parent_id: &str) -> Option<usize> {
    let mut last = rows.iter().position(|row| row.id == parent_id)?;
    let mut thread: Vec<&str> = vec![parent_id];
    // One pass per row is enough: rows are stored parent-before-child, so a
    // descendant is always seen after the ancestor that admits it.
    for (index, row) in rows.iter().enumerate().skip(last + 1) {
        let parent = row.parent_qa_id.as_deref();
        if parent.is_some_and(|p| thread.contains(&p)) {
            thread.push(&row.id);
            last = index;
        }
    }
    Some(last + 1)
}

/// Reorder a stored history so every follow-up sits directly under the thread
/// it continues, the way the live list keeps it.
///
/// Rows are stored — and reloaded — in the order they were asked, but a
/// follow-up is asked *after* whatever else was asked in between. Replaying
/// that order verbatim would leave it indented under an unrelated question,
/// since the renderer takes its placement from the list and only its
/// indentation from `parent_qa_id`. Threading here rather than at render time
/// keeps one notion of order: `learning_enqueue` inserts a new row at exactly
/// this position, so a reopened history reads the way it did when it was
/// written.
///
/// A row whose parent is missing lands at the end rather than disappearing.
pub fn thread_rows(rows: Vec<LearningQa>) -> Vec<LearningQa> {
    let mut out: Vec<LearningQa> = Vec::with_capacity(rows.len());
    for row in rows {
        // Oldest first, so a parent is always placed before the rows that hang
        // off it.
        let at = row
            .parent_qa_id
            .as_deref()
            .and_then(|parent| thread_insert_index(&out, parent))
            .unwrap_or(out.len());
        out.insert(at, row);
    }
    out
}

/// A finished headless run, delivered back to the UI thread.
pub struct LearningAnswer {
    /// `learning_qa.id` the answer belongs to.
    pub qa_id: String,
    /// `Ok(answer)` or a message phrased as what to do about it.
    pub result: Result<String, String>,
}

impl App {
    /// Enqueue a question against the overlay's current anchor and return
    /// immediately. The run happens on its own thread; the row shows "queued"
    /// then "thinking…" until [`App::poll_learning_answers_bg`] files the
    /// answer. Several questions may be in flight at once.
    ///
    /// Returns the new row's id.
    pub fn learning_ask(
        &mut self,
        question: &str,
        intent: LearningQaIntent,
        parent_qa_id: Option<String>,
    ) -> Option<String> {
        self.learning_ask_at(question, intent, parent_qa_id, None)
    }
}

impl App {
    /// As above, against an explicitly captured place rather than wherever the
    /// cursor happens to be. A follow-up passes its parent's capture, so it
    /// asks about the same code even if the file list has moved on.
    pub fn learning_ask_at(
        &mut self,
        question: &str,
        intent: LearningQaIntent,
        parent_qa_id: Option<String>,
        captured: Option<AskAnchor>,
    ) -> Option<String> {
        let question = question.trim().to_string();
        if question.is_empty() {
            return None;
        }
        let ancestors = self.learning_ancestor_turns(parent_qa_id.as_deref());
        let ctx =
            self.learning_prompt_context_at(&question, intent, ancestors, captured.as_ref())?;
        self.learning_enqueue(
            ctx,
            parent_qa_id,
            crate::app::LearningRunMode::NoTools,
            None,
        )
    }
}

impl App {
    /// Write a row for `ctx` and start its run. The single place a `learning_qa`
    /// row is born, so asking and re-asking can't drift apart.
    ///
    /// `deep_dive_of` is set only by [`App::learning_deep_dive`], and always to
    /// the same row as `parent_qa_id`: the pair threads together but does not
    /// converse (see [`LearningQa::deep_dive_of`]).
    pub(super) fn learning_enqueue(
        &mut self,
        mut ctx: LearningPromptContext,
        parent_qa_id: Option<String>,
        run_mode: crate::app::LearningRunMode,
        deep_dive_of: Option<String>,
    ) -> Option<String> {
        let AppMode::Learning(state) = &mut self.mode else {
            return None;
        };
        // Recorded as what will actually run: a harness with no no-tools mode
        // is a deep dive whatever was asked for (see `effective_for`).
        let run_mode = run_mode.effective_for(&state.harness);
        // The prompt is written for the run that will happen, not the one that
        // was asked for, so a downgraded row can't be told to answer from the
        // excerpt alone while its label says it read the repository.
        ctx.run_mode = run_mode;
        let qa = LearningQa {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: state.session_id.clone(),
            parent_qa_id: parent_qa_id.clone(),
            deep_dive_of,
            file_path: ctx.file_path.clone(),
            anchor: ctx.anchor,
            selection_text: ctx.selection_text.clone(),
            selection_is_diff: ctx.selection_is_diff,
            question: ctx.question.clone(),
            intent: ctx.intent,
            // From the context, not the live setting: a re-run preserves the
            // level its original was answered at, so the pair reads alike.
            level: ctx.level,
            answer: None,
            harness: state.harness.clone(),
            run_mode,
            status: crate::app::LearningQaStatus::Pending,
            error: None,
            todo_id: None,
            spawned_session_id: None,
            created_at: crate::db::learning::now_timestamp(),
            updated_at: crate::db::learning::now_timestamp(),
        };
        let qa_id = qa.id.clone();
        let harness = qa.harness.clone();
        let workdir = state.workdir.clone();
        // A follow-up belongs under the thread it continues, not at the bottom
        // of the history — the renderer indents it under its parent, and a row
        // indented under something twenty rows above it reads as a glitch.
        let at = parent_qa_id
            .as_deref()
            .and_then(|parent| thread_insert_index(&state.qa, parent))
            .unwrap_or(state.qa.len());
        state.qa.insert(at, qa.clone());
        // Show the new question, so an answer that takes a while is visibly
        // *this* question's answer. The insert can leave the cursor's index
        // pointing at a different row than it did a moment ago, so the banner
        // goes whether or not the index itself moves.
        state.clear_notice();
        state.select_qa(at);

        // The question runs either way: an answer this session can show is
        // worth more than one refused because history couldn't be written.
        let _ = self.persist_learning_qa(&qa);
        let repo = crate::worktree::WorktreeManager::repo_root(&workdir)
            .unwrap_or_else(|_| workdir.clone());
        let prompt = self.resolve_headless_prompt(
            crate::prompts::PromptId::LearningAnswer,
            &harness,
            &repo,
            &workdir,
            &learning_prompt_context_tokens(&ctx),
        );
        // Learning answers run one-per-question and can queue up; a blocking
        // pre-call modal here would stall the overlay, so this is a toast.
        self.announce_headless_run(crate::prompts::PromptId::LearningAnswer, &harness);
        self.spawn_learning_run(&qa_id, harness, workdir, prompt, run_mode);
        Some(qa_id)
    }
}

impl App {
    /// Re-ask the selected question with the repository open to the agent.
    ///
    /// The first answer comes from a no-tools run that can only see the prompt,
    /// so it can name files, symbols, and line numbers that do not exist — the
    /// failure a newcomer is least equipped to spot. A deep dive is the answer
    /// to that: same question, same anchor, same intent and reading level, run
    /// through [`HeadlessRunner::run_read_only`] in the feature's workdir so the
    /// agent can go and check.
    ///
    /// It lands as its own row indented under the original, and the original
    /// answer is left untouched so the two can be read against each other. The
    /// original answer is deliberately **not** fed into the prompt: a rerun that
    /// re-derives the facts is worth more than one anchored on a guess. The new
    /// row records the original in
    /// [`deep_dive_of`](crate::app::LearningQa::deep_dive_of) as well as in
    /// `parent_qa_id`, which is what keeps that answer out of *later* prompts
    /// too — a follow-up on the deep dive continues from the deep dive.
    ///
    /// Returns the new row's id, or `None` when nothing was started (the banner
    /// says why).
    ///
    /// [`HeadlessRunner::run_read_only`]: crate::headless::HeadlessRunner::run_read_only
    pub fn learning_deep_dive(&mut self) -> Option<String> {
        let Some(origin) = (match &self.mode {
            AppMode::Learning(state) => state.qa.get(state.selected_qa).cloned(),
            _ => return None,
        }) else {
            self.learning_error("Ask something first — a deep dive re-runs a question you already asked, letting the agent read the repo.");
            return None;
        };
        // Checked before the in-flight guard: a row that reads the repository
        // is refused whether or not it has landed, so telling the user to wait
        // for it would be promising something that is then refused. Also the
        // Codex case — `effective_for` already downgraded that row to a deep
        // dive, so there is genuinely nothing deeper to go.
        if origin.run_mode == crate::app::LearningRunMode::DeepDive {
            self.learning_error(if origin.status.is_in_flight() {
                "That one is already reading the repository. Once it lands, ask a follow-up (F) to go further."
            } else {
                "That answer already read the repository. Ask a follow-up (F) to go further."
            });
            return None;
        }
        if origin.status.is_in_flight() {
            self.learning_error(
                "That answer is still generating — you can send it deeper once it arrives.",
            );
            return None;
        }
        // One deep dive per question: a second identical run costs the same and
        // says the same thing, so jump to the one that exists instead.
        //
        // Matched on `deep_dive_of` rather than parent + run mode, which would
        // mistake an ordinary follow-up for a deep dive under Codex, where
        // every row is recorded as one.
        let existing = match &self.mode {
            AppMode::Learning(state) => state
                .qa
                .iter()
                .position(|row| {
                    row.deep_dive_of.as_deref() == Some(origin.id.as_str())
                        && row.status != crate::app::LearningQaStatus::Failed
                })
                .map(|index| (index, state.qa[index].status.is_in_flight())),
            _ => None,
        };
        if let Some((index, in_flight)) = existing {
            if let AppMode::Learning(state) = &mut self.mode {
                state.select_qa(index);
                state.answer_open = false;
                // An unfinished run has nothing to show yet, so it must not be
                // described as something that came back.
                state.error = Some(
                    if in_flight {
                        "You already sent that one deeper — it is still reading the repository."
                    } else {
                        "You already sent that one deeper — here is what it came back with."
                    }
                    .into(),
                );
                state.clear_notice();
            }
            return None;
        }

        let ctx = self.learning_deep_dive_context(&origin)?;
        if let AppMode::Learning(state) = &mut self.mode {
            state.error = None;
            state.answer_open = false;
        }
        self.learning_enqueue(
            ctx,
            Some(origin.id.clone()),
            crate::app::LearningRunMode::DeepDive,
            Some(origin.id.clone()),
        )
    }
}

impl App {
    /// The prompt a deep dive of `origin` would send.
    ///
    /// Everything comes off the row rather than off the live overlay, so a
    /// question sent deeper after browsing elsewhere still asks about its own
    /// code at its own reading level.
    pub(crate) fn learning_deep_dive_context(
        &self,
        origin: &LearningQa,
    ) -> Option<LearningPromptContext> {
        let captured = AskAnchor {
            anchor: origin.anchor,
            file_path: origin.file_path.clone(),
            selection_text: origin.selection_text.clone(),
            selection_is_diff: origin.selection_is_diff,
        };
        // The conversation that led *to* the origin, not including the origin —
        // a deep dive occupies the origin's position in the thread rather than
        // continuing past it, which is what keeps the answer it is checking out
        // of the prompt that checks it.
        let ancestors = self.learning_ancestor_turns(origin.parent_qa_id.as_deref());
        let mut ctx = self.learning_prompt_context_at(
            &origin.question,
            origin.intent,
            ancestors,
            Some(&captured),
        )?;
        ctx.level = origin.level;
        ctx.run_mode = crate::app::LearningRunMode::DeepDive;
        Some(ctx)
    }
}

impl App {
    /// Re-file the selected entry as the other intent: an explanation that
    /// turned out to reveal a problem becomes a change request, and a change
    /// request that only ever produced an explanation goes back to being a
    /// note.
    ///
    /// The answer is left exactly as it was, and the banner says so. Intent is
    /// the user's filing label; the text below it was written under whatever
    /// framing was chosen when the question was asked, and re-labelling cannot
    /// retroactively change that. Saying it out loud is what stops the new
    /// marker from implying the answer was regenerated — the follow-up key is
    /// what actually gets an answer written the other way.
    ///
    /// Allowed on an in-flight row for the same reason: the prompt is already
    /// dispatched either way, so refusing would only withhold the label.
    ///
    /// A re-file that cannot be written through is undone rather than
    /// confirmed: the label is what this key produces, and one that the next
    /// open of the overlay silently drops is worse than one that was refused
    /// out loud.
    ///
    /// Returns the intent the row now carries.
    pub fn learning_relabel_intent(&mut self) -> Option<LearningQaIntent> {
        let Some((qa_id, was, now, was_updated_at, answered)) = (match &self.mode {
            AppMode::Learning(state) => state.qa.get(state.selected_qa).map(|row| {
                (
                    row.id.clone(),
                    row.intent,
                    row.intent.toggled(),
                    row.updated_at.clone(),
                    row.answer.is_some(),
                )
            }),
            _ => return None,
        }) else {
            self.learning_error(
                "Ask something first — re-filing changes how a question you already asked is labelled.",
            );
            return None;
        };

        if let AppMode::Learning(state) = &mut self.mode
            && let Some(row) = state.qa.iter_mut().find(|r| r.id == qa_id)
        {
            row.intent = now;
            row.updated_at = crate::db::learning::now_timestamp();
        }
        if let Err(e) = self.persist_learning_qa_by_id(&qa_id) {
            if let AppMode::Learning(state) = &mut self.mode
                && let Some(row) = state.qa.iter_mut().find(|r| r.id == qa_id)
            {
                row.intent = was;
                row.updated_at = was_updated_at;
            }
            self.learning_error(format!(
                "Couldn't re-file this one — nothing was saved: {e}"
            ));
            return None;
        }

        // Kept short on purpose: the banner is one unwrapped line, and at a
        // 140-column terminal a longer sentence loses its tail — which here is
        // the part that says the answer wasn't rewritten.
        self.learning_notice_for_qa(&qa_id, match (now, answered) {
            (LearningQaIntent::Action, true) => {
                "Re-filed as a change request. The answer is unchanged — ask a follow-up (F) to get the change spelled out."
            }
            (LearningQaIntent::Explain, true) => {
                "Re-filed as an explanation. The answer is unchanged — it was written as a change proposal."
            }
            (LearningQaIntent::Action, false) => {
                "Re-filed as a change request. The answer on its way was asked for as an explanation."
            }
            (LearningQaIntent::Explain, false) => {
                "Re-filed as an explanation. The answer on its way was asked for as a change."
            }
        });
        Some(now)
    }
}

impl App {
    /// Set the overlay's banner — the "why nothing happened" channel.
    pub(super) fn learning_error(&mut self, message: impl Into<String>) {
        if let AppMode::Learning(state) = &mut self.mode {
            state.error = Some(message.into());
            state.clear_notice();
        }
    }
}

impl App {
    /// Set the overlay's banner to something that *did* happen, on a
    /// particular row. Clears any standing refusal, which the successful key
    /// has just answered.
    ///
    /// Bound to the row so it can be taken down again once the row is no
    /// longer what the wording described — the cursor moving off it, or its
    /// run landing.
    pub(super) fn learning_notice_for_qa(&mut self, qa_id: &str, message: impl Into<String>) {
        if let AppMode::Learning(state) = &mut self.mode {
            state.notice = Some(message.into());
            state.notice_qa_id = Some(qa_id.to_string());
            state.error = None;
        }
    }
}

impl App {
    /// Drop a banner raised on `qa_id` because the row has moved on from the
    /// state the wording assumed.
    pub(super) fn learning_invalidate_notice_for(&mut self, qa_id: &str) {
        if let AppMode::Learning(state) = &mut self.mode
            && state.notice_qa_id.as_deref() == Some(qa_id)
        {
            state.clear_notice();
        }
    }
}

impl App {
    /// Start the headless run for an existing row and mark it running.
    pub fn spawn_learning_run(
        &mut self,
        qa_id: &str,
        harness: AgentKind,
        workdir: PathBuf,
        prompt: String,
        run_mode: crate::app::LearningRunMode,
    ) {
        // The dispatch below and the mode stored on the row must agree, so both
        // go through `effective_for` rather than trusting the caller's ask.
        let run_mode = run_mode.effective_for(&harness);
        let id = qa_id.to_string();
        self.log_info(
            "learning",
            format!(
                "asking {} ({}) about {} chars of context",
                harness.display_name(),
                run_mode.as_str(),
                prompt.len()
            ),
        );
        // Remembered so a reopen of this overlay doesn't mistake a run this
        // process is still waiting on for one stranded by an earlier one (see
        // `reconcile_interrupted_qa`).
        let tx = self.learning_runs.begin(id.clone());
        // Tests drive the same channel by hand (see `deliver`). Launching a
        // real agent CLI from a unit test would be slow, flaky, and would spend
        // the developer's tokens, so the row still transitions to Running but
        // no process is started.
        if cfg!(test) {
            self.set_learning_qa_status(qa_id, crate::app::LearningQaStatus::Running, None);
            return;
        }
        std::thread::spawn(move || {
            let result = match run_mode {
                crate::app::LearningRunMode::NoTools => {
                    crate::headless::HeadlessRunner::run(&harness, &workdir, &prompt, None, true)
                }
                crate::app::LearningRunMode::DeepDive => {
                    crate::headless::HeadlessRunner::run_read_only(
                        &harness, &workdir, &prompt, None,
                    )
                }
            };
            let _ = tx.send(LearningAnswer {
                qa_id: id,
                result: result.map_err(|e| headless_failure_message(&harness, &e)),
            });
        });
        self.set_learning_qa_status(qa_id, crate::app::LearningQaStatus::Running, None);
    }
}

impl App {
    /// Drain finished answers. Called from the main loop beside the other
    /// `poll_*_bg` calls; returns true when something changed and the UI
    /// should redraw.
    pub fn poll_learning_answers_bg(&mut self) -> bool {
        let mut changed = false;
        while let Some(answer) = self.learning_runs.next_answer() {
            changed = true;
            let outcome = match answer.result {
                Ok(text) => Ok(text.trim().to_string()),
                Err(message) => {
                    self.log_error("learning", format!("question failed: {message}"));
                    Err(message)
                }
            };
            let mut applied = false;
            if let AppMode::Learning(state) = &mut self.mode
                && let Some(row) = state.qa.iter_mut().find(|r| r.id == answer.qa_id)
            {
                match &outcome {
                    Ok(text) => {
                        row.answer = Some(text.clone());
                        row.status = crate::app::LearningQaStatus::Answered;
                        row.error = None;
                    }
                    // A failure leaves any earlier answer in place: a rerun
                    // that couldn't start is no reason to lose what the first
                    // run already said.
                    Err(message) => {
                        row.status = crate::app::LearningQaStatus::Failed;
                        row.error = Some(message.clone());
                    }
                }
                row.updated_at = crate::db::learning::now_timestamp();
                applied = true;
            }
            if applied {
                // Logged, and the answer is on screen either way.
                let _ = self.persist_learning_qa_by_id(&answer.qa_id);
                // "The answer on its way was asked for as an explanation" was
                // true when the key was pressed and is not any more — the
                // answer is here.
                self.learning_invalidate_notice_for(&answer.qa_id);
            } else {
                self.finish_learning_qa_in_db(&answer.qa_id, &outcome);
            }
        }
        changed
    }
}

impl App {
    /// Complete a row straight in the DB, for a run that finished after the
    /// overlay that started it closed or moved to another project.
    ///
    /// The in-memory row is the overlay's source of truth, so once the overlay
    /// is gone there is nothing for `persist_learning_qa` to write and the row
    /// would sit at `running` in the database for good — reopening the session
    /// would show a question that never finishes.
    pub(super) fn finish_learning_qa_in_db(
        &mut self,
        qa_id: &str,
        outcome: &Result<String, String>,
    ) {
        let Some(db) = self.db.as_ref() else {
            return;
        };
        let result = match outcome {
            Ok(text) => db.finish_learning_qa(
                qa_id,
                Some(text),
                crate::app::LearningQaStatus::Answered,
                None,
            ),
            Err(message) => db.finish_learning_qa(
                qa_id,
                None,
                crate::app::LearningQaStatus::Failed,
                Some(message),
            ),
        };
        match result {
            Ok(true) => self.log_info(
                "learning",
                format!("saved an answer that finished after its overlay closed ({qa_id})"),
            ),
            Ok(false) => self.log_warn(
                "learning",
                format!("an answer arrived for a question that no longer exists ({qa_id})"),
            ),
            Err(e) => self.log_warn(
                "learning",
                format!("couldn't save an answer that finished after its overlay closed: {e}"),
            ),
        }
    }
}

impl App {
    /// The chain of earlier turns leading to `parent_qa_id`, oldest first.
    /// Only answered rows are carried — an unanswered parent has no context to
    /// give.
    ///
    /// A deep dive in the chain is followed *through* the row it re-ran rather
    /// than into it: it stands in that row's place, so the answer it was run to
    /// check — the one that may have invented files and line numbers — never
    /// re-enters a later prompt through the back door.
    pub(super) fn learning_ancestor_turns(&self, parent_qa_id: Option<&str>) -> Vec<ParentTurn> {
        let AppMode::Learning(state) = &self.mode else {
            return Vec::new();
        };
        let mut chain = Vec::new();
        let mut current = parent_qa_id.map(str::to_string);
        // Bounded by the row count, so a cycle in the data can't hang the UI.
        for _ in 0..state.qa.len() {
            let Some(id) = current.take() else { break };
            let Some(row) = state.qa.iter().find(|r| r.id == id) else {
                break;
            };
            if let Some(answer) = &row.answer {
                chain.push(ParentTurn {
                    question: row.question.clone(),
                    answer: answer.clone(),
                });
            }
            current = match row.superseded_id() {
                // Skip the superseded row and resume above it. Its own parent
                // is where the conversation actually continues.
                Some(superseded) => state
                    .qa
                    .iter()
                    .find(|r| r.id == superseded)
                    .and_then(|r| r.parent_qa_id.clone()),
                None => row.parent_qa_id.clone(),
            };
        }
        chain.reverse();
        chain
    }
}

impl App {
    pub(super) fn set_learning_qa_status(
        &mut self,
        qa_id: &str,
        status: crate::app::LearningQaStatus,
        error: Option<String>,
    ) {
        if let AppMode::Learning(state) = &mut self.mode
            && let Some(row) = state.qa.iter_mut().find(|r| r.id == qa_id)
        {
            row.status = status;
            row.error = error;
            row.updated_at = crate::db::learning::now_timestamp();
        }
        let _ = self.persist_learning_qa_by_id(qa_id);
        // Any banner about what this row was doing stops being true the moment
        // it stops doing it.
        if !status.is_in_flight() {
            self.learning_invalidate_notice_for(qa_id);
        }
    }
}

impl App {
    /// Write the in-memory row with this id through to the DB. `Ok(())` when
    /// there was nothing to write — no such row, no session, no database —
    /// since none of those is a failed save.
    pub(super) fn persist_learning_qa_by_id(&mut self, qa_id: &str) -> Result<(), String> {
        let row = match &self.mode {
            AppMode::Learning(state) => state.qa.iter().find(|r| r.id == qa_id).cloned(),
            _ => None,
        };
        match row {
            Some(row) => self.persist_learning_qa(&row),
            None => Ok(()),
        }
    }
}

impl App {
    /// Write a row through to the DB when there is one. History surviving a
    /// restart is a nice-to-have for most callers, not a precondition, so a
    /// failure is logged here and the in-memory row carries on; the error is
    /// returned as well for the callers that have just told the user something
    /// was saved and have to take that back.
    pub fn persist_learning_qa(&mut self, qa: &LearningQa) -> Result<(), String> {
        if qa.session_id.is_empty() {
            return Ok(());
        }
        let Some(db) = self.db.as_ref() else {
            return Ok(());
        };
        if let Err(e) = db.upsert_learning_qa(qa) {
            let message = e.to_string();
            self.log_warn(
                "learning",
                format!("couldn't save this question: {e} (it still works in this session)"),
            );
            return Err(message);
        }
        Ok(())
    }
}

// ── starter questions ────────────────────────────────────────

/// Which anchors a starter question makes sense for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StarterScope {
    /// Only with the whole project selected.
    Project,
    /// With a file open (any anchor inside it included).
    File,
    /// Only with a line range or hunk selected.
    Lines,
}

/// A preset question. The point is that a blank prompt is never the only
/// option: a user who doesn't yet know what to ask still has a first move.
#[derive(Debug, Clone, Copy)]
pub struct StarterQuestion {
    pub text: &'static str,
    pub intent: LearningQaIntent,
    pub scope: StarterScope,
}

/// The v1 preset list. Written for someone reading code they didn't write;
/// see the plan's "the starter-question list is a guess".
pub const STARTER_QUESTIONS: &[StarterQuestion] = &[
    StarterQuestion {
        text: "Give me a tour of this project — what is it, and where does execution start?",
        intent: LearningQaIntent::Explain,
        scope: StarterScope::Project,
    },
    StarterQuestion {
        text: "What should I read first to understand this project, and in what order?",
        intent: LearningQaIntent::Explain,
        scope: StarterScope::Project,
    },
    StarterQuestion {
        text: "What is this file responsible for, and what do I need to know to read it?",
        intent: LearningQaIntent::Explain,
        scope: StarterScope::File,
    },
    StarterQuestion {
        text: "What calls into this file, and what does it call?",
        intent: LearningQaIntent::Explain,
        scope: StarterScope::File,
    },
    StarterQuestion {
        text: "Explain this line by line.",
        intent: LearningQaIntent::Explain,
        scope: StarterScope::Lines,
    },
    StarterQuestion {
        text: "Why is it written this way instead of the obvious way?",
        intent: LearningQaIntent::Explain,
        scope: StarterScope::Lines,
    },
    StarterQuestion {
        text: "What would break if I deleted this?",
        intent: LearningQaIntent::Explain,
        scope: StarterScope::Lines,
    },
    StarterQuestion {
        text: "What do the unfamiliar words here mean?",
        intent: LearningQaIntent::Explain,
        scope: StarterScope::Lines,
    },
    StarterQuestion {
        text: "Suggest how to make this clearer without changing behaviour.",
        intent: LearningQaIntent::Action,
        scope: StarterScope::Lines,
    },
];

/// Indices of the starter questions worth offering for `anchor`.
pub fn starter_questions_for(anchor: LearningAnchor) -> Vec<usize> {
    STARTER_QUESTIONS
        .iter()
        .enumerate()
        .filter(|(_, q)| match (q.scope, anchor) {
            (StarterScope::Project, LearningAnchor::Project) => true,
            // A file-level question still applies when a range inside that
            // file is selected — the file is open either way.
            (StarterScope::File, LearningAnchor::File)
            | (StarterScope::File, LearningAnchor::Lines { .. })
            | (StarterScope::File, LearningAnchor::Hunk { .. }) => true,
            (StarterScope::Lines, LearningAnchor::Lines { .. })
            | (StarterScope::Lines, LearningAnchor::Hunk { .. }) => true,
            _ => false,
        })
        .map(|(i, _)| i)
        .collect()
}

impl App {
    /// Open the question prompt for `intent`, capturing the anchor as it
    /// stands so browsing can't move it under the user.
    pub fn learning_open_question(
        &mut self,
        intent: LearningQaIntent,
        parent_qa_id: Option<String>,
    ) {
        let (text, is_diff) = match &self.mode {
            AppMode::Learning(state) => (selection_text(state), state.selection_is_diff()),
            _ => return,
        };
        if let AppMode::Learning(state) = &mut self.mode {
            state.question = Some(crate::app::LearningQuestionEditor {
                editor: crate::editor::TextEditor::new(String::new()),
                intent,
                parent_qa_id,
                anchor: state.anchor,
                file_path: match state.anchor {
                    LearningAnchor::Project => None,
                    _ => state.content_path.clone(),
                },
                selection_text: text,
                selection_is_diff: is_diff,
                scroll: 0,
                sync_to_cursor: true,
            });
        }
    }
}

impl App {
    /// Ask a follow-up to the selected answer.
    ///
    /// A newcomer's second question ("wait, what's a trait?") matters as much
    /// as their first, so the prompt opens carrying the parent's place in the
    /// project *and* its question and answer — the agent answers against what
    /// the user was just told rather than re-deriving it.
    pub fn learning_open_follow_up(&mut self) {
        let Some(parent) = (match &self.mode {
            AppMode::Learning(state) => state.qa.get(state.selected_qa).cloned(),
            _ => return,
        }) else {
            if let AppMode::Learning(state) = &mut self.mode {
                state.error =
                    Some("Ask something first — a follow-up continues an earlier answer.".into());
            }
            return;
        };
        if parent.answer.is_none() {
            if let AppMode::Learning(state) = &mut self.mode {
                state.error = Some(match parent.status {
                    crate::app::LearningQaStatus::Failed => {
                        "That question never got an answer to follow up on. Ask it again first."
                            .to_string()
                    }
                    _ => "That answer is still generating — you can follow up once it arrives."
                        .to_string(),
                });
            }
            return;
        }
        if let AppMode::Learning(state) = &mut self.mode {
            state.error = None;
            state.answer_open = false;
            state.question = Some(crate::app::LearningQuestionEditor {
                editor: crate::editor::TextEditor::new(String::new()),
                // Inherited, not re-chosen: a follow-up to an explanation is
                // still an explanation unless the user flips it with Ctrl+E.
                intent: parent.intent,
                parent_qa_id: Some(parent.id.clone()),
                anchor: parent.anchor,
                file_path: parent.file_path.clone(),
                selection_text: parent.selection_text.clone(),
                // From the parent row, not the live overlay: the user may have
                // browsed out of branch-changes scope since it was answered.
                selection_is_diff: parent.selection_is_diff,
                scroll: 0,
                sync_to_cursor: true,
            });
        }
    }
}

impl App {
    /// Flip explain ⇄ change without losing what's been typed.
    pub fn learning_question_toggle_intent(&mut self) {
        if let AppMode::Learning(state) = &mut self.mode
            && let Some(q) = &mut state.question
        {
            q.intent = q.intent.toggled();
        }
    }
}

impl App {
    pub fn learning_cancel_question(&mut self) {
        if let AppMode::Learning(state) = &mut self.mode {
            state.question = None;
            state.starter_picker = None;
        }
    }
}

impl App {
    /// Ask what's in the prompt. Returns the new row's id.
    pub fn learning_submit_question(&mut self) -> Option<String> {
        let (text, intent, parent, captured) = match &self.mode {
            AppMode::Learning(state) => {
                let q = state.question.as_ref()?;
                // The prompt captured its place when it opened; honour that
                // capture rather than re-reading the cursor, so a follow-up
                // asks about its parent's code and not wherever browsing left
                // the file list.
                let captured = q.parent_qa_id.as_ref().map(|_| AskAnchor {
                    anchor: q.anchor,
                    file_path: q.file_path.clone(),
                    selection_text: q.selection_text.clone(),
                    selection_is_diff: q.selection_is_diff,
                });
                (
                    q.editor.text().to_string(),
                    q.intent,
                    q.parent_qa_id.clone(),
                    captured,
                )
            }
            _ => return None,
        };
        if text.trim().is_empty() {
            return None;
        }
        if let AppMode::Learning(state) = &mut self.mode {
            state.question = None;
            state.starter_picker = None;
        }
        self.learning_ask_at(&text, intent, parent, captured)
    }
}

impl App {
    /// Offer the presets that fit the current anchor. Opens the prompt first
    /// if it isn't already open, so the picker is a way *into* asking.
    pub fn learning_open_starter_picker(&mut self) {
        let anchor = match &self.mode {
            AppMode::Learning(state) => state
                .question
                .as_ref()
                .map(|q| q.anchor)
                .unwrap_or(state.anchor),
            _ => return,
        };
        let indices = starter_questions_for(anchor);
        if indices.is_empty() {
            if let AppMode::Learning(state) = &mut self.mode {
                state.error = Some(
                    "No starter questions fit what's selected — pick a file or some lines first."
                        .to_string(),
                );
            }
            return;
        }
        if matches!(&self.mode, AppMode::Learning(state) if state.question.is_none()) {
            self.learning_open_question(LearningQaIntent::Explain, None);
        }
        if let AppMode::Learning(state) = &mut self.mode {
            state.starter_picker = Some(crate::app::LearningStarterPicker {
                indices,
                selected: 0,
            });
            state.error = None;
        }
    }
}

impl App {
    pub fn learning_starter_picker_move(&mut self, delta: isize) {
        if let AppMode::Learning(state) = &mut self.mode
            && let Some(picker) = &mut state.starter_picker
        {
            let len = picker.indices.len();
            if len == 0 {
                return;
            }
            picker.selected = (picker.selected as isize + delta).rem_euclid(len as isize) as usize;
        }
    }
}

impl App {
    pub fn learning_close_starter_picker(&mut self) {
        if let AppMode::Learning(state) = &mut self.mode {
            state.starter_picker = None;
        }
    }
}

impl App {
    /// Load the highlighted preset into the prompt — editable, not asked.
    pub fn learning_starter_picker_confirm(&mut self) {
        if let AppMode::Learning(state) = &mut self.mode {
            let picked = state
                .starter_picker
                .as_ref()
                .and_then(|p| p.indices.get(p.selected).copied())
                .and_then(|i| STARTER_QUESTIONS.get(i));
            state.starter_picker = None;
            if let (Some(preset), Some(q)) = (picked, &mut state.question) {
                q.editor = crate::editor::TextEditor::new(preset.text.to_string());
                q.intent = preset.intent;
                q.sync_to_cursor = true;
            }
        }
    }
}

/// Turn a headless failure into something a newcomer can act on. The common
/// case by far is "that CLI isn't installed", which has a specific fix.
pub(super) fn headless_failure_message(harness: &AgentKind, err: &anyhow::Error) -> String {
    let raw = err.to_string();
    let name = harness.display_name();
    if raw.contains("not found") || raw.contains("No such file") {
        format!(
            "{name} isn't installed or isn't on your PATH, so it couldn't answer. \
             Press A on the dashboard to set up a harness, or switch harness here."
        )
    } else {
        format!("{name} couldn't answer: {raw}. Try again, or switch harness here.")
    }
}
