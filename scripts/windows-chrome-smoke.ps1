# Query and exercise native window chrome on one isolated Todo process.
param([Parameter(Mandatory=$true)][string]$Executable)
$ErrorActionPreference = 'Stop'
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class AquaChromeProbe {
    [StructLayout(LayoutKind.Sequential)] public struct Rect { public int Left, Top, Right, Bottom; }
    [StructLayout(LayoutKind.Sequential)] public struct Point { public int X, Y; }
    [StructLayout(LayoutKind.Sequential)] public struct Limits { public Point Reserved, MaxSize, MaxPosition, MinTrack, MaxTrack; }
    [StructLayout(LayoutKind.Sequential)] public struct MonitorInfo { public int Size; public Rect Monitor, Work; public uint Flags; }
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out Rect r);
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out Rect r);
    [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref Point p);
    [DllImport("user32.dll")] public static extern bool IsZoomed(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
    [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
    [DllImport("user32.dll")] public static extern IntPtr MonitorFromWindow(IntPtr h, uint flags);
    [DllImport("user32.dll")] public static extern bool GetMonitorInfo(IntPtr h, ref MonitorInfo info);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll", SetLastError=true)] public static extern IntPtr SendMessageTimeout(IntPtr h, uint m, IntPtr w, IntPtr l, uint flags, uint timeout, out IntPtr result);
    [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int attribute, out Rect r, int size);
    public static int Hit(IntPtr h, int x, int y) {
        IntPtr result;
        long coords = ((long)(ushort)y << 16) | (ushort)x;
        if (SendMessageTimeout(h, 0x84, IntPtr.Zero, new IntPtr(coords), 2, 2000, out result) == IntPtr.Zero)
            throw new Exception("WM_NCHITTEST timed out");
        return result.ToInt32();
    }
    public static Limits TrackingLimits(IntPtr h) {
        IntPtr memory = Marshal.AllocHGlobal(Marshal.SizeOf(typeof(Limits)));
        try {
            Marshal.StructureToPtr(new Limits(), memory, false);
            IntPtr result;
            if (SendMessageTimeout(h, 0x24, IntPtr.Zero, memory, 2, 2000, out result) == IntPtr.Zero)
                throw new Exception("WM_GETMINMAXINFO timed out");
            return (Limits)Marshal.PtrToStructure(memory, typeof(Limits));
        } finally { Marshal.FreeHGlobal(memory); }
    }
}
'@
# DWM bounds and WM_NCHITTEST use physical pixels. Avoid PowerShell's
# default DPI virtualization when querying window/client rectangles.
[void][AquaChromeProbe]::SetThreadDpiAwarenessContext([IntPtr](-4))
$info = New-Object System.Diagnostics.ProcessStartInfo
$info.FileName = [IO.Path]::GetFullPath($Executable)
$info.WorkingDirectory = [IO.Path]::GetDirectoryName($info.FileName)
$info.UseShellExecute = $false
$info.RedirectStandardOutput = $true
$info.RedirectStandardError = $true
$info.StandardOutputEncoding = [Text.Encoding]::UTF8
$info.StandardErrorEncoding = [Text.Encoding]::UTF8
$info.EnvironmentVariables.Remove('AQUA_SMOKE_MS')
$info.EnvironmentVariables.Remove('AQUA_SMOKE_FRAMES')
$child = New-Object System.Diagnostics.Process
$child.StartInfo = $info
$success = $false
try {
    [void]$child.Start()
    $stdout = $child.StandardOutput.ReadToEndAsync()
    $stderr = $child.StandardError.ReadToEndAsync()
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    do {
        if ($child.HasExited) { throw "Child exited before creating a window (exit $($child.ExitCode)): $($stderr.Result)" }
        $child.Refresh()
        $window = $child.MainWindowHandle
        if ($window -ne [IntPtr]::Zero) { break }
        Start-Sleep -Milliseconds 50
    } while ([DateTime]::UtcNow -lt $deadline)
    if ($window -eq [IntPtr]::Zero) { throw 'No test window created' }
    Start-Sleep -Milliseconds 750
    $outer = New-Object AquaChromeProbe+Rect
    $client = New-Object AquaChromeProbe+Rect
    $origin = New-Object AquaChromeProbe+Point
    if (-not [AquaChromeProbe]::GetWindowRect($window, [ref]$outer)) { throw 'GetWindowRect failed' }
    if (-not [AquaChromeProbe]::GetClientRect($window, [ref]$client)) { throw 'GetClientRect failed' }
    if (-not [AquaChromeProbe]::ClientToScreen($window, [ref]$origin)) { throw 'ClientToScreen failed' }
    Write-Output "FRAME outer=$($outer.Left),$($outer.Top),$($outer.Right),$($outer.Bottom) client=$($client.Right)x$($client.Bottom) origin=$($origin.X),$($origin.Y)"
    $scale = [AquaChromeProbe]::GetDpiForWindow($window) / 96.0
    if ($client.Right -ne [Math]::Round(760 * $scale) -or $client.Bottom -ne [Math]::Round(560 * $scale)) {
        throw 'Overlay changed the configured 760x560 logical client size'
    }
    $limits = [AquaChromeProbe]::TrackingLimits($window)
    Write-Output "LIMITS min=$($limits.MinTrack.X)x$($limits.MinTrack.Y)"
    if ($limits.MinTrack.X -ne [Math]::Round(480 * $scale) -or $limits.MinTrack.Y -ne [Math]::Round(360 * $scale)) {
        throw 'Overlay changed the configured 480x360 logical minimum size'
    }
    # A true overlay has no native caption-height offset above the client.
    if (($origin.Y - $outer.Top) -gt 12) { throw 'Native titlebar still consumes client space' }
    $drag = [AquaChromeProbe]::Hit($window, $origin.X + 200, $origin.Y + 20)
    $body = [AquaChromeProbe]::Hit($window, $origin.X + 200, $origin.Y + 180)
    $edge = [AquaChromeProbe]::Hit($window, $outer.Left + 1, $origin.Y + 180)
    Write-Output "HIT drag=$drag body=$body left_edge=$edge"
    if ($drag -ne 2 -or $body -ne 1 -or $edge -ne 10) { throw 'Drag/client/resize hit testing failed' }
    $buttons = New-Object AquaChromeProbe+Rect
    $hr = [AquaChromeProbe]::DwmGetWindowAttribute($window, 5, [ref]$buttons, 16)
    if ($hr -ne 0 -or $buttons.Right -le $buttons.Left) { throw 'No DWM caption button bounds' }
    $buttonY = $outer.Top + [int](($buttons.Top + $buttons.Bottom) / 2)
    $hits = @()
    for ($x = $buttons.Left; $x -lt $buttons.Right; $x += 3) {
        $hits += [AquaChromeProbe]::Hit($window, $outer.Left + $x, $buttonY)
    }
    $unique = @($hits | Sort-Object -Unique)
    Write-Output "CAPTION bounds=$($buttons.Left),$($buttons.Top),$($buttons.Right),$($buttons.Bottom) hits=$($unique -join ',')"
    foreach ($expected in @(8, 9, 20)) {
        if ($unique -notcontains $expected) { throw "Native caption hit code $expected missing" }
    }
    # Exercise the OS operations without sending global mouse/keyboard input.
    [void][AquaChromeProbe]::PostMessage($window, 0x112, [IntPtr]0xF030, [IntPtr]::Zero)
    Start-Sleep -Milliseconds 450
    if (-not [AquaChromeProbe]::IsZoomed($window)) { throw 'Maximize failed' }
    $monitor = New-Object AquaChromeProbe+MonitorInfo
    $monitor.Size = 40
    if (-not [AquaChromeProbe]::GetMonitorInfo([AquaChromeProbe]::MonitorFromWindow($window, 2), [ref]$monitor)) { throw 'GetMonitorInfo failed' }
    $maxOrigin = New-Object AquaChromeProbe+Point
    $maxClient = New-Object AquaChromeProbe+Rect
    [void][AquaChromeProbe]::ClientToScreen($window, [ref]$maxOrigin)
    [void][AquaChromeProbe]::GetClientRect($window, [ref]$maxClient)
    Write-Output "MAXIMIZED client=$($maxOrigin.X),$($maxOrigin.Y),$($maxClient.Right),$($maxClient.Bottom) work=$($monitor.Work.Left),$($monitor.Work.Top),$($monitor.Work.Right),$($monitor.Work.Bottom)"
    if ($maxOrigin.X -lt $monitor.Work.Left -or $maxOrigin.Y -lt $monitor.Work.Top -or
        ($maxOrigin.X + $maxClient.Right) -gt $monitor.Work.Right -or
        ($maxOrigin.Y + $maxClient.Bottom) -gt $monitor.Work.Bottom) { throw 'Maximized client extends beyond monitor work area' }
    [void][AquaChromeProbe]::PostMessage($window, 0x112, [IntPtr]0xF120, [IntPtr]::Zero)
    Start-Sleep -Milliseconds 450
    if ([AquaChromeProbe]::IsZoomed($window)) { throw 'Restore failed' }
    [void][AquaChromeProbe]::PostMessage($window, 0x112, [IntPtr]0xF020, [IntPtr]::Zero)
    Start-Sleep -Milliseconds 450
    if (-not [AquaChromeProbe]::IsIconic($window)) { throw 'Minimize failed' }
    [void][AquaChromeProbe]::PostMessage($window, 0x112, [IntPtr]0xF120, [IntPtr]::Zero)
    Start-Sleep -Milliseconds 450
    if ([AquaChromeProbe]::IsIconic($window)) { throw 'Restore from minimized failed' }
    [void][AquaChromeProbe]::PostMessage($window, 0x10, [IntPtr]::Zero, [IntPtr]::Zero)
    if (-not $child.WaitForExit(15000)) { throw 'Close timed out' }
    Write-Output "NATIVE_EXIT=$($child.ExitCode)"
    Write-Output $stdout.Result
    Write-Output $stderr.Result
    if ($child.ExitCode -ne 0) { throw 'Native app failed' }
    Write-Output 'WINDOWS_CHROME_OK'
    $success = $true
} finally {
    # Only clean up the isolated process this script created.
    if ($child.Id -and -not $child.HasExited) { $child.Kill() }
    if (-not $success -and $stderr) { Write-Output $stderr.Result }
    $child.Dispose()
}
