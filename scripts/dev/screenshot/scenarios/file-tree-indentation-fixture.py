#!/usr/bin/env python3
"""Seed the nested-file screenshot fixture; proxy git/gh without network writes."""
import importlib.util
import json
import os
from pathlib import Path
import runpy
import subprocess
import sys

session = Path(os.environ['XDG_CONFIG_HOME']) / 'indentation-proof'
if sys.argv[1:2] == ['--setup']:
    root = Path(__file__).resolve().parents[4]
    spec = importlib.util.spec_from_file_location(
        'workspace_smoke', root / 'scripts/dev/tui-driver/pr_workspace_smoke.py'
    )
    smoke = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(smoke)
    (session / 'bin').mkdir(parents=True)
    repo, _, git = smoke.fixture(session)
    git('checkout', '--quiet', 'topic')
    for name in [
        'src/components/Button.tsx',
        'src/ui/dialogs/diff.rs',
        'src/ui/dialogs/pr_workspace.rs',
        'src/ui/dialogs/widgets/Badge.rs',
    ]:
        path = repo / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text('// Nested file tree fixture\n')
    git('add', '.')
    git('commit', '--quiet', '-m', 'Nested file tree fixture')
    head = git('rev-parse', 'HEAD')
    git('update-ref', 'refs/pull/7/head', head)
    git('checkout', '--quiet', 'main')
    config_path = session / 'bin/workspace-fixture.json'
    config = json.loads(config_path.read_text())
    config['row']['headRefOid'] = head
    config['row']['title'] = 'One-space file tree indentation'
    config_path.write_text(json.dumps(config))
    seed = session / 'seed.json'
    seed.write_text(json.dumps({
        'path': str(repo), 'project_name': 'Indentation proof', 'dry_run': False,
    }))
    subprocess.run([sys.argv[2], 'automation', 'create-project', '--file', str(seed)], check=True)
else:
    runpy.run_path(str(session / 'bin' / Path(sys.argv[0]).name), run_name='__main__')
