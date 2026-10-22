#!/usr/bin/env python3
"""Reproduce the large-PR screenshot fixture without GitHub or paid agents."""
import importlib.util
import json
import os
from pathlib import Path
import runpy
import subprocess
import sys

session = Path(os.environ['XDG_CONFIG_HOME']) / 'file-tree-proof'
if sys.argv[1:2] == ['--setup']:
    root = Path(__file__).resolve().parents[4]
    spec = importlib.util.spec_from_file_location('workspace_smoke', root / 'scripts/dev/tui-driver/pr_workspace_smoke.py')
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    (session / 'bin').mkdir(parents=True)
    repo, _, git = m.fixture(session)
    git('checkout', '--quiet', 'topic')
    for index in range(120):
        path = repo / f'src/group{index // 10:02}/components/deep/nested/Widget{index:03}.tsx'
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(f'export const Widget{index:03} = () => <button>Review me</button>;\n')
    components = repo / 'aaa/very-long-directory-name/components'
    components.mkdir(parents=True)
    (components / 'Button.tsx').write_text(
        'export const Button = () => <button>Keep filename visible</button>;\n'
    )
    (components / 'VeryLongComponentNameEndingInButton.tsx').write_text(
        'export const LongButton = () => <button>Filename tail</button>;\n'
    )
    git('add', '.')
    git('commit', '--quiet', '-m', 'Large PR tree fixture')
    head = git('rev-parse', 'HEAD')
    git('update-ref', 'refs/pull/7/head', head)
    git('checkout', '--quiet', 'main')
    cpath = session / 'bin/workspace-fixture.json'
    config = json.loads(cpath.read_text())
    config['row']['headRefOid'] = head
    config['row']['title'] = 'File tree: narrow panes and large PRs'
    cpath.write_text(json.dumps(config))
    seed = session / 'seed.json'
    seed.write_text(json.dumps({'path': str(repo), 'project_name': 'File tree proof', 'dry_run': False}))
    subprocess.run([sys.argv[2], 'automation', 'create-project', '--file', str(seed)], check=True)
else:
    runpy.run_path(str(session / 'bin' / Path(sys.argv[0]).name), run_name='__main__')
