#!/usr/bin/python3
import json, os, pathlib, sys
log = pathlib.Path(os.environ["XDG_STATE_HOME"]) / "tmux-commands.jsonl"
with log.open("a") as f:
    f.write(json.dumps(sys.argv[1:])+"\n")
os.execv(os.environ["AMF_GUI_CAPTURE_REAL_TMUX"], ["tmux", *sys.argv[1:]])
