"""Shared delivery, failure and cleanup regression tests for both AI drivers."""

import io
from pathlib import Path
import tempfile
import unittest
from unittest.mock import MagicMock, Mock, patch

import ai_driver


class ObservationDriver(ai_driver.RequestDriver):
    def __init__(self):
        self.previous = "delivered"
        self.actions = []

    def action(self, action, deadline):
        self.actions.append(action["op"])
        if action["op"] == "snapshot":
            previous = self.previous
            self.previous = action["frame"]
            return {"frame": action["frame"], "compared_to": previous}
        if action["op"] == "fail":
            raise RuntimeError("fixture failure")
        return {"ok": True}

    def checkpoint(self):
        return self.previous

    def restore(self, checkpoint):
        self.previous = checkpoint


def connection(request):
    handle = MagicMock()
    handle.__enter__.return_value = handle
    handle.makefile.return_value.__enter__.return_value = io.BytesIO((ai_driver.encode(request) + "\n").encode())
    return handle


class DriverTests(unittest.TestCase):
    def test_truncated_delivery_does_not_consume_observation(self):
        driver = ObservationDriver()
        response = ai_driver.dispatch(driver, {"op": "snapshot", "frame": "x" * 5000, "max_output": 256})
        self.assertTrue(response["result"]["truncated"])
        self.assertEqual(driver.previous, "delivered")
        complete = ai_driver.dispatch(driver, {"op": "snapshot", "frame": "complete"})
        self.assertEqual(complete["result"]["compared_to"], "delivered")
        self.assertEqual(driver.previous, "complete")

    def test_hidden_batch_snapshots_do_not_change_final_comparison(self):
        driver = ObservationDriver()
        response = ai_driver.dispatch(driver, {"op": "batch", "actions": [
            {"op": "snapshot", "frame": "hidden"}, {"op": "snapshot", "frame": "visible"},
        ]})
        self.assertEqual(response["result"]["result"]["compared_to"], "delivered")
        self.assertEqual(driver.previous, "visible")

    def test_failed_batch_retains_delivered_baseline_and_applied_mutations(self):
        driver = ObservationDriver()
        response = ai_driver.dispatch(driver, {"op": "batch", "actions": [
            {"op": "mutate"}, {"op": "snapshot", "frame": "hidden"}, {"op": "fail"}, {"op": "mutate"},
        ]})
        self.assertFalse(response["ok"])
        self.assertIn("Batch action 3", response["error"])
        self.assertEqual(driver.actions, ["mutate", "snapshot", "fail"])
        self.assertEqual(driver.previous, "delivered")

    def test_malformed_batch_is_rejected_before_any_mutation(self):
        driver = ObservationDriver()
        for invalid in (None, "snapshot", [], {}):
            response = ai_driver.dispatch(driver, {"op": "batch", "actions": [{"op": "mutate"}, invalid]})
            self.assertFalse(response["ok"])
        self.assertEqual(driver.actions, [])
        self.assertFalse(ai_driver.dispatch(driver, ["snapshot"])["ok"])

    def test_inspector_close_error_still_stops_runtime_and_removes_ready_socket(self):
        with tempfile.TemporaryDirectory() as temporary:
            session = Path(temporary)
            (session / "driver.sock").touch()
            driver = ObservationDriver()
            driver.close = Mock(side_effect=RuntimeError("inspector close failed"))
            runtime = Mock()
            runtime.start.return_value = driver
            server = MagicMock()
            server.__enter__.return_value = server
            server.accept.return_value = connection({"op": "stop"}), None
            with patch.object(ai_driver.socket, "socket", return_value=server), patch.object(ai_driver.signal, "signal"):
                ai_driver.serve(session, Path("/fixture/amf"), lambda _: runtime)
            runtime.close.assert_called_once()
            self.assertFalse((session / "driver.sock").exists())
            self.assertFalse((session / "ready").exists())
            self.assertTrue((session / "stopped").exists())
            self.assertIn("inspector close failed", (session / "cleanup-error").read_text())
            with patch.object(ai_driver, "call", return_value={"ok": True}):
                with self.assertRaisesRegex(RuntimeError, "Cleanup failed"):
                    ai_driver.stop_session(session)

    def test_disconnected_client_preserves_baseline_and_server_accepts_next_request(self):
        with tempfile.TemporaryDirectory() as temporary:
            session = Path(temporary)
            driver = ObservationDriver()
            runtime = Mock()
            runtime.start.return_value = driver
            disconnected = connection({"op": "snapshot", "frame": "lost"})
            disconnected.sendall.side_effect = OSError("client disconnected")
            next_client = connection({"op": "stop"})
            server = MagicMock()
            server.__enter__.return_value = server
            server.accept.side_effect = [(disconnected, None), (next_client, None)]
            with patch.object(ai_driver.socket, "socket", return_value=server), patch.object(ai_driver.signal, "signal"):
                ai_driver.serve(session, Path("/fixture/amf"), lambda _: runtime)
            self.assertEqual(driver.previous, "delivered")
            self.assertEqual(server.accept.call_count, 2)
            runtime.close.assert_called_once()

    def test_generated_session_falls_back_to_tmp_when_platform_temp_dir_is_too_long(self):
        macos = "/var/folders/zz/" + "y" * 30 + "/T"
        for base, expected in ((macos, "/tmp"), ("/tmp", None)):
            with patch.object(ai_driver.tempfile, "gettempdir", return_value=base), \
                    patch.object(ai_driver.tempfile, "mkdtemp", return_value="/tmp/amf-tui-abcdefgh") as create:
                self.assertEqual(ai_driver.new_session_dir("amf-tui-"), Path("/tmp/amf-tui-abcdefgh"))
            self.assertEqual(create.call_args.kwargs, {"prefix": "amf-tui-", "dir": expected})
        self.assertFalse(ai_driver.session_path_fits(Path(macos) / "amf-tui-abcdefgh"))
        self.assertTrue(ai_driver.session_path_fits(Path("/tmp/amf-tui-abcdefgh")))

    def test_exited_application_keeps_its_final_screen_and_later_calls_say_why(self):
        # A short root: the socket path must fit whatever TMPDIR is in use.
        with tempfile.TemporaryDirectory(dir="/tmp") as temporary:
            session = Path(temporary)
            runtime = Mock()
            runtime.start.return_value = ObservationDriver()
            runtime.alive.return_value = False
            runtime.postmortem.return_value = "last-pane.txt"
            server = MagicMock()
            server.__enter__.return_value = server
            with patch.object(ai_driver.socket, "socket", return_value=server), patch.object(ai_driver.signal, "signal"):
                ai_driver.serve(session, Path("/fixture/amf"), lambda _: runtime)
            # The screen is saved while the application's tmux server exists.
            names = [call[0] for call in runtime.mock_calls]
            self.assertLess(names.index("postmortem"), names.index("close"))
            self.assertIn("final screen saved to last-pane.txt", (session / "error").read_text())
            for request in (lambda: ai_driver.call(session, {"op": "snapshot"}), lambda: ai_driver.stop_session(session)):
                with self.assertRaisesRegex(RuntimeError, "Session ended: An owned application process exited.*last-pane.txt"):
                    request()
            (session / "error").unlink()
            with self.assertRaisesRegex(RuntimeError, "already stopped"):
                ai_driver.call(session, {"op": "snapshot"})
            with self.assertRaisesRegex(RuntimeError, "No session directory"):
                ai_driver.call(session / "missing", {"op": "snapshot"})


if __name__ == "__main__":
    unittest.main()
