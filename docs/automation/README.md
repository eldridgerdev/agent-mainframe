# Automation

AMF now exposes machine-friendly automation entrypoints:

```bash
amf automation create-project --file docs/automation/create-project.example.json
amf automation create-session --file docs/automation/create-session.template.json
amf automation create-feature --file docs/automation/create-feature.example.json
amf automation create-batch-features --file docs/automation/create-batch-features.example.json
amf automation seed-ai-review --file docs/automation/seed-ai-review.template.json
```

The command sends a request to the running AMF dashboard over the same local IPC system used by hook notifications. AMF applies the request inside the dashboard process and prints a JSON response.

## Requirements

- A normal `amf` dashboard instance must already be running.
- For `create-project`, `path` must exist.
- For `create-feature`, `project_name` must already exist in AMF.
- For `create-batch-features`, `workspace_path` must be inside a git repository.
- For `create-project` and `create-batch-features`, `project_name` must not already exist in AMF.

## Seed AI Review (screenshot fixture only)

`seed-ai-review` is a deterministic visual-test seam for a completed AI Review
pane. It writes supplied findings to the cache and does not launch an agent,
spend tokens, or contact GitHub. It is not an agent-review API and must not be
used to fabricate a real review result.

Use [`seed-ai-review.template.json`](seed-ai-review.template.json) as the
field reference. With `open: true`, `workdir` and `repository` (`owner/name`)
are required and AMF opens the fixture pane immediately. Screenshot CI uses
this mode so a scenario can prove review presentation and the post disclosure
without any authenticated Claude/Codex harness. An optional `attribution`
object (the same shape AI Review caches: `harness`, `model`, token counts,
`memory`) adds the attribution and last-run memory lines; `null` omits them.

For a local visual check, the repository scenario supplies those fields from
the environment and opens the fixture in the isolated screenshot harness:

```bash
AMF_SCREENSHOT_PR_NUMBER=123 \
GITHUB_REPOSITORY=owner/repository \
scripts/dev/screenshot/amf-capture.sh \
  --scenario scripts/dev/screenshot/scenarios/ai-review-completed-fixture.txt \
  --out-dir /tmp/amf-ai-review-fixture
```

## Create Project

Use [`create-project.template.json`](create-project.template.json) as the contract reference.

Fields:

- `path`: repo or directory to register as the AMF project
- `project_name`: the AMF-visible project name
- `dry_run`: validate and preview without changing AMF state

Example:

```bash
cat docs/automation/create-project.example.json | amf automation create-project --dry-run
amf automation create-project --file docs/automation/create-project.example.json
```

Typical success response:

```json
{
  "type": "automation-result",
  "action": "create_project",
  "ok": true,
  "dry_run": false,
  "input_path": "/home/you/code/my-repo",
  "project_name": "my-repo",
  "project_path": "/home/you/code/my-repo",
  "is_git": true,
  "message": "Created project 'my-repo'"
}
```

## Create Session

Use [`create-session.template.json`](create-session.template.json) to add a tracked
session to an existing feature, including the single feature of a non-git project:

```bash
amf automation create-session --file docs/automation/create-session.template.json --dry-run
amf automation create-session --file docs/automation/create-session.template.json
```

- `project_name` and `feature`: exact existing AMF project and feature names.
  Alternatively `feature_session_id` (any session id in the feature) or
  `tmux_session` selects the feature, and wins over the names. With no target at
  all, the CLI uses `$AMF_FEATURE_SESSION_ID`, which AMF sets in every agent
  session, so an agent can act on its own feature with just `{"kind": "terminal"}`.
- `kind`: `claude`, `codex`, `opencode`, `pi`, `terminal`, or a custom session name
  configured in `amf.json`. Built-in names take precedence over custom names.
- `label`: optional display label; defaults to the kind name.
- `initial_prompt`: optional nonempty task for a built-in agent harness or a
  custom session declaring `agent`. Overrides a custom session's configured prompt.
- `submit_prompt`: defaults to `false`; load an editable, unsent draft into AMF's
  composer without pasting into the harness. Set `true` to wait for the harness,
  paste and send Enter. Requires an explicit `initial_prompt` in the request.
- `command`: terminal sessions only. Text pasted into the new terminal; with
  `run: true` Enter is pressed after it. Without `run` it is typed but not executed.
  Delivery failure is reported in `message` and does not fail the request.
- `reuse`: terminal sessions only. If a live terminal with the same `label` already
  exists in the feature, return it (`reused: true`) instead of creating another;
  `command` is still sent to it.
- `notes`: terminal sessions only. Markdown reference notes for the user, such as
  the commands to run and what to look for. They are stored in
  `.amf/terminal-notes.json` and a rendered `.amf/terminal-notes.md` (generated, gitignored), one entry per terminal, and
  shown in the agent sidebar as a **Terminal Notes** section that lists the
  terminals with notes. The section is absent when there are none. `leader u` opens
  the notes in the markdown viewer. With `reuse`, new notes replace that terminal's
  earlier ones; removing the terminal removes its notes. Limit 64 KiB.
- `dry_run`: validate the request and describe the session without launching it,
  running custom pre-checks, or changing AMF state.

Sessions inherit the feature's mode and permission settings. Custom sessions run
their configured pre-check before launch. Resource limits produce an error rather
than opening a confirmation dialog. The existing one non-worktree feature rule
is unchanged; multiple sessions can share that feature.

A successful real response includes `ok`, `dry_run`, `tmux_session`, `tmux_window`,
`session_id`, and `message`, with `type: "automation-result"` and
`action: "create_session"`. Dry runs return null window and session IDs because
nothing was created. Failures return `ok: false` and `error`.

### Driving terminal sessions

Four more actions work on a feature's existing sessions. Each takes the same
target fields as `create-session` (defaulting to `$AMF_FEATURE_SESSION_ID`) and
selects a session by `session_id` or `label`; an ambiguous label is an error.
Input and removal refuse agent harness sessions. An empty request means all
defaults for `list-sessions`.

```bash
amf automation list-sessions
echo '{"label":"dev server","lines":100}' | amf automation capture-session
echo '{"label":"dev server","key":"C-c"}' | amf automation send-session-input
echo '{"label":"dev server","text":"npm run dev","run":true}' | amf automation send-session-input
echo '{"label":"dev server"}' | amf automation remove-session
```

- `list-sessions` returns `sessions[]` with `session_id`, `kind`, `label`,
  `tmux_window` and `running`.
- `capture-session` returns `text`: the last `lines` lines (default 200, max
  5000) of plain text including scrollback, trailing blanks trimmed. It also works
  on agent sessions, since reading changes nothing.
- `send-session-input` pastes `text` (Enter after it when `run`) and/or sends one
  tmux key name such as `C-c`, `Enter` or `q`. Requires a running session.
- `remove-session` kills the window and deletes the session record.

### Initial prompts

`create-feature` and `create-batch-features` also accept `initial_prompt` and
`submit_prompt`. A batch gives the same task to each newly started feature's
primary agent. With `submit_prompt: false` (the default), AMF seeds its own
composer draft immediately, without waiting for or pasting into the harness.
The current view stays open; selecting the new agent restores the editable text.
With `submit_prompt: true`, prompts are queued after launch; AMF waits for the
harness input UI before using bracketed paste and sending Enter. The response
acknowledges creation, not completion of the task or submitted prompt delivery.

For submitted prompts, AMF waits up to two minutes for the input UI. Complete any
first-run login/trust screens during that interval. Timeout or paste failures are
recorded in the debug log. Pending submissions live in the running dashboard
process and are cancelled when the target session is removed or stopped.
Restarting AMF drops pending submissions and unsent composer drafts. Features
left stopped by the resource limit receive no prompt; check `started` in the
response. Terminals and custom sessions without `agent` do not accept `initial_prompt`.
Feature requests with `no_start: true` reject `initial_prompt` and
`submit_prompt: true`, since no live session will receive the prompt.

Custom agent sessions can also declare a default in `amf.json`:

```json
{
  "custom_sessions": [{
    "name": "reviewer",
    "agent": "claude",
    "command": "python3 scripts/launch_reviewer.py",
    "initial_prompt": "Review the changes and suggest improvements."
  }]
}
```

Creating this session from the TUI picker, desktop picker, or automation seeds
AMF's composer, unsent. The prompt must be nonempty and contain no NUL bytes.
It is applied once on creation; restarting an existing session does not seed it
again. Wrappers should launch the harness without pasting a second copy of the
prompt into its input.

## Create Feature

Use [`create-feature.template.json`](create-feature.template.json) as the contract reference.

Fields:

- `project_name`: an existing AMF project
- `no_start`: create the feature stopped without launching sessions (default false); also available as `--no-start`. Cannot be combined with `initial_prompt` or `submit_prompt`. The response reports `started: false`.
- `branch`: feature / branch name to create
- `agent`: `claude`, `codex`, `opencode`, or `pi`
- `mode`: `vibeless`, `vibe`, or `supervibe`
- `review`: separate toggle for diff-review / final-review flows
- `design_review`: turn on Design Review (the Decision Log) for the feature (default false). Claude only for now, and not in container projects: any other harness, or `review: true` together with it, is refused with an error and nothing is created.
- `use_worktree`: whether to create a git worktree or reuse the project repo
- `hook_choice`: optional answer for prompted `on_worktree_created` hooks
- `existing_worktree`: optional path of an existing git worktree to attach as the feature instead of creating one (the same as choosing "existing worktree" in the New Feature wizard). It must be a registered worktree of the project's repo (not the repo root) and not already used by another feature, and have a branch checked out (a detached HEAD is rejected); relative paths resolve against the repo. The `on_worktree_created` hook is not run, `branch` is the feature name (the worktree's checked-out branch is recorded when known), and `use_worktree: false` is rejected alongside it.
- `dry_run`: validate and preview without changing AMF state

To create a feature now and start its sessions later:

```bash
amf automation create-feature --file docs/automation/create-feature.example.json --no-start
```

If a repo has a prompted `on_worktree_created` hook, `--dry-run` returns a `worktree_hook_prompt` object with the hook title and valid options. An agent can use that response to pick a `hook_choice` before making the real call.

### Worktree already exists

If the planned `.worktrees/<project>-<branch>` directory already exists, the request fails with `Worktree path already exists: <path>`. If that path is a registered worktree the error includes the fix: resend the request with `"existing_worktree": "<path>"` to build the feature on that worktree, or choose a different `branch`. If it is a stale, unregistered directory, remove it or choose a different `branch`.

```bash
echo '{"project_name":"my-repo","branch":"automation-feature","existing_worktree":"/home/you/code/my-repo/.worktrees/my-repo-automation-feature"}' \
  | amf automation create-feature
```

Example:

```bash
cat docs/automation/create-feature.example.json | amf automation create-feature --dry-run
amf automation create-feature --file docs/automation/create-feature.example.json
```

Typical success response:

```json
{
  "type": "automation-result",
  "action": "create_feature",
  "ok": true,
  "dry_run": false,
  "project_name": "my-repo",
  "branch": "automation-feature",
  "workdir": "/home/you/code/my-repo/.worktrees/automation-feature",
  "is_worktree": true,
  "tmux_session": "amf-automation-feature",
  "started": true,
  "worktree_hook_ran": false,
  "worktree_hook_prompt": {
    "title": "Choose stack",
    "options": ["node", "rust"]
  },
  "message": "Created and started feature 'automation-feature'"
}
```

## Create Batch Features

Use [`create-batch-features.template.json`](create-batch-features.template.json) as the contract reference.

Key fields:

- `workspace_path`: any path inside the target repo
- `project_name`: the AMF project to create
- `feature_count`: how many parallel features to create
- `feature_prefix`: generated features will be `prefix1`, `prefix2`, ...
- `agent`: `claude`, `codex`, `opencode`, or `pi`
- `mode`: `vibeless`, `vibe`, or `supervibe`
- `review`: separate toggle for diff-review / final-review flows
- `dry_run`: validate and preview without changing AMF state

## Example

Dry run from stdin:

```bash
cat docs/automation/create-batch-features.example.json | amf automation create-batch-features --dry-run
```

Apply from a file:

```bash
amf automation create-batch-features --file docs/automation/create-batch-features.example.json
```

Typical success response:

```json
{
  "type": "automation-result",
  "action": "create_batch_features",
  "ok": true,
  "dry_run": false,
  "workspace_path": "/home/you/code/my-repo",
  "project_name": "plan-42",
  "project_repo": "/home/you/code/my-repo",
  "features": [
    {
      "name": "plan-1",
      "branch": "plan-1",
      "workdir": "/home/you/code/my-repo/.worktrees/plan-1",
      "tmux_session": "amf-plan-1",
      "started": true
    }
  ],
  "message": "Created project 'plan-42' with 4 features"
}
```

Typical error response:

```json
{
  "type": "automation-result",
  "action": "create_batch_features",
  "ok": false,
  "error": "Project 'plan-42' already exists"
}
```
