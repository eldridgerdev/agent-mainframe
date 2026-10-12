"""Exercise driver cleanup with real tmux, never the operator's tmux server."""
import json
import os
import pathlib
import re
import shutil
import subprocess
import tempfile
import unittest

SCRIPT = pathlib.Path(__file__).with_name('amf-capture.sh')
REAL_TMUX = shutil.which('tmux')
BASH = shutil.which('bash')
# The portable sun_path budget the driver has to stay inside (macOS: 104 with NUL).
SOCKET_PATH_MAX_BYTES = 103
NAMED_LABEL = 'named'
OWNED_SESSIONS = {'owned-tui', 'owned-automation', 'owned-scenario', 'owned-helper', 'owned-sh', 'owned-python'}


@unittest.skipUnless(REAL_TMUX and BASH, 'tmux and bash are required')
class CaptureIsolationTests(unittest.TestCase):
    def run_capture(self, *, fail=False, keep=False, inherited_function=False, long_root=False):
        with tempfile.TemporaryDirectory(prefix='amf-capture-test-') as directory:
            root = pathlib.Path(directory)
            ambient = root / 'ambient.sock'
            shots = root / 'shots'
            if long_root:
                # One long component is enough to push <scratch>/tmux.sock past
                # the socket-path cap on every platform.
                shots = shots / ('long-' * 24)
            env = os.environ.copy()
            env.pop('BASH_FUNC_tmux%%', None)
            if inherited_function:
                env['BASH_FUNC_tmux%%'] = '() { return 77;\n}'
            # tmux -L resolves its label under TMUX_TMPDIR, so the named server
            # a scenario step asks for below is test-owned as well.
            named_tmpdir = root / 'n'
            named_tmpdir.mkdir()
            env.update(AMF_SHOT_DIR=str(shots), AMF_CAPTURE_TEST_AMBIENT=str(ambient),
                       AMF_CAPTURE_TEST_REAL_TMUX=REAL_TMUX, AMF_CAPTURE_TEST_RECORD=str(root / 'record.jsonl'),
                       AMF_CAPTURE_TEST_LABEL=NAMED_LABEL, TMUX_TMPDIR=str(named_tmpdir))
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
            # tmux runs the pane command through $SHELL, and a non-interactive
            # bash sources BASH_ENV first. This stands in for a startup file that
            # re-exports the operator's tmux identity after the session
            # environment is applied: only the pin on the final exec survives it.
            hostile_startup = root / 'hostile-startup.sh'
            hostile_startup.write_text(f'export AMF_TMUX_BIN="{shim}" AMF_TMUX_SOCKET="{ambient}"\n'
                                       'export AMF_TMUX_DEDICATED_SOCKET=0 AMF_CAPTURE_TEST_STARTUP=1\n')
            env.update(SHELL=BASH, BASH_ENV=str(hostile_startup))
            fake = root / 'fake-amf.py'
            fake.write_text('''#!/usr/bin/env python3
import json,os,subprocess,sys,time
socket=os.environ['AMF_TMUX_SOCKET']
tmux_bin=os.environ['AMF_TMUX_BIN']
kind='automation' if len(sys.argv)>1 else 'tui'
subprocess.run([tmux_bin,'-S',socket,'new-session','-d','-s','owned-'+kind,'sleep 60'],check=True)
with open(os.environ['AMF_CAPTURE_TEST_RECORD'],'a') as record:
    record.write(json.dumps({'kind':kind,'socket':socket,'bin':tmux_bin,'bin_dir':sorted(os.listdir(os.path.dirname(tmux_bin))),
                             'dedicated':os.environ.get('AMF_TMUX_DEDICATED_SOCKET'),'tmux':os.environ.get('TMUX'),
                             'startup':os.environ.get('AMF_CAPTURE_TEST_STARTUP')})+'\\n')
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
run:tmux -u -L "$AMF_CAPTURE_TEST_LABEL" new-session -d -s named-session 'sleep 60'
run:tmux new-session -d -s owned-scenario 'sleep 60'
run:bash -c 'tmux new-session -d -s owned-helper "sleep 60"'
run:sh -c 'tmux new-session -d -s owned-sh "sleep 60"'
run:python3 -c 'import subprocess; subprocess.run(["tmux", "new-session", "-d", "-s", "owned-python", "sleep 60"], check=True)'
note:The controlled driver fixture reached its dashboard.
expect:''' + ('deliberately missing text' if fail else '0 projects, 0 features') + '\nshot:fixture\n')
            command = [BASH, str(SCRIPT), '--amf-bin', str(fake), '--scenario', str(scenario),
                       '--seed', str(seed), '--out-dir', str(root / 'out')]
            if keep:
                command.append('--keep')
            def tmux(server, *args):
                return subprocess.run([REAL_TMUX, '-f', '/dev/null', *server, *args],
                                      env=env, capture_output=True, text=True)
            def sessions(server):
                return set(tmux(server, 'list-sessions', '-F', '#{session_name}').stdout.splitlines())
            ambient_server = ('-S', str(ambient))
            named_server = ('-L', NAMED_LABEL)
            self.assertEqual(tmux(ambient_server, 'new-session', '-d', '-s', 'pre-existing', 'sleep 60').returncode, 0)
            stderr = ''
            try:
                try:
                    result = subprocess.run(command, env=env, capture_output=True, text=True, timeout=30)
                except subprocess.TimeoutExpired as expired:
                    stderr = expired.stderr or ''
                    if isinstance(stderr, bytes):
                        stderr = stderr.decode(errors='replace')
                    raise
                stderr = result.stderr
                self.assertEqual(result.returncode, 1 if fail else 0, stderr)
                records = [json.loads(line) for line in (root / 'record.jsonl').read_text().splitlines()]
                by_kind = {record['kind']: record for record in records}
                self.assertEqual(set(by_kind), {'tui', 'automation'})
                private = pathlib.Path(records[0]['socket'])
                private_server = ('-S', str(private))
                self.assertNotEqual(private, ambient)
                self.assertEqual({record['socket'] for record in records}, {str(private)})
                self.assertEqual({record['dedicated'] for record in records}, {'1'})

                # The pane's shell re-exported the ambient identity, and the
                # application still saw the capture's: the launch pin held.
                self.assertEqual(by_kind['tui']['startup'], '1', 'the hostile startup file never ran in the pane')
                # The pane is a client of the private server, and nothing the
                # driver starts directly inherits the operator's TMUX.
                self.assertEqual((by_kind['tui']['tmux'] or '').split(',')[0], str(private))
                self.assertIsNone(by_kind['automation']['tmux'])

                # AMF prepends this binary's directory to every agent window's
                # PATH, so it must not be a directory of real commands.
                self.assertEqual({record['bin'] for record in records}, {by_kind['tui']['bin']})
                self.assertNotEqual(by_kind['tui']['bin'], str(shim))
                self.assertEqual([record['bin_dir'] for record in records], [['tmux']] * len(records))

                self.assertLessEqual(len(os.fsencode(private)), SOCKET_PATH_MAX_BYTES)
                if long_root:
                    self.assertNotIn(shots, private.parents)
                else:
                    self.assertEqual(private.parent.parent, shots)

                self.assertEqual(sessions(ambient_server), {'pre-existing', 'unrelated-created'}, stderr)
                # An explicit -L is the caller's own server, like an explicit -S:
                # it neither lands on the capture's server nor is cleaned up by it.
                self.assertEqual(sessions(named_server), {'named-session'}, stderr)
                remaining = tmux(private_server, 'list-sessions', '-F', '#{session_name}')
                if keep:
                    self.assertEqual(set(remaining.stdout.splitlines()), OWNED_SESSIONS)
                    self.assertTrue(private.parent.is_dir())
                else:
                    self.assertNotEqual(remaining.returncode, 0, remaining.stdout)
                    self.assertFalse(private.parent.exists())
                    self.assertEqual(list(shots.iterdir()), [])
            finally:
                # Every server here is test-owned; never call the ambient default.
                tmux(ambient_server, 'kill-server')
                tmux(named_server, 'kill-server')
                # A timeout or failure can happen before records are parsed, so
                # reap the socket the driver announced. Do it before the
                # TemporaryDirectory removes the sockets' filesystem names.
                for socket in re.findall(r'^tmux socket: (.+)$', stderr, flags=re.MULTILINE):
                    tmux(('-S', socket), 'kill-server')
                    if pathlib.Path(socket).parent.name.startswith('amf-shot-tmux.'):
                        shutil.rmtree(pathlib.Path(socket).parent, ignore_errors=True)

    def test_success_keeps_old_and_concurrently_created_unrelated_sessions(self):
        self.run_capture()

    def test_assertion_failure_cleans_only_the_owned_server(self):
        self.run_capture(fail=True)

    def test_inherited_tmux_function_uses_an_external_binary_without_recursion(self):
        self.run_capture(inherited_function=True)

    def test_keep_retains_spawned_private_sessions_but_closes_capture_pane(self):
        self.run_capture(keep=True)

    def test_scratch_root_too_long_for_a_socket_path_gets_a_short_private_socket(self):
        self.run_capture(long_root=True)


if __name__ == '__main__':
    unittest.main()
