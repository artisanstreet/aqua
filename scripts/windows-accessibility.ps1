param([Parameter(Mandatory=$true)][string]$Executable, [string]$Arguments = '', [switch]$TextGeometry)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class AquaAccessibilityWindow {
    private delegate bool EnumWindow(IntPtr window, IntPtr parameter);
    [DllImport("user32.dll")] private static extern bool EnumWindows(EnumWindow callback, IntPtr parameter);
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
    [DllImport("user32.dll")] private static extern bool IsWindowVisible(IntPtr window);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int GetWindowText(IntPtr window, StringBuilder text, int length);
    public static IntPtr Find(int process) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((window, parameter) => {
            uint owner;
            GetWindowThreadProcessId(window, out owner);
            if (owner != process || !IsWindowVisible(window)) return true;
            var title = new StringBuilder(256);
            GetWindowText(window, title, title.Capacity);
            if (!title.ToString().StartsWith("Aqua")) return true;
            found = window;
            return false;
        }, IntPtr.Zero);
        return found;
    }
}
'@
$info = New-Object Diagnostics.ProcessStartInfo
$info.FileName = [IO.Path]::GetFullPath($Executable)
$info.Arguments = $Arguments
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
        if ($child.HasExited) { throw "Child exited before creating a window: $($stderr.Result)" }
        $window = [AquaAccessibilityWindow]::Find($child.Id)
        if ($window -ne [IntPtr]::Zero) { break }
        Start-Sleep -Milliseconds 50
    } while ([DateTime]::UtcNow -lt $deadline)
    if ($window -eq [IntPtr]::Zero) { throw 'No Aqua test window' }
    $root = [Windows.Automation.AutomationElement]::FromHandle($window)
    if ($TextGeometry) {
        $fields = @{}
        $deadline = [DateTime]::UtcNow.AddSeconds(10)
        do {
            foreach ($name in @('Original text', 'Transformed text')) {
                $condition = New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::NameProperty, $name)
                $fields[$name] = $root.FindFirst([Windows.Automation.TreeScope]::Descendants, $condition)
            }
            if ($null -ne $fields['Original text'] -and $null -ne $fields['Transformed text']) { break }
            Start-Sleep -Milliseconds 50
        } while ([DateTime]::UtcNow -lt $deadline)
        if ($null -eq $fields['Original text'] -or $null -eq $fields['Transformed text']) { throw 'Native geometry fixture fields missing' }
        $original = [Windows.Automation.TextPattern]$fields['Original text'].GetCurrentPattern([Windows.Automation.TextPattern]::Pattern)
        $transformed = [Windows.Automation.TextPattern]$fields['Transformed text'].GetCurrentPattern([Windows.Automation.TextPattern]::Pattern)
        $original_bounds = $original.DocumentRange.GetBoundingRectangles()
        $transformed_bounds = $transformed.DocumentRange.GetBoundingRectangles()
        if ($original_bounds.Length -ne 1 -or $transformed_bounds.Length -ne 1) { throw "Expected one shaped line per native geometry field: original=$($original_bounds -join ' ,') transformed=$($transformed_bounds -join ' ,')" }
        if ([Math]::Abs($transformed_bounds[0].Width - 2 * $original_bounds[0].Width) -gt 2 -or [Math]::Abs($transformed_bounds[0].Height - 2 * $original_bounds[0].Height) -gt 2) { throw 'Native TextPattern did not scale character geometry with retained transform' }
        $original_element = $fields['Original text'].Current.BoundingRectangle
        $transformed_element = $fields['Transformed text'].Current.BoundingRectangle
        if ([Math]::Abs($transformed_element.Width - 2 * $original_element.Width) -gt 2) { throw 'Native element bounds did not apply retained scale' }
        $emoji = [char]::ConvertFromUtf32(0x1f469) + [char]0x200d + [char]::ConvertFromUtf32(0x1f4bb)
        foreach ($pattern in @($original, $transformed)) {
            $range = $pattern.DocumentRange.Clone()
            [void]$range.MoveEndpointByUnit([Windows.Automation.Text.TextPatternRangeEndpoint]::Start, [Windows.Automation.Text.TextUnit]::Character, 2)
            [void]$range.MoveEndpointByUnit([Windows.Automation.Text.TextPatternRangeEndpoint]::End, [Windows.Automation.Text.TextUnit]::Character, -1)
            if ($range.GetText(-1) -ne $emoji) { throw 'Native emoji text range missing' }
            $bounds = $range.GetBoundingRectangles()
            if ($bounds.Length -ne 1 -or $bounds[0].Width -le 0 -or $bounds[0].Height -le 0) { throw 'Native emoji geometry invalid' }
            $point = New-Object Windows.Point(($bounds[0].X + $bounds[0].Width * 0.2), ($bounds[0].Y + $bounds[0].Height * 0.5))
            $hit = $pattern.RangeFromPoint($point)
            [void]$hit.MoveEndpointByUnit([Windows.Automation.Text.TextPatternRangeEndpoint]::End, [Windows.Automation.Text.TextUnit]::Character, 1)
            if ($hit.GetText(-1) -ne $emoji) { throw 'Native RangeFromPoint did not resolve transformed emoji grapheme' }
            $range.Select()
            $deadline = [DateTime]::UtcNow.AddSeconds(5)
            do {
                $selection = $pattern.GetSelection()
                if ($selection.Length -eq 1 -and $selection[0].GetText(-1) -eq $emoji) { break }
                Start-Sleep -Milliseconds 50
            } while ([DateTime]::UtcNow -lt $deadline)
            if ($selection.Length -ne 1 -or $selection[0].GetText(-1) -ne $emoji) { throw 'Native transformed selection did not round-trip' }
        }
        ([Windows.Automation.WindowPattern]$root.GetCurrentPattern([Windows.Automation.WindowPattern]::Pattern)).Close()
        if (-not $child.WaitForExit(5000)) { throw 'Native geometry window did not close' }
        if ($child.ExitCode -ne 0) { throw "Native geometry child exited $($child.ExitCode): $($stderr.Result)" }
        Write-Output 'WINDOWS_TEXT_GEOMETRY_OK: native transformed element bounds, TextPattern glyph geometry, RangeFromPoint and grapheme selection'
        return
    }
    $edit_condition = New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::ControlTypeProperty, [Windows.Automation.ControlType]::Edit)
    $button_condition = New-Object Windows.Automation.AndCondition(
        (New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::ControlTypeProperty, [Windows.Automation.ControlType]::Button)),
        (New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::NameProperty, 'Add'))
    )
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    do {
        $field = $root.FindFirst([Windows.Automation.TreeScope]::Descendants, $edit_condition)
        $button = $root.FindFirst([Windows.Automation.TreeScope]::Descendants, $button_condition)
        if ($null -ne $field -and $null -ne $button) { break }
        Start-Sleep -Milliseconds 50
    } while ([DateTime]::UtcNow -lt $deadline)
    if ($null -eq $field -or $null -eq $button) {
        $identity = "root name=$($root.Current.Name) handle=$($root.Current.NativeWindowHandle) process=$($root.Current.ProcessId) child handle=$window"
        $child.Kill()
        $child.WaitForExit()
        throw "$identity Editable field or Add button missing from native accessibility tree: $($stderr.Result) $($stdout.Result)"
    }
    $value = [Windows.Automation.ValuePattern]$field.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern)
    $initial_text = [Windows.Automation.TextPattern]$field.GetCurrentPattern([Windows.Automation.TextPattern]::Pattern)
    if ($initial_text.DocumentRange.GetText(-1) -ne '') { throw 'Placeholder must not become accessible document text' }
    $unicode_text = 'A' + [char]::ConvertFromUtf32(0x1f469) + [char]0x200d + [char]::ConvertFromUtf32(0x1f4bb) + [char]0xe9 + 'Z'
    $emoji = [char]::ConvertFromUtf32(0x1f469) + [char]0x200d + [char]::ConvertFromUtf32(0x1f4bb)
    $value.SetValue($unicode_text)
    $text_pattern = [Windows.Automation.TextPattern]$field.GetCurrentPattern([Windows.Automation.TextPattern]::Pattern)
    $deadline = [DateTime]::UtcNow.AddSeconds(5)
    do {
        if ($text_pattern.DocumentRange.GetText(-1) -eq $unicode_text) { break }
        Start-Sleep -Milliseconds 50
    } while ([DateTime]::UtcNow -lt $deadline)
    if ($text_pattern.DocumentRange.GetText(-1) -ne $unicode_text) { throw 'Native TextPattern did not expose Unicode document text' }
    $range = $text_pattern.DocumentRange.Clone()
    [void]$range.MoveEndpointByUnit([Windows.Automation.Text.TextPatternRangeEndpoint]::Start, [Windows.Automation.Text.TextUnit]::Character, 1)
    [void]$range.MoveEndpointByUnit([Windows.Automation.Text.TextPatternRangeEndpoint]::End, [Windows.Automation.Text.TextUnit]::Character, -2)
    $range.Select()
    $deadline = [DateTime]::UtcNow.AddSeconds(5)
    do {
        $selection = $text_pattern.GetSelection()
        if ($selection.Length -eq 1 -and $selection[0].GetText(-1) -eq $emoji) { break }
        Start-Sleep -Milliseconds 50
    } while ([DateTime]::UtcNow -lt $deadline)
    if ($selection.Length -ne 1 -or $selection[0].GetText(-1) -ne $emoji) { throw 'Native TextPattern selection did not round-trip the emoji grapheme' }
    $value.SetValue('Native accessibility task')
    $deadline = [DateTime]::UtcNow.AddSeconds(5)
    do {
        if ($value.Current.Value -eq 'Native accessibility task') { break }
        Start-Sleep -Milliseconds 50
    } while ([DateTime]::UtcNow -lt $deadline)
    if ($value.Current.Value -ne 'Native accessibility task') { throw 'Native SetValue did not reach the bound field' }
    $invoke = [Windows.Automation.InvokePattern]$button.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern)
    $invoke.Invoke()
    $task_condition = New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::NameProperty, 'Native accessibility task')
    $deadline = [DateTime]::UtcNow.AddSeconds(5)
    do {
        $task = $root.FindFirst([Windows.Automation.TreeScope]::Descendants, $task_condition)
        if ($null -ne $task -and $value.Current.Value -eq '') { break }
        Start-Sleep -Milliseconds 50
    } while ([DateTime]::UtcNow -lt $deadline)
    if ($null -eq $task -or $value.Current.Value -ne '') { throw 'Native Invoke did not create the task and clear the draft' }
    if ($text_pattern.DocumentRange.GetText(-1) -ne '') { throw 'Cleared field must expose empty document text' }
    ([Windows.Automation.WindowPattern]$root.GetCurrentPattern([Windows.Automation.WindowPattern]::Pattern)).Close()
    if (-not $child.WaitForExit(5000)) { throw 'Accessible close did not terminate the test window' }
    if ($child.ExitCode -ne 0) { throw "Native accessibility child exited $($child.ExitCode): $($stderr.Result)" }
    Write-Output 'WINDOWS_ACCESSIBILITY_OK: native roles, TextPattern Unicode/grapheme selection, SetValue, Invoke, bound state and close'
} finally {
    if ($child.Id -and -not $child.HasExited) { $child.Kill() }
    $child.Dispose()
}
