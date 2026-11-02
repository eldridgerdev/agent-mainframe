#!/usr/bin/python3
"""Offline `gh` for the native GitHub issue browser screenshot scenario.

Serves canned open issues for acme/billing-api from a state directory and
logs every call. Any GitHub write (a non-GET method, a field upload or a
comment POST) is logged separately and refused, so the capture can prove it
never wrote. `mode.txt` selects ok, offline, unauth or ratelimit so the frame
script can exercise each failure; `issues.json` can be edited between steps to
make an issue change after it was previewed.
"""
import json
import os
import pathlib
import re
import sys

state = pathlib.Path(os.environ["AMF_GUI_GH_STATE"])
args = sys.argv[1:]
with (state / "calls.jsonl").open("a") as calls:
    calls.write(json.dumps(args) + "\n")
mode = (state / "mode.txt").read_text().strip()


def refuse_write():
    with (state / "writes.jsonl").open("a") as writes:
        writes.write(json.dumps(args) + "\n")
    print("offline fixture: GitHub writes are disabled in this capture", file=sys.stderr)
    raise SystemExit(1)


if args[:1] == ["--version"]:
    print("gh version 2.0.0 (offline fixture)")
    raise SystemExit(0)
if args[:2] == ["auth", "status"]:
    if mode == "unauth":
        print("You are not logged into any GitHub hosts.", file=sys.stderr)
        raise SystemExit(1)
    raise SystemExit(0)
if args[:1] != ["api"]:
    refuse_write()
if args[1:2] == ["graphql"]:
    # The dashboard's own read-only PR sweep. A mutation is a write and is refused.
    if "mutation" in " ".join(args).lower():
        refuse_write()
    if mode in ("offline", "unauth", "ratelimit"):
        raise SystemExit(1)
    print(json.dumps({"data": {"repository": {"pullRequests": {"pageInfo": {"hasNextPage": False, "endCursor": None}, "nodes": []}}}}))
    raise SystemExit(0)
write_flags = {"-X", "--method", "-f", "-F", "--field", "--raw-field", "--input"}
if any(arg in write_flags for arg in args):
    refuse_write()
endpoint = next(arg for arg in args[1:] if not arg.startswith("-") and "/" in arg)
if mode == "offline":
    print("error connecting to api.github.com: dial tcp: lookup api.github.com: could not resolve host", file=sys.stderr)
    raise SystemExit(1)
if mode == "ratelimit":
    print("gh: API rate limit exceeded for user ID 1. (HTTP 403)", file=sys.stderr)
    raise SystemExit(1)
issues = json.loads((state / "issues.json").read_text())
single = re.fullmatch(r"repos/acme/billing-api/issues/(\d+)", endpoint)
if single:
    found = [i for i in issues if i["number"] == int(single.group(1))]
    if not found:
        print("gh: Not Found (HTTP 404)", file=sys.stderr)
        raise SystemExit(1)
    print(json.dumps(found[0]))
    raise SystemExit(0)
listing = re.fullmatch(r"repos/acme/billing-api/issues\?(.*)", endpoint)
if listing:
    query = dict(part.split("=") for part in listing.group(1).split("&"))
    page, per_page = int(query["page"]), int(query["per_page"])
    print(json.dumps(issues[(page - 1) * per_page: page * per_page]))
    raise SystemExit(0)
print("gh: Not Found (HTTP 404)", file=sys.stderr)
raise SystemExit(1)
