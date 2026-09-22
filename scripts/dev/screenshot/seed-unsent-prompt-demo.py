#!/usr/bin/env python3
"""Seed a feature/session plus an "unsent prompt" pair directly in a
throwaway AMF database, for the unsent-prompt-recovery screenshot scenario.

Bypasses `automation create-feature` (and its harness-availability gate)
entirely, the same way seed-latest-prompt-session.py does: no real
`claude`/`codex` process is ever launched, so this works identically on a
machine with a harness installed and on a CI runner with a different one. A
plain tmux window stands in for the pane.

Inserts, for the same feature `workdir`:
  - a `features` + `feature_sessions` row (the feature the prompt was headed to)
  - a `prompt_templates` row tagged "unsent" (what App::stash_lost_prompt
    saves into the Prompt Library, leader P)
  - an `unsent_prompts` row (what the same call stashes for Latest Prompt
    recall, leader l, once a session exists to view it from)
"""

import sqlite3
import sys


def main(database: str, workdir: str, tmux_session: str) -> None:
    conn = sqlite3.connect(database)
    project_id = conn.execute("SELECT id FROM projects LIMIT 1").fetchone()[0]
    feature_id = "unsent-prompt-demo-feature"
    session_id = "unsent-prompt-demo-session"
    label = "TODO: Fix the login bug"
    body = (
        "Fix the login bug\n\n"
        "The session field on the login form is not persisted across a "
        "page reload -- users are silently logged out. Reproduce, find the "
        "root cause, and fix it."
    )

    conn.execute(
        "INSERT INTO features "
        "(id, project_id, name, branch, workdir, is_worktree, tmux_session, mode, agent, status, collapsed, created_at, last_accessed) "
        "VALUES (?, ?, 'unsent-prompt-demo', 'unsent-prompt-demo', ?, 0, ?, 'vibe', 'claude', 'active', 0, datetime('now'), datetime('now'))",
        (feature_id, project_id, workdir, tmux_session),
    )
    conn.execute(
        "INSERT INTO feature_sessions "
        "(id, feature_id, kind, label, tmux_window, created_at) "
        "VALUES (?, ?, 'claude', 'Claude 1', 'claude', datetime('now'))",
        (session_id, feature_id),
    )
    conn.execute(
        "INSERT INTO prompt_templates "
        "(id, name, description, body, tags, placeholders, created_at, updated_at, sort_order) "
        "VALUES ('unsent-demo-template', ?, NULL, ?, '[\"unsent\"]', '[]', datetime('now'), datetime('now'), 0)",
        (f"Unsent: {label}", body),
    )
    conn.execute(
        "INSERT INTO unsent_prompts (id, workdir, label, body, created_at) "
        "VALUES ('unsent-demo-row', ?, ?, ?, datetime('now'))",
        (workdir, label, body),
    )
    conn.commit()


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2], sys.argv[3])
