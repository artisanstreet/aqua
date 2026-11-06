#!/usr/bin/env bash
set -euo pipefail
DIR=${1:?usage: pixel-diff.sh DIR [REFERENCE_PREFIX=gpui] [CANDIDATE_PREFIX=aqua]}
REF=${2:-gpui}
CAND=${3:-aqua}
FUZZ=${PIXEL_DIFF_FUZZ:-8%}
MAX_PCT=${PIXEL_DIFF_MAX_PCT:-}
GRID=${PIXEL_DIFF_GRID:-6x4}
shift $(( $# < 3 ? $# : 3 ))
ONLY=" $* "
OUT=$DIR/diff
mkdir -p "$OUT"
REPORT=$OUT/pixel-diff.txt
: > "$REPORT"
worst=0
over=()
for ref_png in "$DIR/$REF"-*.png; do
  [ -e "$ref_png" ] || continue
  name=${ref_png##*/"$REF"-}; name=${name%.png}
  [ "$ONLY" = "  " ] || [[ "$ONLY" == *" $name "* ]] || continue
  cand_png=$DIR/$CAND-$name.png
  [ -s "$cand_png" ] || { echo "$name: missing $CAND shot" | tee -a "$REPORT"; over+=("$name:missing"); continue; }
  size_ref=$(identify -format '%wx%h' "$ref_png"); size_cand=$(identify -format '%wx%h' "$cand_png")
  [ "$size_ref" = "$size_cand" ] || { echo "$name: size mismatch $REF $size_ref $CAND $size_cand" | tee -a "$REPORT"; over+=("$name:size"); continue; }
  total=$((${size_ref%x*} * ${size_ref#*x}))
  changed=$(compare -metric AE -fuzz "$FUZZ" "$ref_png" "$cand_png" "$OUT/$name-highlight.png" 2>&1 || true)
  changed=${changed%% *}
  mask=$OUT/$name-mask.png
  convert "$ref_png" "$cand_png" -compose difference -composite -colorspace Gray -threshold "$FUZZ" "$mask"
  bbox=$(convert "$mask" -format '%@' info:)
  tiles=$(convert "$mask" -crop "${GRID}@" +repage -format '%[fx:mean]\n' info: | awk -v cols="${GRID%x*}" '{ printf "%d,%d %.4f\n", int((NR-1)/cols), (NR-1)%cols, $1 }' | sort -k2 -nr | head -3 | awk '{ printf "%s=%.1f%% ", $1, $2*100 }')
  convert "$ref_png" "$cand_png" +append "$OUT/$name-sbs.png"
  pct=$(awk -v c="$changed" -v t="$total" 'BEGIN { printf "%.3f", c * 100 / t }')
  printf '%-14s changed=%8s px (%6s%%) bbox=%s worst_tiles(row,col)=%s\n' "$name" "$changed" "$pct" "$bbox" "$tiles" | tee -a "$REPORT"
  awk -v p="$pct" -v w="$worst" 'BEGIN { exit !(p > w) }' && worst=$pct
  if [ -n "$MAX_PCT" ] && awk -v p="$pct" -v m="$MAX_PCT" 'BEGIN { exit !(p > m) }'; then over+=("$name:$pct%"); fi
done
echo "worst_changed_pct=$worst" | tee -a "$REPORT"
if [ ${#over[@]} -gt 0 ]; then echo "PIXEL DIFF OVER LIMIT: ${over[*]}" | tee -a "$REPORT"; exit 1; fi
