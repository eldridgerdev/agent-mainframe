Generate a comprehensive code review document.

First, run `git log --oneline -20` to see recent commits and understand the commit context.

Then read `.amf/change-history.json` to get all tracked changes.

Link changes to commits by:
1. Compare change timestamps with commit timestamps
2. Run `git log --since="<change-timestamp>" --until="<change-timestamp + 1 hour>" --oneline` for each change
3. Match file paths in commit diffs to change file paths

Group changes by:
1. File (primary grouping)
2. Change sets within each file (consecutive edits are grouped together)

For each change set:
- Summarize what the changes do together
- Explain how they interact with the larger system
- Include the user-provided reasons
- Link to the git commit hash if found
- Note any reverted changes

Output to `.amf/review.md` with this structure:

```markdown
# Code Review: [date range]

## Summary
[2-3 sentence overview of all changes]

## Commits in Range
[List commits found during this review period with their hashes and messages]

## Files Changed

### src/file1.rs
[Analysis of changes to this file]

#### Change Set 1: [brief description]
**Lines X-Y** | Commit: `hash` (if found)

**What:** [technical explanation of what the code change does]

**Why:** [user's recorded reason + any additional context]

**System Impact:** [how this affects other parts of the codebase]

[... continue for each change set ...]

### src/file2.rs
[... same structure ...]

## Reverted Changes
[List any changes that were tracked then reverted, with explanations if available]

## Open Questions
[Any areas that might need clarification or follow-up]
```

Important guidelines:
- Use line-by-line analysis for complex changes
- Reference specific line numbers where relevant
- If a change has no recorded reason, note "No reason recorded"
- For reverted changes, explain what was attempted and why it was undone
- Keep the executive summary brief but informative
- Use code blocks with syntax highlighting for code snippets
- Always try to link changes to their commits
