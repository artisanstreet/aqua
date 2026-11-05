#!/usr/bin/env bash
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
export AQUA_COMPARE_PHASES=idlegate
export AQUA_COMPARE_TARGETS=aqua
export AQUA_COMPARE_RUNS=${AQUA_COMPARE_RUNS:-1}
export AQUA_COMPARE_OUT=${AQUA_COMPARE_OUT:-/home/sander/.local/share/aqua-port-tools/shots/idle-gate}
exec "$HERE/compare-editors.sh" "$@"
