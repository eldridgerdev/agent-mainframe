#!/usr/bin/env python3
import os, sys, termios, tty, signal
if '--version' in sys.argv:
    print('codex offline screenshot fixture 1.0')
    sys.exit(0)
# Only the tmux-backed interactive fixture accepts input.
if not os.isatty(0):
    sys.exit(0)
old = termios.tcgetattr(0)
tty.setraw(0)
text = ''
submitted = None
pending = b''
pasting = False
def draw():
    screen = '\x1b[?2004h\x1b[2J\x1b[HOffline agent fixture — no model calls\r\n'
    screen += 'AMF automation #771: real session creation and bracketed paste\r\n\r\n'
    if submitted is None:
        screen += 'UNSENT DRAFT — waiting for Enter'
        screen += f'\x1b[{max(6, os.get_terminal_size(1).lines - 4)};1H› ' + text
    else:
        screen += 'SUBMITTED TASK — Enter received\r\n\r\n' + submitted + '\r\n\r\n› '
    os.write(1, screen.encode())
signal.signal(signal.SIGWINCH, lambda *_: draw())
try:
    draw()
    while True:
        chunk = os.read(0, 4096)
        if not chunk: break
        pending += chunk
        while pending:
            if pending.startswith(b'\x1b[200~'):
                pasting = True; pending = pending[6:]; continue
            if pending.startswith(b'\x1b[201~'):
                pasting = False; pending = pending[6:]; continue
            if pending.startswith(b'\x1b') and len(pending) < 6: break
            value = pending[0]; pending = pending[1:]
            if value in (10,13) and not pasting:
                submitted = text; text = ''
            elif value in (8,127): text = text[:-1]
            elif value >= 32 and value < 127: text += chr(value)
        draw()
finally:
    termios.tcsetattr(0, termios.TCSANOW, old)
