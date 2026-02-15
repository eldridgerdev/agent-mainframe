Continue work on the current branch's pull request by
addressing review feedback.

First, run `scripts/amf/pr-checks.sh` to get the current
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

After making changes, run `scripts/amf/pr-info.sh` to
confirm what was changed, then push the updates with
`git push`.
