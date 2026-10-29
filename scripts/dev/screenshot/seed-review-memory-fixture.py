#!/usr/bin/env python3
"""Seed review-memory docs and an AI Review fixture for the screenshot harness.

Usage: seed-review-memory-fixture.py <config_dir> <state_dir> <repo_root> <pr_number> <owner/repo>

Fully offline: no model and no network. It:
  * writes a project and a cross-project review-memory doc under
    <state_dir>/review-memory/, the global one repeating one project finding so
    the pane can show it being counted once;
  * points the scratch AMF config (<config_dir>/amf/config.json) at those two
    docs with absolute paths, so a local capture never reads or writes the
    real checkout's `.amf/review-memory.md` (in a linked worktree that resolves
    to the main checkout) or the real `~/.config/amf/review-memory.md`;
  * writes <state_dir>/ai-review-fixture.json for `amf automation
    seed-ai-review`, with an attribution recording what the "last run" read
    and sent.

Restart the TUI after running this so it reads the updated config.
"""
import json
import subprocess
import sys
from pathlib import Path

config_dir, state_dir, repo_root, pr_number, repository = sys.argv[1:6]
pr_number = int(pr_number)
state = Path(state_dir)
memory_dir = state / "review-memory"
memory_dir.mkdir(parents=True, exist_ok=True)
project_doc = memory_dir / "project.md"
global_doc = memory_dir / "global.md"

project_doc.write_text(
    "# Review memory\n\n"
    "## Concurrency\n"
    "- Hold the lock across the read-modify-write, not just the write\n\n"
    "## Error handling\n"
    "- Report an unreadable file instead of treating it as empty\n\n"
    "## Tests\n"
    "- Cover the error path, not only the happy path\n"
)
global_doc.write_text(
    "# Review memory (cross-project)\n\n"
    "## Tests\n"
    "- Cover the error path, not only the happy path\n"
    "- Assert on the message, not just the error type\n\n"
    "## Naming\n"
    "- Prefer full words over abbreviations\n"
)

config_path = Path(config_dir) / "amf" / "config.json"
config_path.parent.mkdir(parents=True, exist_ok=True)
config = json.loads(config_path.read_text()) if config_path.exists() else {}
config["review_memory_path"] = str(project_doc)
config["global_review_memory_path"] = str(global_doc)
config_path.write_text(json.dumps(config, indent=2) + "\n")

head_sha = subprocess.run(
    ["git", "-C", repo_root, "rev-parse", "HEAD"],
    check=True,
    capture_output=True,
    text=True,
).stdout.strip()


def doc(scope, path, findings, duplicates=0):
    return {
        "scope": scope,
        "path": str(path),
        "state": "loaded",
        "findings": findings,
        "duplicates": duplicates,
    }


fixture = {
    "pr_number": pr_number,
    "head_sha": head_sha,
    "summary": "The deterministic review checked the diff against both review-memory docs.",
    "findings": [
        {
            "path": "src/app/review_memory.rs",
            "line": 120,
            "side": "New",
            "body": "This read swallows a permission error as an empty doc, the exact recurring finding the project memory lists.",
            "diff_hunk": "@@ -118,3 +118,4 @@\n fn load(path: &Path) -> String {\n+    std::fs::read_to_string(path).unwrap_or_default()\n }",
            "skipped": False,
            "published": False,
        }
    ],
    "open": True,
    "workdir": repo_root,
    "repository": repository,
    "head_ref": "screenshot-fixture",
    "attribution": {
        "harness": "Claude",
        "model": "opus",
        "input_tokens": 18400,
        "output_tokens": 2100,
        "estimated_cost": "$0.12",
        "elapsed_ms": 48000,
        "memory": {
            "docs": [
                doc("project", project_doc, 3),
                doc("global", global_doc, 2, duplicates=1),
            ],
            "context_chars": 420,
            "prompts": 1,
            "prompts_with_memory": 1,
        },
    },
}
(state / "ai-review-fixture.json").write_text(json.dumps(fixture, indent=2) + "\n")
