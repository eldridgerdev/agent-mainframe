#!/usr/bin/env python3
"""New-user path through the native TUI; emits one JSON result.

Walks what a first-time user meets: Esc on the dashboard, the grouped help,
a non-git folder, project and feature creation, the session leader menu, the
diff viewer on a single-branch repo, and removing a session. Every step
asserts on rendered text, so a regression in any of the UX-sweep fixes
(docs/backlog/tui-first-run-ux-sweep-plan.md) fails here. No paid harness
runs: the driver's harness stubs stay in place.
"""

import argparse
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile

spec = importlib.util.spec_from_file_location("tui_driver", Path(__file__).with_name("amf-tui-driver.py"))
driver = importlib.util.module_from_spec(spec)
spec.loader.exec_module(driver)


def require(condition, detail):
    if not condition:
        raise RuntimeError(driver.encode(detail))


def git(repo, *args):
    subprocess.run(
        ["git", "-C", str(repo), "-c", "user.name=t", "-c", "user.email=t@t", *args],
        check=True, capture_output=True,
    )


def main():
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("--binary", type=Path)
    cli.add_argument("--driver", type=Path, default=Path(__file__).with_name("amf-tui-driver.py"))
    args = cli.parse_args()
    command = [sys.executable, str(args.driver), "start"]
    if args.binary:
        command += ["--binary", str(args.binary)]
    launch = subprocess.run(command, capture_output=True, text=True)
    require(launch.returncode == 0, launch.stdout or launch.stderr)
    session = Path(json.loads(launch.stdout)["session"])
    steps = []

    def call(request):
        response = driver.call(session, request)
        require(response["ok"], response)
        return response["result"]

    def run(name, actions):
        """Run a batch; its last snapshot (if any) is returned for assertions."""
        result = call({"op": "batch", "actions": actions})
        steps.append(name)
        final = result.get("result") or {}
        return "\n".join(line["text"] for line in final.get("lines", []))

    def keys(*names):
        return {"op": "keys", "keys": list(names)}

    def wait(text, absent=False):
        action = {"op": "wait", "text": text}
        if absent:
            action["absent"] = True
        return action

    snap = {"op": "snapshot"}

    with tempfile.TemporaryDirectory(prefix="amf-newuser-") as scratch:
        plain = Path(scratch) / "plain"
        plain.mkdir()
        repo = Path(scratch) / "app"
        repo.mkdir()
        git(repo, "init", "-q", "-b", "main")
        (repo / "README.md").write_text("# app\n")
        git(repo, "add", "README.md")
        git(repo, "commit", "-qm", "init")

        try:
            # Empty dashboard says what to do next.
            text = run("empty dashboard", [wait("No projects yet"), snap])
            require("Press N to add a project" in text, text)

            # Esc must ask, not quit.
            run("esc asks", [keys("Escape"), wait("Quit AMF?")])
            run("esc cancel", [keys("n"), wait("Quit AMF?", absent=True)])

            # Help is grouped and explains Esc.
            text = run("help", [keys("?"), wait("What Esc does on each screen"), snap])
            require("Getting around" in text and "Projects & features" in text, text)
            run("help close", [keys("Escape"), wait("Keybindings", absent=True)])

            # A non-git folder is flagged and Ctrl+G fixes it.
            run("new project", [keys("N"), wait("New Project"), {"op": "paste", "text": "plain-app"},
                                keys("Tab"), {"op": "paste", "text": str(plain)}])
            text = run("non-git warning", [wait("not a git repository"), snap])
            require("Ctrl+G" in text, text)
            run("git init", [keys("C-g"), wait("Initialized a git repository")])
            require((plain / ".git").exists(), "Ctrl+G did not run git init")
            run("cancel project", [keys("Escape"), wait("New Project", absent=True)])

            # Real project from the git fixture.
            run("create project", [
                keys("N"), wait("New Project"), {"op": "paste", "text": "todo-app"},
                keys("Tab"), {"op": "paste", "text": str(repo)}, keys("Enter", "Enter"),
                wait("Created project"),
            ])
            text = run("project footer", [snap])
            require("n add a feature" in text or "Press n to add a feature" in text, text)

            # One-form feature wizard; Ctrl+S creates and opens the session.
            run("new feature", [keys("n"), wait("New Feature"), keys("C-u"),
                                {"op": "paste", "text": "add-todos"}])
            text = run("wizard footer", [snap])
            require("Ctrl+S create" in text, text)
            run("create feature", [keys("C-s"), wait("Compose")])

            # Session leader menu: grouped, unknown keys stay inside it.
            run("leader open", [keys("Escape"), keys("C-Space"), wait("Ctrl+Space commands")])
            text = run("leader groups", [snap])
            require("Sessions" in text and "Stop session" in text, text)
            run("leader unknown key", [keys("Z"), wait("No command on")])
            run("leader close", [keys("Escape"), wait("Ctrl+Space commands", absent=True)])

            # Diff viewer on a single-branch repo falls back to HEAD.
            (repo / "README.md").write_text("# app\nmore\n")
            run("diff scope", [keys("C-Space", "d"), wait("Choose Diff Scope")])
            text = run("diff viewer", [keys("Enter"), wait("no base branch found"), snap])
            require("README.md" in text, text)
            run("diff close", [keys("Escape"), wait("Current Changes", absent=True)])

            # Back to the dashboard; removing a live session asks first.
            run("dashboard", [keys("C-Space", "q"), wait("Returned to dashboard")])
            run("select session", [keys("Enter"), keys("Down")])
            run("remove asks", [keys("d"), wait("Confirm Remove Session")])
            run("remove cancel", [keys("n"), wait("Confirm Remove Session", absent=True)])
        finally:
            response = driver.stop_session(session)
            require(response["ok"], response)

    print(driver.encode({"ok": True, "new_user_path": "passed", "steps": len(steps)}))


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(driver.encode({"ok": False, "error": str(error)}))
        sys.exit(1)
