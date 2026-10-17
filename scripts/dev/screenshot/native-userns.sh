#!/usr/bin/env bash
# Sourced only by capture scenarios that require private HOME bind mounts.
# This never relaxes a local or self-hosted machine's user namespace policy.
amf_restore_native_userns() {
  local status="$1"
  if ! sudo -n sysctl -w kernel.apparmor_restrict_unprivileged_userns=1 >&2; then
    echo 'Failed to restore hosted runner user namespace policy' >&2
    status=1
  fi
  exit "$status"
}

amf_require_native_userns() {
  # The optional path lets regression tests supply a private policy fixture.
  local policy_file="${1:-/proc/sys/kernel/apparmor_restrict_unprivileged_userns}"
  if bwrap --dev-bind / / -- /bin/true; then
    return 0
  fi
  if [[ "${GITHUB_ACTIONS:-}" != true ||
        "${RUNNER_ENVIRONMENT:-}" != github-hosted ||
        "$(uname -s)" != Linux || ! -f "$policy_file" ]] ||
      [[ "$(cat "$policy_file")" != 1 ]]; then
    echo 'Private capture requires working Bubblewrap user namespaces; no policy changed' >&2
    return 1
  fi

  # This capture owns its ephemeral hosted VM. Restore even if the setter or
  # second probe fails. GUI execution remains unprivileged and isolated.
  trap 'amf_restore_native_userns "$?"' EXIT
  sudo -n sysctl -w kernel.apparmor_restrict_unprivileged_userns=0 || return 1
  if ! bwrap --dev-bind / / -- /bin/true; then
    echo 'Private capture user namespace probe still failed' >&2
    return 1
  fi
}
