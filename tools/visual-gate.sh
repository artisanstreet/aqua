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
SCENES=${VISUAL_GATE_SCENES:-"large visual"}
STREAM=bench-stream
VISUAL=bench-visual
STEPS_FILE=${VISUAL_GATE_STEPS:-$HERE/visual-gate.steps}
VISUAL_STEPS_FILE=${VISUAL_GATE_VISUAL_STEPS:-$HERE/visual-gate-visual.steps}
ALIGNED_SHOTS=${VISUAL_GATE_ALIGNED:-"v-tail v-scroll v-lightbox v-menu v-top v-account v-emoji menu jump-a jump-b bottom notch1"}
die() { echo "VISUAL GATE FAILED: $*" >&2; exit 1; }
win() { wslpath -w "$1"; }

[ -f "$AQUA_EXE" ] || die "missing $AQUA_EXE"
[ -f "$GPUI_EXE" ] || die "missing $GPUI_EXE"
[ -f "$STEPS_FILE" ] || die "missing $STEPS_FILE"
[ -f "$VISUAL_STEPS_FILE" ] || die "missing $VISUAL_STEPS_FILE"
cp "$HERE/win-drive.ps1" "$T/win-drive.ps1"

if [ -z "$SIDE_PORT" ]; then
  exec 9>/tmp/aqua-scoreboard.lock
  flock -w 7200 9 || die "scoreboard lock busy"
  exec 8>"/tmp/aqua-forge-port-$FIXTURE_PORT.lock"
  flock -w 120 8 || die "fixture port locked"
fi

if [ -n "$SIDE_PORT" ] && [ -z "${VISUAL_GATE_LOCKED:-}" ]; then
  exec 9>/tmp/aqua-scoreboard.lock
  flock -w 7200 9 || die "scoreboard lock busy"
fi
"$HERE/compare-editors.sh" fixture >/dev/null 2>&1 || die "fixture preparation failed"
HOST_ID=$(python3 -c "import json;print(json.load(open('$FIXTURE_ROOT/editor-config/hosts.json'))['last'])")
BASE_PORT=$FIXTURE_PORT
CONFIG=$FIXTURE_ROOT/editor-config-visual${SIDE_PORT:+-side}
fresh_config() {
  rm -rf "$CONFIG"; cp -r "$FIXTURE_ROOT/editor-config" "$CONFIG"
  [ -z "$SIDE_PORT" ] || sed -i "s/127.0.0.1:$BASE_PORT/127.0.0.1:$SIDE_PORT/" "$CONFIG/hosts.json"
}
[ -z "$SIDE_PORT" ] || FIXTURE_PORT=$SIDE_PORT

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

defects=()
defect() { echo "DEFECT: $*" >&2; defects+=("$*"); }
steps_of() { grep -v '^#' "$1" | paste -sd';' | sed 's/;/; /g'; }

run_scene() {
  local which=$1 exe=$2 thread=$3 steps_file=$4 attempt
  for attempt in 1 2; do
    fresh_config
    start_forge
    AQUA_STATS_OVERRIDE=${VISUAL_GATE_STATS:-off} WSLENV="${WSLENV:+$WSLENV:}AQUA_STATS_OVERRIDE" \
      AQUA_EDITOR_DIR=$CONFIG AQUA_APP_ARGS="artisan://thread/$HOST_ID/$thread" AQUA_EVENTS=$RUNDIR/events.jsonl \
      "$T/bin/win-app" "$exe" "$(steps_of "$steps_file")" 8 "$OUT" "$which" >"$OUT/$which-$thread.log" 2>&1 || die "$which run failed on $thread, see $OUT/$which-$thread.log"
    stop_forge
    grep -q "probe-open thread=$thread " "$OUT/$which-$thread.log" && return 0
    echo "$which did not show fixture thread $thread on attempt $attempt" >&2
  done
  die "$which did not show fixture thread $thread"
}

for which in ${VISUAL_GATE_TARGETS:-aqua gpui}; do
  case $which in aqua) exe=$AQUA_EXE;; gpui) exe=$GPUI_EXE;; esac
  rm -f "$OUT/$which"-*.png "$OUT/$which"*.log
  case " $SCENES " in *" large "*) run_scene "$which" "$exe" "$LARGE" "$STEPS_FILE";; esac
  case " $SCENES " in *" visual "*) run_scene "$which" "$exe" "$VISUAL" "$VISUAL_STEPS_FILE";; esac
  sha() { sha256sum "$OUT/$which-$1.png" | cut -c1-64; }
  [ -z "${VISUAL_GATE_NOCHECK:-}" ] || continue
  case " $SCENES " in *" large "*)
    for shot in bottom menu jump-a jump-b notch1 depth1 depth8 edge; do
      [ -s "$OUT/$which-$shot.png" ] || die "$which produced no $shot screenshot"
    done
    [ "$(sha bottom)" != "$(sha notch1)" ] || defect "$which: a single wheel notch left the transcript unchanged (glide snapped back to the tail)"
    [ "$(sha notch1)" != "$(sha depth1)" ] || defect "$which: paced wheel notches did not scroll the transcript"
    [ "$(sha jump-a)" != "$(sha jump-b)" ] || defect "$which: jumping to different turns left the transcript unchanged"
  ;; esac
  case " $SCENES " in *" visual "*)
    for shot in v-tail v-scroll v-lightbox v-menu v-top v-account v-emoji; do
      [ -s "$OUT/$which-$shot.png" ] || die "$which produced no $shot screenshot"
    done
    [ "$(sha v-top)" != "$(sha v-account)" ] || defect "$which: the account popover did not open"
    [ "$(sha v-tail)" != "$(sha v-scroll)" ] || defect "$which: wheel notches did not scroll the visual thread"
    [ "$(sha v-scroll)" != "$(sha v-menu)" ] || defect "$which: the turn navigator menu did not open"
    [ "$(sha v-tail)" != "$(sha v-lightbox)" ] || defect "$which: clicking the image did not open the lightbox"
    [ "$(sha v-scroll)" != "$(sha v-top)" ] || defect "$which: jumping to the first turn left the transcript unchanged"
    [ "$(sha v-scroll)" != "$(sha v-emoji)" ] || defect "$which: typed text did not reach the composer"
  ;; esac
done

echo "visual gate shots in $OUT: read the *-NAME.png pairs and compare gpui against aqua"
if [ -z "${VISUAL_GATE_TARGETS:-}" ] || [[ " $VISUAL_GATE_TARGETS " == *" aqua "* && " $VISUAL_GATE_TARGETS " == *" gpui "* ]]; then
  PIXEL_DIFF_MAX_PCT=${VISUAL_GATE_MAX_DIFF_PCT:-} "$HERE/pixel-diff.sh" "$OUT" gpui aqua $ALIGNED_SHOTS || die "aligned shots differ more than ${VISUAL_GATE_MAX_DIFF_PCT}% (see $OUT/diff/pixel-diff.txt)"
fi

if [ ${#defects[@]} -gt 0 ]; then
  printf 'DEFECT: %s\n' "${defects[@]}"
  die "${#defects[@]} visible defect(s): ${defects[0]}"
fi
