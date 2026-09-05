import json
import os
import sys
import time

pdir = sys.argv[1]


def write(name, prompts, mtime):
    path = os.path.join(pdir, name)
    with open(path, "w") as f:
        for p in prompts:
            f.write(json.dumps({"type": "user", "message": {"content": p}}) + "\n")
    os.utime(path, (mtime, mtime))


now = time.time()

# Older session file -- as if this were the file left behind before a
# `claude --resume` or context-compaction started a fresh one.
write(
    "11111111-1111-1111-1111-111111111111.jsonl",
    [
        "Add a CLI flag to skip the confirmation prompt",
        "Also update the help text for the new flag",
    ],
    now - 3600,
)

# Newer session file -- what a resume would produce.
write(
    "22222222-2222-2222-2222-222222222222.jsonl",
    [
        "Picking this back up after a break, run the test suite",
        "Now fix the failing snapshot test",
    ],
    now,
)
