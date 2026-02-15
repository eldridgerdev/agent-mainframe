Update the pull request description for the current branch
to reflect the latest changes.

First, run `scripts/amf/pr-info.sh` to gather the current
branch context (commits, diff stats, changed files).

Then, get the existing PR description with:

```bash
gh pr view --json body --template '{{.body}}'
```

Update the PR body to reflect the current state of changes:

- Regenerate the `## Summary` section from current commits
  and diff stats
- Keep the `## Test plan` section from the existing
  description if it has been manually edited
- Preserve any other manually added sections

Apply the update with:

```bash
gh pr edit --body "$(cat <<'EOF'
...updated body...
EOF
)"
```

Report what changed in the description.
