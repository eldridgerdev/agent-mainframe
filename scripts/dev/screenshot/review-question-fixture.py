#!/usr/bin/env python3
"""Offline screenshot fixture and command shims for reviewer questions.

Installed as codex/gh/git by amf-capture.sh, this supplies deterministic model
text and PR metadata. All git fetches stay inside the throwaway repository.
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time

REPOSITORY = "https://github.com/review-demo/helper-reuse"
REAL_GIT = shutil.which("git", path=os.defpath)


def git(*args, cwd=None):
    return subprocess.check_output([REAL_GIT, *args], cwd=cwd, text=True).strip()


def metadata():
    return json.loads((Path.cwd() / ".git/amf-screenshot.json").read_text())


def codex():
    if "--version" in sys.argv:
        print("codex-cli 0.0.0 (screenshot fixture)")
        return
    if "exec" not in sys.argv:
        print("Deterministic screenshot session; no model is running.", flush=True)
        while True:
            time.sleep(10)
    args = sys.argv[1:]
    if "--sandbox" not in args or args[args.index("--sandbox") + 1] != "read-only":
        raise SystemExit("Screenshot question must use the read-only sandbox")
    prompt = sys.stdin.read()
    time.sleep(1.5)
    if prompt.startswith("Write a concise, constructive review comment"):
        print("Could we reuse `normalized_name` from `src/helpers.rs:1` here to keep normalization consistent?")
    else:
        print("Yes. The unchanged `normalized_name` helper at `src/helpers.rs:1-3` performs the same "
              "`trim().to_ascii_lowercase()` operation added at `src/caller.rs:2` (RIGHT).\n\n"
              "It can replace the duplicate normalization here.\n\n"
              "I checked both tracked source files. The helper is outside the displayed diff.\n\n"
              "Check its import before calling it.")


def gh():
    args = sys.argv[1:]
    if args[:2] == ["auth", "status"] or "--version" in args:
        print("gh screenshot fixture")
        return
    if args[:2] == ["api", "user"]:
        print("reviewer")
        return
    data = metadata()
    pr = dict(number=42, title="Reuse name normalization in the caller", author={"login": "teammate"},
              isDraft=False, updatedAt="2026-09-28T12:00:00Z", baseRefName="main",
              headRefName="review-helper", headRefOid=data["head"], state="OPEN",
              url=REPOSITORY + "/pull/42")
    if args[:2] == ["repo", "view"]:
        print(REPOSITORY)
    elif args[:2] == ["pr", "list"]:
        print(json.dumps([pr]))
    elif args[:2] == ["pr", "view"]:
        print(json.dumps(pr))
    elif args[:2] == ["pr", "diff"]:
        print(git("diff", data["base"], data["head"], "--", "src"))
    else:
        # There is no real-gh fallback: writes and unexpected requests fail.
        raise SystemExit("Unsupported offline screenshot gh request: " + " ".join(args))


def git_shim():
    args = sys.argv[1:]
    if args and args[0] == "fetch":
        args = [metadata()["remote"] if arg in (REPOSITORY, REPOSITORY + ".git") else arg for arg in args]
        if any(arg.startswith(("https://", "ssh://", "git@")) for arg in args):
            raise SystemExit("Screenshot fetches must stay local")
    os.execv(REAL_GIT, [REAL_GIT, *args])


def enable_codex(session):
    def keys(*args):
        subprocess.run(["tmux", "send-keys", "-t", session, *args], check=True)

    keys("A")
    time.sleep(0.5)
    pane = subprocess.check_output(["tmux", "capture-pane", "-p", "-t", session], text=True)
    if "Manage Agent Harnesses" not in pane:
        raise SystemExit("Expected harness setup before creating screenshot feature")
    keys("j", "j")
    if "[ ] Codex" in pane:
        keys("Enter")
        time.sleep(0.5)
    pane = subprocess.check_output(["tmux", "capture-pane", "-p", "-t", session], text=True)
    if "[x] Codex" not in pane:
        raise SystemExit("The deterministic Codex fixture was not enabled")
    keys("c")
    time.sleep(0.5)


def wait_for(session, needle):
    """Wait for the actual UI state before sending its next action."""
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        pane = subprocess.check_output(["tmux", "capture-pane", "-p", "-t", session], text=True)
        if needle in pane:
            return
        time.sleep(0.1)
    raise SystemExit("Timed out waiting for screenshot UI state: " + needle)


def seed(root, amf, session):
    root = Path(root)
    repo = root / "repository"
    repo.mkdir()
    (repo / "src").mkdir()
    git("init", "-q", "-b", "main", cwd=repo)
    git("config", "user.name", "Screenshot fixture", cwd=repo)
    git("config", "user.email", "screenshot@example.invalid", cwd=repo)
    (repo / ".gitignore").write_text(
        ".agents/\n.claude/\n.codex/\n.amf/\namf.json\nAGENTS.md\nCLAUDE.md\nCLAUDE.local.md\nAMF_PLAN.md\n")
    (repo / "src/helpers.rs").write_text(
        "pub fn normalized_name(name: &str) -> String {\n    name.trim().to_ascii_lowercase()\n}\n")
    (repo / "src/caller.rs").write_text(
        "pub fn greeting(name: &str) -> String {\n    format!(\"Hello, {}\", name)\n}\n")
    git("add", ".", cwd=repo)
    git("commit", "-qm", "Add caller and reusable helper", cwd=repo)
    base = git("rev-parse", "HEAD", cwd=repo)
    git("checkout", "-qb", "review-helper", cwd=repo)
    (repo / "src/caller.rs").write_text(
        "pub fn greeting(name: &str) -> String {\n    let name = name.trim().to_ascii_lowercase();\n    format!(\"Hello, {}\", name)\n}\n")
    git("add", ".", cwd=repo)
    git("commit", "-qm", "Normalize the caller's name", cwd=repo)
    head = git("rev-parse", "HEAD", cwd=repo)
    remote = root / "remote.git"
    git("clone", "-q", "--bare", str(repo), str(remote))
    git("--git-dir", str(remote), "update-ref", "refs/pull/42/head", head)
    git("remote", "add", "origin", REPOSITORY + ".git", cwd=repo)
    (repo / ".git/amf-screenshot.json").write_text(json.dumps(dict(base=base, head=head, remote=str(remote))))
    enable_codex(session)
    for action, data in [
        ("create-project", dict(path=str(repo), project_name="review-demo", preferred_agent="codex")),
        ("create-feature", dict(project_name="review-demo", branch="review-helper", agent="codex", mode="vibe",
                                use_worktree=False, create_terminal=True)),
    ]:
        subprocess.run([amf, "automation", action], input=json.dumps(data), text=True, check=True)
    finding = dict(path="src/caller.rs", line=2, side="New", body="Could this reuse the existing normalization helper?",
                   diff_hunk=git("diff", base, head, "--", "src/caller.rs", cwd=repo))
    (root / "ai-review.json").write_text(json.dumps(dict(
        pr_number=42, head_sha=head, summary="Deterministic screenshot review: check reuse of the existing helper.",
        findings=[finding], open=True, workdir=str(repo), repository="review-demo/helper-reuse", head_ref="review-helper")))


if __name__ == "__main__":
    command = Path(sys.argv[0]).name
    if command == "codex":
        codex()
    elif command == "gh":
        gh()
    elif command == "git":
        git_shim()
    elif sys.argv[1] == "wait":
        wait_for(*sys.argv[2:])
    else:
        seed(*sys.argv[1:])
