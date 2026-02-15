Check the status of the pull request for the current branch.

Run `scripts/amf/pr-checks.sh` to gather PR info, CI check
statuses, and review statuses.

Present the results in a readable format:

- PR number, title, state, and URL
- CI check results (passing/failing/pending)
- Review statuses
- Mergeable status

If any checks are failing, suggest what actions to take
(e.g., look at logs, fix lint errors).

If no PR exists, inform the user and suggest using
`/amf:pr-create`.
