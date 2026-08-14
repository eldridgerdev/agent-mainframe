#!/usr/bin/env python3
"""Write a `draft`-stage plan interview, already carrying a generated plan,
into a screenshot instance's scratch database.

Reaching the plan review gate normally costs a real headless synthesis call:
tokens, non-deterministic markdown, and several seconds of loading frame. A
draft that already holds a plan reopens straight at the review gate instead of
synthesizing again, so seeding one is how a capture can show what happens
*after* an accept without paying for one.

Keyed on the single feature the screenshot seeds create, so the interview AMF
opens with `P` on that feature finds it.

`--long` swaps in a plan that overflows the review pane at the harness's
default 120x40 geometry. The short plan fits on one screen, where the pane's
scroll offset is clamped to zero and a scrolling capture would have nothing to
show; anything proving scrollback needs a plan that is actually taller than the
viewport.

Usage: seed_plan_draft.py <path-to-amf.db> [--long]
"""

import json
import sqlite3
import sys

PLAN = """# Plan: sidebar-polish

## Goal
Truncate the sidebar plan preview on word boundaries so half-words never
reach the dashboard.

## Decisions
- Truncate at the last word boundary that fits, then append an ellipsis.
- Scrollback and wrapping are out of scope for this feature.

## Architecture
`read_plan_preview` gains the boundary-aware truncation; the sidebar cache
is unchanged.

## UI
Only the sidebar plan preview changes. No new dialogs.

## Tasks
- [ ] Add boundary-aware truncation to the preview builder
- [ ] Cover the mid-word and no-boundary cases with unit tests
- [ ] Verify the dashboard at narrow widths

## Risks / open questions
- Very long single tokens (paths, URLs) still have no boundary to break on.
"""

# Deliberately taller than a 40-row pane, and sectioned so which part of the
# plan is on screen can be read off a capture at a glance -- a scroll capture
# has to show *different* content, not just a redrawn frame.
LONG_PLAN = """# Plan: plan-review-mouse-scroll

## Goal
Make the mouse wheel scroll the plan at the review gate, instead of
falling through to the dashboard list hidden behind the dialog.

## Background
`handle_scroll_up` / `handle_scroll_down` match a chain of modes -- the
debug log, the markdown viewer, the help overlay, the diff review, the
embedded tmux view -- and then fall through to `select_prev()` /
`select_next()`. `AppMode::PlanInterview` was never in that chain, so a
wheel notch over the plan silently moved the dashboard selection.

## Decisions
- Route the wheel through one accessor on the interview state rather
  than matching phases inside the mouse handler.
- Return an offset only for phases that actually render a scrolling
  pane; every other phase swallows the event.
- Do not clamp in the mouse path: each renderer already clamps the
  offset it is handed against the laid-out content.

## Architecture
`PlanInterviewState::scroll_offset_mut` maps the current phase to the
offset of the pane it renders:

- `Review` -> `review_scroll_offset`
- `Critique` -> `critique_scroll_offset`
- `Editing`, `DirectedFeedback`, `Investigation` -> `edit_scroll_offset`
- everything else -> `None`

The abort confirmation takes the whole dialog over, so it reports
`None` too.

## UI
Three lines per notch, matching the debug log, the markdown viewer and
the help overlay. No new keybinding, no new hint text: the wheel now
does what the `j`/`k` hint already promised.

## Edge cases
- A phase with nothing to scroll still returns early, so the hidden
  dashboard selection cannot drift underneath the dialog.
- Clicks had the same gap: `handle_click` keeps an explicit list of
  dialog modes where a click is inert, and the interview was missing
  from it, so a double-click inside the dialog could reach a feature
  row underneath and start it.
- The editor phases share one offset with the cursor-sync path, which
  re-centres on the next keystroke. Scrolling away and typing snaps
  back, which is the same behaviour every other editor in AMF has.

## Tasks
- [ ] Add the phase-to-offset accessor on the interview state
- [ ] Route both wheel directions through it
- [ ] Add the interview to the inert-click list
- [ ] Cover the review pane, the critique pane, and the swallow case

## Testing
Three handler tests: the review pane moves, the critique pane moves
without touching the plan's offset, and an unscrollable phase leaves
`app.selection` untouched.

## Risks / open questions
- The editors inherit wheel scrolling as a side effect of sharing
  `edit_scroll_offset`. That is a small feature, not a regression, but
  it was not asked for.
- Terminals that do not report SGR mouse events are unaffected either
  way; nothing here changes what AMF asks the terminal for.

## Out of scope
- Drag-selecting text inside the plan pane.
- A scrollbar or any other new chrome on the review gate.
- Wheel support in the dialogs that still swallow it entirely.
"""

QUESTIONS = [
    {
        "id": "scope",
        "text": "What is in scope for this feature, and what is explicitly out of scope?",
        "kind": "free_text",
        "source": "builtin",
        "optional": True,
    },
    {
        "id": "ui-surface",
        "text": "What user interface or interaction changes should this feature introduce?",
        "kind": "free_text",
        "source": "builtin",
        "optional": True,
    },
]

ANSWERS = [
    "Word-boundary truncation in the sidebar plan preview. Scrollback is out of scope.",
    "Only the sidebar preview line changes; no new dialogs or keybindings.",
]

BRIEF = (
    "Truncate the sidebar plan preview on word boundaries instead of mid-word."
)


def main() -> int:
    args = sys.argv[1:]
    long_plan = "--long" in args
    positional = [a for a in args if not a.startswith("--")]
    if len(positional) != 1 or len(positional) != len(args) - int(long_plan):
        print(__doc__, file=sys.stderr)
        return 2
    plan = LONG_PLAN if long_plan else PLAN

    conn = sqlite3.connect(positional[0])
    row = conn.execute("SELECT id, name FROM features").fetchone()
    if row is None:
        print("no seeded feature to key the draft on", file=sys.stderr)
        return 1
    feature_id, feature_name = row

    conn.execute(
        """INSERT OR REPLACE INTO plan_interviews
            (feature_id, stage, feature_name, brief, questions, answers, plan,
             ai_rounds_completed, created_at, updated_at)
           VALUES (?, 'draft', ?, ?, ?, ?, ?, 0,
                   datetime('now'), datetime('now'))""",
        (
            feature_id,
            feature_name,
            BRIEF,
            json.dumps(QUESTIONS),
            json.dumps(ANSWERS),
            plan,
        ),
    )
    conn.commit()
    kind = "long" if long_plan else "short"
    print(f"seeded a {kind} draft plan interview for {feature_name} ({feature_id})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
