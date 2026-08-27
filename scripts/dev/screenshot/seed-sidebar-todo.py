#!/usr/bin/env python3
"""Seed one session-local TODO in a throwaway AMF screenshot database."""

import sqlite3
import sys


def main(database: str) -> None:
    conn = sqlite3.connect(database)
    project_id = conn.execute("SELECT id FROM projects LIMIT 1").fetchone()[0]
    feature_id, workdir = conn.execute(
        "SELECT id, workdir FROM features LIMIT 1"
    ).fetchone()
    session_id = conn.execute(
        "SELECT id FROM feature_sessions WHERE feature_id = ? "
        "AND kind IN ('claude', 'opencode', 'codex', 'pi') "
        "ORDER BY sort_order, rowid LIMIT 1",
        (feature_id,),
    ).fetchone()[0]

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
