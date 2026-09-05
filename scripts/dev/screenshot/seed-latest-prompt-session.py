#!/usr/bin/env python3
"""Seed a Claude feature/session directly in a throwaway AMF database.

Bypasses `automation create-feature` (and its harness-availability gate)
entirely: no real `claude` process is ever launched, so this works
identically on a machine with the Claude CLI installed and on a CI runner
that only has Codex. A plain tmux window stands in for the pane; nothing
here depends on it running any particular program.
"""

import sqlite3
import sys


def main(database: str, workdir: str, tmux_session: str) -> None:
    conn = sqlite3.connect(database)
    project_id = conn.execute("SELECT id FROM projects LIMIT 1").fetchone()[0]
    feature_id = "latest-prompt-proof-feature"
    session_id = "latest-prompt-proof-session"

    conn.execute(
        "INSERT INTO features "
        "(id, project_id, name, branch, workdir, is_worktree, tmux_session, mode, agent, status, collapsed, created_at, last_accessed) "
        "VALUES (?, ?, 'latest-prompt-proof', 'latest-prompt-proof', ?, 0, ?, 'vibe', 'claude', 'active', 0, datetime('now'), datetime('now'))",
        (feature_id, project_id, workdir, tmux_session),
    )
    conn.execute(
        "INSERT INTO feature_sessions "
        "(id, feature_id, kind, label, tmux_window, created_at) "
        "VALUES (?, ?, 'claude', 'Claude 1', 'claude', datetime('now'))",
        (session_id, feature_id),
    )
    conn.commit()


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2], sys.argv[3])
