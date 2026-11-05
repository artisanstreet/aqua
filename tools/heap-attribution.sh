#!/usr/bin/env bash
set -euo pipefail
exe=${1:?usage: heap-attribution.sh EXE CHAINS_FILE [TOP]}
chains=${2:?}
top=${3:-40}
image_base=${AQUA_IMAGE_BASE:-0x140000000}
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
grep '^chain ' "$chains" | sed 's/^chain live=\([0-9]*\) allocs=\([0-9]*\) blocks=\(-\{0,1\}[0-9]*\) rel=\(.*\)$/\1 \3 \4/' > "$tmp/chains"
awk '{n=split($3,a,","); for(i=1;i<=n;i++) print a[i]}' "$tmp/chains" | sort -u > "$tmp/addrs"
while read -r rel; do printf '0x%x\n' $((image_base + 0x$rel)); done < "$tmp/addrs" > "$tmp/abs"
llvm-addr2line -f -C -e "$exe" < "$tmp/abs" | paste - - | cut -f1 > "$tmp/names"
paste -d' ' "$tmp/addrs" "$tmp/names" > "$tmp/symbols"
awk -v prefixes='aqua|editor_aqua|artisan|forge' '
FNR==NR { sym[$1]=substr($0, index($0," ")+1); next }
function infra(n) { return n ~ /^(std::|core::|alloc::|<alloc::|<std::|<core::|hashbrown::|aqua::mem_probe|<aqua::mem_probe|__rust|rust_|\?\?|_start|main$|BaseThreadInitThunk|RtlUserThreadStart|alloc_)/ }
function crate_of(n,   m) { sub(/^</, "", n); split(n, m, "::"); return m[1] }
function owner(n) { return n ~ "^<?(" prefixes ")" }
{
  live=$1; cnt=$2; k=split($3, a, ","); leaf=""; own="";
  for (i=1;i<=k;i++) { n=sym[a[i]]; if (infra(n)) continue; if (leaf=="") leaf=n; if (own=="" && owner(n)) own=n }
  if (leaf=="") leaf="unknown"; if (own=="") own="unowned";
  lc=crate_of(leaf); L[lc]+=live; LC[lc]+=cnt;
  split(own, p, "::"); ok=p[1] "::" p[2] "::" p[3]; O[ok]+=live; OC[ok]+=cnt;
  split(leaf, q, "::"); lk=q[1] "::" q[2] "::" q[3]; S[lk]+=live; SC[lk]+=cnt; T+=live
}
END {
  printf "total attributed %.2f MiB\n-- by leaf crate\n", T/1048576;
  for (k in L) printf "%10.0f KiB %9d blocks %s\n", L[k]/1024, LC[k], k | "sort -rn | head -'"$top"'"; close("sort -rn | head -'"$top"'");
  print "-- by owner (first workspace frame)";
  for (k in O) printf "%10.0f KiB %9d blocks %s\n", O[k]/1024, OC[k], k | "sort -rn | head -'"$top"'"; close("sort -rn | head -'"$top"'");
  print "-- by leaf site";
  for (k in S) printf "%10.0f KiB %9d blocks %s\n", S[k]/1024, SC[k], k | "sort -rn | head -'"$top"'";
}' "$tmp/symbols" "$tmp/chains"
