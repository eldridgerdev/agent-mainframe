#!/usr/bin/python3
"""Offline synthesis and terminal fixture; refuses non-fixture checkouts."""
import json
import os
import pathlib
import sys
import tty

if '--version' in sys.argv:
    print(pathlib.Path(sys.argv[0]).name + ' 2.1.289 (offline planning fixture)')
    raise SystemExit(0)
if '--help' in sys.argv:
    print('--sandbox --ephemeral --skip-git-repo-check --color --json --output-format --tools --safe-mode --permission-mode --no-session-persistence --pure')
    raise SystemExit(0)
repo = pathlib.Path.cwd()
assert (repo / '.amf-gui-planning-fixture').is_file(), 'Refusing a non-fixture checkout'
interactive = sys.stdin.isatty()
with pathlib.Path(os.environ['AMF_GUI_PLANNING_CALLS']).open('a') as calls:
    calls.write(json.dumps({'kind': 'interactive' if interactive else 'synthesis', 'workdir': str(repo), 'args': sys.argv[1:]}) + '\n')
if not interactive:
    prompt = sys.stdin.read()
    assert 'Plan' in prompt and ('synthesi' in prompt.lower() or '# Plan:' in prompt), 'Unexpected headless call'
    print('''# Plan: Validate invoice quotas

## Goal
Validate invoice quotas before accepting new work.

## Decisions
Preserve existing quota settings and report a clear refusal.

## Architecture
Use the existing quota validator before starting work.

## UI
Explain quota failures in the existing dialog.

## Tasks
- [ ] Add quota validation and focused regressions.

## Risks / open questions
Concurrent quota changes must be checked before launch.
''')
    raise SystemExit(0)
received = repo / '.fixture-received.json'
received.write_text('[]')
tty.setraw(sys.stdin.fileno())
print('\x1b[?2004hOffline planning fixture ready. No prompt has been sent.\r\n', end='', flush=True)
buffer = b''
escape = b''
pasting = False
messages = []
while True:
    byte = sys.stdin.buffer.read(1)
    if not byte:
        break
    if escape or byte == b'\x1b':
        escape += byte
        if escape == b'\x1b[200~':
            pasting = True
            escape = b''
        elif escape == b'\x1b[201~':
            pasting = False
            escape = b''
        elif not (b'\x1b[200~'.startswith(escape) or b'\x1b[201~'.startswith(escape)):
            buffer += escape
            escape = b''
        continue
    if byte == b'\x15' and not pasting:
        buffer = b''
    elif byte == b'\r' and not pasting:
        messages.append(buffer.decode('utf-8'))
        received.write_text(json.dumps(messages))
        buffer = b''
    else:
        buffer += b'\n' if pasting and byte == b'\r' else byte
