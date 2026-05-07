#!/usr/bin/env bash
set -euo pipefail

if [[ "${AMF_SKIP_INSTALL:-0}" != "1" ]]; then
    /usr/local/bin/amf-install-release
fi

export PATH="${AMF_INSTALL_ROOT:-/opt/amf}/bin:${PATH}"

exec "$@"
