#!/usr/bin/python3
"""Read-only GitHub fixture for the native screenshot IPC acceptance check."""

import json
import os
import pathlib
import sys
import time

state = pathlib.Path(os.environ["AMF_GUI_GH_STATE"])
args = sys.argv[1:]
head = (state / "head.txt").read_text().strip()
live_artifact = (
    json.loads((state / "public-artifact.json").read_text())
    if (state / "public-artifact.json").exists()
    else None
)
if args == ["auth", "token", "--hostname", "github.com"] and (
    (state / "public-attachment.txt").exists() or live_artifact
):
    os.execv(os.environ["AMF_SCREENSHOT_AUTH_GH"], ["gh", *args])


def out(value):
    print(json.dumps(value))
    raise SystemExit(0)


if "--method" not in args and not any("mutation" in a for a in args):
    if args[:2] == ["pr", "view"] and ("11" in args or live_artifact):
        number = 11 if "11" in args else 12
        repo = (
            "eldridgerdev/agent-mainframe" if live_artifact else "demo-org/invoice-api"
        )
        out(
            dict(
                number=number,
                headRefOid=head,
                url=f"https://github.com/{repo}/pull/{number}",
                state="OPEN",
                headRefName="euro-format" if number == 11 else "round-invoice-totals",
            )
        )
    if args[:3] == ["api", "--paginate", "--slurp"] and "/11/" in args[3]:
        out([[]])
    if args[:1] == ["api"] and len(args) > 1 and args[1].startswith("repos/"):
        endpoint = args[1]
        with (state / "calls.jsonl").open("a") as log:
            log.write(json.dumps(args) + "\n")
        if endpoint.endswith(("/pulls/12", "/pulls/11")):
            time.sleep(float((state / "delay.txt").read_text()))
            body = "![Invoice screenshot](./ready.png)"
            if (state / "public-attachment.txt").exists():
                body += (
                    "\n![Public attachment]("
                    + (state / "public-attachment.txt").read_text().strip()
                    + ")"
                )
            out(
                dict(
                    body=body,
                    head=dict(
                        sha=head,
                        repo=dict(owner=dict(login="demo-org"), name="invoice-api"),
                    ),
                )
            )
        if "/contents/" in endpoint:
            sys.stdout.buffer.write((state / "ready.png").read_bytes())
            raise SystemExit(0)
        if "/actions/runs?" in endpoint:
            run = dict(
                id=70,
                run_attempt=2,
                name="Older visual run",
                head_sha=head,
                status="completed",
                conclusion="success",
            )
            if live_artifact:
                run.update(
                    id=live_artifact["workflow_run"]["id"],
                    head_sha=live_artifact["workflow_run"]["head_sha"],
                    pull_requests=[dict(number=12)],
                )
            out(dict(workflow_runs=[] if "head_sha=" in endpoint else [run]))
        if "/artifacts?" in endpoint:
            out(dict(artifacts=[live_artifact] if live_artifact else []))
        if "comments?" in endpoint or "reviews?" in endpoint:
            out([])
os.execv(
    "/usr/bin/python3", ["python3", os.environ["AMF_SCREENSHOT_GH_FALLBACK"], *args]
)
