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
workdir = root / 'wide-demo'
workdir.mkdir()

def call(action, payload, env=None):
    result = subprocess.run([amf, 'automation', action], input=json.dumps(payload), text=True,
                            capture_output=True, check=True, env={**os.environ, **(env or {})})
    response = json.loads(result.stdout)
    assert response['ok'], response
    return response

call('create-project', dict(path=str(workdir), project_name='wide-demo', preferred_agent='codex'))
call('create-feature', dict(project_name='wide-demo', branch='main', agent='codex', mode='vibe', use_worktree=False))
listing = call('list-sessions', dict(project_name='wide-demo', feature='main'))
agent = next(s for s in listing['sessions'] if s['kind'] == 'codex')

notes = (
    "Request flow, wider than the viewer:\n\n```mermaid\nflowchart LR\n"
    " A[Receive request from the client] --> B[Validate the payload schema]\n"
    " B --> C[Authenticate the caller token]\n"
    " C --> D[Load the account from storage]\n"
    " D --> E[Write the audit log entry]\n"
    " E --> F[Return the final response]\n```\n\n"
    "This prose stays wrapped; only the diagram pans.\n"
)
call('create-session', dict(kind='terminal', label='flow', reuse=True, notes=notes),
     {'AMF_FEATURE_SESSION_ID': agent['session_id']})
print('Seeded a terminal note holding a diagram wider than the viewer.')
