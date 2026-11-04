#!/usr/bin/env bash
set -euo pipefail
for argument in "$@"; do
  case "$argument" in
    --workspace|--all|-j*|--jobs*) echo "win-clippy: --workspace and -j are not allowed" >&2; exit 2 ;;
  esac
done
cd "$(dirname "$0")/.."
exec /home/sander/.local/share/aqua-port-tools/bin/_slot nice -n 10 nix develop "/home/sander/projects/artisanstreet/editor#cargo-windows" --command \
  cargo clippy --target x86_64-pc-windows-gnu --target-dir target/editor-windows-clippy -j6 "$@" --no-deps -- -D warnings
