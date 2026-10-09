#!/usr/bin/env python3
"""Offline raw receiver for the literal-input native capture; launches no AI."""
import json, pathlib, sys, tty
expected = '$HOME ${x} \\ "double" \'single\' #{pane_id}'
tty.setraw(sys.stdin.fileno())
received = pathlib.Path(sys.argv[1])
received.write_text("[]")
print("Literal terminal input proof\r\nOffline byte receiver (no shell expansion, no AI)\r\n\r\nExpected text:\r\n" + expected + "\r\n\r\nReady for literal input.\r\n", end="", flush=True)
buffer = b""
while True:
    byte = sys.stdin.buffer.read(1)
    if not byte:
        break
    if byte == b"\r":
        text = buffer.decode("utf-8")
        received.write_text(json.dumps([text]))
        print("\r\nReceived literal bytes:\r\n" + text + "\r\n\r\n" + ("PASS: every byte matches the expected text." if text == expected else "FAIL: input changed in transit.") + "\r\n", end="", flush=True)
        buffer = b""
    else:
        buffer += byte
