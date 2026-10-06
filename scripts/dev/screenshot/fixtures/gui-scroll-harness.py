#!/usr/bin/env python3
"""Unpaid stand-ins for agent harnesses, for the GUI terminal-scrolling proof.

`transcript` behaves like Claude/Codex/Pi: it writes a long transcript on the
normal screen (so tmux keeps it as history), then prints a further burst
whenever the capture script creates `<workdir>/transcript-more`, without
anything being typed into it. `fullscreen` behaves like OpenCode/Neovim: it
draws a scrollable list on the alternate screen, asks for SGR mouse
reporting, and scrolls when wheel reports arrive.

Every byte either fixture receives on stdin is appended to
`<workdir>/<mode>-received.json`, so the capture can prove scrolling sent the
transcript nothing and sent the full-screen program only wheel reports.
"""

import json
import os
import pathlib
import re
import select
import shutil
import sys
import tty

mode, workdir = sys.argv[1], pathlib.Path(sys.argv[2])
received_path = workdir / f"{mode}-received.json"
trigger = workdir / f"{mode}-more"
received = []
received_path.write_text("[]")
tty.setraw(sys.stdin.fileno())


def out(text):
    sys.stdout.write(text)
    sys.stdout.flush()


def record(data):
    received.append(data.decode("utf-8", "replace"))
    received_path.write_text(json.dumps(received))


DIM, BOLD, CYAN, GREEN, YELLOW, RESET = "\x1b[2m", "\x1b[1m", "\x1b[36m", "\x1b[32m", "\x1b[33m", "\x1b[0m"
STEPS = [
    "Read src/invoice.ts",
    "Read src/invoice.test.ts",
    "Searched for roundCents",
    "Ran npm test (12 passed)",
    "Edited invoiceTotal",
    "Ran npm test (13 passed)",
]


def transcript():
    out(f"{BOLD}AMF capture fixture · long-running agent transcript{RESET}\r\n")
    out(f"{DIM}Offline stand-in: no model is called.{RESET}\r\n\r\n")
    for step in range(1, 121):
        detail = STEPS[step % len(STEPS)]
        out(f"{CYAN}● step {step:03d}{RESET} {detail}\r\n")
    out(f"\r\n{GREEN}Waiting for the next instruction…{RESET}\r\n")
    burst = 0
    while True:
        ready, _, _ = select.select([sys.stdin], [], [], 0.2)
        if ready:
            data = os.read(sys.stdin.fileno(), 4096)
            if not data:
                return
            record(data)
        if trigger.exists():
            trigger.unlink()
            burst += 1
            for line in range(1, 31):
                out(f"{YELLOW}▲ live {burst}.{line:02d}{RESET} new output while you read\r\n")
            out(f"{GREEN}Burst {burst} done.{RESET}\r\n")


def fullscreen():
    items = [f"Message {n:03d} · {STEPS[n % len(STEPS)]}" for n in range(1, 201)]
    wheel = re.compile(rb"\x1b\[<(\d+);\d+;\d+M")
    reports = 0
    out("\x1b[?1049h\x1b[?1000h\x1b[?1006h\x1b[?25l")
    offset = None
    while True:
        cols, rows = shutil.get_terminal_size((80, 24))
        visible = max(1, rows - 3)
        if offset is None:
            offset = len(items) - visible
        offset = max(0, min(offset, len(items) - visible))
        frame = [f"\x1b[H\x1b[2J{BOLD}AMF capture fixture · full-screen program (alternate screen, mouse reporting on){RESET}"]
        frame += [items[i][:cols] for i in range(offset, offset + visible)]
        frame.append(f"{DIM}showing {offset + 1}-{offset + visible} of {len(items)} · wheel reports received: {reports}{RESET}")
        out("\r\n".join(frame))
        ready, _, _ = select.select([sys.stdin], [], [], 0.3)
        if not ready:
            continue
        data = os.read(sys.stdin.fileno(), 4096)
        if not data:
            return
        record(data)
        for match in wheel.finditer(data):
            button = int(match.group(1))
            if button in (64, 65):
                reports += 1
                offset += -1 if button == 64 else 1


try:
    transcript() if mode == "transcript" else fullscreen()
finally:
    if mode != "transcript":
        out("\x1b[?1006l\x1b[?1000l\x1b[?1049l\x1b[?25h")
