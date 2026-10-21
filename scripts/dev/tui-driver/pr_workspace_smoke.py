#!/usr/bin/env python3
"""Exercise the experimental PR workspace in a private TUI with offline GitHub."""

import argparse
import importlib.util
import json
from pathlib import Path
import shutil
import sqlite3
import subprocess
import sys

spec = importlib.util.spec_from_file_location("tui_driver", Path(__file__).with_name("amf-tui-driver.py"))
driver = importlib.util.module_from_spec(spec)
spec.loader.exec_module(driver)


def require(condition, detail):
    if not condition:
        raise RuntimeError(driver.encode(detail))


def fixture(session):
    repo = session / "workspace-repo"
    repo.mkdir()
    real_git = shutil.which("git")

    def git(*args):
        return subprocess.check_output([real_git, "-C", str(repo), *args], text=True).strip()

    git("init", "--quiet", "--initial-branch=main")
    git("config", "user.email", "fixture@example.test")
    git("config", "user.name", "Workspace fixture")
    (repo / "recovery.rs").write_text("fn recover() { before(); }\n")
    git("add", "recovery.rs")
    git("commit", "--quiet", "-m", "base")
    base = git("rev-parse", "HEAD")
    git("checkout", "--quiet", "-b", "topic")
    (repo / "recovery.rs").write_text("fn recover() { after(); }\n")
    git("commit", "--quiet", "-am", "change")
    head = git("rev-parse", "HEAD")
    git("update-ref", "refs/pull/7/head", head)
    git("checkout", "--quiet", "main")
    git("remote", "add", "origin", "https://github.com/demo/repo.git")
    row = {"number": 7, "title": "Workspace offline fixture", "author": {"login": "alice"},
           "isDraft": False, "updatedAt": "2026-01-01T00:00:00Z", "baseRefName": "main",
           "baseRefOid": base, "headRefName": "topic", "headRefOid": head,
           "isCrossRepository": False, "headRepositoryOwner": {"login": "demo"},
           "url": "https://github.com/demo/repo/pull/7", "state": "OPEN", "body": "Fixture PR"}
    config = {"repo": str(repo), "real_git": real_git, "row": row}
    (session / "bin/workspace-fixture.json").write_text(json.dumps(config))
    # Only this private session's PATH uses these adapters. Git fetches from
    # the local fixture rather than GitHub; all unsupported gh calls fail.
    (session / "bin/git").write_text("""#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
c = json.loads(Path(__file__).with_name('workspace-fixture.json').read_text())
args = [c['repo'] if arg.rstrip('/') in ['https://github.com/demo/repo', 'https://github.com/demo/repo.git'] else arg for arg in sys.argv[1:]]
os.execv(c['real_git'], [c['real_git'], *args])
""")
    (session / "bin/git").chmod(0o755)
    (session / "bin/gh").write_text("""#!/usr/bin/env python3
import json, sys
from pathlib import Path
c = json.loads(Path(__file__).with_name('workspace-fixture.json').read_text())
a = sys.argv[1:]
with Path(__file__).with_name('workspace-gh-calls.jsonl').open('a') as log:
    log.write(json.dumps(a) + '\\n')
if a == ['--version']: print('gh version offline-fixture')
elif a[:2] == ['auth', 'status']: pass
elif a[:2] == ['repo', 'view']: print('https://github.com/demo/repo')
elif a[:2] == ['pr', 'list']: print(json.dumps([c['row']]))
elif a[:2] == ['pr', 'view']: print(json.dumps(c['row']))
elif a[:2] == ['api', 'user']: print('reviewer')
elif a[:2] == ['api', 'graphql']:
    print(json.dumps({'data': {'repository': {'pullRequest': {'reviewThreads': {'nodes': [
        {'id': 'T1', 'isResolved': False, 'comments': {'nodes': [{'databaseId': 11}]}}
    ], 'pageInfo': {'hasNextPage': False, 'endCursor': None}}}}}}))
elif a and a[0] == 'api' and any('/pulls/7/comments' in x for x in a):
    print(json.dumps([[{'id': 11, 'path': 'recovery.rs', 'line': 1, 'original_line': 1,
        'side': 'RIGHT', 'body': 'Handle lifetime fixture', 'user': {'login': 'maya', 'type': 'User'},
        'diff_hunk': '@@ -1 +1 @@\\n-fn recover() { before(); }\\n+fn recover() { after(); }'}]]))
elif a and a[0] == 'api' and any('/pulls/7/reviews' in x or '/issues/7/comments' in x for x in a): print('[[]]')
else:
    print('Unsupported offline gh request: ' + repr(a), file=sys.stderr)
    sys.exit(1)
""")
    (session / "bin/gh").chmod(0o755)
    return repo, base, git


def main():
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("--binary", type=Path, required=True)
    args = cli.parse_args()
    launch = subprocess.run([sys.executable, str(Path(__file__).with_name("amf-tui-driver.py")),
                             "start", "--binary", str(args.binary.resolve()), "--geometry", "160x40"],
                            capture_output=True, text=True)
    require(launch.returncode == 0, launch.stdout or launch.stderr)
    session = Path(json.loads(launch.stdout)["session"])

    def call(request):
        response = driver.call(session, request)
        require(response["ok"], response)
        return response["result"]

    def batch(*actions):
        return call({"op": "batch", "actions": actions, "timeout": 30})

    def keys(*names):
        return {"op": "keys", "keys": names}

    def wait(text):
        return {"op": "wait", "text": text}

    try:
        repo, base, git = fixture(session)
        call({"op": "automation", "action": "create-project", "request": {
            "path": str(repo), "project_name": "Workspace fixture", "dry_run": False}})
        batch(wait("Workspace fixture"), keys("C-Space", "G"), wait("choose a PR"),
              wait("Workspace offline fixture"), keys("Enter"), wait("PR Workspace (experimental) · #7"))
        batch(keys("2"), wait("Handle lifetime fixture"), wait("after"),
              keys("Tab", "Tab", "z"), wait("Handle lifetime fixture"), keys("Escape"),
              wait("GitHub threads"), keys("1"), wait("Your review"), keys("m"))
        batch({"op": "paste", "text": "Native workspace draft"}, keys("Tab", "3"),
              wait("No findings yet"), keys("1", "p"), wait("Submit review"), keys("Escape"))
        for geometry in ["100x30", "60x24", "160x40"]:
            batch({"op": "resize", "geometry": geometry}, keys("1", "Tab"), wait("after"))
        batch(keys("q"), wait("Workspace fixture"))
        with sqlite3.connect(session / "config/amf/amf.db") as database:
            draft = database.execute("SELECT progress FROM pr_review_drafts WHERE repo_key=? AND pr_number=7",
                                     ("github.com/demo/repo",)).fetchone()
        require(draft and "Native workspace draft" in draft[0], "Human draft was not saved")
        require(git("rev-parse", "HEAD") == base, "Workspace changed the checked-out revision")
        require(git("status", "--porcelain") == "", "Workspace changed source files")
        batch(keys("C-Space", "G"), wait("choose a PR"), wait("Workspace offline fixture"), keys("Enter"),
              wait("PR Workspace (experimental) · #7"), wait("Native workspace draft"),
              keys("q", "C-Space", "U"), wait("Review"))
        snapshot = call({"op": "snapshot", "text_limit": 2400})
        require("PR Workspace (experimental)" not in driver.encode(snapshot), "Legacy leader+U opened experimental workspace")
    except Exception as error:
        snapshot = driver.call(session, {"op": "snapshot", "text_limit": 2000})
        raise RuntimeError(driver.encode({"error": str(error), "session": str(session), "snapshot": snapshot})) from error
    finally:
        response = driver.stop_session(session)
        require(response["ok"], response)
    print(driver.encode({"ok": True, "session": str(session), "workspace": "passed",
                         "draft_resume": "passed", "responsive_layout": "passed", "legacy_entry": "passed"}))


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(driver.encode({"ok": False, "error": str(error)}))
        sys.exit(1)
