#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
OUT=$(mktemp -d)
trap 'rm -rf "$OUT"' EXIT

convert -size 30x30 xc:black -fill white -draw 'polygon 8,10 15,17 22,10 22,13 15,20 8,13' "$OUT/gpui-control.png"
convert -size 30x30 xc:'#555555' -fill white -draw 'polygon 8,10 15,17 22,10 22,13 15,20 8,13' "$OUT/aqua-control.png"

check() {
  local expected=$1 region=$2 status=0
  PIXEL_DIFF_MAX_PCT=${3:-} PIXEL_DIFF_REGIONS=$region "$ROOT/tools/pixel-diff.sh" "$OUT" gpui aqua control >"$OUT/result.log" 2>&1 || status=$?
  if [ "$status" -ne "$expected" ]; then
    cat "$OUT/result.log"
    echo "expected exit $expected, received $status" >&2
    exit 1
  fi
}

check 0 'control:30,30,0,0:30:chevron:60%'
check 1 'control:30,30,0,0:30:chevron:60%' 1
check 1 'control:30,30,0,0:30:chevron'

convert -size 30x30 xc:'#555555' "$OUT/aqua-control.png"
check 1 'control:30,30,0,0:30:chevron:60%'

convert -size 30x30 xc:'#555555' -fill white -draw 'rectangle 4,12 26,15' "$OUT/aqua-control.png"
check 1 'control:30,30,0,0:30:chevron:60%'

convert -size 30x30 xc:'#555555' -fill white -draw 'polygon 8,16 15,9 22,16 22,19 15,12 8,19' "$OUT/aqua-control.png"
check 1 'control:30,30,0,0:30:chevron:60%'

convert -size 30x30 xc:black "$OUT/gpui-control.png"
convert -size 30x30 xc:black "$OUT/aqua-control.png"
check 1 'control:30,30,0,0:30:chevron:60%'
check 0 'control:30,30,0,0:30:chevron'

echo 'pixel-diff foreground regressions passed'
