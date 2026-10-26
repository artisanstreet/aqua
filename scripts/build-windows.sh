#!/usr/bin/env bash
# Reuse Artisan Editor's pinned Nix cross-compilation environment from WSL.
set -euo pipefail

workspace=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
editor_checkout=${AQUA_EDITOR_CHECKOUT:-"$HOME/projects/artisanstreet/editor"}
example=${1:-todo}
profile=${2:-release}

case "$example" in
    todo|showcase|collections) ;;
    *) printf 'Usage: %s [todo|showcase|collections] [release|dev]\n' "$0" >&2; exit 2 ;;
esac
case "$profile" in
    release) profile_directory=release ;;
    dev) profile_directory=debug ;;
    *) printf 'Profile must be release or dev.\n' >&2; exit 2 ;;
esac
if [[ ! -f "$editor_checkout/nix/workspace.nix" ]]; then
    printf 'Set AQUA_EDITOR_CHECKOUT to the Artisan Street editor checkout.\n' >&2
    exit 1
fi
editor_checkout=$(cd -- "$editor_checkout" && pwd)
target_directory="$workspace/target/editor-windows"
output_directory="$workspace/dist/windows"

nix develop "$editor_checkout#cargo-windows" --command \
    cargo build --locked --manifest-path "$workspace/Cargo.toml" \
    --package aqua --example "$example" --profile "$profile" \
    --target x86_64-pc-windows-gnu --target-dir "$target_directory"

mkdir -p -- "$output_directory"
binary="$target_directory/x86_64-pc-windows-gnu/$profile_directory/examples/$example.exe"
destination="$output_directory/aqua-$example.exe"
cp -- "$binary" "$destination"
(cd -- "$output_directory" && sha256sum -- "aqua-$example.exe" > "aqua-$example.exe.sha256")
printf 'Built %s\n' "$destination"
