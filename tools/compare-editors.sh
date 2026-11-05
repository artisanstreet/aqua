#!/usr/bin/env bash
set -euo pipefail
trap 'echo "COMPARE ABORTED: line $LINENO exit $? command: $BASH_COMMAND" >&2' ERR
trap 'echo "COMPARE TERMINATED by signal" >&2; exit 143' TERM HUP INT
HERE=$(cd "$(dirname "$0")" && pwd)
T=/home/sander/.local/share/aqua-port-tools
W=/home/sander/projects/artisanstreet
OUT=${AQUA_COMPARE_OUT:-$T/shots/compare}
FIXTURE_ROOT=${AQUA_FIXTURE_ROOT:-/mnt/c/Users/sander/AppData/Local/aqua-bench}
FIXTURE_PORT=${AQUA_FIXTURE_PORT:-47950}
FIXTURE_TURNS=${AQUA_FIXTURE_TURNS:-150}
USER_EDITOR_DATA="/mnt/c/Users/sander/AppData/Local/Artisan Street/Editor/data"
HOST_ID=
LARGE=bench-large
STREAM=bench-stream
BOOT=${AQUA_COMPARE_BOOT:-8}
IDLE_SECONDS=${AQUA_COMPARE_IDLE_SECONDS:-30}
IDLE_THREAD_SECONDS=${AQUA_COMPARE_IDLE_THREAD_SECONDS:-15}
GATE_IDLE_SECONDS=${AQUA_COMPARE_GATE_IDLE_SECONDS:-10}
DIAG_IDLE_SECONDS=${AQUA_COMPARE_DIAG_IDLE_SECONDS:-20}
DIAG_SCROLL_FRAMES=${AQUA_COMPARE_DIAG_SCROLL_FRAMES:-300}
SCROLL_FRAMES=${AQUA_COMPARE_SCROLL_FRAMES:-600}
SCROLL_GAP_MS=${AQUA_COMPARE_SCROLL_GAP_MS:-16}
SCROLL_X=${AQUA_COMPARE_SCROLL_X:-760}
SCROLL_Y=${AQUA_COMPARE_SCROLL_Y:-450}
COMPOSER_CLICK=${AQUA_COMPARE_COMPOSER_CLICK:-"760 800"}
COMPOSER_RECT=${AQUA_COMPARE_COMPOSER_RECT:-"445 772 500 40"}
SIDEBAR_THREAD=${AQUA_COMPARE_SIDEBAR_THREAD:-"136 118"}
TYPE_COUNT=${AQUA_COMPARE_TYPE_COUNT:-40}
RESIZE_COUNT=${AQUA_COMPARE_RESIZE_COUNT:-120}
RUNS=${AQUA_COMPARE_RUNS:-3}
TARGETS=${AQUA_COMPARE_TARGETS:-"aqua gpui"}
PHASES=${AQUA_COMPARE_PHASES:-"main open stream keys wakesrc spikes"}
DIAG_PHASES="wakesrc spikes memdiag"
SNAPSHOT_DIR=${AQUA_COMPARE_SNAPSHOT_DIR:-}
MAX_SYSTEM_LOAD=${AQUA_COMPARE_MAX_SYSTEM_LOAD:-25}
export AQUA_STATS_OVERRIDE=off
PROFILE=${AQUA_COMPARE_PROFILE:-release}
case $PROFILE in
  release) PROFILE_FLAG=--release ;;
  debug) PROFILE_FLAG= ;;
  *) echo "AQUA_COMPARE_PROFILE must be release or debug" >&2; exit 2 ;;
esac
AQUA_EXE=${AQUA_COMPARE_AQUA_EXE:-$W/editor-aqua-port/target/editor-aqua-windows/x86_64-pc-windows-gnu/$PROFILE/editor-aqua.exe}
GPUI_EXE=${AQUA_COMPARE_GPUI_EXE:-$W/editor/target/x86_64-pc-windows-gnu/$PROFILE/artisan-editor.exe}
BF_DIR=$HERE/bench-fixture
BF_EXE=$BF_DIR/target/x86_64-pc-windows-gnu/release/bench-fixture.exe
win() { wslpath -w "$1"; }
die() { echo "COMPARE FAILED: $*" >&2; exit 1; }
is_diag() { case " $DIAG_PHASES " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }

build_aqua() { "$T/bin/port-win-build" $PROFILE_FLAG; }
build_gpui() {
  (cd "$W/editor" && "$T/bin/_slot" nice -n 10 nix develop "$W/editor#cargo-windows" --command \
    cargo build $PROFILE_FLAG -p editor --bin artisan-editor --target x86_64-pc-windows-gnu -j6)
}
build_fixture() {
  (cd "$BF_DIR" && "$T/bin/_slot" nice -n 10 nix develop "$W/editor#cargo-windows" --command \
    cargo build --release --target x86_64-pc-windows-gnu -j6)
}

if [ "${1:-}" = build ] || [ "${AQUA_COMPARE_BUILD:-0}" = 1 ]; then
  build_fixture
  for which in $TARGETS; do
    case $which in
      aqua|port) build_aqua ;;
      gpui) build_gpui ;;
    esac
  done
  [ "${1:-}" = build ] && exit 0
fi

fixture_prepare() {
  [ -f "$BF_EXE" ] || die "missing $BF_EXE; run: $0 build"
  mkdir -p "$FIXTURE_ROOT"
  HOST_ID=$(python3 - "$USER_EDITOR_DATA" <<'PY'
import glob, json, sys
files = glob.glob(sys.argv[1] + "/accounts/*.json")
print(json.load(open(files[0]))["machine_id"] if files else "")
PY
)
  [ -n "$HOST_ID" ] || die "no onboarding record in $USER_EDITOR_DATA/accounts: the Editor's account gate needs the signed-in machine id as the fixture host id"
  local spec=$FIXTURE_ROOT/spec.json template=$FIXTURE_ROOT/template config=$FIXTURE_ROOT/editor-config
  local stamp="host=$HOST_ID turns=$FIXTURE_TURNS gen=$(sha256sum "$BF_DIR/generate.py" | cut -c1-16) bin=$(sha256sum "$BF_EXE" | cut -c1-16)"
  if [ "$(cat "$FIXTURE_ROOT/stamp" 2>/dev/null)" != "$stamp" ]; then
    echo "preparing fixture ($stamp)"
    rm -rf "$template" "$config" "$spec"
    python3 "$BF_DIR/generate.py" "$spec" "$FIXTURE_TURNS"
    mkdir -p "$template" "$config"
    "$BF_EXE" identity --out "$(win "$FIXTURE_ROOT/identity.json")"
    "$BF_EXE" serve --seed-only --data "$(win "$template")\\forge" --listen "127.0.0.1:$FIXTURE_PORT" \
      --spec "$(win "$spec")" --device "$(win "$FIXTURE_ROOT/identity.json")" \
      --ready "$(win "$template")\\ready.json" --events "$(win "$template")\\events.jsonl" --trigger "$(win "$template")\\trigger" \
      || die "seeding the fixture Forge failed"
    [ -f "$template/ready.json" ] || die "seeding produced no ready.json"
    local user=$USER_EDITOR_DATA
    cp "$FIXTURE_ROOT/identity.json" "$config/identity.json"
    [ -f "$user/account.json" ] && cp "$user/account.json" "$config/"
    [ -d "$user/accounts" ] && cp -r "$user/accounts" "$config/"
    python3 - "$template/ready.json" "$config" "$HOST_ID" "$FIXTURE_PORT" <<'PY'
import json, sys
ready = json.load(open(sys.argv[1]))
config, host, port = sys.argv[2], sys.argv[3], sys.argv[4]
hosts = {"hosts": [{"id": host, "name": "Bench Forge", "address": f"127.0.0.1:{port}",
                    "fingerprint": ready["fingerprint"], "kind": {"type": "local"}}], "last": host}
json.dump(hosts, open(f"{config}/hosts.json", "w"), indent=2)
json.dump({"frame_rate_limit": 165, "fps_overlay": False, "dark_mode": True, "theme": "Kull",
           "font_set": "spline", "outline_text": True, "lower_refresh_rate": False,
           "streaming_mode": "swift"}, open(f"{config}/settings.json", "w"), indent=2)
PY
    echo "$stamp" > "$FIXTURE_ROOT/stamp"
  fi
}

FORGE_PID=""
stop_forge() {
  if [ -n "$FORGE_PID" ]; then
    powershell.exe -NoProfile -Command "Stop-Process -Id $FORGE_PID -Force -ErrorAction SilentlyContinue" >/dev/null 2>&1 || true
    FORGE_PID=""
    sleep 1
  fi
}
trap stop_forge EXIT

start_forge() {
  local rundir=$1
  powershell.exe -NoProfile -Command "Get-Process bench-fixture -ErrorAction SilentlyContinue | Stop-Process -Force" >/dev/null 2>&1 || true
  sleep 1
  rm -rf "$rundir" || die "cannot clear $rundir: a stale fixture Forge still holds it"
  mkdir -p "$rundir"
  cp -r "$FIXTURE_ROOT/template/forge" "$rundir/forge"
  local r; r=$(win "$rundir")
  cmp -s "$BF_EXE" "$FIXTURE_ROOT/bench-fixture.exe" || cp "$BF_EXE" "$FIXTURE_ROOT/bench-fixture.exe"
  FORGE_PID=$(powershell.exe -NoProfile -Command "\$p = Start-Process -FilePath '$(win "$FIXTURE_ROOT/bench-fixture.exe")' -WindowStyle Hidden -PassThru -ArgumentList 'serve','--data','$r\\forge','--listen','127.0.0.1:$FIXTURE_PORT','--spec','$(win "$FIXTURE_ROOT/spec.json")','--device','$(win "$FIXTURE_ROOT/identity.json")','--ready','$r\\ready.json','--events','$r\\events.jsonl','--trigger','$r\\trigger'; \$p.Id" | tr -d '\r')
  local waited=0
  until [ -f "$rundir/ready.json" ]; do
    sleep 0.5; waited=$((waited + 1))
    [ $waited -lt 120 ] || die "fixture Forge did not become ready in $rundir"
  done
}

phase_args() {
  case $1 in
    main) echo "artisan://thread/$HOST_ID/$LARGE" ;;
    open) echo "artisan://new/$HOST_ID" ;;
    stream) echo "artisan://thread/$HOST_ID/$STREAM" ;;
    wakesrc|spikes|memdiag|idlegate) echo "artisan://thread/$HOST_ID/$LARGE" ;;
  esac
}

phase_steps() {
  local phase=$1 rundir=$2
  case $phase in
    main)
      echo "waitopen $LARGE 12 $SIDEBAR_THREAD 30; sleep 3; display state; load boot_load 3; cpu boot; gpumem boot; sleep 2; cpu idle0; wakes idle $IDLE_SECONDS; threads idle_threads $IDLE_THREAD_SECONDS; cpu idle1; lat idle_latency 300 10; snapshot pre; shot thread; scrolllat scroll $SCROLL_X $SCROLL_Y $SCROLL_FRAMES $SCROLL_GAP_MS 120; changed pre 0 0 1518 900; cpu scroll1; sleep 4; cpu settle; gpumem settle; click $COMPOSER_CLICK; sleep 1; typelat type $COMPOSER_RECT $TYPE_COUNT 60; resizelat resize $RESIZE_COUNT 16; sleep 3; cpu rs0; wakes after_resize $IDLE_SECONDS; cpu rs1; threads after_resize_threads $IDLE_THREAD_SECONDS; load end_load 3; shot final"
      ;;
    open)
      echo "sleep 4; cpu boot; shot landing; click $SIDEBAR_THREAD; waitopen $LARGE 60; sleep 2; shot opened; cpu opened; gpumem opened"
      ;;
    wakesrc)
      echo "waitopen $LARGE 45 $SIDEBAR_THREAD 30; sleep 8; cpu wake0; sleep $DIAG_IDLE_SECONDS; cpu wake1; setsize 1100 760; sleep 4; sleep $DIAG_IDLE_SECONDS; cpu wake2"
      ;;
    idlegate)
      local poke=""
      [ "${AQUA_COMPARE_GATE_CONTROL:-0}" = 1 ] && poke="wheel $SCROLL_X $SCROLL_Y -120; "
      echo "waitopen $LARGE 45 $SIDEBAR_THREAD 30; sleep 8; ${poke}wakelines idle_gate $GATE_IDLE_SECONDS"
      ;;
    memdiag)
      echo "waitopen $LARGE 45 $SIDEBAR_THREAD 30; sleep 4; gpumem boot; scrolllat mem_scroll $SCROLL_X $SCROLL_Y $SCROLL_FRAMES $SCROLL_GAP_MS 120; sleep 5; gpumem settle"
      ;;
    spikes)
      echo "waitopen $LARGE 45 $SIDEBAR_THREAD 30; sleep 6; scrolllat spike_scroll $SCROLL_X $SCROLL_Y $DIAG_SCROLL_FRAMES $SCROLL_GAP_MS 120; sleep 2; scrolllat spike_scroll2 $SCROLL_X $SCROLL_Y $DIAG_SCROLL_FRAMES $SCROLL_GAP_MS 120"
      ;;
    stream)
      echo "waitopen $STREAM 45; sleep 3; cpu stream0; trigger $(win "$rundir")\\trigger; lat stream_latency 300 30; cpu stream1; wakes stream 10; cpu stream2; sleep 8; shot streamed; gpumem stream"
      ;;
  esac
}

drive() {
  local which=$1 exe=$2 phase=$3 rundir=$4 prefix=$5
  local steps; steps=$(phase_steps "$phase" "$rundir")
  local stats=off profile=0
  [ "$phase" = wakesrc ] || [ "$phase" = idlegate ] && stats=wakes
  [ "$phase" = memdiag ] && stats=memory
  [ "$phase" = spikes ] && { stats=profile; profile=1; }
  AQUA_STATS_OVERRIDE=$stats AQUA_PROFILE=$profile AQUA_PROFILE_SPIKE_MS=${AQUA_PROFILE_SPIKE_MS:-4} \
    WSLENV="${WSLENV:+$WSLENV:}AQUA_STATS_OVERRIDE:AQUA_PROFILE:AQUA_PROFILE_SPIKE_MS:AQUA_REBUILD_STATS" \
    AQUA_EDITOR_DIR=$FIXTURE_ROOT/editor-config AQUA_APP_ARGS=$(phase_args "$phase") AQUA_EVENTS=$rundir/events.jsonl \
    "$T/bin/win-app" "$exe" "$steps" "$BOOT" "$OUT" "$prefix"
}

drive_keys() {
  local which=$1 exe=$2 prefix=$3
  KEY_LATENCY_PROFILE=$PROFILE KEY_LATENCY_OUT=$OUT KEY_LATENCY_PREFIX=$prefix KEY_LATENCY_EXE=$exe \
    KEY_LATENCY_LOCKED=1 "$HERE/key-latency.sh" "$which" keys
}

verify_phase() {
  local log=$1 phase=$2 which=$3 prefix=$4
  local thread=$LARGE
  [ "$phase" = stream ] && thread=$STREAM
  grep -q "probe-exited" "$log" && die "$which exited early during $phase (see $log)"
  grep -q "probe-open thread=$thread " "$log" || die "$which is not showing fixture thread $thread in phase $phase: Forge never saw its first-paint read acknowledgement (see $log)"
  case $phase in
    keys)
      local line; line=$(grep "^key-latency target=" "$log" | tail -1)
      [ -n "$line" ] || die "$which keys run produced no ETW key-latency line (see $log)"
      local matched posts; matched=$(sed -n 's/.* matched=\([0-9]*\).*/\1/p' <<<"$line"); posts=$(sed -n 's/.* posts=\([0-9]*\).*/\1/p' <<<"$line")
      [ "${matched:-0}" -ge $(( ${posts:-1} * 8 / 10 )) ] || die "$which keys run matched only ${matched:-0} of ${posts:-0} keystrokes to presents (see $log)"
      ;;
    wakesrc|spikes|memdiag) ;;
    idlegate)
      local lines; lines=$(sed -n 's/.*probe-wakelines name=idle_gate .* lines=\([0-9]*\).*/\1/p' "$log" | tail -1)
      [ -n "$lines" ] || die "$which idle gate produced no probe-wakelines result (see $log)"
      if [ "${AQUA_COMPARE_GATE_CONTROL:-0}" = 1 ]; then
        [ "$lines" -gt 0 ] || die "IDLE GATE CONTROL FAILED: a scroll right before the window produced no aqua-wakes lines, so the gate cannot see wakes (see $log)"
        echo "IDLE GATE CONTROL PASSED: $lines aqua-wakes lines after a deliberate scroll"
        return 0
      fi
      [ "$lines" -eq 0 ] || die "IDLE GATE FAILED: $which logged $lines aqua-wakes lines in $GATE_IDLE_SECONDS idle seconds on the open $LARGE thread: $(grep 'probe-wakeline name=idle_gate' "$log" | head -3) (see $log)"
      echo "IDLE GATE PASSED: 0 aqua-wakes lines in $GATE_IDLE_SECONDS idle seconds on the open $LARGE thread"
      ;;
    main)
      grep -q "probe-changed name=pre .*" "$log" || die "$which produced no post-scroll presented state (see $log)"
      local px; px=$(sed -n 's/.*probe-changed name=pre pixels=\([-0-9]*\).*/\1/p' "$log" | tail -1)
      [ "${px:-0}" -gt 2000 ] || die "$which scroll changed only ${px:-0} pixels: nothing was presented or the thread did not scroll (see $log)"
      ;;
    stream)
      grep -q '"stream_end"' "$OUT/$prefix-events.jsonl" 2>/dev/null || die "$which: the streaming scenario did not finish, so the live-streaming numbers are void (see $OUT/$prefix-events.jsonl)"
      ;;
  esac
}

if [ "${1:-}" = fixture ]; then fixture_prepare; echo "fixture ready in $FIXTURE_ROOT"; exit 0; fi

for which in $TARGETS; do
  case $which in
    aqua|port) exe=$AQUA_EXE; hint="$0 build" ;;
    gpui) exe=$GPUI_EXE; hint="$0 build (AQUA_COMPARE_TARGETS=gpui)" ;;
    *) echo "unknown target $which" >&2; exit 2 ;;
  esac
  [ -f "$exe" ] || die "missing $PROFILE executable $exe; run: $hint"
done

exec 9>/tmp/aqua-scoreboard.lock
echo "waiting for /tmp/aqua-scoreboard.lock (one scoreboard at a time)" >&2
flock -w 7200 9 || die "another scoreboard run held /tmp/aqua-scoreboard.lock for two hours"

fixture_prepare
mkdir -p "$OUT"
rm -f "$OUT"/verdict.txt "$OUT"/scoreboard.txt "$OUT"/manifest.txt

SNAP=${SNAPSHOT_DIR:-$FIXTURE_ROOT/exe-snapshot}
mkdir -p "$SNAP"
for which in $TARGETS; do
  case $which in
    gpui) cp --reflink=auto "$GPUI_EXE" "$SNAP/gpui.exe" ;;
    aqua|port) cp --reflink=auto "$AQUA_EXE" "$SNAP/aqua.exe" ;;
  esac
done
GPUI_EXE_RUN=$SNAP/gpui.exe
AQUA_EXE_RUN=$SNAP/aqua.exe
{
  echo "date $(date -Is)"
  echo "profile $PROFILE runs $RUNS idle_seconds $IDLE_SECONDS scroll_frames $SCROLL_FRAMES"
  echo "aqua_git $(git -C "$W/aqua" rev-parse --short HEAD) port_git $(git -C "$W/editor-aqua-port" rev-parse --short HEAD 2>/dev/null || echo none) dirty_aqua=$(git -C "$W/aqua" status --porcelain | wc -l)"
  for f in "$AQUA_EXE:aqua" "$GPUI_EXE:gpui"; do
    src=${f%%:*}; name=${f##*:}
    [ -f "$src" ] && echo "$name exe $src built $(date -r "$src" -Is) sha256 $(sha256sum "$src" | cut -c1-16) size $(stat -c %s "$src")"
  done
} > "$OUT/manifest.txt"
cat "$OUT/manifest.txt"

QUIET_WAIT=${AQUA_COMPARE_QUIET_WAIT:-300}
wait_quiet() {
  local waited=0 load
  while true; do
    load=$(powershell.exe -NoProfile -Command "[math]::Round((Get-Counter '\\Processor(_Total)\\% Processor Time' -SampleInterval 1 -MaxSamples 3 | ForEach-Object { \$_.CounterSamples[0].CookedValue } | Measure-Object -Average).Average, 1)" </dev/null 2>/dev/null | tr -d '\r' || true)
    awk -v l="${load:-100}" -v m="$MAX_SYSTEM_LOAD" 'BEGIN { exit !(l <= m) }' && return 0
    [ "$waited" -lt "$QUIET_WAIT" ] || { echo "WARNING: machine still at ${load}% CPU after ${QUIET_WAIT}s; measuring anyway, the verdict will flag it" >&2; return 0; }
    sleep 10; waited=$((waited + 13))
  done
}

run_phase() {
  local run=$1 which=$2 phase=$3 exe=$4
  wait_quiet
  echo "=== run $run $which $phase ==="
  local rundir=$FIXTURE_ROOT/run
  local prefix=$which-$phase-$run
  local attempt
  for attempt in 1 2; do
    if [ "$phase" = keys ]; then
      drive_keys "$which" "$exe" "$prefix" 2>&1 | tee "$OUT/$prefix.log" || true
      grep -q "^foreground_changed=" "$OUT/$prefix.log" && break
    else
      start_forge "$rundir"
      drive "$which" "$exe" "$phase" "$rundir" "$prefix" 2>&1 | tee "$OUT/$prefix.log" || true
      cp "$rundir/events.jsonl" "$OUT/$prefix-events.jsonl" 2>/dev/null || true
      stop_forge
      if [ "$phase" = stream ] && ! grep -q '"stream_end"' "$OUT/$prefix-events.jsonl" 2>/dev/null; then
        echo "WARNING: $which stream run $run: the Forge never finished the streaming scenario (attempt $attempt of 2)" >&2
        [ "$attempt" -lt 2 ] || break
        continue
      fi
      grep -q "^foreground_changed=" "$OUT/$prefix.log" && break
    fi
    echo "WARNING: $which $phase run $run did not finish (host interop failure?); attempt $attempt of 2" >&2
    [ "$attempt" -lt 2 ] || die "$which $phase run $run never completed twice (see $OUT/$prefix.log)"
  done
  if [ "${DIAG_SOFT:-0}" = 1 ]; then
    ( verify_phase "$OUT/$prefix.log" "$phase" "$which" "$prefix" ) || { echo "WARNING: diagnostic phase $phase did not run cleanly; its report section is void" >&2; rm -f "$OUT/$prefix-stderr.log"; }
  else
    verify_phase "$OUT/$prefix.log" "$phase" "$which" "$prefix"
  fi
}

for run in $(seq 1 "$RUNS"); do
  for phase in $PHASES; do
    is_diag "$phase" && continue
    order=$TARGETS
    [ $((run % 2)) = 0 ] && order=$(echo $TARGETS | tr ' ' '\n' | tac | tr '\n' ' ')
    for which in $order; do
      case $which in
        gpui) run_phase "$run" gpui "$phase" "$GPUI_EXE_RUN" ;;
        aqua|port) run_phase "$run" aqua "$phase" "$AQUA_EXE_RUN" ;;
      esac
    done
  done
done

for phase in $PHASES; do
  is_diag "$phase" || continue
  case " $TARGETS " in
    *" aqua "*|*" port "*) DIAG_SOFT=1 run_phase 1 aqua "$phase" "$AQUA_EXE_RUN" ;;
  esac
done

[ "$PHASES" = idlegate ] && exit 0

status=0
AQUA_COMPARE_MAX_SYSTEM_LOAD=$MAX_SYSTEM_LOAD AQUA_COMPARE_PROFILE=$PROFILE python3 "$HERE/compare-editors-report.py" "$OUT" "$RUNS" "$SCROLL_FRAMES" || status=$?
exit $status
