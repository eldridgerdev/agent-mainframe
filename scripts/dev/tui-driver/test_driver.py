"""TUI snapshot, input encoding and fail-fast regression checks."""

import importlib.util
from pathlib import Path
import tempfile
import time
import unittest
from unittest.mock import Mock, patch

spec = importlib.util.spec_from_file_location("tui_driver", Path(__file__).with_name("amf-tui-driver.py"))
driver = importlib.util.module_from_spec(spec)
spec.loader.exec_module(driver)


def metadata(rows=4, cols=80):
    return {"pane_width": cols, "pane_height": rows, "cursor_x": 3, "cursor_y": 2,
            "cursor_flag": 1, "alternate_on": 1, "keypad_cursor_flag": 1}


class DriverTests(unittest.TestCase):
    def setUp(self):
        self.runtime = Mock(target="driver:dashboard.0")
        self.driver = driver.Driver(self.runtime, Path("/fixture/amf"))
        self.driver.frame = Mock(return_value=(metadata(), ["┌───────┐", "│ Hello │", "│       │", "└───────┘"]))
        self.deadline = time.monotonic() + 1

    def test_installed_binary_fallback_ignores_stale_amf_bin(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            installed = root / "installed-amf"
            installed.write_text("#!/bin/sh\nexit 0\n")
            installed.chmod(0o700)
            with patch.object(driver, "ROOT", root), patch.dict(driver.os.environ, {"CARGO_TARGET_DIR": str(root / "target"), "AMF_BIN": str(root / "missing")}), patch.object(driver.shutil, "which", return_value=str(installed)):
                self.assertEqual(driver.default_binary(), installed)
                with patch.dict(driver.os.environ, {"AMF_BIN": str(installed)}), patch.object(driver.shutil, "which") as lookup:
                    self.assertEqual(driver.default_binary(), installed)
                    lookup.assert_not_called()
                local = root / "target/debug/amf"
                local.parent.mkdir(parents=True)
                local.write_text("development binary")
                self.assertEqual(driver.default_binary(), local)

    def test_compact_rows_retain_coordinates_and_changed_rows_clear_old_text(self):
        full = self.driver.action({"op": "snapshot"}, self.deadline)
        self.assertEqual(full["lines"], [{"row": 1, "text": "│ Hello"}])
        self.assertTrue(full["alternate_screen"])
        self.assertTrue(full["application_cursor"])
        self.assertEqual(full["cursor"], {"col": 3, "row": 2, "visible": True})
        unchanged = self.driver.action({"op": "snapshot", "changes": True}, self.deadline)
        self.assertTrue(unchanged["delta"])
        self.assertEqual(unchanged["lines"], [])
        self.driver.frame.return_value = metadata(), ["┌───────┐", "│       │", "│ 世界  │", "└───────┘"]
        changed = self.driver.action({"op": "snapshot", "changes": True}, self.deadline)
        self.assertEqual(changed["lines"], [{"row": 1, "text": ""}, {"row": 2, "text": "│ 世界"}])

    def test_scope_and_resize_reset_delta_and_truncation_retains_previous_baseline(self):
        self.driver.action({"op": "snapshot"}, self.deadline)
        scoped = self.driver.action({"op": "snapshot", "changes": True, "first_row": 1, "last_row": 2}, self.deadline)
        self.assertFalse(scoped["delta"])
        self.driver.frame.return_value = metadata(), ["", "│ New long text │", "", ""]
        partial = self.driver.action({"op": "snapshot", "changes": True, "first_row": 1, "last_row": 2, "text_limit": 2}, self.deadline)
        self.assertTrue(partial["text_truncated"])
        retry = self.driver.action({"op": "snapshot", "changes": True, "first_row": 1, "last_row": 2}, self.deadline)
        self.assertEqual(retry["lines"], [{"row": 1, "text": "│ New long text"}])
        self.driver.action({"op": "resize", "geometry": "100x30"}, self.deadline)
        self.assertIsNone(self.driver.previous)

    def test_keys_use_tmux_names_and_literal_input_cannot_be_options_or_escape_sequences(self):
        self.driver.action({"op": "keys", "keys": ["Up", "C-a", "Enter"]}, self.deadline)
        self.assertEqual(self.runtime.tmux.call_args.args, ("send-keys", "-t", self.runtime.target, "--", "Up", "C-a", "Enter"))
        self.driver.action({"op": "text", "text": "-n café 世界"}, self.deadline)
        self.assertEqual(self.runtime.tmux.call_args.args[-3:], ("-l", "--", "-n café 世界"))
        with self.assertRaises(ValueError):
            self.driver.action({"op": "keys", "keys": ["-t", "somewhere"]}, self.deadline)
        for operation in ["text", "paste"]:
            with self.assertRaises(ValueError):
                self.driver.action({"op": operation, "text": "\x1b[201~danger"}, self.deadline)

    def test_paste_uses_private_buffer_and_tmux_bracketed_mode_then_deletes_it(self):
        self.driver.action({"op": "paste", "text": "one\r\ntwo\t世界"}, self.deadline)
        calls = self.runtime.tmux.call_args_list
        self.assertEqual(calls[0].args[0], "load-buffer")
        self.assertEqual(calls[0].kwargs["input"], "one\ntwo\t世界")
        self.assertEqual(calls[1].args[0], "paste-buffer")
        self.assertIn("-p", calls[1].args)
        self.assertEqual(calls[2].args[0], "delete-buffer")
        self.assertEqual(calls[0].args[2], calls[1].args[2])

    def test_waits_do_not_consume_delta_baseline_and_batch_stops_on_timeout(self):
        self.driver.action({"op": "snapshot"}, self.deadline)
        original = self.driver.previous
        self.driver.run({"op": "wait", "text": "Hello"})
        self.assertIs(self.driver.previous, original)
        with self.assertRaisesRegex(RuntimeError, "Batch action 1"):
            self.driver.run({"op": "batch", "timeout": 0.01, "actions": [
                {"op": "wait", "text": "missing"}, {"op": "keys", "keys": ["q"]},
            ]})
        self.runtime.tmux.assert_not_called()

    def test_copy_mode_and_dead_panes_refuse_input(self):
        self.driver.frame = driver.Driver.frame.__get__(self.driver)
        for values in ["80\t4\t0\t0\t1\t1\t1\t0\t0", "80\t4\t0\t0\t1\t0\t0\t1\t0"]:
            self.runtime.tmux.return_value = values
            with self.assertRaises(RuntimeError):
                self.driver.action({"op": "keys", "keys": ["Enter"]}, self.deadline)


if __name__ == "__main__":
    unittest.main()
