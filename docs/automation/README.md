# Automation

AMF now exposes a machine-friendly entrypoint for batch feature creation:

```bash
amf automation create-batch-features --file docs/automation/create-batch-features.example.json
```

The command sends a request to the running AMF dashboard over the same local IPC system used by hook notifications. AMF applies the request inside the dashboard process and prints a JSON response.

## Requirements

- A normal `amf` dashboard instance must already be running.
- The `workspace_path` must be inside a git repository.
- `project_name` must not already exist in AMF.

## Input

Use [`create-batch-features.template.json`](create-batch-features.template.json) as the contract reference.

Key fields:

- `workspace_path`: any path inside the target repo
- `project_name`: the AMF project to create
- `feature_count`: how many parallel features to create
- `feature_prefix`: generated features will be `prefix1`, `prefix2`, ...
- `agent`: `claude`, `codex`, or `opencode`
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
