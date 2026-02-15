Merge the pull request for the current branch.

First, run `scripts/amf/pr-checks.sh` to check the current
PR status.

If no PR exists, inform the user and stop.

If the PR exists, evaluate whether it is safe to merge:

1. If CI checks are failing, report the failures and do NOT
   merge. Suggest fixing the issues first.

2. If there are requested changes from reviewers, report
   them and do NOT merge. Suggest addressing the feedback.

3. If the PR is not mergeable (conflicts, etc.), report the
   issue and suggest rebasing.

4. If checks pass and reviews are approved (or no reviews
   required), merge with:

   ```bash
   gh pr merge --merge --delete-branch
   ```

   Use `--merge` (merge commit) by default. Report the
   result and confirm the branch was cleaned up.
