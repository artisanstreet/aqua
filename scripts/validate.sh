#!/usr/bin/env bash
# Canonical Aqua validation (Linux). Owner: verification agent.
# No push/publish. Usage:
#   scripts/validate.sh [--smoke]
# --smoke runs the WSLg showcase smoke (finite frames, auto-close).
# Strict GPU mode: AQUA_REQUIRE_GPU=1 turns a no-adapter SKIP into FAIL.
set -euo pipefail
cd "$(dirname "$0")/.."

JOBS="${CARGO_BUILD_JOBS:-2}"
export CARGO_BUILD_JOBS="$JOBS"

pass=0; fail=0; skip=0
report() { # $1=name $2=status(PASS/FAIL/SKIP) $3=detail
  echo "[$2] $1 ${3:-}"
  case "$2" in
    PASS) pass=$((pass+1));; FAIL) fail=$((fail+1));; SKIP) skip=$((skip+1));;
  esac
}

echo "== fmt =="
if cargo fmt --check >/tmp/opencode-fmt.log 2>&1; then report "cargo fmt --check" PASS; else report "cargo fmt --check" FAIL "(see /tmp/opencode-fmt.log)"; fi

echo "== targeted tests (shared target lock; compile errors are FAIL, never SKIP) =="
for p in aqua-core aqua-reactive aqua-platform aqua-text aqua-ui; do
  if CARGO_BUILD_JOBS="$JOBS" cargo test -p "$p" -- --nocapture >/tmp/opencode-test-$p.log 2>&1; then
    report "cargo test -p $p" PASS
  else
    report "cargo test -p $p" FAIL "(see /tmp/opencode-test-$p.log)"
  fi
done

echo "== render + facade verification (GPU serial; --nocapture so SKIP lines are visible) =="
for t in "aqua-render:--test-threads=1" "aqua:--test-threads=1"; do
  p="${t%%:*}"; args="${t#*:}"
  if CARGO_BUILD_JOBS="$JOBS" cargo test -p "$p" -- --nocapture "$args" >/tmp/opencode-test-$p.log 2>&1; then
    if grep -q "SKIP-NO-ADAPTER" /tmp/opencode-test-$p.log; then
      if [[ "${AQUA_REQUIRE_GPU:-0}" == "1" ]]; then
        report "cargo test -p $p" FAIL "(AQUA_REQUIRE_GPU=1 but adapter missing)"
      else
        report "cargo test -p $p" SKIP "(no GPU adapter: untested-GPU, not a pixel pass)"
      fi
    else
      report "cargo test -p $p" PASS
    fi
  else
    if grep -q "could not compile" /tmp/opencode-test-$p.log; then
      report "cargo test -p $p" FAIL "(peer crate mid-edit; see /tmp/opencode-test-$p.log)"
    elif grep -q "no usable GPU adapter" /tmp/opencode-test-$p.log; then
      if [[ "${AQUA_REQUIRE_GPU:-0}" == "1" ]]; then
        report "cargo test -p $p" FAIL "(AQUA_REQUIRE_GPU=1 and no adapter)"
      else
        report "cargo test -p $p" SKIP "(no GPU adapter: untested-GPU, not a pixel pass)"
      fi
    else
      report "cargo test -p $p" FAIL "(see /tmp/opencode-test-$p.log)"
    fi
  fi
done

echo "== clippy (all targets: lib + examples + tests + all crates) =="
if CARGO_BUILD_JOBS="$JOBS" cargo clippy --workspace --all-targets -- -D warnings >/tmp/opencode-clippy.log 2>&1; then
  report "cargo clippy --workspace --all-targets" PASS
else
  report "cargo clippy --workspace --all-targets" FAIL "(see /tmp/opencode-clippy.log)"
fi

echo "== windows cross-check (whole workspace, all targets) =="
if cargo check --workspace --all-targets --target x86_64-pc-windows-gnu >/tmp/opencode-windows.log 2>&1; then
  report "windows-gnu check --workspace --all-targets" PASS
else
  report "windows-gnu check --workspace --all-targets" FAIL "(see /tmp/opencode-windows.log)"
fi

if [[ "${1:-}" == "--smoke" ]]; then
  echo "== WSLg smoke =="
  export DISPLAY="${DISPLAY:-:0}" WAYLAND_DISPLAY="${WAYLAND_DISPLAY:-wayland-0}" XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/1001}"
  if AQUA_SMOKE_FRAMES=120 CARGO_BUILD_JOBS="$JOBS" timeout 120 cargo run -p aqua --example showcase -- --smoke-test >/tmp/opencode-smoke.log 2>&1; then
    report "showcase smoke (finite frames)" PASS
  else
    report "showcase smoke (finite frames)" FAIL "(see /tmp/opencode-smoke.log)"
  fi
fi

echo
echo "validate: $pass passed, $fail failed, $skip skipped"
[[ "$fail" -eq 0 ]]
