"""Cleanup for a runner-owned child launched with start_new_session=True."""

import os
import signal
import subprocess


def stop_owned_process_group(process):
    """Stop the child and its descendants without trusting a detached PID.

    Keep the Popen handle alive until signalling is complete. If poll() says
    the child exited, it may have reaped it and released its PID: never signal
    that number again. Otherwise the unreaped child pins both its PID and its
    dedicated process-group ID while we signal the group.
    """
    if process is None or process.poll() is not None:
        return
    try:
        if os.getpgid(process.pid) != process.pid:
            raise RuntimeError("Refusing cleanup of a stand-in without its own process group")
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        # wait() timed out without reaping the child, so the group ID is
        # still pinned even if the child exits just before this signal.
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait()
