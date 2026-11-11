"""Seed a feature whose agent left notes on two Terminal Sessions.

The agent's calls carry no project or feature name: like a real agent they
rely on AMF_FEATURE_SESSION_ID, which `create-session` resolves to the
agent's own feature.
"""
import json, pathlib, subprocess, sys, os, sqlite3, time
amf, root = sys.argv[1:]
root = pathlib.Path(root).resolve()
assert pathlib.Path(os.environ['XDG_CONFIG_HOME']).resolve().is_relative_to(root), 'private screenshot config required'
db = pathlib.Path(os.environ['XDG_CONFIG_HOME']) / 'amf/amf.db'
with sqlite3.connect(db) as conn:
    conn.execute("INSERT OR REPLACE INTO store_meta(key,value) VALUES('available_harnesses',?)", (json.dumps(['codex']),))
    conn.execute("UPDATE store_meta SET value=CAST(value AS INTEGER)+1 WHERE key='store_version'")
time.sleep(1.5)
workdir = root / 'notes-demo'
workdir.mkdir()

def call(action, payload, env=None):
    result = subprocess.run([amf, 'automation', action], input=json.dumps(payload), text=True,
                            capture_output=True, check=True, env={**os.environ, **(env or {})})
    response = json.loads(result.stdout)
    assert response['ok'], response
    return response

call('create-project', dict(path=str(workdir), project_name='notes-demo', preferred_agent='codex'))
call('create-feature', dict(project_name='notes-demo', branch='main', agent='codex', mode='vibe', use_worktree=False))
listing = call('list-sessions', dict(project_name='notes-demo', feature='main'))
agent = next(s for s in listing['sessions'] if s['kind'] == 'codex')

# From here on, behave like the agent: its own session id and nothing else.
env = {'AMF_FEATURE_SESSION_ID': agent['session_id']}
call('create-session', dict(kind='terminal', label='dev server', reuse=True, notes=(
    "Run the server and try the login form:\n\n```bash\nnpm run dev\n```\n\n"
    "Open http://localhost:3000/login and sign in as `demo@example.com`.\n")), env)
call('create-session', dict(kind='terminal', label='tests', reuse=True, notes=(
    "Run the integration tests and tell me which ones fail:\n\n```bash\nnpm test -- --runInBand\n```\n")), env)
# Reusing a label replaces that terminal's notes instead of adding a block.
again = call('create-session', dict(kind='terminal', label='dev server', reuse=True, notes=(
    "Run the server and try the login form:\n\n```bash\nnpm run dev\n```\n\n"
    "Open http://localhost:3000/login and sign in as `demo@example.com`.\n")), env)
assert again['reused'], again
entries = json.loads((workdir / '.amf/terminal-notes.json').read_text())
assert [e['label'] for e in entries] == ['dev server', 'tests'], entries
print('Seeded two terminals with notes through the agent session id.')
