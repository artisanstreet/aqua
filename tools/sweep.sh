#!/usr/bin/env bash
set -u
root=/home/sander/projects/artisanstreet/aqua
out=${AQUA_SWEEP_OUT:-$root/target/sweep}
packages=${AQUA_SWEEP_PACKAGES:-"aqua-gpu-d3d12 aqua-shell-win32 aqua-transport aqua-ime-tsf aqua-a11y-uia aqua-fonts-system aqua-runtime aqua-ui aqua-text aqua-verify aqua"}
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
pass=0; fail=0; failed=()
while read -r name f; do
  log="$out/${name//\//__}.log"
  start=$(date +%s)
  (cd "$root" && timeout "${AQUA_SWEEP_TIMEOUT:-900}" "$f" --test-threads="${AQUA_SWEEP_THREADS:-1}" --nocapture > "$log" 2>&1 < /dev/null)
  code=$?
  if [ $code -eq 0 ]; then pass=$((pass+1)); else fail=$((fail+1)); failed+=("$name:$code"); fi
  skipped=$(grep -c SKIP "$log"); echo "$name exit=$code skips=$skipped $(( $(date +%s) - start ))s"
done < "$list"
echo "SWEEP exes=$((pass+fail)) pass=$pass fail=$fail ${failed[*]:-}"
