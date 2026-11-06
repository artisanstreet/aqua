#!/usr/bin/env bash
set -u
root=/home/sander/projects/artisanstreet/aqua
out=${AQUA_SWEEP_OUT:-$root/target/sweep}
packages=${AQUA_SWEEP_PACKAGES:-"aqua-gpu-d3d11 aqua-gpu-d3d12 aqua-shell-win32 aqua-transport aqua-ime-tsf aqua-a11y-uia aqua-fonts-system aqua-runtime aqua-ui aqua-text aqua-verify aqua"}
mkdir -p "$out"
list=$out/exes.txt
args=()
for p in $packages; do args+=(-p "$p"); done
/home/sander/.local/share/aqua-port-tools/bin/aqua-win-build "${args[@]}" --tests --release --message-format=json 2>"$out/build.log" \
  | python3 -c '
import json,sys
for line in sys.stdin:
    try: m=json.loads(line)
    except ValueError: continue
    if m.get("reason")=="compiler-artifact" and m.get("executable") and m["profile"]["test"]:
        pkg=m["manifest_path"].rsplit("/",2)[-2]
        print(pkg+"/"+m["target"]["name"], m["executable"])
' | sort -u > "$list"
skip=${AQUA_SWEEP_SKIP:-"aqua-verify/banned aqua-verify/enforcement aqua-verify/kernels"}
for x in $skip; do grep -vxF -f <(echo "$x") "$list" > /dev/null; grep -v "^$x " "$list" > "$list.tmp"; mv "$list.tmp" "$list"; done
if [ ! -s "$list" ] || grep -q "^error" "$out/build.log"; then echo "SWEEP-BUILD-FAILED"; grep -m3 "^error" "$out/build.log"; exit 3; fi
advisory=${AQUA_SWEEP_ADVISORY:-"aqua-gpu-d3d12/"}
desktop_sensitive=${AQUA_SWEEP_DESKTOP_SENSITIVE:-"aqua/host_trim aqua/host_material aqua/host_swapchain"}
retried=()
pass=0; fail=0; advisory_fail=0; failed=(); advised=()
while read -r name f; do
  log="$out/${name//\//__}.log"
  start=$(date +%s)
  run_once() { (cd "$root" && timeout "${AQUA_SWEEP_TIMEOUT:-900}" "$f" --test-threads="${AQUA_SWEEP_THREADS:-1}" --nocapture > "$1" 2>&1 < /dev/null); }
  run_once "$log"
  code=$?
  if [ $code -ne 0 ] && [[ " $desktop_sensitive " == *" $name "* ]]; then
    cp "$log" "$log.first"
    run_once "$log"
    code=$?
    retried+=("$name:first-run-failed")
  fi
  if [ $code -eq 0 ]; then pass=$((pass+1))
  elif [[ " $advisory " == *" ${name%%/*}/ "* ]]; then advisory_fail=$((advisory_fail+1)); advised+=("$name:$code")
  else fail=$((fail+1)); failed+=("$name:$code"); fi
  skipped=$(grep -c SKIP "$log"); echo "$name exit=$code skips=$skipped $(( $(date +%s) - start ))s"
done < "$list"
visual=skipped
if [ "${AQUA_SWEEP_VISUAL:-1}" = 1 ]; then
  visual_out=$out/visual-gate
  if VISUAL_GATE_OUT=$visual_out VISUAL_GATE_PORT=${AQUA_SWEEP_VISUAL_PORT:-47951} "$root/tools/visual-gate.sh" > "$out/visual-gate.log" 2>&1; then
    visual="pass shots=$visual_out pixel_diff=$visual_out/diff/pixel-diff.txt"
  else
    visual="FAIL $(grep -m1 '^VISUAL GATE FAILED' "$out/visual-gate.log" || echo 'see visual-gate.log')"
    fail=$((fail+1)); failed+=("visual-gate:1")
  fi
fi
echo "VISUAL GATE $visual"
echo "SWEEP exes=$((pass+fail+advisory_fail)) pass=$pass fail=$fail advisory_fail=$advisory_fail ${failed[*]:-} advisory: ${advised[*]:-} retried: ${retried[*]:-}"
[ $fail -eq 0 ]
