#!/usr/bin/env bash
set -euo pipefail
T=/home/sander/.local/share/aqua-port-tools
OUT=${AQUA_COMPARE_OUT:-$T/shots/compare}
BOOT=${AQUA_COMPARE_BOOT:-8}
IDLE_SECONDS=${AQUA_COMPARE_IDLE_SECONDS:-12}
SCROLL_FRAMES=${AQUA_COMPARE_SCROLL_FRAMES:-600}
SCROLL_GAP_MS=${AQUA_COMPARE_SCROLL_GAP_MS:-16}
SCROLL_X=${AQUA_COMPARE_SCROLL_X:-760}
SCROLL_Y=${AQUA_COMPARE_SCROLL_Y:-450}
RUNS=${AQUA_COMPARE_RUNS:-3}
TARGETS=${AQUA_COMPARE_TARGETS:-"port gpui"}
mkdir -p "$OUT"

steps="cpu boot; gpumem boot; sleep 3; cpu idle0; wakes idle $IDLE_SECONDS; cpu idle1; lat idle_latency 300 10; scrolllat scroll $SCROLL_X $SCROLL_Y $SCROLL_FRAMES $SCROLL_GAP_MS 120; cpu scroll1; sleep 4; cpu settle; gpumem settle"

for run in $(seq 1 "$RUNS"); do
  for which in $TARGETS; do
    echo "=== run $run $which ==="
    "$T/bin/win-app" "$which" "$steps" "$BOOT" "$OUT" "$which-$run" | tee "$OUT/$which-$run.log"
  done
done

python3 "$(dirname "$0")/compare-editors-report.py" "$OUT" "$RUNS" "$SCROLL_FRAMES"
