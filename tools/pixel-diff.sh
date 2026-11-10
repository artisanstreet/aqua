#!/usr/bin/env bash
set -euo pipefail
DIR=${1:?usage: pixel-diff.sh DIR [REFERENCE_PREFIX=gpui] [CANDIDATE_PREFIX=aqua]}
REF=${2:-gpui}
CAND=${3:-aqua}
FUZZ=${PIXEL_DIFF_FUZZ:-8%}
MAX_PCT=${PIXEL_DIFF_MAX_PCT:-}
GRID=${PIXEL_DIFF_GRID:-6x4}
REGIONS=${PIXEL_DIFF_REGIONS:-}
shift $(( $# < 3 ? $# : 3 ))
ONLY=" $* "
OUT=$DIR/diff
mkdir -p "$OUT"
REPORT=$OUT/pixel-diff.txt
: > "$REPORT"
worst=0
over=()
compared=()
rm -f "$OUT"/contact-*.png
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
  compared+=("$name")
  pct=$(awk -v c="$changed" -v t="$total" 'BEGIN { printf "%.3f", c * 100 / t }')
  printf '%-14s changed=%8s px (%6s%%) bbox=%s worst_tiles(row,col)=%s\n' "$name" "$changed" "$pct" "$bbox" "$tiles" | tee -a "$REPORT"
  awk -v p="$pct" -v w="$worst" 'BEGIN { exit !(p > w) }' && worst=$pct
  if [ -n "$MAX_PCT" ] && awk -v p="$pct" -v m="$MAX_PCT" 'BEGIN { exit !(p > m) }'; then over+=("$name:$pct%"); fi
done
for ((i = 0; i < ${#compared[@]}; i += 2)); do
  first=${compared[i]}; second=${compared[i + 1]:-$first}
  convert \( "$OUT/$first-sbs.png" -resize 45% -gravity NorthWest -fill white -undercolor '#00000099' -pointsize 22 -annotate +6+6 " $first " \) \( "$OUT/$second-sbs.png" -resize 45% -gravity NorthWest -fill white -undercolor '#00000099' -pointsize 22 -annotate +6+6 " $second " \) -append "$OUT/contact-$((i / 2 + 1)).png"
done
for region in $REGIONS; do
  IFS=':' read -r name geometry limit label foreground <<<"$region"
  ref_png=$DIR/$REF-$name.png; cand_png=$DIR/$CAND-$name.png
  [ -s "$ref_png" ] && [ -s "$cand_png" ] || { echo "region ${label:-$geometry} of $name: missing shot" | tee -a "$REPORT"; over+=("$name@${label:-region}:missing"); continue; }
  IFS=',' read -r rw rh rx ry <<<"$geometry"
  crop="${rw}x${rh}+${rx}+${ry}"
  mask=()
  total=$((rw * rh))
  if [ -n "$foreground" ]; then
    mask=(-colorspace Gray -threshold "$foreground")
    total=$(convert "$ref_png" -crop "$crop" +repage "${mask[@]}" -format '%[fx:round(mean*w*h)]' info:)
    if [ "$total" -eq 0 ]; then
      echo "region ${label:-$geometry} of $name: reference has no foreground above $foreground" | tee -a "$REPORT"
      over+=("$name@${label:-region}:empty-foreground")
      continue
    fi
  fi
  changed=$(compare -metric AE -fuzz "$FUZZ" <(convert "$ref_png" -crop "$crop" +repage "${mask[@]}" png:-) <(convert "$cand_png" -crop "$crop" +repage "${mask[@]}" png:-) null: 2>&1 || true)
  changed=${changed%% *}
  if ! [[ "$changed" =~ ^[0-9]+$ ]]; then
    echo "region ${label:-$geometry} of $name: comparison failed: $changed" | tee -a "$REPORT"
    over+=("$name@${label:-region}:comparison-error")
    continue
  fi
  pct=$(awk -v c="$changed" -v t="$total" 'BEGIN { printf "%.1f", c * 100 / t }')
  printf 'region %-12s %-14s changed=%5s px (%5s%%, limit %s%%, reference_pixels=%s foreground=%s)\n' "$name" "${label:-$geometry}" "$changed" "$pct" "$limit" "$total" "${foreground:-all}" | tee -a "$REPORT"
  awk -v p="$pct" -v m="$limit" 'BEGIN { exit !(p > m) }' && over+=("$name@${label:-region}:$pct%")
done
echo "worst_changed_pct=$worst" | tee -a "$REPORT"
if [ ${#over[@]} -gt 0 ]; then echo "PIXEL DIFF OVER LIMIT: ${over[*]}" | tee -a "$REPORT"; exit 1; fi
