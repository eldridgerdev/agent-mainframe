# Docs site feature coverage

- **Status:** All 7 epics shipped and verified.
- **Owner:** unassigned
- **Relates to:** `site/content/docs/*.md` (the Zola docs site), `README.md`
  (source of truth the site was seeded from), `CLAUDE.md` (implementation
  detail for the features below, not user-facing).

## Why / problem

`site/content/docs/` (10 pages: quick-start, installation, core-concepts,
keybindings, learning-mode, review-and-pr-feedback, prompts-and-todos,
custom-prompts, attention-and-limits, configuration) covers the features
README leads with, but an audit against `src/` and `CLAUDE.md`'s feature
notes turned up shipped, user-facing functionality with **no page, section,
or mention anywhere on the site** — including two (`amf doctor`, the
automation CLI) that are already written up in README's Troubleshooting/
Automation sections and simply never got a site page. A new user has no way
to discover these except reading source or `CLAUDE.md`, neither of which is
meant for them.

## Proposed design

Grouped into 7 epics by how they'll land on the site — a new page vs. a
section added to an existing page. Every epic only touches its own file(s),
so none blocks another; sequencing below is by user-facing value, not
dependency.

| Epic | Priority | Needs | Summary |
|---|---|---|---|
| 1. Operational docs parity | P0 | — | Port `amf doctor`, the automation CLI, and the debug log from README into site pages |
| 2. Attention & usage visibility | P0 | — | Document context-window tracking, token/usage tracking, and session summaries |
| 3. Multi-harness capability matrix | P0 | — | New reference page: what Claude/Codex/OpenCode/Pi each support |
| 4. Project config reference (`amf.json`) | P1 | — | Document the extension schema: presets, sessions, hooks, prompts, prompt overrides |
| 5. Session lifecycle extras | P1 | — | Bookmarks, saved-transcript resume, custom session icons, Fresh Context handoff |
| 6. Plan-interview reference docs | P2 | — | `Ctrl+D` attach-docs flow, folded into core-concepts.md's Plan mode section |
| 7. Niche/advanced features | P2 | — | Remote Control badge, steering prompt constraints, combined-batch fix-cost disclosure |

Each epic below has its own checklist and verification. Check items off as
they land; keep this doc current.

### Epic 1 — Operational docs parity (P0)

The lowest-effort, highest-value epic: this content already exists in
README, it just needs a home on the site. Landed as a new
`troubleshooting.md` page rather than folding into `configuration.md`, to
keep operational/support content separate from setup content.

- [x] `amf doctor` — read-only health report (agent count vs. limit, editor
      windows, memory/swap, orphaned `amf-*` tmux sessions/worktrees,
      stopped-feature editors still running, legacy `.amf/config.json`
      projects), `--json` output, always exits `0`.
- [x] Automation CLI (`amf automation create-project` /
      `create-feature` / `create-batch-features`) — links out to
      `docs/automation/README.md` on GitHub for the request templates
      rather than duplicating them, the way README does.
- [x] Debug log — `D` on the dashboard, `~/.local/state/amf/debug.log`.
- [x] "Agent not appearing at creation" tip. Nerd Font/icon fallback was
      **not** duplicated — `configuration.md` already documents
      `nerd_font: false`, so the new page cross-links it instead.

Verification: `zola build` and `zola check` (v0.23.5, matching
`.github/workflows/site.yml`) both pass — all 11 docs pages render, the new
page appears in the nav at its weight-100 slot with no template changes
needed (`docs-index.html`/`docs-page.html` iterate `section.pages`
automatically), and both internal cross-links resolve
(`@/docs/installation.md#release-bundle-recommended`,
`@/docs/configuration.md`).

Getting a working `zola` to actually run those checks turned up a second,
pre-existing, unrelated bug that blocked **every** docs page (not just this
one): `components.html`/`docs-index.html`/`docs-page.html` used Tera v1
macro/import syntax, which Tera v2 (bundled since Zola 0.22) removed
outright — the site's own CI "Build" step has been failing on every push to
`site/**` on `main`. Fixed as part of landing this epic; full writeup and
repro in
[bug-backlog-plan.md](bug-backlog-plan.md#docs-site-fails-to-build-tera-v1-macro-syntax-under-tera-v2-fixed).

### Epic 2 — Attention & usage visibility (P0)

These are UI elements a user sees on every session (context budget, usage
sidebar, auto-generated titles) with zero explanatory text anywhere on the
site today. Landed as two new sections in `attention-and-limits.md` (it
already owns "what does AMF show me about my agents") rather than a new
page.

- [x] Context-window tracking — the per-session context-budget indicator
      and band (`src/context_tracking.rs`, `src/context_display.rs`,
      `src/app/context_hints.rs`). Documented the exact sidebar text
      (`Ctx 42% · 12,345`, `~` for estimated, `WARNING`/`CRITICAL`/`STALE`
      suffixes) and the warning/critical-band hint's `<leader F>` fresh-
      context / `<leader X>` dismiss actions, pulled from
      `format_context_indicator` (`src/context_display.rs`) and the
      sidebar layout in `src/ui/pane.rs`.
- [x] Token/usage tracking — the sidebar usage windows
      (`src/usage.rs::format_sidebar_usage_windows`, e.g. `5h  62% left ·
      3h`), including that OpenCode and Pi have no known usage API and so
      show no Usage section at all.
- [x] Harness-aware session summaries — `Z` on the dashboard / leader `g`
      in a session (`src/app/sync.rs::trigger_summary_for_selected`,
      `src/summary.rs`) generates a ≤60-char AI summary of recent
      terminal activity onto the feature's dashboard row. On-demand only,
      not automatic; runs in the same restricted no-tools headless mode
      as Learning Mode's quick answers. Both keys were missing from
      `keybindings.md`'s tables too — added there for consistency with
      how `z`/`i` (also covered on `attention-and-limits.md`) are listed
      in both places.

Verification: `zola build`/`zola check` (v0.23.5) both pass; content was
derived from the actual formatting/keybinding functions in source rather
than the doc comments alone, since sidebar text is exactly what a user
sees. Screenshots already exist at `docs/screenshots/context-hint/`,
`docs/screenshots/sidebar-usage-limit/`, and
`docs/screenshots/harness-aware-session-summary/` if the page later wants
images.

### Epic 3 — Multi-harness capability matrix (P0)

Harness differences are currently scattered one-liners (Vibeless support in
`core-concepts.md`, hook fidelity in `attention-and-limits.md`, Codex's
forced read-only Learning Mode in `learning-mode.md`). A user choosing an
agent for a new feature has to piece this together from four pages. Landed
as a new page, `harnesses.md` (weight 32, right after Core Concepts).

- [x] One table: Claude Code / Codex / OpenCode / Pi × permission-mode
      support (Vibeless/Vibe/SuperVibe), attention-state fidelity
      (Question/Completed/Waiting), Learning Mode default run mode,
      context-window tracking, usage/quota sidebar, saved-session resume,
      prompt-override per-harness variants.
  - [x] Every row checked against source rather than re-derived from
        scratch: Codex's Vibeless block is a hard `bail!` in
        `App::ensure_agent_mode_supported` (`src/app/mod.rs`, not just "no
        edit-review hook"); Pi's launch path
        (`TmuxManager::launch_pi`/`launch_opencode_with_session`,
        `src/tmux.rs`) takes no permission flags at all, unlike
        `launch_claude`/`launch_codex`; context-window collectors exist
        for all four harnesses (`src/context_collectors.rs`) while usage
        windows are Claude/Codex-only (`src/usage.rs`); saved-session
        pickers exist for Claude/Codex/OpenCode
        (`claude_session_picker.rs`, `codex_session_picker.rs`,
        `opencode_storage.rs`) and not for Pi (confirmed no
        `pi_session_picker` exists).
- [x] Linked from `core-concepts.md` (Permission modes paragraph) and
      `attention-and-limits.md` (fidelity paragraph, plus the new Epic 2
      context/usage section) instead of duplicating the table there.

Verification: `zola build`/`zola check` (v0.23.5) both pass, all cross-links
(including the two new heading anchors this epic added to
`attention-and-limits.md`) resolve. Not yet spot-checked against a real
session per harness — flagged in Open questions.

### Epic 4 — Project config reference (`amf.json`) (P1)

`configuration.md` names global/`amf.json` locations and the resource-guard
keys but never describes what a project can actually declare —
`extension.rs`'s schema for presets, custom sessions, hooks, plan-interview
questions, and project-scope prompt overrides (the last of which
`custom-prompts.md` already documents in isolation). Landed as a new
`project-config.md` page (weight 92, right after Configuration), since the
full `ExtensionConfig` struct turned out to have 12 top-level keys — too
much for a clean append to `configuration.md`.

- [x] Table of all 12 `ExtensionConfig` fields (`src/extension.rs`):
      `custom_sessions`, `feature_presets`, `lifecycle_hooks`,
      `keybindings`, `allowed_agents`, `plan_questions`,
      `skip_builtin_questions`, `prompt_templates`, `prompt_overrides`,
      `final_review_check_command`, `review_memory_path`,
      `review_prompt_budget_tokens` — more than the epic's original scope
      (presets/sessions/hooks/questions/`prompt_overrides`), since reading
      the whole struct turned up the last four as equally undocumented.
- [x] Which have a config-wizard path vs. hand-edit-only, checked against
      `ConfigCategory` in `src/app/config_wizard.rs` (six categories:
      CustomSessions, FeaturePresets, PlanQuestions, LifecycleHooks,
      Keybindings, AllowedAgents — matching `configuration.md`'s existing
      bullet list exactly) rather than assumed; `prompt_templates` and
      `prompt_overrides` each have their own dedicated UI (`L`, `E`)
      instead, noted separately from "hand-edit."
- [x] Per-field merge rule (project-appends-and-wins-on-collision vs.
      project-replaces-outright vs. project-only) taken directly from the
      doc comment on `merge_project_extension_config`
      (`src/extension.rs`), not re-derived — it already states the exact
      rule per field.
- [x] Cross-linked from `custom-prompts.md`'s existing `amf.json`
      `prompt_overrides` example and from `configuration.md`'s wizard
      bullet list, instead of re-documenting either.

Verification: `zola build`/`zola check` (v0.23.5) both pass — 13 pages,
new page slots in at weight 92 between Configuration and Troubleshooting,
all three new cross-links resolve. Every key name and merge rule was
checked against `src/extension.rs` source (the struct definition and the
`merge_project_extension_config` doc comment) rather than assumed.

### Epic 5 — Session lifecycle extras (P1)

Smaller session-management features with no mention anywhere. Landed as a
new page, `session-tools.md` (weight 37, right after Keybindings) — once
all four turned out to carry their own keys (bookmarks alone has four:
`h`/`H`/`M`/digit), cramming them into `keybindings.md`'s tables would have
broken that page's own stated convention of keeping feature-specific keys
on their own page (Learning Mode, PR Triage, TODOs already work this way).
Added one pointer to `keybindings.md`'s "Where the rest live" list instead.

- [x] Session bookmarks — up to 9 pinned slots (`src/app/bookmarks.rs`).
      Keys confirmed in `src/handlers/normal.rs` (`handle_normal_leader_key`
      — yes, the dashboard has its own `Ctrl+Space` leader menu too, not
      just embedded sessions, which keybindings.md never mentioned) and
      `src/handlers/view.rs`: leader `h` opens the picker, `H` bookmarks,
      `M` unbookmarks, `1`–`9` jump to a slot. Oldest-slot eviction and
      stale-slot self-clearing pulled from `bookmark_current_session` /
      `jump_to_bookmark`.
- [x] Saved-transcript resume — dashboard/session `S` on a Claude, Codex,
      or OpenCode feature (`src/handlers/normal.rs`), which the code
      comment notes works even for a stopped feature and reaches
      transcripts older than what AMF recorded. Confirmed Pi has no
      picker (no `pi_session_picker` anywhere), consistent with the
      Epic 3 harness matrix.
- [x] Fresh Context handoff (`src/app/handoff.rs`) — leader `Shift+F` opens
      an editable prompt; the new session is seeded with the feature's
      plan file, a capped sample of changed files, and the typed
      instruction, left unsent for review. Documented that triggering it
      from the Epic 2 context-hint (vs. the plain leader command) swaps in
      a generated continuation instruction instead of the diff+plan
      template — confirmed via `FreshContextPromptSource` in
      `src/app/handoff.rs`. Leader `Shift+X` (dismiss the hint) documented
      alongside it and cross-linked back to the Epic 2 section.
- [x] Custom session icons (`src/custom_session_icons.rs`) — 14 curated
      Nerd Font icons or any typed glyph/emoji, folded into the page as a
      short closing section rather than `configuration.md`'s bullet list,
      to keep all "session" content together.

Verification: `zola build`/`zola check` (v0.23.5) both pass — 14 pages, new
page slots in at weight 37, all cross-links (including the Epic 2/3 back-
references) resolve. Not driven against a live `amf` session — flagged in
Open questions alongside Epic 3's.

### Epic 6 — Plan-interview reference docs (P2)

One flow, folded into existing content: `core-concepts.md`'s Plan mode
section previously stopped at "review and edit the resulting plan." It was
missing that attaching reference docs (`Ctrl+D` on the brief step, up to 4
files) switches the round/synthesis/critique passes from no-tools to
read-only-repo mode.

- [x] Added a short paragraph to `core-concepts.md`'s Plan mode section:
      `Ctrl+D`/`Ctrl+X`, the exact cap and per-file size limit (4 docs,
      512KB each — `MAX_ATTACHED_DOCS`/`ATTACHED_DOC_MAX_BYTES` in
      `src/plan_interview.rs`), and that attaching a doc is what unlocks
      repo-reading during the interview, confirmed brief-step-only via the
      `PlanInterviewPhase::Brief` guard in
      `src/handlers/plan_interview.rs`.

Verification: `zola build`/`zola check` (v0.23.5) pass.

### Epic 7 — Niche/advanced features (P2)

Lowest-traffic features; bundled into whichever adjacent pages made sense
rather than a dedicated page each.

- [x] Remote Control badge — the `[remote ●]` indicator
      (`src/app/remote_control.rs`) plus its three leader keys (`c` copy
      URL, `Shift+O` open URL, `Shift+C` send `/rc` to toggle — confirmed
      in `src/handlers/view.rs` and `src/app/view.rs`). Landed as a new
      section in `session-tools.md` rather than `keybindings.md`, to keep
      it with the rest of Epic 5's session-surface content.
- [x] Steering prompt coaching — turned out to be more than "constraints
      attachable to a prompt": it's a live-scored task-prompt editor
      (`src/app/steering.rs`'s five checks — file scope, acceptance
      criteria, invariants, validation commands, risks — each with a
      missing-explanation and a teaching tip) reachable both as a
      create-feature wizard toggle (seeds the *first* prompt) and via
      leader `s` on a running session (pre-filled with the latest prompt,
      for mid-course correction) — confirmed via
      `open_startup_steering_prompt` / `open_steering_prompt_from_view` in
      `src/app/feature_ops.rs`. Landed in `prompts-and-todos.md`'s "Reuse
      prompts" area as its own section, since the epic's original framing
      undersold what this feature actually does.
- [x] Combined-batch fix-cost disclosure — added the exact wording
      (`Fix cost (est.): $0.04 · combined (3)`, pulled verbatim from
      `fix_cost_line`'s doctest in `src/app/fix_cost.rs`) to
      `review-and-pr-feedback.md`'s existing cost-disclosure paragraph,
      and named the `B` batch key in the PR Triage paragraph above it
      (previously described only as "an individual or batched fix
      prompt" with no key named).

Verification: `zola build`/`zola check` (v0.23.5) pass — 14 pages, all
three new headings/anchors render, the exact `combined (3)` string appears
verbatim in the built HTML.

## Open questions

- **Nothing in this doc has been checked against live sessions.** Every
  key, table cell, and piece of wording across all 7 epics is sourced from
  reading `src/` (launch-arg construction, hook/collector presence,
  handler match arms, exact format strings) rather than from actually
  running the four harnesses and pressing the keys. That's reliable for
  "does the code path exist and what does it literally render," but a
  live walkthrough with all four CLIs installed would still be worth
  doing before treating this as the last word — especially anything
  UI-layout-dependent that source alone can't show (the harness matrix
  from Epic 3, the bookmark/resume/handoff flows from Epic 5).
- **Screenshots.** Several of these features already have screenshots under
  `docs/screenshots/` (context-hint, sidebar-usage-limit,
  harness-aware-session-summary, agent-limits). None of the new pages use
  images yet; worth reusing those via the `amf-screenshot` skill's gallery
  rather than recapturing, where the existing ones are still current.
- **Keeping README and the site in sync going forward.** This audit exists
  because README grew ahead of the site, and by the end of it the site
  covers several things (the harness matrix, the `amf.json` field
  reference, session tools) that README itself doesn't mention either. No
  process currently catches that drift in either direction — worth a
  follow-up note (maybe in `docs/development/checks.md`).

## Reasoning / when to build

All 7 epics are now shipped. They were picked up in priority order:
1–3 first (independent, quick, closed gaps in content real users hit
immediately — troubleshooting, and UI elements visible in every session),
then 4–5 (reference material for customizing AMF), then the smaller
6–7. Landing each turned up something the epic's original framing hadn't
anticipated — a second pre-existing site-build bug (Epic 1), four more
`amf.json` fields than scoped (Epic 4), an undocumented dashboard leader
menu (Epic 5), and a task-prompt coaching feature bigger than "constraints
attachable to a prompt" (Epic 7) — which is the risk of writing docs from
source rather than from watching a user get confused: source shows what
exists but not how big a topic actually is until you're reading all of it.
