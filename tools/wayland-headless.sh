#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$(readlink -f "$0")")/.." && pwd)
compositor=${AQUA_TEST_COMPOSITOR:-sway}
if ! command -v "$compositor" >/dev/null 2>&1; then
  echo "SKIP-NO-COMPOSITOR: $compositor is not on PATH (nix-shell nix/linux.nix provides it)" >&2
  [[ "${AQUA_REQUIRE_WAYLAND:-0}" == "1" ]] && exit 1
  exit 0
fi
runtime=$(mktemp -d "${TMPDIR:-/tmp}/aqua-wayland.XXXXXX")
chmod 0700 "$runtime"
log="$runtime/compositor.log"
cleanup() {
  [[ -n "${pid:-}" ]] && kill "$pid" 2>/dev/null && wait "$pid" 2>/dev/null
  rm -rf "$runtime"
}
trap cleanup EXIT
trap 'exit 130' INT TERM
env -u WAYLAND_DISPLAY -u DISPLAY -u SWAYSOCK \
  XDG_RUNTIME_DIR="$runtime" \
  WLR_BACKENDS=headless \
  WLR_RENDERER=pixman \
  WLR_LIBINPUT_NO_DEVICES=1 \
  "$compositor" --config "$root/tests/sway.conf" >"$log" 2>&1 &
pid=$!
socket=""
for _ in $(seq 100); do
  socket=$(find "$runtime" -maxdepth 1 -name 'wayland-*' -type s -printf '%f\n' | head -n1)
  sway_socket=$(find "$runtime" -maxdepth 1 -name 'sway-ipc.*.sock' -type s | head -n1)
  [[ -n "$socket" && -n "$sway_socket" ]] && break
  kill -0 "$pid" 2>/dev/null || break
  sleep 0.05
done
if [[ -z "$socket" || -z "$sway_socket" ]]; then
  echo "wayland-headless: $compositor did not come up; its log follows" >&2
  cat "$log" >&2
  exit 1
fi
export XDG_RUNTIME_DIR="$runtime" WAYLAND_DISPLAY="$socket" SWAYSOCK="$sway_socket"
unset DISPLAY
if [[ $# -gt 0 ]]; then
  "$@"
  exit
fi
echo "export XDG_RUNTIME_DIR=$runtime WAYLAND_DISPLAY=$socket SWAYSOCK=$sway_socket"
wait "$pid"
