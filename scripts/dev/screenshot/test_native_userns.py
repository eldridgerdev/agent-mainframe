#!/usr/bin/env python3
"""Test namespace prerequisites using private executables; never real sysctl."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

HELPER = Path(__file__).with_name("native-userns.sh")


class NativeUsernsTests(unittest.TestCase):
    def run_probe(self, *, actions="true", runner="github-hosted", system="Linux",
                  initial_probe=False, always_fail=False, policy=True, child_status=0):
        with tempfile.TemporaryDirectory(prefix="amf-userns-test-") as directory:
            root = Path(directory)
            policy_file = root / "policy"
            if policy:
                policy_file.write_text("1\n")
            scripts = {
                "bwrap": '#!/bin/bash\nif [[ "$INITIAL_PROBE" == 1 ]]; then exit 0; fi\n'
                         '[[ "$ALWAYS_FAIL" != 1 && "$(cat "$POLICY")" == 0 ]]\n',
                "uname": '#!/bin/bash\nprintf "%s\\n" "$FAKE_SYSTEM"\n',
                "sudo": '#!/bin/bash\n[[ "$*" == "-n sysctl -w kernel.apparmor_restrict_unprivileged_userns="[01] ]] || exit 99\n'
                        'value="${4##*=}"\nprintf "%s\\n" "$value" >> "$CALLS"\n'
                        'printf "%s\\n" "$value" > "$POLICY"\n',
            }
            for name, content in scripts.items():
                target = root / name
                target.write_text(content)
                target.chmod(0o700)
            calls_file = root / "calls"
            env = dict(os.environ, PATH=f"{root}:/usr/bin:/bin", POLICY=str(policy_file),
                       CALLS=str(calls_file), GITHUB_ACTIONS=actions, RUNNER_ENVIRONMENT=runner,
                       FAKE_SYSTEM=system, INITIAL_PROBE=str(int(initial_probe)),
                       ALWAYS_FAIL=str(int(always_fail)))
            result = subprocess.run(
                ["bash", "-ec", 'source "$1"; amf_require_native_userns "$2"; exit "$3"',
                 "fixture", str(HELPER), str(policy_file), str(child_status)],
                env=env, capture_output=True, text=True, timeout=10,
            )
            calls = calls_file.read_text().splitlines() if calls_file.exists() else []
            value = policy_file.read_text().strip() if policy_file.exists() else None
            return result.returncode, calls, value

    def test_successful_probe_never_changes_policy(self):
        self.assertEqual(self.run_probe(initial_probe=True), (0, [], "1"))

    def test_local_or_self_hosted_or_nonlinux_never_changes_policy(self):
        for kwargs in ({"actions": ""}, {"runner": "self-hosted"}, {"system": "Darwin"}):
            with self.subTest(kwargs=kwargs):
                self.assertEqual(self.run_probe(**kwargs), (1, [], "1"))

    def test_missing_policy_never_invokes_setter(self):
        self.assertEqual(self.run_probe(policy=False), (1, [], None))

    def test_hosted_failed_probe_adjusts_and_restores(self):
        self.assertEqual(self.run_probe(), (0, ["0", "1"], "1"))

    def test_repeated_failure_restores_and_refuses_capture(self):
        self.assertEqual(self.run_probe(always_fail=True), (1, ["0", "1"], "1"))

    def test_capture_failure_preserves_status_and_restores(self):
        self.assertEqual(self.run_probe(child_status=7), (7, ["0", "1"], "1"))


if __name__ == "__main__":
    unittest.main()
