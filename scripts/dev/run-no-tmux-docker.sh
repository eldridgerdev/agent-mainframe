#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
IMAGE_NAME="${AMF_NO_TMUX_IMAGE:-amf-no-tmux}"
CONTAINER_NAME="${AMF_NO_TMUX_CONTAINER:-amf-no-tmux}"

usage() {
    cat <<EOF
Usage: $(basename "$0") [docker run args...]

Builds the no-tmux AMF image, then runs it interactively.

Pass Docker run flags before \`--\` and the container command after it.

Environment variables:
  AMF_NO_TMUX_IMAGE      Docker image name to build/run (default: $IMAGE_NAME)
  AMF_NO_TMUX_CONTAINER  Docker container name to use (default: $CONTAINER_NAME)
  AMF_RELEASE_ARCHIVE    Optional local release tarball path for the container
  AMF_RELEASE_BASE       Optional release download base URL

Examples:
  $0
  $0 --rm -it -- bash
  AMF_RELEASE_ARCHIVE=$PWD/amf.tar.gz $0
EOF
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
    usage
    exit 0
fi

docker_run_args=()
container_cmd=()
docker_env_args=()
docker_tty_args=()
parsing_run_args=1

for arg in "$@"; do
    if [[ "$arg" == "--" ]]; then
        parsing_run_args=0
        continue
    fi

    if [[ "$parsing_run_args" -eq 1 ]]; then
        docker_run_args+=("$arg")
    else
        container_cmd+=("$arg")
    fi
done

for var in AMF_RELEASE_ARCHIVE AMF_RELEASE_BASE AMF_INSTALL_ROOT AMF_SKIP_INSTALL; do
    if [[ -n "${!var:-}" ]]; then
        docker_env_args+=("-e" "$var=${!var}")
    fi
done

if [[ -n "${AMF_RELEASE_ARCHIVE:-}" && -f "${AMF_RELEASE_ARCHIVE:-}" ]]; then
    docker_run_args+=("-v" "${AMF_RELEASE_ARCHIVE}:${AMF_RELEASE_ARCHIVE}:ro")
fi

if [[ -t 0 && -t 1 ]]; then
    docker_tty_args=(-it)
fi

docker build \
    -f "$REPO_ROOT/docker/no-tmux/Dockerfile" \
    -t "$IMAGE_NAME" \
    "$REPO_ROOT"

exec docker run \
    --rm \
    --name "$CONTAINER_NAME" \
    "${docker_tty_args[@]}" \
    "${docker_env_args[@]}" \
    "${docker_run_args[@]}" \
    "$IMAGE_NAME" \
    "${container_cmd[@]}"
