import json, pathlib, subprocess, sys, os, sqlite3, time
amf, root = sys.argv[1:]
root = pathlib.Path(root).resolve()
assert pathlib.Path(os.environ['XDG_CONFIG_HOME']).resolve().is_relative_to(root), 'private screenshot config required'
db = pathlib.Path(os.environ['XDG_CONFIG_HOME']) / 'amf/amf.db'
with sqlite3.connect(db) as conn:
    conn.execute("INSERT OR REPLACE INTO store_meta(key,value) VALUES('available_harnesses',?)", (json.dumps(['codex']),))
    conn.execute("UPDATE store_meta SET value=CAST(value AS INTEGER)+1 WHERE key='store_version'")
time.sleep(1.5)
workdir = root / 'non-git-demo'
workdir.mkdir()
def call(action, payload):
    result = subprocess.run([amf, 'automation', action], input=json.dumps(payload), text=True, capture_output=True, check=True)
    response = json.loads(result.stdout)
    assert response['ok'], response
    (root / (action + '-' + str(payload.get('label','setup')).replace(' ','-') + '.json')).write_text(json.dumps(response))
    return response
call('create-project', dict(path=str(workdir), project_name='non-git-demo', preferred_agent='codex'))
call('create-feature', dict(project_name='non-git-demo', branch='main', agent='codex', mode='vibe', use_worktree=False))
base = dict(project_name='non-git-demo', feature='main', kind='codex')
preview = call('create-session', dict(base, label='Preview only', dry_run=True))
assert preview['session_id'] is None
for label, prompt, submit in [('Docs draft', 'Write a concise README for this project.', False), ('Run task', 'Summarize the documentation gaps.', True)]:
    response = call('create-session', dict(base, label=label, initial_prompt=prompt, submit_prompt=submit))
    assert response['session_id'] and response['tmux_window']
print('Verified dry-run and two tracked sessions on a non-git feature.')
