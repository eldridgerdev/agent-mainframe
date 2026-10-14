"""Process-lifetime regressions; these tests do not start a GUI or capture images."""

import os
import pathlib
import signal
import subprocess
import sys
import time
import unittest
from unittest.mock import Mock, patch

from owned_process import stop_owned_process_group


class OwnedProcessTests(unittest.TestCase):
    def child(self, returncode=None):
        process = Mock(spec=subprocess.Popen)
        process.pid = 12345
        process.poll.return_value = returncode
        return process

    @patch("owned_process._wait_for_empty_group", return_value=True)
    @patch("owned_process.os.killpg")
    @patch("owned_process.os.getpgid", return_value=12345)
    def test_live_owned_child_signals_its_dedicated_group_and_reaps(self, getpgid, killpg, empty):
        process = self.child()
        stop_owned_process_group(process)
        getpgid.assert_called_once_with(process.pid)
        killpg.assert_called_once_with(process.pid, signal.SIGTERM)
        empty.assert_called_once_with(process, 5)
        process.wait.assert_called_once_with()

    @patch("owned_process.os.killpg")
    @patch("owned_process.os.getpgid")
    def test_exited_child_never_signals_a_reused_pid_even_if_alive(self, getpgid, killpg):
        process = self.child(returncode=0)
        # Simulate the number now belonging to another live process/group.
        getpgid.return_value = process.pid
        stop_owned_process_group(process)
        getpgid.assert_not_called()
        killpg.assert_not_called()
        process.wait.assert_not_called()

    @patch("owned_process.os.killpg")
    @patch("owned_process.os.getpgid")
    def test_missing_owned_handle_never_signals(self, getpgid, killpg):
        stop_owned_process_group(None)
        getpgid.assert_not_called()
        killpg.assert_not_called()

    @patch("owned_process.os.killpg")
    @patch("owned_process.os.getpgid", return_value=999)
    def test_group_identity_mismatch_is_refused(self, getpgid, killpg):
        with self.assertRaisesRegex(RuntimeError, "without its own process group"):
            stop_owned_process_group(self.child())
        killpg.assert_not_called()

    @patch("owned_process._wait_for_empty_group", side_effect=[False, True])
    @patch("owned_process.os.killpg")
    @patch("owned_process.os.getpgid", return_value=12345)
    def test_lingering_group_escalates_before_reaping(self, getpgid, killpg, empty):
        process = self.child()
        stop_owned_process_group(process)
        self.assertEqual(killpg.call_args_list, [
            unittest.mock.call(process.pid, signal.SIGTERM),
            unittest.mock.call(process.pid, signal.SIGKILL),
        ])
        process.wait.assert_called_once_with()

    @patch("owned_process.group_members", return_value=[23456])
    @patch("owned_process._wait_for_empty_group", return_value=False)
    @patch("owned_process.os.killpg")
    @patch("owned_process.os.getpgid", return_value=12345)
    def test_group_surviving_sigkill_is_reported_without_unpinning_it(self, getpgid, killpg, empty, members):
        process = self.child()
        with self.assertRaisesRegex(RuntimeError, "survived SIGKILL"):
            stop_owned_process_group(process)
        process.wait.assert_not_called()

    @patch("owned_process._wait_for_empty_group", return_value=True)
    @patch("owned_process.os.killpg", side_effect=ProcessLookupError)
    @patch("owned_process.os.getpgid", return_value=12345)
    def test_exit_between_identity_check_and_signal_is_reaped(self, getpgid, killpg, empty):
        process = self.child()
        stop_owned_process_group(process)
        process.wait.assert_called_once_with()

    def test_real_owned_group_cleanup_ends_the_stand_in_child(self):
        # A short maximum lifetime bounds the fixture even if an assertion fails.
        process = subprocess.Popen(
            [sys.executable, "-c", "import subprocess; p=subprocess.Popen(['sleep','2']); print(p.pid,flush=True); p.wait()"],
            stdout=subprocess.PIPE, text=True, start_new_session=True,
        )
        try:
            child_pid = int(process.stdout.readline())
            self.assertEqual(os.getpgid(child_pid), process.pid)
            stop_owned_process_group(process)
            self.assertIsNotNone(process.returncode)
            for _ in range(100):
                try:
                    state = pathlib.Path(f"/proc/{child_pid}/stat").read_text().rsplit(")", 1)[1].split()[0]
                except FileNotFoundError:
                    state = "gone"
                if state in ["gone", "Z"]:
                    break
                time.sleep(0.01)
            self.assertIn(state, ["gone", "Z"])
        finally:
            stop_owned_process_group(process)
            process.stdout.close()


    def test_launcher_exit_does_not_orphan_a_child_ignoring_sigterm(self):
        # Like AppImage's runtime: the leader dies on SIGTERM at once, while
        # its child ignores SIGTERM and must still be gone before return.
        child = "import signal,time; signal.signal(signal.SIGTERM, signal.SIG_IGN); import os; print(os.getpid(), flush=True); time.sleep(10)"
        process = subprocess.Popen(
            [sys.executable, "-c", f"import subprocess,sys; subprocess.Popen([sys.executable,'-c',{child!r}]).wait()"],
            stdout=subprocess.PIPE, text=True, start_new_session=True,
        )
        try:
            child_pid = int(process.stdout.readline())
            self.assertEqual(os.getpgid(child_pid), process.pid)
            stop_owned_process_group(process, timeout=0.5)
            self.assertIsNotNone(process.returncode)
            try:
                state = pathlib.Path(f"/proc/{child_pid}/stat").read_text().rsplit(")", 1)[1].split()[0]
            except FileNotFoundError:
                state = "gone"
            self.assertIn(state, ["gone", "Z"])
        finally:
            stop_owned_process_group(process)
            process.stdout.close()

if __name__ == "__main__":
    unittest.main()
