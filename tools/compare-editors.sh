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
TARGETS=${AQUA_COMPARE_TARGETS:-"aqua gpui"}
W=/home/sander/projects/artisanstreet
PROFILE=${AQUA_COMPARE_PROFILE:-debug}
AQUA_EXE=${AQUA_COMPARE_AQUA_EXE:-$W/editor-aqua-port/target/editor-aqua-windows/x86_64-pc-windows-gnu/$PROFILE/editor-aqua.exe}
GPUI_EXE=${AQUA_COMPARE_GPUI_EXE:-$W/editor/target/x86_64-pc-windows-gnu/$PROFILE/artisan-editor.exe}
DRIVER=/mnt/c/Users/sander/AppData/Local/aqua-port-tools/win-drive.ps1
mkdir -p "$OUT"

steps="cpu boot; gpumem boot; sleep 3; cpu idle0; wakes idle $IDLE_SECONDS; cpu idle1; lat idle_latency 300 10; scrolllat scroll $SCROLL_X $SCROLL_Y $SCROLL_FRAMES $SCROLL_GAP_MS 120; cpu scroll1; sleep 4; cpu settle; gpumem settle"

drive() {
  local which=$1 exe=$2 prefix=$3 prelude=""
  [ "$which" = gpui ] && prelude="sleep 2; click 810 418; sleep 4; "
  [ -f "$exe" ] || { echo "missing executable $exe" >&2; return 1; }
  cp "$T/win-drive.ps1" "$DRIVER"
  AQUA_STEPS="$prelude$steps" WSLENV="${WSLENV:+$WSLENV:}AQUA_STEPS" powershell.exe -NoProfile -ExecutionPolicy Bypass \
    -File "$(wslpath -w "$DRIVER")" -Exe "$(wslpath -w "$exe")" -Out "$(wslpath -w "$OUT")" -Prefix "$prefix" -Boot "$BOOT" \
    -Stats "$([ "$which" = gpui ] && echo 0 || echo 1)" | tr -d '\r'
}

for run in $(seq 1 "$RUNS"); do
  for which in $TARGETS; do
    echo "=== run $run $which ==="
    case $which in
      gpui) exe=$GPUI_EXE ;;
      aqua|port) exe=$AQUA_EXE; which=aqua ;;
      *) echo "unknown target $which" >&2; exit 2 ;;
    esac
    drive "$which" "$exe" "$which-$run" | tee "$OUT/$which-$run.log"
  done
done

python3 "$(dirname "$0")/compare-editors-report.py" "$OUT" "$RUNS" "$SCROLL_FRAMES"
