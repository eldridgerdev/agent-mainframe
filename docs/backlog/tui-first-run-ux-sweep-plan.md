# TUI first-run UX sweep

- **Status:** Ready
- **Owner:** unassigned
- **Relates to:** dashboard key handling (`src/handlers/normal.rs`), leader menu
  (`src/ui/pane.rs` — `draw_leader_menu`), diff base resolution (`src/diff.rs`),
  feature/project wizards (`src/handlers/feature_creation.rs`,
  `src/ui/dialogs/feature_setup.rs`, `src/ui/dialogs/project.rs`), help overlay
  (`src/ui/dialogs/help.rs`), toasts (`src/ui/toast.rs`), TUI driver
  ([`tui-ai-driver.md`](../development/tui-ai-driver.md)),
  [`public-launch-polish-plan.md`](public-launch-polish-plan.md).

## Why / problem

A sweep of the TUI as a brand-new user building an app with an agent harness
(2026-10-26, AMF 0.50.0, 120×40 and 80×24, private driver instance, fake
`claude` stub, scratch git repo). The user knew no keybinds and had to find
them through `?`, the leader menu and on-screen footers. Source was **not**
read during the sweep: every finding below is an observation from the rendered
screen, and each epic's first task is to confirm the cause in code before
changing it.

Four findings can lose work or strand a new user, and are first. The rest are
confusion and polish.

## Priorities and parallelization

| Epic | Priority | Size | Touches | Depends on |
| --- | --- | --- | --- | --- |
| 1. Destructive / dead-end actions | P0 | S | `handlers/normal.rs`, `app/` session removal | none |
| 2. Diff viewer on a plain repo | P0 | M | `diff.rs`, `app/review.rs`, `ui/dialogs/diff.rs` | none |
| 3. Non-git projects | P0 | S | `app/project_ops.rs`, `github.rs` | none |
| 4. Error and toast hygiene | P1 | S | `app/*`, `handlers/picker.rs`, `ui/toast.rs` | none |
| 5. Leader menu clipping | P1 | S | `ui/pane.rs` | none |
| 6. Wizard consistency | P1 | M | project + feature wizards | none |
| 7. Discoverability | P1 | M | `ui/dashboard.rs`, `ui/dialogs/help.rs` | 5 (shared key table) |
| 8. Polish | P2 | S each | various | none |
| 9. Verification | P0 | S | driver scenarios | 1–8 as they land |

- Epics 1, 2 and 3 are independent; start with them.
- **Run at most two implementation subagents at a time** (more has crashed WSL).
  Suggested pairs: (1 + 2), then (3 + 4), then (5 + 6), then (7), then (8).
- Epics 5 and 7 both want a single source of truth for key names/descriptions
  (`LEADER_COMMANDS`, the help table). Do 5 first, then reuse its data in 7.
- Epic 9 is not a final step: add each regression check in the same change
  that fixes the finding.

## Epic 1 — Destructive and dead-end actions (P0)

**1a. Esc on the dashboard quits AMF with no confirmation.** `handlers/normal.rs`
binds `KeyCode::Char('q') | KeyCode::Esc` to `request_dashboard_quit()`, which
sets `should_quit` unless a plan interview is parked. During the sweep it
happened four times: Esc was pressed to dismiss a toast or a modal that wasn't
capturing keys, and AMF exited with status 0.

- [x] Confirm every path that reaches the dashboard handler with Esc (dismissed
      toast, closed non-modal popup, overlay that returned without consuming it).
- [x] Decided: Esc prompts "Quit AMF? y/n"; `q` still quits directly.
- [x] Keep the parked-plan-interview behaviour of `request_dashboard_quit()`.
- [x] Update the `?` help line ("q / Esc  Quit") and any docs/README/site text
      that says Esc quits (`grep -rn "Esc" site/content/docs/keybindings.md`).
- [x] Unit test: Esc on the dashboard does not set `should_quit`; `q` does.

**1b. `d` on a session removes it immediately.** The `Selection::Session` arm of
`KeyCode::Char('d')` calls `app.remove_session()` directly. On a live agent
session this printed "Removed 'Claude 1'" and the agent was killed. Feature and
project deletes already confirm.

- [x] Confirm what `remove_session()` does to a running tmux window.
- [x] Add a confirm step (reuse the `DeletingFeature` dialog shape) that names the
      session and says whether an agent is running. Decided: always ask, even
      for a stopped session.
- [x] GUI path (`gui_contract.rs`): left unchanged. It is a direct API call
      whose frontend owns its own confirmation; the remote web UI already
      confirms. TUI only.
- [x] Unit test: `d` on a live session opens the confirm and removes nothing;
      `y` removes; `n`/Esc cancels.

## Epic 2 — Diff viewer on a plain repo (P0)

The diff scope picker and viewer fail for a repo on its only branch (`main`, no
remote): "Could not determine a base branch for 'main'; tried origin/HEAD,
origin/main, main, origin/master, and master" (`diff.rs`, end of base
resolution). The viewer renders "Could not load current changes" with
"(unknown branch) vs (no base)" and 0 files, even though the working tree has an
uncommitted edit.

- [x] Reproduce with a fresh `git init -b main` repo, one commit, one edited file.
- [x] When no base branch resolves, fall back to `HEAD` as the base so staged,
      unstaged and untracked changes still show, and label the header
      "vs HEAD (no base branch found)".
- [x] Keep the strict behaviour (and its message) for the case where a base is
      expected, e.g. a feature branch that has diverged from nothing.
- [x] Make the base-ref override discoverable: when resolution falls back or
      fails, say "press `b` to choose a base ref" in the body, not only the footer.
- [x] Choose Diff Scope: stop printing the raw error under "0 commit(s)"; show
      a one-line explanation instead.
- [x] Tests in `diff.rs`: single-branch repo returns the working-tree diff; repo
      with `main` + feature branch is unchanged.
- [x] Check the Final Review viewer and Learning Mode's branch-changes scope for
      the same failure (both call `load_snapshot`).

## Epic 3 — Non-git projects (P0)

A folder that is not a git repository is accepted as a project without comment.
`g` then fails with "Error: Error: Selected project is not a git repository"
(`github.rs` `GithubRepositoryError::NotGitRepository`), and `S` showed a
garbled error popup at the bottom of the screen. (The non-git state in the
sweep was an artifact of the sweep's fixture; the missing warning is the finding.)

- [x] On project creation (`app/project_ops.rs`), detect "not a git repo" and warn
      in the dialog: what still works, what needs git (worktrees, diffs, issues,
      PRs), and offer `git init`.
- [x] Mark such projects in the tree (e.g. a `[no git]` tag), consistent with the
      existing `[repo]` / `[vibeless]` tags.
- [x] Features that need git (worktree choice, diff, GitHub issues) say so and why
      instead of failing late; hide or disable the worktree "Yes" option.
- [x] Same for "has no remotes" (`g`): say "add a GitHub remote to browse issues".
- [x] Tests: project creation on a non-git path produces the warning; `g` on it
      does not raise the generic error.

## Epic 4 — Error and toast hygiene (P1)

- [x] **Doubled prefix.** `handlers/picker.rs` (`push_toast_error(format!("Error: {}", e))`
      at three sites, plus the toast renderer's own prefix) produces
      "Error: Error: …". Pick one place to add the prefix; grep every
      `push_toast_error(format!("Error:` and `self.message = Some(format!("Error:`
      (`app/feature_ops.rs`).
- [x] Drop the trailing "Check debug log for details." when the message is already
      actionable; otherwise name the key to open the log (`D`).
- [x] **Stacked toasts overlap the bottom status line** (`┌ ! ──` fragments across
      rows 35–38). Reserve toast space above the status bar, or clear the
      status line while a toast is up, and cap simultaneous toasts.
- [x] `S` on a project with no transcripts/remotes shows a garbled popup; reproduce
      and fix the border drawing at the bottom-right corner.
- [x] Render tests (`ui/usability_tests.rs`): no row contains both a toast border
      and the status message; no doubled "Error:".

## Epic 5 — Leader menu clipping (P1)

The Ctrl+Space menu draws ~35 entries into a box that is cut off: `x Stop session`
and `c …` are partly or wholly hidden, descriptions are truncated ("Diff viewer
(all changes / commit", "Scroll m"), and there is no bottom border or scroll
hint. A taller terminal (50 rows) did not help. Arrow keys and `j` close it and
leak through to the agent.

- [x] Find why the box height is capped (`draw_leader_menu` in `ui/pane.rs`,
      `LEADER_COMMANDS`) and size it to content, bounded by the terminal.
- [x] If it can't fit, make it scrollable with `j/k`/arrows and show a
      "more ▼" indicator and a bottom border.
- [x] Wrap or shorten descriptions instead of cutting mid-word.
- [x] Group entries (sessions, review, plan, settings) with dim headings, so ~35
      single keys are scannable.
- [x] Unrecognised keys: show "no command on `j`" and keep the menu open (or close
      it **without** forwarding the key to the agent).
- [x] Render tests at 80×24, 120×40 and 120×50: last `LEADER_COMMANDS` entry is
      visible or reachable by scroll; no truncated descriptions.

## Epic 6 — Wizard consistency (P1)

**New Project** (`N`)

- [x] Repo path is prefilled with AMF's working directory (the launch directory),
      which is rarely the right project. Prefill from the selected project's
      parent, or leave blank with a placeholder; never the cwd silently.
- [x] Add a clear-field key (Ctrl+U) that works. Currently `Ctrl+U` inserted a
      literal `u`. Check what the other text fields (feature name, compose)
      do and make them agree.
- [x] Path field wraps across lines with the hint overlapping; keep the hint on
      its own row.

**New Feature** (`n`)

- [x] Footer says "Enter confirm"/"Enter next" but Enter only advances one field
      at a time (about ten presses to reach the end). Say "Enter next field"
      and show "Ctrl+Enter create" (or make a final Create row).
- [x] Tab does not move between fields here but does in New Project. Pick one.
- [x] Checkboxes toggle with `j/k` only; accept Space and show `[x]` state change
      clearly. Footer: "Space toggle".
- [x] Show which field is focused with a highlight, not only by footer text.
- [x] Explain Vibeless / Vibe / SuperVibe in one plain line each in the wizard
      (what the agent may do without asking), not only the short phrase.
- [x] The extra "Name Session" prompt after the wizard defaults to "Claude 1";
      fold it into the wizard as an optional field or skip it when unchanged.
- [x] After creating a feature, open its session (or offer "Enter to open") instead
      of leaving the user on the dashboard with a toast.

**Batch features** (`B`)

- [x] Use the selected project instead of asking for workspace path and project
      name again.
- [x] Explain on the first screen what a batch is and what it creates
      (`feature-1..N`, worktrees, no plan interview, no prompt per feature).
- [x] Offer a per-feature prompt or name list, or state that none is supported.

**Harness manager** (`A`)

- [x] Header ("Enter to check & enable") and footer ("Enter/Space toggle  c confirm
      Esc cancel  q exit") disagree; write one set of keys.
- [x] Drop the redundant `q exit` or define it versus Esc.

Done: blank path with placeholder (Ctrl+B browses beside the selected project);
`Ctrl+U` clears every single-line form field and the rename prompts (compose
keeps `Ctrl+L`); New Feature is one form with `Tab`/`Shift+Tab`, `Space`,
a `▸` focus marker, `Enter` next field and `Ctrl+S` create (`Ctrl+Enter` is
not delivered by most terminals); the session name is the last row; creating
opens the session; `B` on a selected project adds to it; the manager's `q`
only quits on first run.

## Epic 7 — Discoverability (P1)

- [x] **First-run dashboard footer.** Show a short key strip on an empty store
      and for the selected row type (project: `n` feature · `s` session ·
      `Enter` open; session: `Enter` open · `x` stop · `d` remove), with
      `? help` always visible. Keep it one line at 80 columns.
- [x] **`?` help is a flat list of ~70 keys.** Group by area (Projects and
      features · Sessions · Review · Plan · Settings) with headings; add `/`
      search inside help; move the plan-interview block to the interview's own
      screen (it already has footer hints).
- [x] Share the key/description table between help, leader menu and footers so
      they can't drift (see Epic 5).
- [x] **Session view:** show "Ctrl+Space: commands · type to compose" in the header
      next to the title. Document that Esc with compose closed goes to the agent
      (as `^[`), and what reopens compose.
- [x] Start Session picker: explain `*`, `>`, `V` glyphs or replace them with
      words; say what the "(5)" count is.
- [x] The first-time experience on an empty store points at `N`; after the project
      exists, say "press `n` to add a feature" on the project row (already done)
      **and** in the footer.

## Epic 8 — Polish (P2)

- [ ] Syntax Parsers (`p`): drop the repeated green "AVAILABLE" badge from rows;
      show only "installed" for installed parsers and a summary "0/16 installed".
- [ ] Headless Prompt Overrides (`E`): drop the repeated "built-in" tag and the
      `···` filler, align the id column, and show only the **overridden**
      entries by default with a "show all" toggle.
- [ ] `O` (AMF settings): stop adding an "amf-settings" project row to the tree as
      a side effect, or label it and explain what it is. Check the GUI path.
- [ ] Plan interview step titles: "Step 2/6 Built-in" → use the question's section
      name; the last screen also says "6/6" (as "Optional AI") — renumber.
- [ ] Footers truncated at 120 columns (compose bar `Ctrl+T vim  C┘└──`, switcher
      `s new  r`, Learning Mode "Tour this whole proj…"): wrap or shorten per
      width, and add `usability_tests.rs` coverage at 80 and 120 columns.
- [ ] Session sidebar: nested bordered boxes produce garbled edges (`└────�`);
      check ratatui border sets and wide-character clipping.
- [ ] `Esc` closes a compose box and the dashboard's behaviours differ (see 1a);
      list Esc's meaning per screen in help.

## Epic 9 — Verification (P0, runs alongside every epic)

- [ ] Add a TUI-driver scenario for the new-user path: `N` → project (non-git and
      git) → `n` → session → leader menu → diff → delete session → `q`.
      Assert on rendered text; use the fake-`claude` stub pattern from the
      sweep for a live session.
- [ ] Re-run the sweep at 80×24 and 120×40 after Epics 1–7 and compare to the
      list above; tick items here only after the rerun.
- [ ] `cargo fmt`, `cargo clippy -- -W clippy::all`, and the full test suite
      before each PR (CI lint includes `cargo fmt`).
- [ ] Update `CHANGELOG.md`, `site/content/docs/keybindings.md` and the README
      key tables for every changed key (Esc, `d`, Ctrl+U, Space toggle).
- [ ] If screenshots are wanted for the PR, capture them only on request.

## Decisions (2026-10-26)

- Esc on the dashboard **asks "Quit AMF? y/n"** (`q` behaviour unchanged).
- `d` on a session **asks for confirmation**; the session need not be stopped first.
- With no base branch, the diff viewer **falls back to `HEAD`** and says so.
- Non-git folder: **warn and offer `git init`**.
- Work happens in AMF-created worktrees off `ux-sweep`; each epic ships as its
  own **draft PR to `main`**. At most two subagents run at once.

## Open questions

- Do the GUI (`gui_contract.rs`) and remote UIs share the behaviours in Epics 1,
  3 and 6, or are they separate fixes?
- Grouped help: keep one list with headings, or split into a tabbed overlay?

## Reasoning / when to build

Epics 1–3 stop a new user from losing a running session, quitting by accident,
or hitting a diff that says nothing works. They are small, independent and
should go first. Epics 4–7 are what makes the first hour feel unfinished, and
epic 7 pays back every later feature that adds a key. Epic 8 is cleanup that can
ride along with whichever PR touches the file.
