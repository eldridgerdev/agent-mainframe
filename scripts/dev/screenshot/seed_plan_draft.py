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

Usage: seed_plan_draft.py <path-to-amf.db>
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
    if len(sys.argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2

    conn = sqlite3.connect(sys.argv[1])
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
            PLAN,
        ),
    )
    conn.commit()
    print(f"seeded a draft plan interview for {feature_name} ({feature_id})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
