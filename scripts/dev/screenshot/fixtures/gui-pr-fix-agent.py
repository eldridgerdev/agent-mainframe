#!/usr/bin/env python3
"""Offline interactive agent: accepts one bracketed prompt and returns a receipt."""
import json
import os
import pathlib
import re
import subprocess
import sys
import termios
import tty

previous = termios.tcgetattr(sys.stdin.fileno())
try:
    tty.setraw(sys.stdin.fileno())
    sys.stdout.write("\x1b[?2004hOffline Codex fix fixture ready\r\n")
    sys.stdout.flush()
    buffer = b""
    while True:
        buffer += os.read(sys.stdin.fileno(), 4096)
        if b"\x1b[201~" not in buffer or not buffer.endswith(b"\r"):
            continue
        prompt = buffer.split(b"\x1b[200~", 1)[-1].split(b"\x1b[201~", 1)[0].decode()
        pathlib.Path(os.environ["AMF_GUI_FIX_PROMPTS"]).write_text(prompt)
        matches = list(re.finditer(r"amf reply-draft --pr-number (\d+) --comment-id (\d+) --request-id ([\w-]+)", prompt))
        assert matches, prompt
        for match in matches:
            subprocess.run([os.environ["AMF_GUI_REPLY_BIN"], "reply-draft", "--pr-number", match[1], "--comment-id", match[2], "--request-id", match[3]], input="Fixed negative invoice rounding. Added regression coverage.", text=True, check=True)
        sys.stdout.write("\r\nOffline fix complete; returned reply draft.\r\n")
        sys.stdout.flush()
        buffer = b""
finally:
    termios.tcsetattr(sys.stdin.fileno(), termios.TCSANOW, previous)
