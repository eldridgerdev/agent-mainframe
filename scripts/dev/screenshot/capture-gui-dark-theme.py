#!/usr/bin/env python3
"""Compare main's frontend with the new palette in two isolated native GUIs.

Both runs share the current Rust backend, private Git/SQLite/tmux fixtures and
an offline terminal stand-in. HOME stays unchanged. Only palette and terminal files
are replaced in the temporary baseline; repository files are never changed.
"""
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile

workspace = pathlib.Path(__file__).resolve().parents[3]
scripts = workspace / "scripts/dev/screenshot"
out = pathlib.Path(sys.argv[1]).resolve()
out.mkdir(parents=True, exist_ok=True)
# PR #713's merge is the main baseline immediately before this increment.
base = '6183a91afc1901614ddea9dac55e70e3d320e561'
if subprocess.run(["git", "cat-file", "-e", base + "^{commit}"], cwd=workspace, stderr=subprocess.DEVNULL).returncode:
    subprocess.run(["git", "fetch", "--no-tags", "origin", base], cwd=workspace, check=True)
with tempfile.TemporaryDirectory(prefix="amf-theme-baseline-") as temporary:
    baseline = pathlib.Path(temporary) / "gui"
    shutil.copytree(workspace / "gui", baseline, ignore=shutil.ignore_patterns("node_modules", "dist", "src-tauri"))
    (baseline / "node_modules").symlink_to(workspace / "gui/node_modules", target_is_directory=True)
    for name in ["styles.css", "syntax.css", "sessionSidebar.css", "sessions.css", "TerminalPane.tsx"]:
        content = subprocess.check_output(["git", "show", f"{base}:gui/src/{name}"], cwd=workspace)
        (baseline / "src" / name).write_bytes(content)
    notes = []
    for mode, frontend in [("before", baseline), ("after", workspace / "gui")]:
        frames = out / mode
        env = os.environ | {
            "AMF_PROOF_MODE": mode,
            "AMF_GUI_CAPTURE_FRONTEND": str(frontend),
            "AMF_GUI_CAPTURE_HARNESS": str(scripts / "fixtures/gui-theme-harness.py"),
            "AMF_GUI_CAPTURE_FRAMES": str(scripts / "capture-gui-dark-theme-frames.py"),
        }
        subprocess.run(["/usr/bin/python3", str(scripts / "capture-gui-composer.py"), str(frames)], env=env, check=True)
        for line in (frames / "capture-notes.jsonl").read_text().splitlines():
            note = json.loads(line)
            source = pathlib.Path(note["file"])
            name = f"{len(notes)+1:03d}-{mode}-{source.stem[4:]}.png"
            shutil.copy2(frames / source, out / name)
            shutil.copy2(frames / source.with_suffix(".txt"), out / pathlib.Path(name).with_suffix(".txt"))
            notes.append({"file": pathlib.Path(name).with_suffix(".ansi").name, "note": note["note"]})
    (out / "capture-notes.jsonl").write_text("".join(json.dumps(note)+"\n" for note in notes))
