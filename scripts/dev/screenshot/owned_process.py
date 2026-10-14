"""Cleanup for a runner-owned child launched with start_new_session=True."""

import os
import pathlib
import signal
import subprocess
import time


def group_members(pgid):
    """Live processes in group `pgid` other than its leader (Linux /proc)."""
    members = []
    for entry in pathlib.Path("/proc").iterdir():
        if not entry.name.isdigit() or int(entry.name) == pgid:
            continue
        try:
            # Fields after the parenthesised command: state, ppid, pgrp, ...
            fields = (entry / "stat").read_text().rsplit(")", 1)[1].split()
        except (OSError, IndexError):
            continue
        if fields[0] != "Z" and int(fields[2]) == pgid:
            members.append(int(entry.name))
    return members


def _leader_exited(process):
    # WNOWAIT leaves the exited leader unreaped, so its PID and process-group
    # ID stay pinned while the rest of the group is waited for or killed.
    return os.waitid(os.P_PID, process.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT) is not None


def _wait_for_empty_group(process, timeout):
    deadline = time.monotonic() + timeout
    while True:
        if _leader_exited(process) and not group_members(process.pid):
            return True
        if time.monotonic() >= deadline:
            return False
        time.sleep(0.05)


def stop_owned_process_group(process, timeout=5):
    """Stop the child and its descendants without trusting a detached PID.

    Keep the Popen handle alive until signalling is complete. If poll() says
    the child exited, it may have reaped it and released its PID: never signal
    that number again. Otherwise the unreaped child pins both its PID and its
    dedicated process-group ID while we signal the group.

    The child is a launcher for some callers (AppImage's runtime exits on
    SIGTERM while its GUI child may not), so the leader is only reaped once
    every other group member has gone, escalating to SIGKILL if they linger.
    """
    if process is None or process.poll() is not None:
        return
    try:
        if os.getpgid(process.pid) != process.pid:
            raise RuntimeError("Refusing cleanup of a stand-in without its own process group")
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    if not _wait_for_empty_group(process, timeout):
        # The leader is still unreaped, so the group ID cannot have been reused.
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        if not _wait_for_empty_group(process, timeout):
            raise RuntimeError(f"Owned process group {process.pid} survived SIGKILL: {group_members(process.pid)}")
    process.wait()
