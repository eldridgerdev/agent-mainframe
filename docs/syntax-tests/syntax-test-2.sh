#!/usr/bin/env bash

set -euo pipefail

name="${1:-syntax}"
readonly label="fixture-${name}"
items=("alpha" "beta" "gamma")

for item in "${items[@]}"; do
  if [[ "$item" == b* ]]; then
    printf 'match: %s -> %s\n' "$item" "${label^^}"
  fi
done

case "$name" in
  syntax)
    echo "default fixture"
    ;;
  *)
    echo "custom fixture: $name"
    ;;
esac
