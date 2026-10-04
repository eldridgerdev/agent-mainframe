#!/usr/bin/env python3
"""Unpaid, raw-terminal fixture receiving real bracketed-paste messages."""

import json
import pathlib
import sys
import tty

tty.setraw(sys.stdin.fileno())
received = pathlib.Path(sys.argv[1])
received.write_text("[]")
print("\x1b[?2004hAMF capture fixture · " + sys.argv[2] + "\r\n", end="", flush=True)
print("Ready. Nothing is sent while you compose in the GUI.\r\n", end="", flush=True)
buffer = b""
messages = []
pasting = False
escape = b""
while True:
    byte = sys.stdin.buffer.read(1)
    if not byte:
        break
    if escape or byte == b"\x1b":
        escape += byte
        if escape == b"\x1b[200~":
            pasting = True
            escape = b""
        elif escape == b"\x1b[201~":
            pasting = False
            escape = b""
        elif not (b"\x1b[200~".startswith(escape) or b"\x1b[201~".startswith(escape)):
            buffer += escape
            escape = b""
        continue
    if byte == b"\x15" and not pasting:
        buffer = b""
    elif byte == b"\r" and not pasting:
        message = buffer.decode("utf-8")
        messages.append(message)
        received.write_text(json.dumps(messages))
        print("\r\nReceived one complete prompt:\r\n", end="", flush=True)
        print(message.replace("\n", "\r\n") + "\r\n\r\nReady for the next prompt.\r\n", end="", flush=True)
        buffer = b""
    else:
        # tmux's paste-buffer maps line endings to CR; harness editors
        # normalize those to LF within a bracketed paste.
        buffer += b"\n" if pasting and byte == b"\r" else byte
