"""Exercise driver cleanup with real tmux, never the operator's tmux server."""
import json
import os
import pathlib
import shutil
import subprocess
import tempfile
import unittest

SCRIPT = pathlib.Path(__file__).with_name('amf-capture.sh')
REAL_TMUX = shutil.which('tmux')


@unittest.skipUnless(REAL_TMUX, 'tmux is required')
class CaptureIsolationTests(unittest.TestCase):
    def run_capture(self, *, fail=False, keep=False, inherited_function=False):
        with tempfile.TemporaryDirectory(prefix='amf-capture-test-') as directory:
            root = pathlib.Path(directory)
            ambient = root / 'ambient.sock'
            env = os.environ.copy()
            env.pop('BASH_FUNC_tmux%%', None)
            if inherited_function:
                env['BASH_FUNC_tmux%%'] = '() { return 77;\n}'
            env.update(AMF_SHOT_DIR=str(root / 'shots'), AMF_CAPTURE_TEST_AMBIENT=str(ambient),
                       AMF_CAPTURE_TEST_REAL_TMUX=REAL_TMUX, AMF_CAPTURE_TEST_RECORD=str(root / 'record.jsonl'))
            bin_dir = root / 'bin'
            bin_dir.mkdir()
            # Plain tmux would target this test-owned ambient server. Explicit
            # socket arguments pass through, so the new driver's private server
            # is genuinely separate rather than mocked as a successful command.
            shim = bin_dir / 'tmux'
            shim.write_text('#!/usr/bin/env python3\nimport os,sys\nargs=sys.argv[1:]\n'
                            "if '-S' not in args and '-L' not in args:\n    args=['-S',os.environ['AMF_CAPTURE_TEST_AMBIENT'],*args]\n"
                            "os.execv(os.environ['AMF_CAPTURE_TEST_REAL_TMUX'],['tmux',*args])\n")
            shim.chmod(0o755)
            env['PATH'] = str(bin_dir) + os.pathsep + env['PATH']
            env['AMF_TMUX_BIN'] = str(shim)
            env['AMF_TMUX_SOCKET'] = str(ambient)
            # Supply a hostile inherited TMUX identity as well as AMF's socket.
            env['TMUX'] = str(ambient) + ',1,0'
            fake = root / 'fake-amf.py'
            fake.write_text('''#!/usr/bin/env python3
import json,os,subprocess,sys,time
socket=os.environ['AMF_TMUX_SOCKET']
kind='automation' if len(sys.argv)>1 else 'tui'
subprocess.run([os.environ['AMF_TMUX_BIN'],'-S',socket,'new-session','-d','-s','owned-'+kind,'sleep 60'],check=True)
with open(os.environ['AMF_CAPTURE_TEST_RECORD'],'a') as record:
    record.write(json.dumps({'kind':kind,'socket':socket,'dedicated':os.environ.get('AMF_TMUX_DEDICATED_SOCKET'),'tmux':os.environ.get('TMUX')})+'\\n')
if kind=='automation':
    raise SystemExit(0)
print('No projects yet\\n0 projects, 0 features',flush=True)
while True:
    time.sleep(1)
''')
            fake.chmod(0o755)
            seed = root / 'seed.json'
            seed.write_text('{"path":"/unused-fixture-path"}')
            scenario = root / 'scenario.txt'
            scenario.write_text('''run:"$AMF_CAPTURE_TEST_REAL_TMUX" -S "$AMF_CAPTURE_TEST_AMBIENT" new-session -d -s unrelated-created 'sleep 60'
run:tmux -S "$AMF_CAPTURE_TEST_AMBIENT" has-session -t pre-existing
run:tmux new-session -d -s owned-scenario 'sleep 60'
run:bash -c 'tmux new-session -d -s owned-helper "sleep 60"'
note:The controlled driver fixture reached its dashboard.
expect:''' + ('deliberately missing text' if fail else '0 projects, 0 features') + '\nshot:fixture\n')
            command = ['bash', str(SCRIPT), '--amf-bin', str(fake), '--scenario', str(scenario),
                       '--seed', str(seed), '--out-dir', str(root / 'out')]
            if keep:
                command.append('--keep')
            def tmux(socket, *args):
                return subprocess.run([REAL_TMUX, '-f', '/dev/null', '-S', str(socket), *args],
                                      env=env, capture_output=True, text=True)
            self.assertEqual(tmux(ambient, 'new-session', '-d', '-s', 'pre-existing', 'sleep 60').returncode, 0)
            private = None
            try:
                result = subprocess.run(command, env=env, capture_output=True, text=True, timeout=30)
                self.assertEqual(result.returncode, 1 if fail else 0, result.stderr)
                records = [json.loads(line) for line in (root / 'record.jsonl').read_text().splitlines()]
                self.assertEqual({record['kind'] for record in records}, {'tui', 'automation'})
                private = pathlib.Path(records[0]['socket'])
                self.assertNotEqual(private, ambient)
                self.assertEqual({record['socket'] for record in records}, {str(private)})
                self.assertEqual({record['dedicated'] for record in records}, {'1'})
                self.assertEqual(set(tmux(ambient, 'list-sessions', '-F', '#{session_name}').stdout.splitlines()),
                                 {'pre-existing', 'unrelated-created'}, result.stderr)
                remaining = tmux(private, 'list-sessions', '-F', '#{session_name}')
                if keep:
                    self.assertEqual(set(remaining.stdout.splitlines()),
                                     {'owned-tui', 'owned-automation', 'owned-scenario', 'owned-helper'})
                    self.assertTrue(private.parent.is_dir())
                else:
                    self.assertNotEqual(remaining.returncode, 0, remaining.stdout)
                    self.assertFalse(private.parent.exists())
            finally:
                # Both servers are test-owned; never call the ambient default.
                tmux(ambient, 'kill-server')
                # A timeout can happen before records are parsed. Reap only
                # sockets beneath this test's owned scratch root before the
                # TemporaryDirectory removes their filesystem names.
                owned_sockets = set((root / 'shots').glob('*/tmux.sock'))
                if private is not None:
                    owned_sockets.add(private)
                for socket in owned_sockets:
                    tmux(socket, 'kill-server')

    def test_success_keeps_old_and_concurrently_created_unrelated_sessions(self):
        self.run_capture()

    def test_assertion_failure_cleans_only_the_owned_server(self):
        self.run_capture(fail=True)

    def test_inherited_tmux_function_uses_an_external_binary_without_recursion(self):
        self.run_capture(inherited_function=True)

    def test_keep_retains_spawned_private_sessions_but_closes_capture_pane(self):
        self.run_capture(keep=True)


if __name__ == '__main__':
    unittest.main()
