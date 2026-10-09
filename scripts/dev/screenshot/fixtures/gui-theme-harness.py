#!/usr/bin/env python3
"""Offline ANSI sample; receives nothing unless a prompt is explicitly sent."""
import pathlib
import runpy
import tty
import sys

tty.setraw(sys.stdin.fileno())
print("Invoice checks\r\n\x1b[32mPASS\x1b[0m rounding preserves cents\r\n\x1b[33mWARN\x1b[0m review negative totals\r\n\x1b[34mINFO\x1b[0m formatter uses USD\r\n\x1b[90mDim output: waiting for an explicit prompt.\x1b[0m\r\n", end="", flush=True)
runpy.run_path(str(pathlib.Path(__file__).with_name("gui-composer-harness.py")), run_name="__main__")
