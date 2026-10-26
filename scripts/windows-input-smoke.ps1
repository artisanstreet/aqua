# Drive real Windows key/character messages through one isolated Aqua test window.
param([Parameter(Mandatory=$true)][string]$Executable)
$ErrorActionPreference = 'Stop'
Add-Type @'
using System;
using System.Runtime.InteropServices;
public class AquaKeyboardReplay {
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern uint MapVirtualKey(uint code, uint type);
}
'@
$info = New-Object System.Diagnostics.ProcessStartInfo
$info.FileName = [IO.Path]::GetFullPath($Executable)
$info.UseShellExecute = $false
$info.RedirectStandardOutput = $true
$info.RedirectStandardError = $true
$child = New-Object System.Diagnostics.Process
$child.StartInfo = $info
try {
    [void]$child.Start()
    $stdout = $child.StandardOutput.ReadToEndAsync()
    $stderr = $child.StandardError.ReadToEndAsync()
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    do {
        if ($child.HasExited) { throw "Test child exited before creating a window: $($stderr.Result)" }
        $child.Refresh()
        $window = $child.MainWindowHandle
        if ($window -ne [IntPtr]::Zero) { break }
        Start-Sleep -Milliseconds 50
    } while ([DateTime]::UtcNow -lt $deadline)
    if ($window -eq [IntPtr]::Zero) { throw 'No test window created' }
    # Let initial focus/layout settle. Only this child's queue receives input.
    Start-Sleep -Milliseconds 500
    function Send-Key([int]$vk, $character = $null) {
        $scan = [int64][AquaKeyboardReplay]::MapVirtualKey($vk, 0)
        $down = [IntPtr](($scan -shl 16) -bor 1)
        $up = [IntPtr](($scan -shl 16) -bor 3221225473L)
        if (-not [AquaKeyboardReplay]::PostMessage($window, 0x0100, [IntPtr]$vk, $down)) { throw 'WM_KEYDOWN failed' }
        if ($null -ne $character) {
            if (-not [AquaKeyboardReplay]::PostMessage($window, 0x0102, [IntPtr]([int]$character), $down)) { throw 'WM_CHAR failed' }
        }
        if (-not [AquaKeyboardReplay]::PostMessage($window, 0x0101, [IntPtr]$vk, $up)) { throw 'WM_KEYUP failed' }
    }
    Send-Key 0x23                 # End: initial AB, caret at end
    Send-Key 0x43 0x63            # type c -> ABc
    Send-Key 0x08 0x08            # one Backspace must leave exactly AB
    Send-Key 0x7a                 # F11 asserts the intermediate model
    1..3 | ForEach-Object { Send-Key 0x08 0x08 }  # finish deleting, including one empty-buffer Backspace
    Send-Key 0x2e 0x7f            # Delete with a control-text payload
    Send-Key 0x09 0x09            # Tab must not insert a tab
    Send-Key 0x0d 0x0d            # Enter must not insert CR into a single line
    Send-Key 0x7b                 # F12 asks the child to assert its final model
    if (-not $child.WaitForExit(15000)) { throw 'Native input replay timed out' }
    $output = $stdout.Result
    $errors = $stderr.Result
    Write-Output "NATIVE_EXIT=$($child.ExitCode)"
    Write-Output $output
    Write-Output $errors
    if ($child.ExitCode -ne 0 -or $output -notmatch 'WINDOWS_INPUT_OK') { throw 'Native keyboard regression failed' }
} finally {
    if ($child.Id -and -not $child.HasExited) { $child.Kill() }
    $child.Dispose()
}
