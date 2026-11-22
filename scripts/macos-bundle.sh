#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: macos-bundle.sh --executable <path> --app-id <reverse-dns id> --name <display name> --out <directory>" >&2
  echo "                       --dxcompiler <libdxcompiler.dylib> --spirv-cross <libspirv-cross-c-shared dylib>" >&2
  echo "                       --license <file> [--license <file> ...] [--version <x.y.z>]" >&2
  exit 2
}

executable=""
app_id=""
name=""
out=""
dxcompiler=""
spirv_cross=""
version="0.1.0"
licenses=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --executable) executable="$2"; shift 2 ;;
    --app-id) app_id="$2"; shift 2 ;;
    --name) name="$2"; shift 2 ;;
    --out) out="$2"; shift 2 ;;
    --dxcompiler) dxcompiler="$2"; shift 2 ;;
    --spirv-cross) spirv_cross="$2"; shift 2 ;;
    --license) licenses+=("$2"); shift 2 ;;
    --version) version="$2"; shift 2 ;;
    *) usage ;;
  esac
done

[[ -n "$executable" && -n "$app_id" && -n "$name" && -n "$out" && -n "$dxcompiler" && -n "$spirv_cross" ]] || usage
[[ ${#licenses[@]} -gt 0 ]] || { echo "macos-bundle: at least one --license file is required" >&2; exit 2; }
[[ "$app_id" =~ ^[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)+$ ]] || { echo "macos-bundle: $app_id is not a reverse-DNS bundle identifier" >&2; exit 2; }
for file in "$executable" "$dxcompiler" "$spirv_cross" "${licenses[@]}"; do
  [[ -f "$file" ]] || { echo "macos-bundle: $file does not exist" >&2; exit 1; }
done

program=$(basename "$executable")
app="$out/$name.app"
contents="$app/Contents"
rm -rf "$app"
mkdir -p "$contents/MacOS" "$contents/Frameworks" "$contents/Resources/licenses"
cp "$executable" "$contents/MacOS/$program"

escape() {
  local text="$1"
  text="${text//&/&amp;}"
  text="${text//</&lt;}"
  text="${text//>/&gt;}"
  printf '%s' "$text"
}

cat > "$contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleExecutable</key><string>$(escape "$program")</string>
  <key>CFBundleIdentifier</key><string>$(escape "$app_id")</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleName</key><string>$(escape "$name")</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$(escape "$version")</string>
  <key>CFBundleVersion</key><string>$(escape "$version")</string>
  <key>LSMinimumSystemVersion</key><string>14.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
PLIST
plutil -lint "$contents/Info.plist" >/dev/null

framework() {
  local source="$1" target="$contents/Frameworks/$2"
  if lipo -archs "$source" | tr ' ' '\n' | grep -qx arm64; then
    case "$(lipo -archs "$source")" in
      arm64) cp "$source" "$target" ;;
      *) lipo "$source" -thin arm64 -output "$target" ;;
    esac
  else
    echo "macos-bundle: $source has no arm64 slice ($(lipo -archs "$source"))" >&2
    exit 1
  fi
  chmod u+w "$target"
  install_name_tool -id "@rpath/$2" "$target"
  echo "framework $2 $(lipo -archs "$target") $(stat -f %z "$target") bytes"
}

framework "$dxcompiler" libdxcompiler.dylib
framework "$spirv_cross" libspirv-cross-c-shared.dylib

for license in "${licenses[@]}"; do
  cp "$license" "$contents/Resources/licenses/"
done

codesign --force --sign - "$contents/Frameworks/libdxcompiler.dylib" "$contents/Frameworks/libspirv-cross-c-shared.dylib"
codesign --force --sign - --identifier "$app_id" "$app"
codesign --verify --strict "$app"
echo "bundle $app"
