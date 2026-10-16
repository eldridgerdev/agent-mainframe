"""Protocol, output-budget and batch failure regression tests (no GUI needed)."""

import importlib.util
import json
import io
from pathlib import Path
import tempfile
import time
import unittest
from unittest.mock import MagicMock, Mock, patch

spec = importlib.util.spec_from_file_location("gui_driver", Path(__file__).with_name("amf-gui-driver.py"))
driver = importlib.util.module_from_spec(spec)
spec.loader.exec_module(driver)


class DriverTests(unittest.TestCase):
    def test_external_source_path_reaches_daemon_and_selects_matching_binary(self):
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary)
            (source / "gui").mkdir()
            (source / "gui/package.json").write_text("{}")
            binary = source / "target/debug/amf-gui"
            binary.parent.mkdir(parents=True)
            binary.touch()
            with patch.dict(driver.os.environ, {}, clear=True), patch.object(driver.sys, "argv", ["driver", "start", "--source", str(source)]), patch.dict("sys.modules", {"websocket": Mock()}), patch.object(driver, "start_session", return_value={"ready": True}) as start, patch("sys.stdout", new_callable=io.StringIO):
                driver.main()
            self.assertEqual(start.call_args.args[1], binary)
            self.assertEqual(start.call_args.kwargs["extra_args"], [str(source)])
            with patch.object(driver.sys, "argv", ["driver", "_serve", "/session", str(binary), str(source)]), patch.object(driver, "serve") as serve:
                driver.main()
            runtime = serve.call_args.args[2](Path("/session"))
            self.assertEqual(runtime.source, source)

    def test_output_budget_includes_escaped_json_and_reports_truncation(self):
        value = {"text": '"\\\n世界' * 1000}
        result = driver.bounded(value, 256)
        self.assertLessEqual(len(driver.encode(result)), 256)
        self.assertTrue(result["truncated"])
        self.assertEqual(result["total_chars"], len(driver.encode(value)))
        self.assertEqual(driver.bounded({"ok": True}, 256), {"ok": True})

    def test_batch_stops_at_failure_without_retry_or_later_actions(self):
        inspector = Mock()
        inspector.evaluate.side_effect = [{"ok": True}, RuntimeError("Stale reference")]
        actions = [{"op": "click", "ref": "e1"}, {"op": "fill", "ref": "e2", "value": "hello"}, {"op": "click", "ref": "e3"}]
        with self.assertRaisesRegex(RuntimeError, "Batch action 2.*Stale reference"):
            driver.Driver(inspector).run({"op": "batch", "actions": actions})
        self.assertEqual(inspector.evaluate.call_count, 2)

    def test_batch_returns_only_last_result_and_uses_one_deadline(self):
        inspector = Mock()
        inspector.evaluate.side_effect = [{"ok": True}, {"text": "ready"}]
        result = driver.Driver(inspector).run({"op": "batch", "actions": [{"op": "click", "ref": "e1"}, {"op": "snapshot"}]})
        self.assertEqual(result, {"completed": 2, "result": {"text": "ready"}})
        deadlines = [call.args[1] for call in inspector.evaluate.call_args_list]
        self.assertEqual(deadlines[0], deadlines[1])

    def test_wait_polls_locally_and_times_out(self):
        inspector = Mock()
        inspector.evaluate.side_effect = [False, True]
        with patch.object(driver.time, "sleep"):
            self.assertEqual(driver.Driver(inspector).run({"op": "wait", "text": "Ready"}), {"ok": True})
        inspector.evaluate.side_effect = None
        inspector.evaluate.return_value = False
        with self.assertRaises(TimeoutError):
            driver.Driver(inspector).run({"op": "wait", "expression": "false", "timeout": 0.01})

    def test_inspector_ignores_unrelated_events_and_surfaces_javascript_error(self):
        inspector = driver.Inspector.__new__(driver.Inspector)
        inspector.ws = Mock()
        inspector.sequence = 0
        inspector.target = "page-1"
        inspector.ws.recv.side_effect = [
            json.dumps({"id": 1000001, "result": {}}),
            json.dumps({"method": "Target.dispatchMessageFromTarget", "params": {"message": json.dumps({"id": 99, "result": {}})}}),
            json.dumps({"method": "Target.dispatchMessageFromTarget", "params": {"message": json.dumps({"id": 1, "result": {"wasThrown": True, "result": {"description": "Error: Target is disabled"}}})}}),
        ]
        with self.assertRaisesRegex(RuntimeError, "Target is disabled"):
            inspector.evaluate("click()", time.monotonic() + 1)
        sent = json.loads(inspector.ws.send.call_args.args[0])
        self.assertEqual(sent["params"]["targetId"], "page-1")

    def test_cleanup_handles_exited_parent_and_only_private_tmux(self):
        with tempfile.TemporaryDirectory() as temporary:
            runtime = driver.Runtime(Path(temporary))
            child = Mock(pid=12345)
            child.poll.return_value = 0
            runtime.children = [child]
            runtime.env["AMF_TMUX_SOCKET"] = str(Path(temporary) / "tmux.sock")
            with patch.object(driver.os, "killpg") as kill, patch.object(driver.subprocess, "run") as run:
                runtime.close()
            self.assertEqual(kill.call_args_list[0].args, (12345, driver.signal.SIGTERM))
            self.assertEqual(run.call_args.args[0], ["tmux", "-S", str(Path(temporary) / "tmux.sock"), "kill-server"])
            runtime.env["AMF_TMUX_SOCKET"] = "/some/other/socket"
            with patch.object(driver.os, "killpg"), patch.object(driver.subprocess, "run") as run:
                runtime.close()
            run.assert_not_called()

    def test_occupied_frontend_refuses_launch_and_prepares_only_private_state(self):
        with tempfile.TemporaryDirectory() as temporary:
            session = Path(temporary)
            with patch.dict(driver.os.environ, {"DISPLAY": ":99", "HOME": "/host-home"}):
                runtime = driver.Runtime(session)
                with patch.object(driver.socket, "socket") as socket, patch.object(runtime, "launch") as launch:
                    socket.return_value.__enter__.return_value.connect_ex.return_value = 0
                    with self.assertRaisesRegex(RuntimeError, "Port 1420 is occupied"):
                        runtime.start(Path("/unused/amf-gui"), time.monotonic() + 1)
                launch.assert_not_called()
                self.assertEqual(driver.os.environ["HOME"], "/host-home")
                self.assertEqual(runtime.env["HOME"], str(session / "home"))
                self.assertTrue((session / "home/.codex/sessions").is_dir())
                self.assertEqual(runtime.env["AMF_TMUX_SOCKET"], str(session / "tmux.sock"))
                self.assertEqual(runtime.env["PATH"].split(driver.os.pathsep)[0], str(session / "bin"))
                for executable in ["claude", "codex", "opencode", "pi", "gh", "code"]:
                    stub = session / "bin" / executable
                    self.assertTrue(stub.stat().st_mode & 0o100)
                    self.assertIn("exit 127", stub.read_text())

    def test_reload_reinjects_helper_once_and_marks_later_references(self):
        inspector = Mock()
        gui = driver.Driver(inspector, helper="HELPER")
        inspector.evaluate.return_value = None
        gui.inject(time.monotonic() + 1)
        self.assertEqual(inspector.evaluate.call_args.args[0], "HELPER")
        inspector.evaluate.side_effect = [gui.MISSING, None, {"ok": True}, {"ok": True}]
        self.assertEqual(gui.run({"op": "click", "name": "Save"}), {"ok": True})
        expressions = [call.args[0] for call in inspector.evaluate.call_args_list]
        self.assertEqual(expressions[2], "window.__amfGuiDriverEpoch = 1;\nHELPER")
        self.assertEqual(expressions[1], expressions[3])
        self.assertIn("typeof window.__amfGuiDriver === 'function'", expressions[1])
        # A helper that is present is not injected again.
        self.assertEqual(gui.run({"op": "click", "name": "Save"}), {"ok": True})
        self.assertEqual(inspector.evaluate.call_count, 5)

    def test_replaced_page_target_receives_the_undelivered_message(self):
        def dispatched(identifier, value):
            return json.dumps({"method": "Target.dispatchMessageFromTarget", "params": {"message": json.dumps({"id": identifier, "result": {"result": {"value": value}}})}})

        inspector = driver.Inspector.__new__(driver.Inspector)
        inspector.ws = Mock()
        inspector.sequence = 0
        inspector.target = "page-1"
        inspector.ws.recv.side_effect = [
            json.dumps({"method": "Target.targetCreated", "params": {"targetInfo": {"targetId": "page-2", "type": "page", "isProvisional": True}}}),
            json.dumps({"method": "Target.didCommitProvisionalTarget", "params": {"oldTargetId": "page-1", "newTargetId": "page-2"}}),
            json.dumps({"method": "Target.targetDestroyed", "params": {"targetId": "page-1"}}),
            json.dumps({"id": 1000001, "error": {"message": "Missing target for given targetId"}}),
            dispatched(2, 7),
        ]
        self.assertEqual(inspector.evaluate("1 + 6", time.monotonic() + 1), 7)
        targets = [json.loads(call.args[0])["params"]["targetId"] for call in inspector.ws.send.call_args_list]
        self.assertEqual(targets, ["page-1", "page-2"])
        # A destroyed page with no successor yet is waited for, not addressed.
        inspector.ws.recv.side_effect = [
            json.dumps({"method": "Target.targetDestroyed", "params": {"targetId": "page-2"}}),
            json.dumps({"id": 1000003, "error": {"message": "Missing target for given targetId"}}),
            json.dumps({"method": "Target.targetCreated", "params": {"targetInfo": {"targetId": "worker-1", "type": "worker"}}}),
            json.dumps({"method": "Target.targetCreated", "params": {"targetInfo": {"targetId": "page-3", "type": "page"}}}),
            dispatched(4, "again"),
        ]
        self.assertEqual(inspector.evaluate("'again'", time.monotonic() + 1), "again")
        self.assertEqual(json.loads(inspector.ws.send.call_args.args[0])["params"]["targetId"], "page-3")
        # The same refusal for an unchanged target is a real error.
        inspector.ws.recv.side_effect = [json.dumps({"id": 1000005, "error": {"message": "refused"}})]
        with self.assertRaisesRegex(RuntimeError, "refused"):
            inspector.evaluate("1", time.monotonic() + 1)

    def test_frontend_on_ipv6_loopback_alone_counts_as_occupied(self):
        def listening_on(family_in_use):
            def create(family=driver.socket.AF_INET, *_):
                probe = MagicMock()
                probe.__enter__.return_value.connect_ex.return_value = 0 if family == family_in_use else 111
                return probe
            return create

        resolved = [(driver.socket.AF_INET6, driver.socket.SOCK_STREAM, 6, "", ("::1", 1420, 0, 0))]
        with patch.object(driver.socket, "getaddrinfo", return_value=resolved):
            with patch.object(driver.socket, "socket", side_effect=listening_on(driver.socket.AF_INET6)):
                self.assertTrue(driver.frontend_port_occupied())
            with patch.object(driver.socket, "socket", side_effect=listening_on(None)):
                self.assertFalse(driver.frontend_port_occupied())
        # No IPv6 at all: the address cannot be occupied, and that is not an error.
        with patch.object(driver.socket, "getaddrinfo", side_effect=OSError("no resolver")), \
                patch.object(driver.socket, "socket", side_effect=OSError("address family not supported")):
            self.assertFalse(driver.frontend_port_occupied())


class ContrastAuditTests(unittest.TestCase):
    RAW = {"themes": ["light", "dark", "a", "b", "c", "d", "e", "f"], "findings": [
        {"kind": "text", "element": "div.card > p.muted", "sample": "Hint", "disabled": False, "worst": 2.1,
         "themes": {"light": 3.0, "dark": 2.5, "a": 2.1, "b": 4.0, "c": 3.9, "d": 3.8, "e": 3.7, "f": 3.6}},
        {"kind": "controls", "element": "button.btn", "disabled": True, "worst": 1.5, "themes": {"dark": 1.5}},
    ]}

    def gui(self, error=None, ready=(True,), raw=None):
        inspector = Mock()
        answers = {"ready": iter(ready), "error": error}

        def evaluate(expression, deadline):
            if "ready()" in expression:
                return next(answers["ready"], True)
            if "state.error" in expression:
                return answers["error"]
            if "auditThemes" in expression:
                return raw if raw is not None else self.RAW
            return True

        inspector.evaluate.side_effect = evaluate
        return driver.Driver(inspector, helper="H", contrast="CONTRAST"), inspector

    def test_arguments_reach_the_request(self):
        args = driver.parser().parse_args(["--session", "/s", "audit-contrast", "--scope", "main", "--themes", "dark,x", "--include-disabled", "--json"])
        self.assertEqual((args.op, args.scope, args.themes, args.include_disabled, args.json), ("audit-contrast", "main", "dark,x", True, True))
        args = driver.parser().parse_args(["audit-contrast"])
        self.assertEqual((args.scope, args.themes, args.include_disabled, args.json), (None, None, False, False))

    def test_lines_list_worst_themes_first_capped_and_omit_disabled(self):
        gui, inspector = self.gui(ready=(False, True))
        with patch.object(driver.time, "sleep"):
            result = gui.run({"op": "audit-contrast", "scope": "main", "themes": "light, dark"})
        self.assertEqual((result["themes"], result["findings"]), (8, 1))
        self.assertEqual(result["lines"], ['8/8 text div.card > p.muted "Hint" worst=2.1 :: a=2.1 dark=2.5 light=3.0 f=3.6 e=3.7 d=3.8 +2 more'])
        self.assertEqual(inspector.evaluate.call_args_list[0].args[0], "CONTRAST")
        audit = [c.args[0] for c in inspector.evaluate.call_args_list if "auditThemes" in c.args[0]][0]
        self.assertEqual(audit, 'window.__amfContrast.auditThemes("main", ["light","dark"])')

    def test_include_disabled_tags_and_json_returns_raw(self):
        gui, _ = self.gui()
        result = gui.run({"op": "audit-contrast", "include_disabled": True})
        self.assertEqual(result["findings"], 2)
        self.assertEqual(result["lines"][1], "1/8 controls button.btn worst=1.5 [disabled] :: dark=1.5")
        self.assertEqual(gui.run({"op": "audit-contrast", "json": True}), self.RAW)
        audit = [c.args[0] for c in gui.inspector.evaluate.call_args_list if "auditThemes" in c.args[0]][-1]
        self.assertEqual(audit, "window.__amfContrast.auditThemes(null, null)")

    def test_catalog_error_and_timeout_fail_clearly(self):
        gui, _ = self.gui(error="no such command")
        with self.assertRaisesRegex(RuntimeError, "Theme catalog failed to load: no such command"):
            gui.run({"op": "audit-contrast"})
        gui, _ = self.gui(ready=__import__("itertools").repeat(False))
        with patch.object(driver.time, "sleep"), self.assertRaisesRegex(TimeoutError, "catalog did not load"):
            gui.run({"op": "audit-contrast", "timeout": 0.05})

    def test_output_stays_within_the_budget(self):
        raw = {"themes": ["t"], "findings": [{"kind": "text", "element": "p", "sample": "x", "disabled": False, "worst": 1, "themes": {"t": 1}}] * 500}
        result = driver.bounded(driver.format_contrast(raw), 1000)
        self.assertLessEqual(len(driver.encode(result)), 1000)
        self.assertTrue(result["truncated"])


if __name__ == "__main__":
    unittest.main()
