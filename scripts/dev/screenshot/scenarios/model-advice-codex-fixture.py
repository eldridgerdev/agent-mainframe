#!/usr/bin/env python3
import json, sys
args = sys.argv[1:]
if '--version' in args or '--help' in args:
    print('codex screenshot fixture; exec --sandbox read-only --ephemeral --skip-git-repo-check --color --json')
elif 'app-server' in args:
    replies = {'initialize': {}, 'account/read': {'account': {'type': 'chatgpt'}, 'requiresOpenaiAuth': True}, 'model/list': {'data': [{'model': 'gpt-6.1-sol', 'hidden': False, 'supportedReasoningEfforts': [{'reasoningEffort': e} for e in ['low','medium','high']]}], 'nextCursor': None}, 'configRequirements/read': {'requirements': None}, 'config/read': {'config': {'model_provider': 'openai'}}}
    for line in sys.stdin:
        req = json.loads(line)
        if 'id' in req:
            print(json.dumps({'id':req['id'], 'result':replies[req['method']]}), flush=True)
elif 'exec' in args:
    prompt = sys.stdin.read()
    options = json.loads(prompt.split('Eligible options: ',1)[1].split('\nAttributed research:',1)[0])
    choices = []
    for effort, priority in [('low','speed'),('medium','balance'),('high','depth')]:
        row = next(c for c in options if c['reasoning'] == effort)
        choices.append({'option_id':row['option_id'],'evidence_ids':row['evidence_ids'],'priority':priority})
    print(json.dumps({'status':'qualified','choices':choices}))
else:
    raise SystemExit('This screenshot fixture never launches an interactive agent')
