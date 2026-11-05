#!/usr/bin/env bash
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
T=/home/sander/.local/share/aqua-port-tools
W=/home/sander/projects/artisanstreet
OUT=${VISUAL_GATE_OUT:-$T/shots/visual-gate}
FIXTURE_ROOT=${AQUA_FIXTURE_ROOT:-/mnt/c/Users/sander/AppData/Local/aqua-bench}
FIXTURE_PORT=${AQUA_FIXTURE_PORT:-47950}
SIDE_PORT=${VISUAL_GATE_PORT:-}
PROFILE=${VISUAL_GATE_PROFILE:-release}
AQUA_EXE=${VISUAL_GATE_AQUA_EXE:-$W/editor-aqua-port/target/editor-aqua-windows/x86_64-pc-windows-gnu/$PROFILE/editor-aqua.exe}
GPUI_EXE=${VISUAL_GATE_GPUI_EXE:-$W/editor/target/x86_64-pc-windows-gnu/$PROFILE/artisan-editor.exe}
BF_EXE=$HERE/bench-fixture/target/x86_64-pc-windows-gnu/release/bench-fixture.exe
LARGE=bench-large
STREAM=bench-stream
STEPS_FILE=${VISUAL_GATE_STEPS:-$HERE/visual-gate.steps}
die() { echo "VISUAL GATE FAILED: $*" >&2; exit 1; }
win() { wslpath -w "$1"; }

[ -f "$AQUA_EXE" ] || die "missing $AQUA_EXE"
[ -f "$GPUI_EXE" ] || die "missing $GPUI_EXE"
[ -f "$STEPS_FILE" ] || die "missing $STEPS_FILE"

if [ -z "$SIDE_PORT" ]; then
  exec 9>/tmp/aqua-scoreboard.lock
  flock -w 7200 9 || die "scoreboard lock busy"
  exec 8>"/tmp/aqua-forge-port-$FIXTURE_PORT.lock"
  flock -w 120 8 || die "fixture port locked"
fi

"$HERE/compare-editors.sh" fixture >/dev/null 2>&1 || die "fixture preparation failed"
HOST_ID=$(python3 -c "import json;print(json.load(open('$FIXTURE_ROOT/editor-config/hosts.json'))['last'])")
CONFIG=$FIXTURE_ROOT/editor-config
if [ -n "$SIDE_PORT" ]; then
  CONFIG=$FIXTURE_ROOT/editor-config-visual
  rm -rf "$CONFIG"; cp -r "$FIXTURE_ROOT/editor-config" "$CONFIG"
  sed -i "s/127.0.0.1:$FIXTURE_PORT/127.0.0.1:$SIDE_PORT/" "$CONFIG/hosts.json"
  FIXTURE_PORT=$SIDE_PORT
fi

FORGE_PID=""
stop_forge() {
  [ -n "$FORGE_PID" ] && powershell.exe -NoProfile -Command "Stop-Process -Id $FORGE_PID -Force -ErrorAction SilentlyContinue" >/dev/null 2>&1 || true
  FORGE_PID=""
}
trap stop_forge EXIT

start_forge() {
  local rundir=$FIXTURE_ROOT/run-visual${SIDE_PORT:+-side}
  if [ -n "$SIDE_PORT" ]; then powershell.exe -NoProfile -Command "Get-Process bench-fixture-visual -ErrorAction SilentlyContinue | Stop-Process -Force" >/dev/null 2>&1 || true; else powershell.exe -NoProfile -Command "Get-Process bench-fixture -ErrorAction SilentlyContinue | Stop-Process -Force" >/dev/null 2>&1 || true; fi
  sleep 1
  rm -rf "$rundir"; mkdir -p "$rundir"
  cp -r "$FIXTURE_ROOT/template/forge" "$rundir/forge"
  local r; r=$(win "$rundir")
  FORGE_EXE=$FIXTURE_ROOT/bench-fixture${SIDE_PORT:+-visual}.exe
  cp "$BF_EXE" "$FORGE_EXE"
  FORGE_PID=$(powershell.exe -NoProfile -Command "\$p = Start-Process -FilePath '$(win "$FORGE_EXE")' -WindowStyle Hidden -PassThru -ArgumentList 'serve','--data','$r\\forge','--listen','127.0.0.1:$FIXTURE_PORT','--spec','$(win "$FIXTURE_ROOT/spec.json")','--device','$(win "$FIXTURE_ROOT/identity.json")','--ready','$r\\ready.json','--events','$r\\events.jsonl','--trigger','$r\\trigger'; \$p.Id" | tr -d '\r')
  local waited=0
  until [ -f "$rundir/ready.json" ]; do
    sleep 0.5; waited=$((waited + 1)); [ $waited -lt 120 ] || die "fixture Forge not ready"
  done
  RUNDIR=$rundir
}

mkdir -p "$OUT"

STEPS=$(grep -v '^#' "$STEPS_FILE" | paste -sd';' | sed 's/;/; /g')

for which in ${VISUAL_GATE_TARGETS:-aqua gpui}; do
  case $which in aqua) exe=$AQUA_EXE;; gpui) exe=$GPUI_EXE;; esac
  rm -f "$OUT/$which"-*.png "$OUT/$which"*.log
  start_forge
  AQUA_STATS_OVERRIDE=${VISUAL_GATE_STATS:-off} WSLENV="${WSLENV:+$WSLENV:}AQUA_STATS_OVERRIDE" \
    AQUA_EDITOR_DIR=$CONFIG AQUA_APP_ARGS="artisan://thread/$HOST_ID/$LARGE" AQUA_EVENTS=$RUNDIR/events.jsonl \
    "$T/bin/win-app" "$exe" "$STEPS" 8 "$OUT" "$which" >"$OUT/$which.log" 2>&1 || die "$which run failed, see $OUT/$which.log"
  grep -q "probe-open thread=$LARGE " "$OUT/$which.log" || die "$which did not show fixture thread $LARGE"
  stop_forge
  sha() { sha256sum "$OUT/$which-$1.png" | cut -c1-64; }
  for shot in bottom notch1 depth1 depth8 dialog emoji; do
    [ -s "$OUT/$which-$shot.png" ] || die "$which produced no $shot screenshot"
  done
  [ "$(sha bottom)" != "$(sha notch1)" ] || die "$which: a single wheel notch left the transcript unchanged (glide snapped back to the tail)"
  [ "$(sha notch1)" != "$(sha depth1)" ] || die "$which: paced wheel notches did not scroll the transcript"
  [ "$(sha bottom)" != "$(sha dialog)" ] || die "$which: the account popover did not open"
done

echo "visual gate shots in $OUT: read the *-NAME.png pairs and compare gpui against aqua"
