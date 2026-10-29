# Canonical Aqua validation (Windows / PowerShell). Owner: verification agent.
# Mirrors scripts/validate.sh. No push/publish.
# Usage: scripts/validate.ps1 [-Smoke]
# Strict GPU mode: $env:AQUA_REQUIRE_GPU=1 turns a no-adapter SKIP into FAIL.
param([switch]$Smoke)
$ErrorActionPreference = "Continue"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location (Join-Path $here "..")
$jobs = if ($env:CARGO_BUILD_JOBS) { $env:CARGO_BUILD_JOBS } else { "2" }
$env:CARGO_BUILD_JOBS = $jobs
$pass = 0; $fail = 0; $skip = 0
function Report($name, $status, $detail = "") {
  Write-Output "[$status] $name $detail"
  switch ($status) { "PASS" { $script:pass++ } "FAIL" { $script:fail++ } "SKIP" { $script:skip++ } }
}
function Run-Test($crate, $extraArgs = @()) {
  # Compile errors are FAIL, never SKIP. Only missing GPU hardware may SKIP.
  $log = Join-Path $env:TEMP "aqua-test-$crate.log"
  & cargo test -p $crate -- --nocapture @extraArgs > $log 2>&1
  $code = $LASTEXITCODE
  $txt = Get-Content $log -Raw
  if ($txt -match "SKIP-NO-ADAPTER") {
    if ($env:AQUA_REQUIRE_GPU -eq "1") { Report "cargo test -p $crate" "FAIL" "(AQUA_REQUIRE_GPU=1 but adapter missing)" }
    else { Report "cargo test -p $crate" "SKIP" "(no GPU adapter: untested-GPU, not a pixel pass)" }
    return
  }
  if ($code -eq 0) { Report "cargo test -p $crate" "PASS"; return }
  if ($txt -match "could not compile") { Report "cargo test -p $crate" "FAIL" "(peer crate mid-edit; see $log)"; return }
  if ($txt -match "no usable GPU adapter") {
    if ($env:AQUA_REQUIRE_GPU -eq "1") { Report "cargo test -p $crate" "FAIL" "(AQUA_REQUIRE_GPU=1 and no adapter)" }
    else { Report "cargo test -p $crate" "SKIP" "(no GPU adapter: untested-GPU, not a pixel pass)" }
    return
  }
  Report "cargo test -p $crate" "FAIL" "(see $log)"
}
Write-Output "== fmt =="
& cargo fmt --check > (Join-Path $env:TEMP "aqua-fmt.log") 2>&1
if ($LASTEXITCODE -eq 0) { Report "cargo fmt --check" "PASS" } else { Report "cargo fmt --check" "FAIL" }
foreach ($p in @("aqua-core","aqua-reactive","aqua-platform","aqua-text","aqua-ui")) { Run-Test $p }
Run-Test "aqua-render" @("--test-threads=1")
Run-Test "aqua" @("--test-threads=1")
Write-Output "== clippy (all targets) =="
& cargo clippy --workspace --all-targets -- -D warnings > (Join-Path $env:TEMP "aqua-clippy.log") 2>&1
if ($LASTEXITCODE -eq 0) { Report "cargo clippy --workspace --all-targets" "PASS" } else { Report "cargo clippy --workspace --all-targets" "FAIL" }
Write-Output "== windows target (whole workspace, all targets) =="
& cargo check --workspace --all-targets --target x86_64-pc-windows-msvc > (Join-Path $env:TEMP "aqua-windows.log") 2>&1
if ($LASTEXITCODE -eq 0) { Report "windows-msvc check --workspace --all-targets" "PASS" } else { Report "windows check" "FAIL" }
if ($Smoke) {
  Write-Output "== smoke =="
  $env:AQUA_SMOKE_FRAMES = "120"
  & cargo run -p aqua --example showcase -- --smoke-test > (Join-Path $env:TEMP "aqua-smoke.log") 2>&1
  if ($LASTEXITCODE -eq 0) { Report "showcase smoke" "PASS" } else { Report "showcase smoke" "FAIL" }
}
Write-Output ""
Write-Output "validate: $pass passed, $fail failed, $skip skipped"
if ($fail -ne 0) { exit 1 }
