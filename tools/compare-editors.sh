#!/usr/bin/env bash
set -euo pipefail
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
IDLE_SECONDS=${AQUA_COMPARE_IDLE_SECONDS:-12}
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
PHASES=${AQUA_COMPARE_PHASES:-"main open stream"}
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
  rm -rf "$rundir"; mkdir -p "$rundir"
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
  esac
}

phase_steps() {
  local phase=$1 rundir=$2
  case $phase in
    main)
      echo "waitopen $LARGE 12 $SIDEBAR_THREAD 30; sleep 3; cpu boot; gpumem boot; sleep 2; cpu idle0; wakes idle $IDLE_SECONDS; cpu idle1; lat idle_latency 300 10; snapshot pre; shot thread; scrolllat scroll $SCROLL_X $SCROLL_Y $SCROLL_FRAMES $SCROLL_GAP_MS 120; changed pre 0 0 1518 900; cpu scroll1; sleep 4; cpu settle; gpumem settle; click $COMPOSER_CLICK; sleep 1; typelat type $COMPOSER_RECT $TYPE_COUNT 60; resizelat resize $RESIZE_COUNT 16; sleep 1; shot final"
      ;;
    open)
      echo "sleep 4; cpu boot; shot landing; click $SIDEBAR_THREAD; waitopen $LARGE 60; sleep 2; shot opened; cpu opened; gpumem opened"
      ;;
    stream)
      echo "waitopen $STREAM 45; sleep 3; cpu stream0; trigger $(win "$rundir")\\trigger; lat stream_latency 300 30; cpu stream1; wakes stream 10; cpu stream2; sleep 8; shot streamed; gpumem stream"
      ;;
  esac
}

drive() {
  local which=$1 exe=$2 phase=$3 rundir=$4 prefix=$5
  local steps; steps=$(phase_steps "$phase" "$rundir")
  AQUA_EDITOR_DIR=$FIXTURE_ROOT/editor-config AQUA_APP_ARGS=$(phase_args "$phase") AQUA_EVENTS=$rundir/events.jsonl \
    "$T/bin/win-app" "$exe" "$steps" "$BOOT" "$OUT" "$prefix"
}

verify_phase() {
  local log=$1 phase=$2 which=$3 prefix=$4
  local thread=$LARGE
  [ "$phase" = stream ] && thread=$STREAM
  grep -q "probe-exited" "$log" && die "$which exited early during $phase (see $log)"
  grep -q "probe-open thread=$thread " "$log" || die "$which is not showing fixture thread $thread in phase $phase: Forge never saw its first-paint read acknowledgement (see $log)"
  case $phase in
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

fixture_prepare
mkdir -p "$OUT"
rm -f "$OUT"/verdict.txt "$OUT"/scoreboard.txt

for run in $(seq 1 "$RUNS"); do
  for which in $TARGETS; do
    case $which in
      gpui) exe=$GPUI_EXE ;;
      aqua|port) exe=$AQUA_EXE; which=aqua ;;
    esac
    for phase in $PHASES; do
      echo "=== run $run $which $phase ==="
      rundir=$FIXTURE_ROOT/run
      prefix=$which-$phase-$run
      for attempt in 1 2; do
        start_forge "$rundir"
        drive "$which" "$exe" "$phase" "$rundir" "$prefix" 2>&1 | tee "$OUT/$prefix.log" || true
        cp "$rundir/events.jsonl" "$OUT/$prefix-events.jsonl" 2>/dev/null || true
        stop_forge
        grep -q "^foreground_changed=" "$OUT/$prefix.log" && break
        echo "WARNING: $which $phase run $run did not finish (host interop failure?); attempt $attempt of 2" >&2
        [ "$attempt" -lt 2 ] || die "$which $phase run $run never completed twice (see $OUT/$prefix.log)"
      done
      verify_phase "$OUT/$prefix.log" "$phase" "$which" "$prefix"
    done
  done
done

AQUA_COMPARE_PROFILE=$PROFILE python3 "$HERE/compare-editors-report.py" "$OUT" "$RUNS" "$SCROLL_FRAMES"
