# Container Projects

Cloning a repository you do not trust (a job-interview take-home, say) and
opening it normally means its code runs with your user's access. A
**containerized** project keeps that code off the host.

## Using it

Cloning from a URL (`N`, then the URL field):

- Terminal: press `Ctrl+O` to toggle `[x] run in container`.
- Desktop app: tick *Run in container*.

The project is then registered as containerized and stays that way. AMF needs
Docker with a reachable daemon. The first container clone builds the sandbox
image from `docker/sandbox/Dockerfile` (Node, git, `claude`, `codex`,
`tinyproxy`), which takes a few minutes.

Container mode requires an `https://` URL. The sandbox deliberately has no SSH
agent, keys or `gh` login, so private repositories are not supported.

## What runs where

- **Clone**: `git clone` runs in a throwaway container with only the new
  directory mounted. Host git never touches the untrusted checkout.
- **Sessions**: each feature gets one container `amf-sbx-<tmux session>`.
  Every pane (agent, terminal, custom session) is a `docker exec` shell in it.
- **Egress proxy**: with the allowlist enabled, a second container runs
  `tinyproxy`. The sandbox is on an `--internal` Docker network whose only
  member besides itself is the proxy, so there is no direct route out.

The sandbox container gets: the host uid (never root), `--cap-drop ALL`,
`no-new-privileges`, a read-only root filesystem, a 512 MB `/tmp`, memory and
pid limits, the project's checkout (read-write) with `.git/config`,
`.git/hooks`, every linked worktree's `.git` file and each
`.git/worktrees/<n>/{gitdir,commondir}` on top of it read-only, and a named volume `amf-sbx-home-<project>`
as `$HOME`. Only git files that exist can be mounted read-only, so a
`.git/commondir` created later from inside the sandbox is not blocked; treat
host git commands run in a containerized checkout as a residual risk. It never gets your home directory, SSH keys, host `~/.claude`, the
Docker socket, or AMF's state.

## Settings

In `~/.config/amf/config.json` under `projects.container`:

| Key | Default | Meaning |
|---|---|---|
| `runtime` | `runc` | Docker OCI runtime. `runsc` = gVisor, `kata` = Kata Containers. Must be registered with the daemon. |
| `image` | `amf-sandbox:latest` | Built automatically only for the default name. |
| `egress_allowlist_enabled` | `true` | `false` gives the sandbox unfiltered network. |
| `egress_allowlist` | agent APIs, GitHub, npm, PyPI, crates.io | `host.example.com` or `*.example.com`. |
| `memory` | `4g` | `docker run --memory`. |
| `pids_limit` | `512` | `docker run --pids-limit`. |

`runc` shares the host kernel. For a hypervisor-grade boundary use `kata`; for a
user-space kernel use `runsc`. Neither is a guarantee.

## Logging in

The agent inside the container has no credentials. Log in once per project
inside its terminal; the login lives in that project's home volume only, so one
hostile project cannot read another's. The login is reachable by code running in
that container, so prefer a scoped, revocable API key to your main account.

## Limits

- The allowlist filters by hostname. It does not stop data leaving to an
  allowlisted host.
- A prompt-injected agent in the container can still act within it.
- Host-side AMF reads the checkout (diffs, file lists). It does not execute it,
  but the host harness is refused in a containerized project unless it runs
  isolated with no tools.
- Opening the project in an editor on the host (the `code` launcher) runs your
  editor's workspace automation on the host. Open it only if you trust it.
- Agent status hooks cannot reach the host from inside the container, so
  "thinking" indicators for containerized sessions may be missing.
- Cancelling a container clone sends SIGTERM to the Docker client, which
  forwards it; a client killed with SIGKILL can leave the clone container
  running until it finishes.
