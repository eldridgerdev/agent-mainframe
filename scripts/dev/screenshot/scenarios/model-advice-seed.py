import json, os, sqlite3, subprocess, sys
from pathlib import Path
root, binary = Path(sys.argv[1]), sys.argv[2]
repo = root / 'demo-repository'
repo.mkdir()
subprocess.run(['git','init','-q','-b','main',str(repo)],check=True)
(repo / 'README.md').write_text('Isolated screenshot fixture for model advice.\n')
subprocess.run(['git','-C',str(repo),'add','README.md'],check=True)
subprocess.run(['git','-C',str(repo),'-c','user.name=Screenshot fixture','-c','user.email=fixture@example.invalid','commit','-qm','Initial fixture'],check=True)
(repo / 'amf.json').write_text(json.dumps({'allowed_agents':['codex']}))
conn = sqlite3.connect(Path(os.environ['XDG_CONFIG_HOME']) / 'amf/amf.db')
pid = 'proof-project'
conn.execute("INSERT INTO projects(id,name,repo,collapsed,preferred_agent,is_git,created_at) VALUES (?,'model-advice-demo',?,0,'codex',1,datetime('now'))",(pid,str(repo)))
conn.execute("INSERT OR REPLACE INTO store_meta(key,value) VALUES ('available_harnesses','[\"codex\"]')")
conn.execute("INSERT INTO features(id,project_id,name,branch,workdir,is_worktree,tmux_session,mode,agent,status,collapsed,created_at,last_accessed) VALUES ('proof-host',?,'parser-maintenance','main',?,0,'amf-proof-model-advice-host','vibe','codex','stopped',0,datetime('now'),datetime('now'))",(pid,str(repo)))
conn.execute("INSERT INTO feature_sessions(id,feature_id,kind,label,tmux_window,created_at) VALUES ('proof-todos','proof-host','todos','TODOs','todos',datetime('now'))")
conn.execute("INSERT INTO todo_lists(id,project_id,feature_id,scope,created_at,updated_at) VALUES ('proof-list',?,'proof-host','project',datetime('now'),datetime('now'))",(pid,))
conn.execute("INSERT INTO todos(id,list_id,title,body,priority,sort_order,status,created_at,updated_at) VALUES ('proof-todo','proof-list','Implement parser','Handle malformed Unicode input without panics.','med',0,'not_started',datetime('now'),datetime('now'))")
plan = '# Plan: parser boundaries\n\n## Goal\nHandle malformed Unicode input without panics.\n\n## Decisions\n- Validate boundaries before parsing.\n- Keep the public interface unchanged.\n\n## Tasks\n- [ ] Add deterministic boundary validation.\n- [ ] Test malformed and empty input.\n- [ ] Run the existing parser suite.\n'
for key, name in [('proof-host','parser-maintenance'),('todo:proof-todo','parser-maintenance'),('pending:model-advice-demo/implement-parser','implement-parser')]:
    conn.execute("INSERT OR REPLACE INTO plan_interviews(feature_id,stage,feature_name,brief,questions,answers,plan,ai_rounds_completed,created_at,updated_at) VALUES (?,'draft',?,'Handle malformed Unicode input safely.','[]','[]',?,0,datetime('now'),datetime('now'))",(key,name,plan))
conn.commit()
