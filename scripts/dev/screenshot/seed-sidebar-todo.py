#!/usr/bin/env python3
"""Seed one session-local TODO in a throwaway AMF screenshot database."""

import sqlite3
import sys


def main(database: str) -> None:
    conn = sqlite3.connect(database)
    project_id = conn.execute("SELECT id FROM projects LIMIT 1").fetchone()[0]
    feature_id = "sidebar-todo-feature"
    session_id = "sidebar-todo-session"
    workdir = "."
    conn.execute(
        "INSERT INTO features "
        "(id, project_id, name, branch, workdir, tmux_session, mode, agent, status, collapsed, created_at, last_accessed) "
        "VALUES (?, ?, 'sidebar-todo-proof', 'sidebar-todo-proof', ?, "
        "'amf-sidebar-todo-proof-sidebar-todo-proof', 'vibe', 'codex', 'active', 0, datetime('now'), datetime('now'))",
        (feature_id, project_id, workdir),
    )
    conn.execute(
        "INSERT INTO feature_sessions "
        "(id, feature_id, kind, label, tmux_window, todo_id, todo_launched_from_menu, created_at) "
        "VALUES (?, ?, 'codex', 'Codex 1', 'codex', 'sidebar-todo', 1, datetime('now'))",
        (session_id, feature_id),
    )

    conn.execute(
        "INSERT INTO todo_lists "
        "(id, project_id, feature_id, scope, workdir, carry_over, created_at, updated_at) "
        "VALUES ('sidebar-todo-list', ?, ?, 'worktree', ?, NULL, datetime('now'), datetime('now'))",
        (project_id, feature_id, workdir),
    )
    conn.execute(
        "INSERT INTO todos "
        "(id, list_id, title, body, priority, sort_order, status, agent_session_id, linked_feature_id, created_at, updated_at) "
        "VALUES ('sidebar-todo', 'sidebar-todo-list', 'TODO sidebar', NULL, 'med', 0, 'not_started', ?, NULL, datetime('now'), datetime('now'))",
        (session_id,),
    )
    conn.execute(
        "UPDATE feature_sessions SET todo_id = 'sidebar-todo', todo_launched_from_menu = 1 WHERE id = ?",
        (session_id,),
    )
    conn.commit()


if __name__ == "__main__":
    main(sys.argv[1])
