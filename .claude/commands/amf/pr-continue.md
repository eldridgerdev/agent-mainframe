Continue work on the current branch's pull request by
addressing review feedback.

First, run `scripts/dev/amf/pr-checks.sh` to get the current
PR status.

If no PR exists, inform the user and stop.

Then gather review comments with:

```bash
gh pr view --json reviews,comments \
    --template '{{range .reviews}}=== Review by {{.author.login}} ({{.state}}) ===
{{.body}}
{{end}}{{range .comments}}=== Comment by {{.author.login}} ===
{{.body}}
{{end}}'
```

Also get inline review comments:

```bash
gh api repos/{owner}/{repo}/pulls/{number}/comments \
    --jq '.[] | "--- \(.path):\(.line) by \(.user.login) ---\n\(.body)\n"'
```

Present a summary of all feedback, then address each piece
of feedback by making the requested code changes.

After making changes, run `scripts/dev/amf/pr-info.sh` to
confirm what was changed, then push the updates with
`git push`.

Do not post GitHub replies confirming a fix (e.g. "Done in
`<sha>`") on your own initiative. If asked to reply to review
threads, only reference a commit after it has actually been
pushed, and only for threads whose file/line the pushed commit
actually touches — check with `git show <sha> -- <path>` (or
equivalent) rather than assuming `HEAD` addressed every open
comment. A comment about a file you didn't touch, or a fix
that hasn't been pushed yet, should not get a "done" reply. When
unsure, say so or leave it unreplied rather than posting an
inaccurate confirmation. Prefer AMF's own PR Triage pane (`G`)
for reply posting when it's available — it derives and caveats
the commit for you.
