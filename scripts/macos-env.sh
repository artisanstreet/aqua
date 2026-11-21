#!/usr/bin/env bash
set -uo pipefail
root=$(cd "$(dirname "$(readlink -f "$0")")/.." && pwd)
artifacts="$root/target/aqua-mac-artifacts"
work=$(mktemp -d "${TMPDIR:-/tmp}/aqua-macos-env.XXXXXX")
trap 'rm -rf "$work"' EXIT
mkdir -p "$artifacts"
missing=0
facts=()

report() {
  case "$1" in
    ok) echo "[ok]      $2" ;;
    missing)
      echo "[missing] $2: $3"
      if [[ "${4:-required}" == required ]]; then
        missing=$((missing + 1))
      fi
      ;;
  esac
}

fact() {
  facts+=("$1=$2")
}

os="macOS $(sw_vers -productVersion) ($(sw_vers -buildVersion)), $(uname -m), $(sysctl -n hw.model), $(sysctl -n hw.ncpu) CPUs, $(($(sysctl -n hw.memsize) / 1073741824)) GiB"
fact host "$os"
if [[ "$(uname -m)" == arm64 ]]; then
  report ok "$os"
else
  report missing "Apple Silicon" "$os is not arm64"
fi

xcode=$(xcodebuild -version 2>/dev/null | tr '\n' ' ')
swift_version=$(xcrun swift --version 2>&1 | head -n1)
fact xcode "$xcode"
fact swift "$swift_version"
fact sdk "$(xcrun --sdk macosx --show-sdk-version 2>&1)"
if [[ -n "$xcode" ]]; then
  report ok "$xcode($(xcode-select -p)), $swift_version"
else
  report missing Xcode "xcodebuild -version failed; select an Xcode with xcode-select"
fi

dxc=${AQUA_DXC_SPIRV:-$(command -v dxc || true)}
if [[ -z "$dxc" || ! -x "$dxc" ]]; then
  report missing "dxc with SPIR-V codegen" "set AQUA_DXC_SPIRV to the LunarG SDK macOS/bin/dxc"
else
  dxc_version=$("$dxc" --version 2>&1 | head -n1)
  fact dxc_version "$dxc_version"
  if "$dxc" -spirv -fspv-target-env=vulkan1.1 -fvk-use-dx-layout -T ps_6_0 -E ps_quad \
    "$root/crates/aqua-shaders/hlsl/quad.hlsl" -Fo "$work/ps_quad.spv" >"$work/dxc.log" 2>&1; then
    report ok "dxc -spirv ($dxc_version)"
  else
    report missing "dxc with SPIR-V codegen" "$dxc could not compile ps_quad: $(head -n3 "$work/dxc.log" | tr '\n' ' ')"
  fi
fi

if [[ -f "$work/ps_quad.spv" ]] && command -v spirv-val >/dev/null 2>&1; then
  if spirv-val --target-env vulkan1.1 "$work/ps_quad.spv" >"$work/val.log" 2>&1; then
    report ok "spirv-val accepts ps_quad"
  else
    report missing spirv-val "ps_quad is invalid SPIR-V: $(head -n3 "$work/val.log" | tr '\n' ' ')"
  fi
fi

if [[ -f "$work/ps_quad.spv" ]] && command -v spirv-cross >/dev/null 2>&1; then
  if spirv-cross --msl --msl-version 20300 "$work/ps_quad.spv" --output "$work/ps_quad.metal" >"$work/cross.log" 2>&1; then
    fact spirv_cross_msl ok
    report ok "spirv-cross --msl ($(command -v spirv-cross))"
    if xcrun -sdk macosx metal -c "$work/ps_quad.metal" -o "$work/ps_quad.air" >"$work/metal.log" 2>&1; then
      fact offline_metal_compiler ok
      report ok "xcrun metal compiles the cross-compiled ps_quad"
    else
      fact offline_metal_compiler "$(head -n2 "$work/metal.log" | tr '\n' ' ')"
      report missing "xcrun metal" "$(head -n2 "$work/metal.log" | tr '\n' ' ')" fact
    fi
  else
    fact spirv_cross_msl "$(head -n2 "$work/cross.log" | tr '\n' ' ')"
    report missing "spirv-cross --msl" "$(head -n2 "$work/cross.log" | tr '\n' ' ')" fact
  fi
else
  fact spirv_cross_msl "spirv-cross not on PATH"
  report missing "spirv-cross CLI" "not on PATH" fact
fi

dxcompiler=${AQUA_DXCOMPILER:-}
if [[ -n "$dxcompiler" && -f "$dxcompiler" ]]; then
  fact dxcompiler_lipo "$(lipo -info "$dxcompiler" 2>&1)"
  fact dxcompiler_bytes "$(stat -L -f %z "$dxcompiler")"
  report ok "libdxcompiler ($dxcompiler, $(stat -L -f %z "$dxcompiler") bytes, $(lipo -archs "$dxcompiler" 2>&1))"
else
  report missing libdxcompiler "set AQUA_DXCOMPILER to the LunarG SDK macOS/lib/libdxcompiler.dylib"
fi

spirv_cross_c=${AQUA_SPIRV_CROSS_C:-}
if [[ -n "$spirv_cross_c" && -f "$spirv_cross_c" ]]; then
  fact spirv_cross_c_lipo "$(lipo -info "$spirv_cross_c" 2>&1)"
  report ok "SPIRV-Cross C library ($spirv_cross_c, $(lipo -archs "$spirv_cross_c" 2>&1))"
else
  fact spirv_cross_c missing
  report missing "SPIRV-Cross C library" "set AQUA_SPIRV_CROSS_C to the LunarG SDK macOS/lib/libspirv-cross-c-shared.dylib"
fi

if xcrun swiftc -O -target arm64-apple-macos14.0 "$root/scripts/macos-probe.swift" -o "$work/macos-probe" >"$work/swiftc.log" 2>&1; then
  report ok "macos-probe compiled"
  "$work/macos-probe" "$artifacts/probe.json" "$artifacts" "${facts[@]}" >"$work/probe.tsv" 2>"$work/probe.log"
  status=$?
  while IFS=$'\t' read -r kind name detail weight; do
    case "$kind" in
      ok) report ok "$name ($detail)" ;;
      missing) report missing "$name" "$detail" "$weight" ;;
    esac
  done <"$work/probe.tsv"
  cp "$work/probe.log" "$artifacts/probe-stderr.log"
  if [[ $status -ne 0 ]] || ! grep -q $'^ok\tprobe.json' "$work/probe.tsv"; then
    report missing macos-probe "exited $status before writing probe.json: $(tail -n5 "$work/probe.log" | tr '\n' ' ')"
  fi
else
  cp "$work/swiftc.log" "$artifacts/swiftc.log"
  report missing macos-probe "swiftc failed: $(grep -m5 'error:' "$work/swiftc.log" | tr '\n' ' ')"
fi

if [[ $missing -gt 0 ]]; then
  echo "macos-env: $missing missing; the macOS CI leg installs the pinned LunarG SDK" >&2
  exit 1
fi
