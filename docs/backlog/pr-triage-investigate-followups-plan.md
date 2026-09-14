# PR Triage — Investigate: follow-ups

- **Status:** Backlog
- **Owner:** unassigned
- **Relates to:** [PR Triage](pr-comment-review-plan.md) (the shipped
  "Investigate a review comment" backlog entry), PR #602. The working
  plan and task-by-task notes that drove the initial build lived in a
  local `AMF_PLAN.md`; its settled decisions are folded into the PR
  Triage plan's backlog entry, and the open items are collected here.

## Why / problem

The Investigate feature shipped in PR #602: `v` in the PR Triage list
runs a strictly read-only headless pass on the selected comment, the
answer persists per PR and renders in the detail panel, and `a` opens an
action menu (post reply / ask follow-up / dismiss / keep as TODO). Two
gaps surfaced immediately in use.

## Proposed design

### 1. Reply with the investigation from `R`

Today the only way to send an investigation's findings as a GitHub reply
is `a` → "Post a reply". The `R` reply-kind picker still offers just
**Done** and **Not needed** (`ReplyKind::ALL`). When the selected comment
has a *completed* investigation, `R` should also offer **Investigation
findings** — the same path `a` uses (`ReplyKind::Investigation`, seed =
the answer under a short preamble, posting marks the comment `Replied`).

Shape:
- `pr_review_open_reply_pick` builds the picker's row list dynamically
  instead of iterating `ReplyKind::ALL`: `[Done, NotNeeded]`, plus
  `Investigation` when `state.investigations` holds a `Complete` row for
  the selected comment.
- `ReplyKindPickState` carries the row list (or an index into a computed
  `Vec<ReplyKind>`), and `pr_review_reply_pick_confirm` routes the chosen
  kind — `Investigation` goes through the existing `pr_investigation_post_reply`
  / `open_reply(ReplyKind::Investigation, …)` seam.
- `?` help + the pane footer note that `R` includes the investigation
  when one exists.

### 2. Detail-pane scroll ergonomics

`Ctrl+D` / `Ctrl+U` and `PgUp` / `PgDn` already scroll the right-side
detail pane (clamped against `detail_content_lines`), and the pane footer
now advertises `^d/^u scroll`. Remaining niceties:

- **Auto-scroll to the answer.** When an investigation completes, scroll
  the detail pane to the `Investigation (read-only)` section so the
  answer (and not the diff hunk above it) is on screen without manual
  scrolling. Reset `detail_scroll` on selection change as today.
- **Wheel / `j`-`k` in the detail pane.** Consider a mouse-wheel binding
  over the detail rect, and/or letting `j`/`k` scroll the detail pane
  once the comment list is at a boundary (or under a modifier), so a long
  answer + follow-up thread doesn't require reaching for `Ctrl`.
- **Follow-up thread affordance.** A long answer can push the follow-up
  turns below the fold with no hint they exist; a `▾ N follow-ups`
  marker at the section header (or a count in the status chip line) would
  make them discoverable.

## Progress

- [ ] `R` reply-kind picker offers "Investigation findings" when the
  selected comment has a completed investigation.
- [ ] Auto-scroll the detail pane to the investigation answer on
  completion.
- [ ] Mouse-wheel / `j`-`k` scrolling for the detail pane.
- [ ] Follow-up-thread count/affordance on the section header.

## Shipped since

- **Optional user-provided context for Investigate.** `e` in the PR
  Triage pane opens an "Investigation context (optional)" box; the note
  is folded into the next `v` run's prompt as a hypothesis to verify
  against the PR and repo (not a fact to assume), shown in a banner so
  it can be reviewed or cleared, and consumed by the run it applies to.
  Empty box keeps the prior behaviour byte-for-byte. Built from a local
  `AMF_PLAN.md`; the Investigate prompt is an inline builder
  (`build_investigation_prompt`), not a registry template, so the note
  is threaded as a `user_context` field on `InvestigationPromptContext`.

## Open questions

- Should posting an investigation reply (from `R` or `a`) offer to also
  **dismiss** the investigation in the same step, since a reply is often
  the end of it?
- If the comment already has an AMF-authored reply, should the
  investigation reply thread under it or post fresh (same question the
  Done/Not-needed replies already answer via `reply_target`)?

## Reasoning / when to build

Both are ergonomic polish on a shipped feature — pick them up the next
time PR Triage is open for work, or when the detail pane grows enough
content elsewhere (AI Review notes, longer threads) that scrolling it is
a common friction rather than an Investigate-only one.
