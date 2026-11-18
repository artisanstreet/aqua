param(
    [Parameter(Mandatory)][string]$TargetDir,
    [Parameter(Mandatory)][string]$OutDir,
    [Parameter(Mandatory)][string]$StopFile,
    [int]$LimitMinutes = 10,
    [string]$ProcDump = ''
)

$symbols = "srv*$env:RUNNER_TEMP\symbols*https://msdl.microsoft.com/download/symbols"
$cdb = @(
    "${env:ProgramFiles(x86)}\Windows Kits\10\Debuggers\x64\cdb.exe",
    "$env:ProgramFiles\Windows Kits\10\Debuggers\x64\cdb.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
$dumper = if ($cdb) { "cdb at $cdb" } elseif ($ProcDump) { "procdump at $ProcDump" } else { 'nothing (stacks and dumps unavailable)' }
Write-Host "test watchdog: limit $LimitMinutes min under $TargetDir, dumping with $dumper"

$handled = @{}
while (-not (Test-Path $StopFile)) {
    $limit = (Get-Date).AddMinutes(-$LimitMinutes)
    $hung = Get-CimInstance Win32_Process | Where-Object {
        $_.ExecutablePath -and
        $_.ExecutablePath.StartsWith($TargetDir, [StringComparison]::OrdinalIgnoreCase) -and
        $_.Name -notlike 'build-script-*' -and
        $_.CreationDate -lt $limit -and
        -not $handled.ContainsKey($_.ProcessId)
    }
    foreach ($process in $hung) {
        $handled[$process.ProcessId] = $true
        $id = $process.ProcessId
        $name = [IO.Path]::GetFileNameWithoutExtension($process.Name)
        $dir = Join-Path $OutDir "$name-$id"
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
        Write-Host "::error::test watchdog: $($process.Name) (pid $id) ran longer than $LimitMinutes minutes; capturing stacks and a full dump, then killing it"
        $image = $process.ExecutablePath
        Copy-Item $image $dir
        $pdb = [IO.Path]::ChangeExtension($image, 'pdb')
        if (Test-Path $pdb) { Copy-Item $pdb $dir }
        $dump = Join-Path $dir 'hang.dmp'
        if ($cdb) {
            $commands = ".reload /f; lm; ~*kb 80; .dump /ma /o `"$dump`"; qd"
            & $cdb -pv -p $id -y "$symbols;$([IO.Path]::GetDirectoryName($image))" -c $commands *> (Join-Path $dir 'stacks.txt')
        }
        if (-not (Test-Path $dump) -and $ProcDump) {
            & $ProcDump -accepteula -ma $id $dump *> (Join-Path $dir 'procdump.txt')
        }
        Stop-Process -Id $id -Force -ErrorAction SilentlyContinue
    }
    Start-Sleep -Seconds 20
}
