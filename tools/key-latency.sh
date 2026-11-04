#!/usr/bin/env bash
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
T=/home/sander/.local/share/aqua-port-tools
W=/home/sander/projects/artisanstreet
WHICH=${1:?aqua|gpui}
MODE=${2:-keys}
SIDEBAR_THREAD=${KEY_LATENCY_SIDEBAR_THREAD:-"136 118"}
PROFILE=${KEY_LATENCY_PROFILE:-release}
OUT=${KEY_LATENCY_OUT:-$T/shots/latency}
BOOT=${KEY_LATENCY_BOOT:-8}
CLICK=${KEY_LATENCY_CLICK:-"760 800"}
KEYS=${KEY_LATENCY_KEYS:-150}
RECT=${KEY_LATENCY_RECT:-"445 772 500 40"}
PIXEL_KEYS=${KEY_LATENCY_PIXEL_KEYS:-60}
GAP_MS=${KEY_LATENCY_GAP_MS:-110}
FIXTURE_ROOT=${AQUA_FIXTURE_ROOT:-/mnt/c/Users/sander/AppData/Local/aqua-bench}
FIXTURE_PORT=${KEY_LATENCY_PORT:-47961}
LARGE=bench-large
BF_EXE=$HERE/bench-fixture/target/x86_64-pc-windows-gnu/release/bench-fixture.exe
case $WHICH in
  aqua) exe=$W/editor-aqua-port/target/editor-aqua-windows/x86_64-pc-windows-gnu/$PROFILE/editor-aqua.exe ;;
  gpui) exe=$W/editor/target/x86_64-pc-windows-gnu/$PROFILE/artisan-editor.exe ;;
  *) echo "aqua|gpui" >&2; exit 2 ;;
esac
[ -f "$exe" ] || { echo "missing $exe" >&2; exit 1; }
[ -d "$FIXTURE_ROOT/template" ] || "$HERE/compare-editors.sh" fixture
win() { wslpath -w "$1"; }
mkdir -p "$OUT"
rundir=$FIXTURE_ROOT/run-keys
HOST_ID=$(python3 -c "import json,sys;print(json.load(open(sys.argv[1]))['last'])" "$FIXTURE_ROOT/editor-config/hosts.json")
rm -rf "$rundir"; mkdir -p "$rundir"
cp -r "$FIXTURE_ROOT/template/forge" "$rundir/forge"
cp -r "$FIXTURE_ROOT/editor-config" "$rundir/editor-config"
python3 - "$rundir/editor-config/hosts.json" "$FIXTURE_PORT" <<'PY'
import json, sys
hosts = json.load(open(sys.argv[1]))
for host in hosts["hosts"]:
    host["address"] = f"127.0.0.1:{sys.argv[2]}"
json.dump(hosts, open(sys.argv[1], "w"), indent=2)
PY
r=$(win "$rundir")
FORGE_PID=$(powershell.exe -NoProfile -Command "\$p = Start-Process -FilePath '$(win "$FIXTURE_ROOT/bench-fixture.exe")' -WindowStyle Hidden -PassThru -ArgumentList 'serve','--data','$r\\forge','--listen','127.0.0.1:$FIXTURE_PORT','--spec','$(win "$FIXTURE_ROOT/spec.json")','--device','$(win "$FIXTURE_ROOT/identity.json")','--ready','$r\\ready.json','--events','$r\\events.jsonl','--trigger','$r\\trigger'; \$p.Id" | tr -d '\r')
stop_forge() { powershell.exe -NoProfile -Command "Stop-Process -Id $FORGE_PID -Force -ErrorAction SilentlyContinue" >/dev/null 2>&1 || true; }
trap stop_forge EXIT
waited=0
until [ -f "$rundir/ready.json" ]; do sleep 0.5; waited=$((waited + 1)); [ $waited -lt 120 ] || { echo "fixture Forge not ready" >&2; exit 1; }; done
ETL='C:\Users\sander\AppData\Local\Temp\aqua-key-latency.etl'
CSV='C:\Users\sander\AppData\Local\Temp\aqua-key-latency.csv'
text=$(python3 -c "import sys;w='the quick brown fox jumps over the lazy dog ';n=int(sys.argv[1]);print((w*(n//len(w)+1))[:n].replace(' ','_'))" "$KEYS")
case $MODE in
  keys) steps="waitopen $LARGE 60; sleep 3; click $CLICK; sleep 1; keypost $text $GAP_MS; sleep 1; typelat type $RECT $PIXEL_KEYS 60; sleep 1; shot after"
        app_args="artisan://thread/$HOST_ID/$LARGE" ;;
  open) steps="sleep 4; shot landing; click $SIDEBAR_THREAD; waitopen $LARGE 60; sleep 2; shot opened"
        app_args="artisan://new/$HOST_ID" ;;
  *) echo "keys|open" >&2; exit 2 ;;
esac
cat > "$OUT/etw-start.ps1" <<PS
logman stop aquakeylat -ets 2>&1 | Out-Null
Remove-Item "$ETL" -ErrorAction SilentlyContinue
logman create trace aquakeylat -p "Microsoft-Windows-DXGI" 0xFFFFFFFFFFFFFFFF 5 -ct system -o "$ETL" -ets -bs 1024 -nb 64 256 | Out-Null
PS
cat > "$OUT/etw-stop.ps1" <<PS
logman stop aquakeylat -ets | Out-Null
tracerpt "$ETL" -o "$CSV" -of CSV -y | Out-Null
PS
ps() { powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$(win "$1")" | tr -d '\r'; }
[ "$MODE" = keys ] && ps "$OUT/etw-start.ps1"
log=$OUT/$WHICH-keylat.log
AQUA_RENDER_WHEN_COVERED=1 AQUA_REBUILD_STATS=1 AQUA_INPUT_LATENCY=${AQUA_INPUT_LATENCY:-2} \
  WSLENV="${WSLENV:+$WSLENV:}AQUA_RENDER_WHEN_COVERED:AQUA_INPUT_LATENCY:AQUA_REBUILD_STATS:AQUA_PROFILE:AQUA_PROFILE_SPIKE_MS:AQUA_PROFILE_OUT:AQUA_OPEN_TIMING:AQUA_TM_MS" \
  AQUA_EDITOR_DIR=$rundir/editor-config AQUA_APP_ARGS="$app_args" AQUA_EVENTS=$rundir/events.jsonl \
  "$T/bin/win-app" "$exe" "$steps" "$BOOT" "$OUT" "$WHICH-keylat" | tee "$log"
if [ "$MODE" = open ]; then grep -h "probe-open\|open-timing" "$log" "$OUT/$WHICH-keylat-stderr.log" | grep -v "stage=rebuild" || true; exit 0; fi
ps "$OUT/etw-stop.ps1"
python3 "$HERE/key-latency-report.py" "$WHICH" "$log" "$OUT/$WHICH-keylat-keyposts.txt" /mnt/c/Users/sander/AppData/Local/Temp/aqua-key-latency.csv "$OUT/$WHICH-keylat-stderr.log"
