#!/usr/bin/env bash
set -uo pipefail
root=$(cd "$(dirname "$(readlink -f "$0")")/.." && pwd)
work=$(mktemp -d "${TMPDIR:-/tmp}/aqua-linux-env.XXXXXX")
trap 'rm -rf "$work"' EXIT
missing=0

report() {
  case "$1" in
    ok) echo "[ok]      $2" ;;
    missing)
      echo "[missing] $2: $3"
      missing=$((missing + 1))
      ;;
  esac
}

dxc=${AQUA_DXC_SPIRV:-$(command -v dxc || true)}
if ! command -v spirv-val >/dev/null 2>&1; then
  report missing spirv-val "install spirv-tools"
elif [[ -z "$dxc" || "$dxc" == *.exe ]]; then
  report missing "dxc with SPIR-V codegen" "set AQUA_DXC_SPIRV to a Linux dxc; the Windows SDK dxc.exe has no SPIR-V backend"
elif "$dxc" -spirv -fspv-target-env=vulkan1.1 -fvk-use-dx-layout -T ps_6_0 -E ps_quad \
  "$root/crates/aqua-shaders/hlsl/quad.hlsl" -Fo "$work/ps_quad.spv" >"$work/dxc.log" 2>&1 &&
  spirv-val --target-env vulkan1.1 "$work/ps_quad.spv" >>"$work/dxc.log" 2>&1; then
  report ok "dxc -spirv ($("$dxc" --version 2>&1 | head -n1)) and spirv-val"
else
  report missing "dxc with SPIR-V codegen" "$dxc could not compile ps_quad to valid SPIR-V: $(head -n3 "$work/dxc.log" | tr '\n' ' ')"
fi

if ! command -v vulkaninfo >/dev/null 2>&1; then
  report missing vulkaninfo "install vulkan-tools"
else
  vulkaninfo --summary >"$work/vulkan.txt" 2>&1
  if grep -q 'deviceType *= PHYSICAL_DEVICE_TYPE_CPU' "$work/vulkan.txt" && grep -q 'llvmpipe' "$work/vulkan.txt"; then
    report ok "lavapipe ($(grep -m1 'driverInfo' "$work/vulkan.txt" | sed 's/.*= //'))"
  else
    report missing lavapipe "install mesa-vulkan-drivers and point VK_ICD_FILENAMES at lvp_icd.x86_64.json"
  fi
  if grep -q 'VK_LAYER_KHRONOS_validation' "$work/vulkan.txt"; then
    report ok "VK_LAYER_KHRONOS_validation"
  else
    report missing "VK_LAYER_KHRONOS_validation" "install vulkan-validationlayers or set VK_LAYER_PATH"
  fi
fi

compositor=${AQUA_TEST_COMPOSITOR:-sway}
if ! command -v "$compositor" >/dev/null 2>&1; then
  report missing "$compositor" "install sway 1.9 or newer"
elif AQUA_REQUIRE_WAYLAND=1 "$root/tools/wayland-headless.sh" swaymsg -t get_outputs >"$work/outputs.json" 2>"$work/compositor.log" &&
  grep -q '"name": "HEADLESS-1"' "$work/outputs.json"; then
  report ok "$("$compositor" --version) headless with HEADLESS-1"
else
  report missing "headless $compositor" "tools/wayland-headless.sh failed: $(head -n3 "$work/compositor.log" | tr '\n' ' ')"
fi

if command -v dbus-daemon >/dev/null 2>&1; then
  report ok "dbus-daemon"
else
  report missing dbus-daemon "install dbus"
fi

launcher=${AQUA_ATSPI_BUS_LAUNCHER:-}
for candidate in "$(command -v at-spi-bus-launcher || true)" /usr/libexec/at-spi-bus-launcher /usr/lib/at-spi2-core/at-spi-bus-launcher; do
  [[ -z "$launcher" && -n "$candidate" && -x "$candidate" ]] && launcher=$candidate
done
if [[ -n "$launcher" && -x "$launcher" ]]; then
  report ok "at-spi-bus-launcher ($launcher)"
else
  report missing at-spi-bus-launcher "install at-spi2-core or set AQUA_ATSPI_BUS_LAUNCHER"
fi

if [[ $missing -gt 0 ]]; then
  echo "linux-env: $missing missing; nix-shell nix/linux.nix provides everything" >&2
  exit 1
fi
