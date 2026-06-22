# Prompt Library

- **Status:** In progress _(phases 1–3 shipped in v0.26.0: multi-source picker,
  fill-in flow, and inline select-menus landed; phase 3 tags + phase 4 polish
  remain, plus phase 5 editor/injection enhancements)_
- **Owner:** unassigned
- **Relates to:** compose box (`src/app/compose.rs`),
  `AppMode::LatestPrompt` (`src/app/view.rs`), `session_bookmarks`
  (`src/app/bookmarks.rs`, `src/db/store.rs`), `FeaturePreset` /
  `CustomSessionConfig` (`src/extension.rs`), `TextEditor`
  (`src/editor.rs`)

## Why / problem

Users retype the same prompts over and over. They want to save a
prompt once and inject it into a session on demand. The feature
should grow into templates with fill-in placeholders and
select-menu choices.

## Proposed design

A **prompt library** is a named collection of prompt templates.
The user can:

- Save the current compose prompt as a library entry.
- Open a picker (`leader+P`) listing saved prompts, fuzzy-filtered.
- Inject a chosen prompt into the **compose box** when compose
  interception is on, so they can review/edit before sending. When
  compose is off for that session, paste the prompt straight into
  the agent window **without sending** (no Enter).
- Manage entries (create / edit / rename / delete) through a small
  editor dialog reusing `TextEditor`.

Templates carry optional `{{placeholder}}` slots. When a prompt
with placeholders is injected, a short fill-in flow collects values
(free text in phase 2, select menus in phase 3) and substitutes
them before the prompt reaches the session.

### Why these choices fit the codebase

- `AppMode::LatestPrompt` is a near-exact precedent: a
  `Vec<PromptEntry>` with a `selected` index, opened from a
  `ViewState`, navigated next/prev, then injected via `paste_text`
  + `Enter` or copied. The library picker is the same shape with
  CRUD added.
- `session_bookmarks` shows the pattern for runtime-mutable data
  persisted in the SQLite store (`ProjectStore`, `db/store.rs`, a
  migration, load/save).
- `FeaturePreset` / `CustomSessionConfig` show the config-file
  pattern (`~/.config/amf/config.json` global +
  `{repo}/.amf/config.json` project merge) for declarative,
  version-controllable entries.
- `TextEditor` (with vim support) is already used by `ComposeState`
  and `SteeringPromptState` and is reusable for the editor dialog.
- The leader menu (`handlers/view.rs::handle_leader_key`) is where
  a new `P` binding slots in alongside `s`, `e`, `/`.

### Injection seam

The injection seam is the **compose box** (`src/app/compose.rs`).
A shared `deliver_prompt(rendered, from_view)` step branches on
`compose_intercept_active(view)`:

- `true` → seed the compose box with the rendered text (reuses the
  `open_compose_from_view` seeding path); the user reviews and
  submits with existing send logic.
- `false` (session is a `compose_direct_targets` entry) →
  `paste_text` into the window with **no** trailing `Enter`, so
  nothing is sent automatically.

### Storage

Hybrid, source-tagged:

- Phase 1–2: user prompts live in **SQLite** (`ProjectStore`),
  runtime-mutable, like `session_bookmarks`.
- Phase 3: also read read-only templates from `ExtensionConfig`
  (global + project), merged into the same picker with a `source`
  badge (`User` / `Global` / `Project`), like `ComposeCommandSource`
  in the command picker. Read-only entries can be duplicated to the
  user library to edit.

This keeps the fast path (save now, use now) trivial while leaving
the declarative/advanced path open without a schema rewrite.

### Data model

New module `src/prompt_library.rs` (mirroring `extension.rs`):

```rust
pub struct PromptTemplate {
    pub id: String,            // uuid v4
    pub name: String,          // display title
    pub description: Option<String>,
    pub body: String,          // prompt text; may hold {{slots}}
    pub tags: Vec<String>,     // future filtering/grouping
    pub placeholders: Vec<PromptPlaceholder>, // explicit defs
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub struct PromptPlaceholder {
    pub key: String,           // matches {{key}} in body
    pub label: Option<String>, // prompt shown in fill-in flow
    pub kind: PlaceholderKind,
    pub required: bool,
}

pub enum PlaceholderKind {
    Text { default: Option<String> },
    MultiLine { default: Option<String> },
    Select { options: Vec<String> },
}
```

Notes:

- `placeholders` is optional. If empty, the fill-in flow infers
  `Text` placeholders from `{{...}}` tokens in `body`, so plain
  templates need no explicit defs.
- `source: PromptSource` (`User` / `Global` / `Project`) is
  attached at load time, not serialized into the user store.

Placeholder syntax: `{{name}}` is a text slot (label defaults to
`name`). Richer slots (select options, multi-line, defaults) are
authored via the explicit `placeholders` array in config.json in
phase 3.

### Persistence detail

**SQLite (phase 1).** `ProjectStore` gains
`#[serde(default)] pub prompt_templates: Vec<PromptTemplate>`.
Migration `007`:

```sql
CREATE TABLE IF NOT EXISTS prompt_templates (
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL,
    description  TEXT,
    body         TEXT NOT NULL,
    tags         TEXT NOT NULL DEFAULT '[]',   -- JSON array
    placeholders TEXT NOT NULL DEFAULT '[]',   -- JSON array
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    sort_order   INTEGER NOT NULL DEFAULT 0
);
```

`db/store.rs::load` reads the table into `store.prompt_templates`;
`save` clears and re-inserts (same delete-then-insert pattern used
for bookmarks/projects). `tags` and `placeholders` serialize as
JSON columns.

**config.json (phase 3).** `ExtensionConfig` gains
`#[serde(default)] pub prompt_templates: Vec<PromptTemplate>`, and
`merge_project_extension_config` merges by `name` collision
(project wins), like `feature_presets`.

### App state & modes

Add to `src/app/state.rs`:

```rust
pub struct PromptLibraryState {
    pub templates: Vec<PromptLibraryEntry>, // user + config, tagged
    pub filtered: Vec<usize>,               // fuzzy-match indices
    pub query: String,
    pub search_active: bool,
    pub selected: usize,
    pub from_view: Option<ViewState>,       // where to inject back
}

pub struct PromptEditorState {
    pub editing_id: Option<String>,   // None = new
    pub name: String,
    pub name_field_active: bool,
    pub editor: TextEditor,           // body, multi-line + vim
    pub return_to: Box<AppMode>,      // picker or view
}

pub struct PlaceholderFillState {
    pub template: PromptTemplate,
    pub placeholders: Vec<PromptPlaceholder>, // resolved order
    pub values: Vec<String>,                  // collected input
    pub current: usize,
    pub input: TextEditor,            // or select index for Select
    pub select_index: usize,
    pub from_view: Option<ViewState>,
}
```

Add to `AppMode`: `PromptLibrary(PromptLibraryState)`,
`PromptEditor(PromptEditorState)`,
`PlaceholderFill(PlaceholderFillState)`.

### App methods

New `src/app/prompt_library.rs` (mirrors `bookmarks.rs`):

- `open_prompt_library(from_view)` — build the merged,
  source-tagged list and enter `PromptLibrary`.
- `prompt_library_select_next / _prev`, `prompt_library_filter`
  (fuzzy via the existing worktree scoring helper).
- `save_current_prompt_as_template(...)` — MVP: capture the active
  compose buffer and open `PromptEditor` pre-seeded.
- `start_new_prompt_template`, `start_edit_selected_template`,
  `submit_prompt_editor` (insert/update + `save()`),
  `delete_selected_template`,
  `duplicate_selected_template_to_user`.
- `inject_selected_template()`:
   1. Resolve placeholders (explicit ∪ inferred from `{{...}}`).
   2. If none → render and `deliver_prompt` straight away.
   3. Else enter `PlaceholderFill`.
- `deliver_prompt(rendered, from_view)` — shared delivery step
  (compose-vs-window, see Injection seam).
- `submit_placeholder_fill` — substitute `{{key}}` → value, then
  call `deliver_prompt`.
- `render_template(body, &[(key, value)]) -> String` — replaces
  `{{key}}`; unfilled optional slots collapse to empty string.

### Key handlers

- `handlers/view.rs::handle_leader_key`: add `KeyCode::Char('P')`
  → `open_prompt_library(Some(view))`. (`leader+p` is prev-feature;
  use uppercase `P`.)
- `handlers/normal.rs`: dashboard `P` opens the library
  manage-only (or injects into the selected session).
- New `handlers/prompt_library.rs`:
   - picker: `j/k`/arrows nav, `/` search, `Enter` inject
     (compose-or-window per compose mode), `n` new, `e` edit, `d`
     delete (confirm), `y` duplicate-to-user, `Esc`/`Ctrl+Q` close.
   - editor: `Tab` toggle name/body focus, forward to `TextEditor`,
     `Ctrl+S`/`Enter`(name field) save, `Esc` cancel.
   - fill: per-field input, `Tab`/`Enter` next field, `Esc`
     back/cancel; `Select` uses up/down + Enter.
- Wire dispatch in `handlers/mod.rs` and the `main.rs` mode match.

### UI rendering

New `src/ui/dialogs/prompt_library.rs` (model on
`ui/dialogs/session.rs` + `ui/picker.rs`):

- `draw_prompt_library` — centered list (name, source badge,
  description), search line, right-hand body preview, footer hints.
- `draw_prompt_editor` — name field + multi-line body editor
  (reuse compose/steering rendering), placeholder summary line.
- `draw_placeholder_fill` — one field at a time with progress
  ("2/3"), label, input or option list for `Select`.
- Dispatch from `ui/dashboard.rs::draw`.

## Progress

### Phase 1 — Save & inject plain prompts (MVP)

- [x] `prompt_library.rs` model (placeholder types defined for schema
  forward-compat; no fill-in flow yet)
- [x] Migration `007` + `prompt_templates` on `ProjectStore`
- [x] `db/store.rs` load/save round-trip
- [x] `PromptLibrary` + `PromptEditor` modes in `state.rs` / `AppMode`
- [x] `app/prompt_library.rs`: open/nav/filter, CRUD, save-current,
  `inject_selected_template`, `deliver_prompt`, `render_template`
- [x] `handlers/prompt_library.rs` + dispatch wiring
- [x] `leader+P` (view) and dashboard `L` binding (dashboard `P` was
  already taken by the syntax parser picker)
- [x] `ui/dialogs/prompt_library.rs` picker + editor UI
- [x] Inject: compose box when on; paste-without-send when off
- [x] Help overlay + README keybindings + CHANGELOG entry
- [x] Tests: store round-trip, picker nav/filter clamp, `render_template`

### Phase 2 — Fill-in placeholders

- [x] `{{slot}}` inference from body
- [x] `PlaceholderFill` mode + UI + substitution
- [x] `Text` / `MultiLine` placeholder kinds with defaults
- [x] "Save to library" action from the `LatestPrompt` menu
  (save a previous prompt as a template — `s` in the recent-prompts menu)
- [x] Tests: `render_template` (filled, missing optional, repeated
  slot, no-slot); placeholder inference

### Phase 3 — Declarative templates & select menus

- [x] `ExtensionConfig.prompt_templates` + project `.amf/config.json`
  with global + project merge (by name, project wins)
- [x] Export a user template to global/project `config.json` from the
  picker (`x` → `g`/`p`); same-name entries are replaced in place
- [x] Load `Global` / `Project` templates into the picker. The picker now
  merges three sources: editable `User` (SQLite) first, then read-only
  `Project` (`{repo}/.amf/config.json`), then `Global`
  (`~/.config/amf/config.json`). A `Global` entry whose name collides
  with a `Project` one is dropped (project wins). `User` entries are
  intentionally *not* deduped against config, so an exported template
  shows as both an editable copy and a read-only declarative one.
- [x] Source badges (`User` / `Project` / `Global`), color-coded by
  source, + duplicate-to-user (`y`). Edit/delete are blocked on
  read-only sources with a "duplicate first" hint.
- [x] `Select` placeholder kind (option lists) + explicit
  `placeholders` authoring via config.json. Authored inline in the body as
  `{{name|opt1|opt2}}` (key before the first `|`, options after); a bare
  `{{name}}` stays free text. Explicit config-authored `placeholders` defs
  (label / kind / default / required) still win over inferred slots.
- [ ] Tags/grouping and fuzzy filtering by tag
- [ ] Optional `amf-add-prompt` skill (parallel to `amf-add-preset`)
- [ ] Tests: config merge by name (project wins) — merge logic exists in
  `extension.rs::merge_project_extension_config` but has no dedicated test

### Phase 4 — Location handling & polish

- [ ] **Unify the project export/display repo resolution.** Two
  different resolvers disagree on what "project" means:
   - `resolve_library_repo` (picker display) uses the viewed feature's
     `project.repo` or the selected project's `repo` — i.e. the
     project's **main** repo root, shared across all its worktrees.
   - `resolve_export_repo` (export `x` → `p`) uses the same
     project-repo path *when opened from a session*, but otherwise falls
     back to `detect_repo_path()` — the git toplevel of AMF's current
     working directory, which is the **worktree** root when AMF runs
     inside one.
  Result: exporting from the dashboard (no view) can write to a
  worktree's `.amf/config.json` instead of the main repo's, so the
  picker (which reads the main repo) won't show what was just exported.
  Fix: point `resolve_export_repo`'s fallback at the same selection-based
  `resolve_library_repo` logic so display and export always agree, and
  the export always targets the project's main repo. Add a test covering
  the dashboard-export-then-reopen round-trip.
- [ ] **Show the destination path in the UI.** Make where a prompt lives
  / will be written explicit:
   - Picker: show the resolved source file path for the selected entry
     (e.g. in the preview pane header or footer) — SQLite DB for `User`,
     the `config.json` path for `Project` / `Global`.
   - Export confirm: the `x` → `g`/`p` prompt should name the exact
     target path *before* writing (the success toast already shows the
     project path after the fact, and global currently only says
     "global config"); show the global `~/.config/amf/config.json` path
     too, and ideally the project path up front so the user can confirm
     which repo it lands in (ties into the resolver-unification item).
   - Editor: when editing a `User` template, a small hint that saving
     writes to the local SQLite store (not version-controlled) vs. an
     exported config entry.

### Phase 5 — Editor & injection enhancements

- [ ] **VIM support in the prompt library editing surfaces.** `TextEditor`
  already implements vim mode (used by compose / steering), so the New/Edit
  Prompt body editor and any multi-line (`MultiLine`) placeholder fill field
  should honor the same vim keybindings and respect the user's vim toggle.
  Surface a small mode indicator and verify normal/insert/visual behave the
  same as in the compose box. Add a test that the editor enters vim normal
  mode on `Esc` and that motions/operators reach the prompt body.
- [ ] **Inject an agent skill into the prompt.** Add a way to pick an agent
  skill (the user-invocable skills available in the workspace) and inject a
  reference to it — or its expanded content — into the prompt being composed
  or filled. Likely shapes: a dedicated `Skill` placeholder kind whose option
  list is the available skills, or a picker hotkey in the editor/fill flow
  that inserts the chosen skill's invocation (e.g. `/skill-name`) at the
  cursor. Resolve where the skill list comes from (same source the command
  picker uses) and whether injection inserts the invocation token vs. the
  skill body text.

## Resolved decisions

1. **Inject target:** into the compose box when compose
   interception is on; when off, paste into the agent window
   without sending (no Enter). Never auto-submits.
2. **Save scope (MVP):** save the current (compose) prompt only.
   Saving previous prompts — including a "save to library" action
   in the existing `LatestPrompt` menu — is a phase 2 follow-up.
3. **Project-scoped templates** (`.amf/config.json`) wait until
   phase 3 with the rest of the declarative config path; phase 1 is
   SQLite-only.

## Reasoning / when to build

The MVP is mostly a recombination of existing patterns
(`LatestPrompt` picker shape, `session_bookmarks` persistence,
`TextEditor` editing, compose-box delivery), so phase 1 is
low-risk and high-value on its own. Placeholders (phase 2) and
declarative/team templates (phase 3) are additive and can be
scheduled independently once phase 1 ships.
