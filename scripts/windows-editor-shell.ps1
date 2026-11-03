param([Parameter(Mandatory=$true)][string]$Executable, [string]$Screenshot)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class AquaEditorShell {
    private delegate bool EnumWindow(IntPtr window, IntPtr parameter);
    [DllImport("user32.dll")] private static extern bool EnumWindows(EnumWindow callback, IntPtr parameter);
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
    [DllImport("user32.dll")] private static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int GetWindowText(IntPtr window, StringBuilder text, int length);
    [DllImport("user32.dll")] private static extern IntPtr SendMessage(IntPtr window, uint message, IntPtr wparam, IntPtr lparam);
    [DllImport("user32.dll")] private static extern bool ScreenToClient(IntPtr window, ref Position point);
    [StructLayout(LayoutKind.Sequential)] private struct Position { public int X; public int Y; }
    public static IntPtr Find(int process) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((window, parameter) => {
            uint owner; GetWindowThreadProcessId(window, out owner);
            if (owner != process || !IsWindowVisible(window)) return true;
            var title = new StringBuilder(256); GetWindowText(window, title, title.Capacity);
            if (!title.ToString().Contains("Editor shell")) return true;
            found = window; return false;
        }, IntPtr.Zero);
        return found;
    }
    public static void Pointer(IntPtr window, double x, double y, uint message, int buttons) {
        var point = new Position { X = (int)x, Y = (int)y }; ScreenToClient(window, ref point);
        var packed = new IntPtr((point.Y << 16) | (point.X & 0xffff));
        SendMessage(window, 0x0200, new IntPtr(buttons), packed);
        SendMessage(window, message, new IntPtr(buttons), packed);
    }
    public static void Escape(IntPtr window) { SendMessage(window, 0x0100, new IntPtr(27), IntPtr.Zero); SendMessage(window, 0x0101, new IntPtr(27), IntPtr.Zero); }
}
'@
function Find-Element([string]$Name, $Type = $null) {
    $name_condition = New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::NameProperty, $Name)
    $condition = $name_condition
    if ($null -ne $Type) {
        $condition = New-Object Windows.Automation.AndCondition($name_condition, (New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::ControlTypeProperty, $Type)))
    }
    return $script:root.FindFirst([Windows.Automation.TreeScope]::Descendants, $condition)
}
function Wait-Element([string]$Name, $Type = $null) {
    $deadline = [DateTime]::UtcNow.AddSeconds(8)
    do {
        $element = Find-Element $Name $Type
        if ($null -ne $element) { return $element }
        Start-Sleep -Milliseconds 50
    } while ([DateTime]::UtcNow -lt $deadline)
    $all = $script:root.FindAll([Windows.Automation.TreeScope]::Descendants, [Windows.Automation.Condition]::TrueCondition)
    $names = @($all | ForEach-Object { "$($_.Current.Name) [$($_.Current.ControlType.ProgrammaticName)]" })
    throw "Missing native element: $Name. Found: $($names -join ' | ')"
}
function Click-Element($Element) {
    $bounds = $Element.Current.BoundingRectangle
    if ($bounds.Width -le 0 -or $bounds.Height -le 0) { throw "Element has empty bounds: $($Element.Current.Name)" }
    $x = $bounds.X + $bounds.Width * 0.5
    $y = $bounds.Y + $bounds.Height * 0.5
    [AquaEditorShell]::Pointer($script:window, $x, $y, 0x0201, 1)
    [AquaEditorShell]::Pointer($script:window, $x, $y, 0x0202, 0)
}
$info = New-Object Diagnostics.ProcessStartInfo
$info.FileName = [IO.Path]::GetFullPath($Executable)
$info.UseShellExecute = $false
$info.RedirectStandardOutput = $true
$info.RedirectStandardError = $true
$info.EnvironmentVariables.Remove('AQUA_SMOKE_MS')
$info.EnvironmentVariables.Remove('AQUA_SMOKE_FRAMES')
$child = New-Object Diagnostics.Process
$child.StartInfo = $info
try {
    [void]$child.Start()
    $stdout = $child.StandardOutput.ReadToEndAsync()
    $stderr = $child.StandardError.ReadToEndAsync()
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    do {
        if ($child.HasExited) { throw "Child exited before window: $($stderr.Result)" }
        $script:window = [AquaEditorShell]::Find($child.Id)
        if ($script:window -ne [IntPtr]::Zero) { break }
        Start-Sleep -Milliseconds 50
    } while ([DateTime]::UtcNow -lt $deadline)
    if ($script:window -eq [IntPtr]::Zero) { throw 'No native editor shell window' }
    $script:root = [Windows.Automation.AutomationElement]::FromHandle($script:window)
    $scale = Wait-Element 'Text scale' ([Windows.Automation.ControlType]::Slider)
    $range = [Windows.Automation.RangeValuePattern]$scale.GetCurrentPattern([Windows.Automation.RangeValuePattern]::Pattern)
    $range.SetValue(125)
    [void](Wait-Element 'Text scale: 125%')
    Click-Element (Wait-Element 'Model engine' ([Windows.Automation.ControlType]::ComboBox))
    Click-Element (Wait-Element 'Precise' ([Windows.Automation.ControlType]::MenuItem))
    $engine = Wait-Element 'Model engine' ([Windows.Automation.ControlType]::ComboBox)
    $engine_value = [Windows.Automation.ValuePattern]$engine.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern)
    $deadline = [DateTime]::UtcNow.AddSeconds(5)
    do { if ($engine_value.Current.Value -eq 'Precise') { break }; Start-Sleep -Milliseconds 50 } while ([DateTime]::UtcNow -lt $deadline)
    if ($engine_value.Current.Value -ne 'Precise') { throw "Native dropdown selection did not update: $($engine_value.Current.Value)" }
    Click-Element (Wait-Element 'Sources' ([Windows.Automation.ControlType]::TabItem))
    Click-Element (Wait-Element 'Remove source 1' ([Windows.Automation.ControlType]::Button))
    [void](Wait-Element 'Removed source 1')
    if ($null -ne (Find-Element 'Remove source 1')) { throw 'Removed source remained mounted' }
    Click-Element (Wait-Element 'Market' ([Windows.Automation.ControlType]::TabItem))
    Click-Element (Wait-Element 'Install extension 2' ([Windows.Automation.ControlType]::Button))
    [void](Wait-Element 'Installed extension 2' ([Windows.Automation.ControlType]::Button))
    if ($Screenshot) {
        $bounds = $script:root.Current.BoundingRectangle
        $bitmap = New-Object Drawing.Bitmap([int]$bounds.Width, [int]$bounds.Height)
        $graphics = [Drawing.Graphics]::FromImage($bitmap)
        try { $graphics.CopyFromScreen([int]$bounds.X, [int]$bounds.Y, 0, 0, $bitmap.Size); $bitmap.Save([IO.Path]::GetFullPath($Screenshot), [Drawing.Imaging.ImageFormat]::Png) } finally { $graphics.Dispose(); $bitmap.Dispose() }
    }
    Click-Element (Wait-Element 'Commands' ([Windows.Automation.ControlType]::Button))
    $search = Wait-Element 'Command search' ([Windows.Automation.ControlType]::Edit)
    ([Windows.Automation.ValuePattern]$search.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern)).SetValue('sources')
    [void](Wait-Element 'Manage sources' ([Windows.Automation.ControlType]::Button))
    $deadline = [DateTime]::UtcNow.AddSeconds(5)
    do { if ($null -eq (Find-Element 'Open settings')) { break }; Start-Sleep -Milliseconds 50 } while ([DateTime]::UtcNow -lt $deadline)
    if ($null -ne (Find-Element 'Open settings')) { throw 'Native palette did not filter structural results' }
    Click-Element (Wait-Element 'Manage sources' ([Windows.Automation.ControlType]::Button))
    [void](Wait-Element 'Command: Manage sources')
    [AquaEditorShell]::Escape($script:window)
    $deadline = [DateTime]::UtcNow.AddSeconds(5)
    do { if ($null -eq (Find-Element 'Command search')) { break }; Start-Sleep -Milliseconds 50 } while ([DateTime]::UtcNow -lt $deadline)
    if ($null -ne (Find-Element 'Command search')) { throw 'Escape did not dismiss native palette' }
    $composer = Wait-Element 'Composer' ([Windows.Automation.ControlType]::Edit)
    ([Windows.Automation.ValuePattern]$composer.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern)).SetValue('Native composer text')
    $thread = Wait-Element 'Thread 42' ([Windows.Automation.ControlType]::Button)
    $source_bounds = $thread.Current.BoundingRectangle
    $target_bounds = $composer.Current.BoundingRectangle
    [AquaEditorShell]::Pointer($script:window, $source_bounds.X + 15, $source_bounds.Y + 15, 0x0201, 1)
    [AquaEditorShell]::Pointer($script:window, $target_bounds.X + 20, $target_bounds.Y + 20, 0x0200, 1)
    [AquaEditorShell]::Pointer($script:window, $target_bounds.X + 20, $target_bounds.Y + 20, 0x0202, 0)
    [void](Wait-Element 'Attached thread 42')
    Click-Element (Wait-Element 'Settings' ([Windows.Automation.ControlType]::TabItem))
    [void](Wait-Element 'Text scale: 125%')
    Start-Sleep -Milliseconds 100
    if ($Screenshot) {
        $bounds = $script:root.Current.BoundingRectangle
        $bitmap = New-Object Drawing.Bitmap([int]$bounds.Width, [int]$bounds.Height)
        $graphics = [Drawing.Graphics]::FromImage($bitmap)
        try { $graphics.CopyFromScreen([int]$bounds.X, [int]$bounds.Y, 0, 0, $bitmap.Size); $bitmap.Save([IO.Path]::GetFullPath($Screenshot), [Drawing.Imaging.ImageFormat]::Png) } finally { $graphics.Dispose(); $bitmap.Dispose() }
    }
    ([Windows.Automation.WindowPattern]$script:root.GetCurrentPattern([Windows.Automation.WindowPattern]::Pattern)).Close()
    if (-not $child.WaitForExit(5000)) { throw 'Native editor shell did not close' }
    if ($child.ExitCode -ne 0) { throw "Native shell exit $($child.ExitCode): $($stderr.Result)" }
    Write-Output 'WINDOWS_EDITOR_SHELL_OK: native range, dropdown, tabs, collections, filtered palette, modal editing, typed thread drag and close'
} finally {
    if ($child.Id -and -not $child.HasExited) { $child.Kill(); $child.WaitForExit() }
    if ($child.Id -and $child.HasExited -and $child.ExitCode -ne 0) { Write-Output $stderr.Result; Write-Output $stdout.Result }
    $child.Dispose()
}
