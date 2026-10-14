#!/usr/bin/env python3
"""Run an unmodified source-built debug AppImage and host Python on its fresh server.

HOME is preserved. AppRun variables are never injected or stripped. Only the
runner's private XDG paths, TMPDIR, socket and host test-module path are supplied.
"""
import hashlib
import json
import os
import pathlib
import re
import shlex
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time

from owned_process import stop_owned_process_group

image = pathlib.Path(sys.argv[1]).resolve(strict=True)
out = pathlib.Path(sys.argv[2]).resolve()
out.mkdir(parents=True, exist_ok=True)
scripts = pathlib.Path(__file__).resolve().parent
original_hash = hashlib.sha256(image.read_bytes()).hexdigest()

with tempfile.TemporaryDirectory(prefix="amf-source-appimage-") as temporary:
    scratch = pathlib.Path(temporary)
    config = scratch / "config"
    repo = scratch / "demo-api"
    host_modules = scratch / "host-python"
    runtime_tmp = scratch / "image-runtime"
    for directory in [config / "amf", repo, host_modules, runtime_tmp, scratch / "state", scratch / "cache", scratch / "data"]:
        directory.mkdir(parents=True)
    config_path = config / "amf/config.json"
    config_path.write_text('{"config_version":1,"dormant_idle_minutes":0}\n')
    (host_modules / "amf_host_python_probe.py").write_text('value = "host-module-imported"\n')
    receiver = scratch / "python-receiver.py"
    receiver.write_text('import json, amf_host_python_probe; assert amf_host_python_probe.value == "host-module-imported"\n' + (scripts / "fixtures/gui-composer-harness.py").read_text())
    with socket.socket() as inspector_socket:
        inspector_socket.bind(("127.0.0.1", 0))
        inspector = f"127.0.0.1:{inspector_socket.getsockname()[1]}"
    env = os.environ.copy()
    host_path = str(host_modules) + (os.pathsep + env["PYTHONPATH"] if env.get("PYTHONPATH") else "")
    env.update(XDG_CONFIG_HOME=str(config), XDG_STATE_HOME=str(scratch / "state"),
               XDG_DATA_HOME=str(scratch / "data"), XDG_CACHE_HOME=str(scratch / "cache"),
               TMPDIR=str(runtime_tmp), GDK_BACKEND="x11", WEBKIT_INSPECTOR_HTTP_SERVER=inspector,
               AMF_TMUX_SOCKET=str(scratch / "tmux.sock"), PYTHONPATH=host_path)
    git_env = env | {"GIT_CONFIG_GLOBAL":"/dev/null", "GIT_CONFIG_NOSYSTEM":"1", "GIT_AUTHOR_NAME":"AMF demo", "GIT_COMMITTER_NAME":"AMF demo", "GIT_AUTHOR_EMAIL":"demo@example.com", "GIT_COMMITTER_EMAIL":"demo@example.com"}
    for args in [["init", "-b", "main"], ["commit", "--allow-empty", "-m", "Private AppImage fixture"], ["checkout", "-b", "round-invoice-totals"]]:
        subprocess.run(["git", *args], cwd=repo, env=git_env, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    tmux = ["/usr/bin/tmux", "-S", env["AMF_TMUX_SOCKET"]]
    name = "amf-source-appimage-proof"
    assert not pathlib.Path(env["AMF_TMUX_SOCKET"]).exists()
    gui = None
    gui_pid = None
    gui_environment = None
    report = {"artifact":str(image), "sha256":original_hash, "profile":"debug", "launch":"actual AppImage --appimage-extract-and-run", "frontend":"embedded production assets; no Vite", "server_started_by":"GUI before any receivers", "app_run_environment_injected":False, "payload_modified":False}
    try:
        with (out / "gui.log").open("w") as log:
            gui = subprocess.Popen([str(image), "--appimage-extract-and-run"], cwd=scratch, env=env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        for _ in range(200):
            if gui.poll() is not None:
                raise RuntimeError(f"AppImage launcher exited {gui.returncode}: {(out/'gui.log').read_text()}")
            result = subprocess.run(tmux + ["show-environment", "-g", "AMF_TMUX_SOCKET"], text=True, capture_output=True)
            if result.returncode == 0:
                assert result.stdout.strip() == "AMF_TMUX_SOCKET=" + env["AMF_TMUX_SOCKET"]
                break
            time.sleep(0.1)
        else:
            raise RuntimeError("Actual AppImage did not start its private tmux server")
        # Find only the real GUI in the still-owned launcher's process group.
        for _ in range(100):
            assert gui.poll() is None, "AppImage launcher exited before GUI ownership was established"
            for process in pathlib.Path('/proc').iterdir():
                if not process.name.isdigit():
                    continue
                try:
                    candidate = int(process.name)
                    if os.getpgid(candidate) != gui.pid or (process/'comm').read_text().strip() != 'amf-gui':
                        continue
                    gui_pid = candidate
                    # Retain only this explicit allowlist; never dump the environment.
                    entries = (process/'environ').read_bytes().split(b'\0')
                    gui_environment = {key.decode(): value.decode() for entry in entries if b'=' in entry for key,value in [entry.split(b'=',1)] if key in [b'APPDIR',b'PYTHONHOME',b'PYTHONPATH']}
                    break
                except (OSError, ProcessLookupError):
                    continue
            if gui_pid:
                break
            time.sleep(0.1)
        assert gui_pid and gui_environment and gui_environment.get('APPDIR'), 'Actual AppRun GUI environment unavailable'
        app_dir = pathlib.Path(gui_environment['APPDIR'])
        assert app_dir.is_relative_to(runtime_tmp), (app_dir, runtime_tmp)
        assert gui_environment.get('PYTHONHOME') == str(app_dir/'usr')+'/', gui_environment
        assert gui_environment.get('PYTHONPATH') == str(app_dir/'usr/share/pyshared')+'/:'+host_path, gui_environment
        report['bundle_redirects_observed_in_gui'] = True
        home = subprocess.run(tmux + ["show-environment", "-g", "PYTHONHOME"], text=True, capture_output=True)
        report['allowlisted_gui_environment'] = gui_environment
        report['allowlisted_server_environment'] = {key: subprocess.run(tmux + ['show-environment','-g',key],text=True,capture_output=True).stdout.strip() for key in ['PYTHONHOME','PYTHONPATH']}
        report['expected_host_python_environment'] = {'PYTHONHOME': None, 'PYTHONPATH': host_path}
        assert home.returncode != 0 or home.stdout == '-PYTHONHOME\n', 'Bundle Python home reached tmux server'
        server_path = subprocess.check_output(tmux + ["show-environment", "-g", "PYTHONPATH"], text=True).strip()
        assert server_path == 'PYTHONPATH='+host_path, 'Host Python path was not preserved'
        report['bundle_redirects_removed_from_server'] = True
        report['host_python_path_preserved'] = True
        def command(label, filename):
            return shlex.join(['/usr/bin/python3', str(receiver), str(repo/filename), label])
        subprocess.run(tmux + ['new-session','-d','-s',name,'-n','claude','-x','120','-y','30',command('Host module imported - Claude tab','claude-received.json')], check=True)
        subprocess.run(tmux + ['new-window','-d','-t',name,'-n','codex',command('Host module imported - Codex tab','codex-received.json')], check=True)
        identities = []
        for label,filename in [('claude','claude-received.json'),('codex','codex-received.json')]:
            for _ in range(120):
                if (repo/filename).is_file():
                    assert json.loads((repo/filename).read_text()) == []
                    break
                time.sleep(0.1)
            else:
                raise RuntimeError('Host Python receiver could not import json and host module')
            identity = subprocess.check_output(tmux+['display-message','-p','-t',name+':'+label,'#{session_id}:#{pane_id}:#{pane_pid}:#{pid}'],text=True).strip()
            assert re.fullmatch(r'\$[0-9]+:%[0-9]+:[1-9][0-9]*:[1-9][0-9]*',identity), identity
            identities.append(identity)
        report['two_python_receivers_imported_json_and_host_module'] = True
        dbpath = config/'amf/amf.db'
        with sqlite3.connect(dbpath, timeout=30) as db:
            db.execute('BEGIN IMMEDIATE')
            stamp = '2026-10-12T12:00:00Z'
            db.execute('INSERT INTO projects(id,name,repo,is_git,created_at) VALUES(?,?,?,?,?)',('shot-project','Invoice API',str(repo),1,stamp))
            db.execute('INSERT INTO features(id,project_id,name,branch,workdir,tmux_session,status,created_at,last_accessed) VALUES(?,?,?,?,?,?,?,?,?)',('shot-feature','shot-project','Round invoice totals','round-invoice-totals',str(repo),name,'idle',stamp,stamp))
            for index,kind in enumerate(['claude','codex']):
                db.execute('INSERT INTO feature_sessions(id,feature_id,kind,label,tmux_window,created_at,sort_order) VALUES(?,?,?,?,?,?,?)',('shot-'+kind,'shot-feature',kind,kind.capitalize(),kind,stamp,index))
            version = db.execute("SELECT value FROM store_meta WHERE key='store_version'").fetchone()
            db.execute("INSERT OR REPLACE INTO store_meta(key,value) VALUES('store_version',?)",((int(version[0]) if version else 0)+1,))
        frame_env = env | {'AMF_GUI_CAPTURE_SOURCE_APPIMAGE':'1'}
        subprocess.run(['/usr/bin/python3',str(scripts/'capture-gui-appimage-python-frames.py'),str(out),str(gui_pid),inspector,str(config_path)], env=frame_env,check=True)
        report['frontend_location'] = json.loads((out/'appimage-frontend-origin.json').read_text())
        for index,label in enumerate(['claude','codex']):
            assert subprocess.check_output(tmux+['display-message','-p','-t',name+':'+label,'#{session_id}:#{pane_id}:#{pane_pid}:#{pid}'],text=True).strip() == identities[index]
            assert json.loads((repo/(label+'-received.json')).read_text()) == []
        assert hashlib.sha256(image.read_bytes()).hexdigest() == original_hash
        report['identity_preserved_and_no_prompt_sent'] = True
    finally:
        stop_owned_process_group(gui)
        subprocess.run(tmux + ['kill-server'], stderr=subprocess.DEVNULL, check=False)
        closed = subprocess.run(tmux + ['has-session'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode != 0
        report['owned_gui_and_private_server_closed'] = closed and (gui is None or gui.returncode is not None)
        (out/'source-appimage-proof.json').write_text(json.dumps(report,indent=2)+'\n')
    assert report['owned_gui_and_private_server_closed'], 'Owned AppImage resources did not close'
    print('PASS: actual debug AppImage, fresh private server, two host Python receivers, embedded assets, no paid harness or prompt', flush=True)
