Create a pull request for the current branch.

First, run `scripts/amf/pr-info.sh` to gather branch context
(commits, diff stats, changed files, push status).

Then follow these steps:

1. If the branch has not been pushed to origin, push it with
   `git push -u origin HEAD`.

2. Use the commit messages and diff stats to generate:
   - A concise PR title (under 70 characters)
   - A description body with a `## Summary` section
     (2-4 bullet points) and a `## Test plan` section

3. Create the PR with `gh pr create` using a heredoc for
   the body:

   ```bash
   gh pr create --title "title" --body "$(cat <<'EOF'
   ## Summary
   - ...

   ## Test plan
   - ...
   EOF
   )"
   ```

4. Report the PR URL when done.

Do NOT ask for confirmation before creating the PR - just
create it based on the gathered context.
