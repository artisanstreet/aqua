#!/usr/bin/env bash
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
T=/home/sander/.local/share/aqua-port-tools
W=/home/sander/projects/artisanstreet
OUT=${VISUAL_GATE_OUT:-$T/shots/visual-gate}
FIXTURE_ROOT=${AQUA_FIXTURE_ROOT:-/mnt/c/Users/sander/AppData/Local/aqua-bench}
FIXTURE_PORT=${AQUA_FIXTURE_PORT:-}
SIDE_PORT=${VISUAL_GATE_PORT:-}
PORT_LOW=${VISUAL_GATE_PORT_LOW:-48716}
PORT_HIGH=${VISUAL_GATE_PORT_HIGH:-49100}
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
ALIGNED_SHOTS=${VISUAL_GATE_ALIGNED:-"bottom menu notch1 depth1 depth2 depth3 depth4 depth5 depth6 depth7 depth8 edge edge2 far1 far2 far3 far4 far5 far6 far7 far8 far9 far10 far11 far12 emoji jump-a jump-b dialog v-tail v-scroll v-lightbox v-menu v-top v-account v-emoji"}
MAX_DIFF_PCT=${VISUAL_GATE_MAX_DIFF_PCT:-18}
JUMP_BUTTON_LIMIT=${VISUAL_GATE_JUMP_BUTTON_LIMIT:-30}
REGIONS=${VISUAL_GATE_REGIONS:-"notch1:30,30,995,855:$JUMP_BUTTON_LIMIT:jump-latest:60% depth1:30,30,995,855:$JUMP_BUTTON_LIMIT:jump-latest:60% depth2:30,30,995,855:$JUMP_BUTTON_LIMIT:jump-latest:60% depth3:30,30,995,855:$JUMP_BUTTON_LIMIT:jump-latest:60% depth4:30,30,995,855:$JUMP_BUTTON_LIMIT:jump-latest:60%"}
die() { echo "VISUAL GATE FAILED: $*" >&2; exit 1; }
win() { wslpath -w "$1"; }

bindable_ports() {
  powershell.exe -NoProfile -Command "\$o=@();foreach(\$p in $1..$2){ if(\$o.Count -ge $3){break}; try{\$l=[Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback,\$p);\$l.Start();\$l.Stop();\$o+=\$p}catch{} }; \$o -join ' '" | tr -d '\r'
}
claim_port() {
  local fd
  exec {fd}>"/tmp/aqua-forge-port-$1.lock"
  flock -n "$fd" || { exec {fd}>&-; return 1; }
}
claim_ports() {
  local candidates port claimed=()
  candidates=$(bindable_ports "$((PORT_LOW + (RANDOM % 40) * 2))" "$PORT_HIGH" 30)
  [ -n "$candidates" ] || candidates=$(bindable_ports "$PORT_LOW" "$PORT_HIGH" 30)
  for port in $candidates; do
    claim_port "$port" && claimed+=("$port")
    [ ${#claimed[@]} -lt 2 ] || break
  done
  [ ${#claimed[@]} -ge 2 ] || die "no two free loopback ports in $PORT_LOW-$PORT_HIGH (46000-48715 are reserved by Windows on this machine)"
  FIXTURE_PORT=${claimed[0]}
  SIDE_PORT=${claimed[1]}
}
if [ -n "$FIXTURE_PORT" ] && [ -n "$SIDE_PORT" ] && [ "$(bindable_ports "$FIXTURE_PORT" "$FIXTURE_PORT" 1)" = "$FIXTURE_PORT" ] && [ "$(bindable_ports "$SIDE_PORT" "$SIDE_PORT" 1)" = "$SIDE_PORT" ] && claim_port "$FIXTURE_PORT" && claim_port "$SIDE_PORT"; then :; else claim_ports; fi

mkdir -p "$OUT"
[ -f "$AQUA_EXE" ] || die "missing $AQUA_EXE"
[ -f "$GPUI_EXE" ] || die "missing $GPUI_EXE"
[ -f "$STEPS_FILE" ] || die "missing $STEPS_FILE"
[ -f "$VISUAL_STEPS_FILE" ] || die "missing $VISUAL_STEPS_FILE"
[ "$HERE/win-drive.ps1" -ef "$T/win-drive.ps1" ] || cp "$HERE/win-drive.ps1" "$T/win-drive.ps1"

if [ -z "${VISUAL_GATE_LOCKED:-}" ]; then
  exec 9>/tmp/aqua-scoreboard.lock
  flock -w 7200 9 || die "scoreboard lock busy"
fi
AQUA_FIXTURE_PORT=$FIXTURE_PORT "$HERE/compare-editors.sh" fixture >"$OUT/fixture.log" 2>&1 || die "fixture preparation failed, see $OUT/fixture.log"
HOST_ID=$(python3 -c "import json;print(json.load(open('$FIXTURE_ROOT/editor-config/hosts.json'))['last'])")
CONFIG=$FIXTURE_ROOT/editor-config-visual-side
fresh_config() {
  rm -rf "$CONFIG"; cp -r "$FIXTURE_ROOT/editor-config" "$CONFIG"
  sed -i -E "s/127\.0\.0\.1:[0-9]+/127.0.0.1:$SIDE_PORT/g" "$CONFIG/hosts.json"
}
FIXTURE_PORT=$SIDE_PORT

FORGE_PID=""
stop_forge() {
  [ -n "$FORGE_PID" ] && powershell.exe -NoProfile -Command "Stop-Process -Id $FORGE_PID -Force -ErrorAction SilentlyContinue" >/dev/null 2>&1 || true
  FORGE_PID=""
}
trap stop_forge EXIT

start_forge() {
  local rundir=$FIXTURE_ROOT/run-visual-side
  powershell.exe -NoProfile -Command "Get-Process bench-fixture-visual -ErrorAction SilentlyContinue | Stop-Process -Force" >/dev/null 2>&1 || true
  sleep 1
  rm -rf "$rundir"; mkdir -p "$rundir"
  cp -r "$FIXTURE_ROOT/template/forge" "$rundir/forge"
  local r; r=$(win "$rundir")
  FORGE_EXE=$FIXTURE_ROOT/bench-fixture-visual.exe
  cp "$BF_EXE" "$FORGE_EXE"
  FORGE_PID=$(powershell.exe -NoProfile -Command "\$p = Start-Process -FilePath '$(win "$FORGE_EXE")' -WindowStyle Hidden -PassThru -ArgumentList 'serve','--data','$r\\forge','--listen','127.0.0.1:$FIXTURE_PORT','--spec','$(win "$FIXTURE_ROOT/spec.json")','--device','$(win "$FIXTURE_ROOT/identity.json")','--ready','$r\\ready.json','--events','$r\\events.jsonl','--trigger','$r\\trigger'; \$p.Id" | tr -d '\r')
  local waited=0
  until [ -f "$rundir/ready.json" ]; do
    sleep 0.5; waited=$((waited + 1)); [ $waited -lt 120 ] || die "fixture Forge not ready"
  done
  RUNDIR=$rundir
}


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
    if grep -q "probe-open thread=$thread " "$OUT/$which-$thread.log"; then
      echo "$which $thread opened on attempt $attempt" >>"$OUT/open-attempts.txt"
      [ "$attempt" = 1 ] || [ -z "${VISUAL_GATE_STRICT_OPEN:-}" ] || die "$which needed $attempt launches to show $thread (VISUAL_GATE_STRICT_OPEN)"
      return 0
    fi
    echo "$which did not show fixture thread $thread on attempt $attempt" | tee -a "$OUT/open-attempts.txt" >&2
  done
  die "$which did not show fixture thread $thread"
}

[ -n "${VISUAL_GATE_TARGETS:-}" ] || : >"$OUT/open-attempts.txt"
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
  PIXEL_DIFF_MAX_PCT=$MAX_DIFF_PCT PIXEL_DIFF_REGIONS=$REGIONS "$HERE/pixel-diff.sh" "$OUT" gpui aqua $ALIGNED_SHOTS || die "aligned shots differ more than ${MAX_DIFF_PCT}% or a checked region differs (see $OUT/diff/pixel-diff.txt)"
fi

if [ ${#defects[@]} -gt 0 ]; then
  printf 'DEFECT: %s\n' "${defects[@]}"
  die "${#defects[@]} visible defect(s): ${defects[0]}"
fi
