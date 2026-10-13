#!/usr/bin/python3
"""Offline synthesis and terminal fixture; refuses non-fixture checkouts."""
import json
import os
import pathlib
import sys
import tty

if '--version' in sys.argv:
    print(pathlib.Path(sys.argv[0]).name + ' 2.1.289 (offline learning fixture)')
    raise SystemExit(0)
if '--help' in sys.argv:
    print('--sandbox --ephemeral --skip-git-repo-check --color --json --output-format --tools --safe-mode --permission-mode --no-session-persistence --pure')
    raise SystemExit(0)
repo = pathlib.Path.cwd()
assert (repo / '.amf-gui-learning-fixture').is_file(), 'Refusing a non-fixture checkout'
interactive = sys.stdin.isatty()
prompt = '' if interactive else sys.stdin.read()
with pathlib.Path(os.environ['AMF_GUI_LEARNING_CALLS']).open('a') as calls:
    calls.write(json.dumps({'kind': 'interactive' if interactive else 'answer', 'workdir': str(repo), 'args': sys.argv[1:], 'prompt': prompt}) + '\n')
if not interactive:
    assert 'read a codebase' in prompt, 'Unexpected headless call'
    assert '--sandbox' in sys.argv and sys.argv[sys.argv.index('--sandbox') + 1] == 'read-only', 'Learning must remain read-only'
    print('## Quota explanation\n\nThe quota guard accepts work only while used is below limit. At the limit, it returns false.\n\nAdd a boundary test for used equal to limit before changing this guard.')
    raise SystemExit(0)
received = repo / '.fixture-received.json'
received.write_text('[]')
tty.setraw(sys.stdin.fileno())
print('\x1b[?2004hOffline learning fixture ready. No prompt has been sent.\r\n', end='', flush=True)
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
