param(
  [Parameter(Mandatory=$true)][string]$Exe,
  [Parameter(Mandatory=$true)][string]$Out,
  [string]$Prefix = "app",
  [string]$Steps = "",
  [double]$Boot = 6,
  [int]$LogicalW = 1518,
  [int]$LogicalH = 900,
  [string]$Data = "",
  [string]$Stats = "0",
  [string]$EditorDir = "",
  [string]$AppArgs = "",
  [string]$Events = ""
)
$ErrorActionPreference = "Stop"
if (-not $Steps -and $env:AQUA_STEPS) { $Steps = $env:AQUA_STEPS }
Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @"
using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using System.Text;
public static class W {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc p, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint f);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int c);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint f);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("kernel32.dll")] public static extern void GetSystemTimePreciseAsFileTime(out long t);
  public static long PreciseNow() { long t; GetSystemTimePreciseAsFileTime(out t); return t; }
  [DllImport("user32.dll")] public static extern bool AdjustWindowRectExForDpi(ref RECT r, uint style, bool menu, uint ex, uint dpi);
  [DllImport("user32.dll")] public static extern int GetWindowLong(IntPtr h, int i);
  [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
  [DllImport("user32.dll", SetLastError=true)] public static extern IntPtr SendMessageTimeout(IntPtr h, uint msg, IntPtr w, IntPtr l, uint flags, uint timeout, out IntPtr result);
  public sealed class Frame { public int W, H; public byte[] Px; }
  public static Frame Grab(IntPtr h) {
    RECT wr; GetWindowRect(h, out wr);
    RECT cr; GetClientRect(h, out cr);
    POINT o = new POINT(); ClientToScreen(h, ref o);
    int ww = wr.R - wr.L, wh = wr.B - wr.T;
    int cw = cr.R - cr.L, ch = cr.B - cr.T;
    if (ww <= 0 || wh <= 0 || cw <= 0 || ch <= 0) return null;
    using (Bitmap full = new Bitmap(ww, wh, PixelFormat.Format32bppArgb)) {
      using (Graphics g = Graphics.FromImage(full)) {
        IntPtr dc = g.GetHdc();
        PrintWindow(h, dc, 2);
        g.ReleaseHdc(dc);
      }
      Rectangle rc = new Rectangle(o.X - wr.L, o.Y - wr.T, cw, ch);
      BitmapData d = full.LockBits(rc, ImageLockMode.ReadOnly, PixelFormat.Format32bppArgb);
      Frame f = new Frame(); f.W = cw; f.H = ch; f.Px = new byte[cw * ch * 4];
      for (int y = 0; y < ch; y++) Marshal.Copy(IntPtr.Add(d.Scan0, y * d.Stride), f.Px, y * cw * 4, cw * 4);
      full.UnlockBits(d);
      return f;
    }
  }
  public static int ChangedPixels(Frame a, Frame b, int x, int y, int w, int h) {
    if (a == null || b == null || a.W != b.W || a.H != b.H) return -1;
    int n = 0;
    int x1 = Math.Min(a.W, x + w), y1 = Math.Min(a.H, y + h);
    for (int j = Math.Max(0, y); j < y1; j++) {
      int row = j * a.W * 4;
      for (int i = Math.Max(0, x); i < x1; i++) {
        int k = row + i * 4;
        int d = Math.Abs(a.Px[k] - b.Px[k]) + Math.Abs(a.Px[k + 1] - b.Px[k + 1]) + Math.Abs(a.Px[k + 2] - b.Px[k + 2]);
        if (d > 24) n++;
      }
    }
    return n;
  }
  public static double NonDominant(Frame a) {
    if (a == null) return 0;
    System.Collections.Generic.Dictionary<int, int> counts = new System.Collections.Generic.Dictionary<int, int>();
    int total = 0;
    for (int j = 0; j < a.H; j += 5) {
      for (int i = 0; i < a.W; i += 5) {
        int k = (j * a.W + i) * 4;
        int key = ((a.Px[k] >> 3) << 10) | ((a.Px[k + 1] >> 3) << 5) | (a.Px[k + 2] >> 3);
        int c; counts.TryGetValue(key, out c); counts[key] = c + 1; total++;
      }
    }
    int best = 0; foreach (int v in counts.Values) if (v > best) best = v;
    return total == 0 ? 0 : 1.0 - (double)best / total;
  }
  public static IntPtr Find(uint pid) {
    IntPtr found = IntPtr.Zero; long best = 0;
    EnumWindows((h, l) => {
      uint p; GetWindowThreadProcessId(h, out p);
      if (p == pid && IsWindowVisible(h)) {
        RECT r; GetClientRect(h, out r);
        long a = (long)(r.R - r.L) * (r.B - r.T);
        if (a > best) { best = a; found = h; }
      }
      return true;
    }, IntPtr.Zero);
    return found;
  }
  public static void Shot(IntPtr h, string path) {
    RECT wr; GetWindowRect(h, out wr);
    RECT cr; GetClientRect(h, out cr);
    POINT o = new POINT(); ClientToScreen(h, ref o);
    int ww = wr.R - wr.L, wh = wr.B - wr.T;
    using (Bitmap full = new Bitmap(ww, wh, PixelFormat.Format32bppArgb)) {
      using (Graphics g = Graphics.FromImage(full)) {
        IntPtr dc = g.GetHdc();
        PrintWindow(h, dc, 2);
        g.ReleaseHdc(dc);
      }
      int cw = cr.R - cr.L, ch = cr.B - cr.T;
      int cx = o.X - wr.L, cy = o.Y - wr.T;
      using (Bitmap c = full.Clone(new Rectangle(cx, cy, cw, ch), PixelFormat.Format32bppArgb)) {
        c.Save(path, ImageFormat.Png);
      }
    }
  }
}
"@
[void][W]::SetProcessDPIAware()
New-Item -ItemType Directory -Force -Path $Out | Out-Null
$run = Join-Path $env:TEMP ("aqua-drive-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $run | Out-Null
$dataDir = Join-Path $run "editor-config"
New-Item -ItemType Directory -Force -Path $dataDir | Out-Null
if (-not $Data) { $Data = Join-Path $env:LOCALAPPDATA "Artisan Street\Editor\data" }
foreach ($n in @("identity.json","hosts.json","settings.json","account.json")) {
  $s = Join-Path $Data $n
  if (Test-Path -LiteralPath $s) { Copy-Item -LiteralPath $s -Destination $dataDir }
}
if (Test-Path -LiteralPath (Join-Path $Data "accounts")) { Copy-Item -Recurse -LiteralPath (Join-Path $Data "accounts") -Destination $dataDir }
if ($EditorDir) {
  Remove-Item -Recurse -Force $dataDir -ErrorAction SilentlyContinue
  Copy-Item -Recurse -LiteralPath $EditorDir -Destination $dataDir
}
$bin = Join-Path $run ("aquarun" + [Guid]::NewGuid().ToString("N").Substring(0, 8) + ".exe")
Copy-Item -LiteralPath $Exe -Destination $bin
$env:ARTISAN_EDITOR_DATA = $dataDir
if ($env:AQUA_RWC_OVERRIDE -eq "off") { Remove-Item Env:AQUA_RENDER_WHEN_COVERED -ErrorAction SilentlyContinue } else { $env:AQUA_RENDER_WHEN_COVERED = "1" }
$keepProfile = $env:AQUA_PROFILE
$override = $env:AQUA_STATS_OVERRIDE
if ($override -in @("off","wakes","profile","profilewakes","memory")) {
  foreach ($n in @("AQUA_FRAME_STATS","AQUA_PERF","AQUA_WAKE_STATS","AQUA_PROFILE","AQUA_INPUT_LATENCY","AQUA_REBUILD_STATS","AQUA_MEMORY_REPORT")) { Remove-Item "Env:$n" -ErrorAction SilentlyContinue }
  if ($override -eq "wakes" -or $override -eq "profilewakes") { $env:AQUA_WAKE_STATS = "1" }
  if ($override -eq "memory") { $env:AQUA_MEMORY_REPORT = "1" }
  if ($override -eq "profile" -or $override -eq "profilewakes") { $env:AQUA_PROFILE = $keepProfile }
} elseif ($Stats -eq "1") { $env:AQUA_FRAME_STATS = "1"; $env:AQUA_PERF = "1"; $env:AQUA_WAKE_STATS = "1" } else { Remove-Item Env:AQUA_FRAME_STATS -ErrorAction SilentlyContinue }
$errLog = Join-Path $Out "$Prefix-stderr.log"
$outLog = Join-Path $Out "$Prefix-stdout.log"
$fg0 = [W]::GetForegroundWindow()
$launchMs = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
$launchClock = [System.Diagnostics.Stopwatch]::StartNew()
$startArgs = @{ FilePath = $bin; WorkingDirectory = $run; PassThru = $true; RedirectStandardError = $errLog; RedirectStandardOutput = $outLog }
if ($AppArgs) { $startArgs.ArgumentList = $AppArgs }
$proc = Start-Process @startArgs
$procId = $proc.Id
"probe-launch unix_ms=$launchMs pid=$procId"
try {
  $h = [IntPtr]::Zero
  $deadline = (Get-Date).AddSeconds(30)
  while ($h -eq [IntPtr]::Zero -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 15; $h = [W]::Find([uint32]$procId) }
  if ($h -eq [IntPtr]::Zero) { throw "no window for pid $procId" }
  $windowMs = $launchClock.Elapsed.TotalMilliseconds
  $dpi = [W]::GetDpiForWindow($h); $scale = $dpi / 96.0
  $pw = [int][Math]::Round($LogicalW * $scale); $ph = [int][Math]::Round($LogicalH * $scale)
  $r = New-Object W+RECT; $r.L = 0; $r.T = 0; $r.R = $pw; $r.B = $ph
  $style = [W]::GetWindowLong($h, -16); $ex = [W]::GetWindowLong($h, -20)
  [void][W]::AdjustWindowRectExForDpi([ref]$r, [uint32]$style, $false, [uint32]$ex, $dpi)
  if ($env:AQUA_NO_RESIZE -ne "1") { [void][W]::SetWindowPos($h, $(if ($env:AQUA_PLACE -eq "top") { [IntPtr](-1) } else { [IntPtr]1 }), 40, 40, ($r.R - $r.L), ($r.B - $r.T), 0x0010) }
  $firstMs = -1; $firstFrac = 0
  while ($launchClock.Elapsed.TotalSeconds -lt 30) {
    $f = [W]::Grab($h)
    $frac = [W]::NonDominant($f)
    if ($frac -gt 0.06) { $firstMs = $launchClock.Elapsed.TotalMilliseconds; $firstFrac = $frac; break }
    Start-Sleep -Milliseconds 10
  }
  "probe-startup window_ms=$([math]::Round($windowMs,0)) first_content_ms=$([math]::Round($firstMs,0)) nondominant=$([math]::Round($firstFrac,3))"
  for ($i = 0; $i -lt $(if ($env:AQUA_NO_RESIZE -eq "1") { 0 } else { 4 }); $i++) {
    Start-Sleep -Milliseconds 400
    $cr = New-Object W+RECT; [void][W]::GetClientRect($h, [ref]$cr)
    $wr = New-Object W+RECT; [void][W]::GetWindowRect($h, [ref]$wr)
    $dw = $pw - ($cr.R - $cr.L); $dh = $ph - ($cr.B - $cr.T)
    if ($dw -eq 0 -and $dh -eq 0) { break }
    [void][W]::SetWindowPos($h, $(if ($env:AQUA_PLACE -eq "top") { [IntPtr](-1) } else { [IntPtr]1 }), 40, 40, ($wr.R - $wr.L + $dw), ($wr.B - $wr.T + $dh), 0x0010)
  }
  Start-Sleep -Seconds $Boot
  $proc.Refresh()
  if ($proc.HasExited) { "probe-exited exit_code=$($proc.ExitCode) after_boot_s=$Boot" }
  function Pack($x, $y) { $px = [int][Math]::Round($x * $scale); $py = [int][Math]::Round($y * $scale); [IntPtr](($py -shl 16) -bor ($px -band 0xFFFF)) }
  function Post($m, $w, $l) { [void][W]::PostMessage($h, [uint32]$m, [IntPtr]$w, $l) }
function Probe-Latency($name, $kind, $count, $gap, $scroll, $x, $y, $delta) {
  $sw = [System.Diagnostics.Stopwatch]::new()
  $total = [System.Diagnostics.Stopwatch]::StartNew()
  $lat = New-Object System.Collections.Generic.List[double]
  $timeouts = 0
  $pt = New-Object W+POINT; $pt.X = [int]($x * $scale); $pt.Y = [int]($y * $scale)
  [void][W]::ClientToScreen($h, [ref]$pt)
  for ($i = 0; $i -lt $count; $i++) {
    $tick = $total.Elapsed.TotalMilliseconds
    if ($scroll) {
      $d = if (($i % 40) -lt 20) { $delta } else { -$delta }
      $w = [IntPtr](([int64]($d -band 0xFFFF)) -shl 16)
      $l = [IntPtr](([int64]($pt.Y -band 0xFFFF) -shl 16) -bor ($pt.X -band 0xFFFF))
      Post 0x20A $w $l
    }
    $r = [IntPtr]::Zero
    $sw.Restart()
    $ok = [W]::SendMessageTimeout($h, 0, [IntPtr]::Zero, [IntPtr]::Zero, 3, 2000, [ref]$r)
    $sw.Stop()
    if ($ok -eq [IntPtr]::Zero) { $timeouts++ } else { $lat.Add($sw.Elapsed.TotalMilliseconds) }
    $rest = $gap - ($total.Elapsed.TotalMilliseconds - $tick)
    if ($rest -gt 1) { Start-Sleep -Milliseconds ([int]$rest) }
  }
  $series = @(); for ($b = 0; $b -lt $lat.Count; $b += 100) { $chunk = @($lat[$b..([math]::Min($b + 99, $lat.Count - 1))] | Sort-Object); $series += "$([math]::Round($chunk[[int](($chunk.Count - 1) / 2)],1))/$([math]::Round($chunk[-1],0))" }
  $v = @($lat | Sort-Object)
  if ($v.Count -eq 0) { "probe-latency name=$name kind=$kind samples=0 timeouts=$timeouts"; return }
  function Pc($f) { $v[[int][math]::Min($v.Count - 1, [math]::Round(($v.Count - 1) * $f))] }
  $mean = ($v | Measure-Object -Average).Average
  "probe-latency name=$name kind=$kind samples=$($v.Count) timeouts=$timeouts p50_ms=$([math]::Round((Pc 0.5),3)) p90_ms=$([math]::Round((Pc 0.9),3)) p99_ms=$([math]::Round((Pc 0.99),3)) max_ms=$([math]::Round($v[-1],3)) mean_ms=$([math]::Round($mean,3)) over4_ms=$(@($v | Where-Object { $_ -gt 4 }).Count) over8_ms=$(@($v | Where-Object { $_ -gt 8.3 }).Count) over16_ms=$(@($v | Where-Object { $_ -gt 16.7 }).Count) over50_ms=$(@($v | Where-Object { $_ -gt 50 }).Count) p50_max_per_100=$($series -join ',')"
}
  function Summarize-Latency($name, $kind, $lat, $timeouts, $extra) {
    $v = @($lat | Sort-Object)
    if ($v.Count -eq 0) { "probe-latency name=$name kind=$kind samples=0 timeouts=$timeouts"; return }
    function Pc2($f) { $v[[int][math]::Min($v.Count - 1, [math]::Round(($v.Count - 1) * $f))] }
    $mean = ($v | Measure-Object -Average).Average
    "probe-latency name=$name kind=$kind samples=$($v.Count) timeouts=$timeouts p50_ms=$([math]::Round((Pc2 0.5),3)) p90_ms=$([math]::Round((Pc2 0.9),3)) p99_ms=$([math]::Round((Pc2 0.99),3)) max_ms=$([math]::Round($v[-1],3)) mean_ms=$([math]::Round($mean,3)) over4_ms=$(@($v | Where-Object { $_ -gt 4 }).Count) over8_ms=$(@($v | Where-Object { $_ -gt 8.3 }).Count) over16_ms=$(@($v | Where-Object { $_ -gt 16.7 }).Count) over50_ms=$(@($v | Where-Object { $_ -gt 50 }).Count) $extra"
  }
  $frames = @{}
  $lastClickMs = 0
  $vk = @{ enter=13; esc=27; escape=27; tab=9; backspace=8; space=32; left=37; up=38; right=39; down=40; delete=46; home=36; end=35; pageup=33; pagedown=34 }
  foreach ($step in ($Steps -split ";")) {
    $p = $step.Trim() -split "\s+", 2
    $cmd = $p[0]; $arg = if ($p.Count -gt 1) { $p[1] } else { "" }
    $a = @(if ($arg) { $arg -split "\s+" })
    switch ($cmd) {
      "" { }
      "sleep" { Start-Sleep -Milliseconds ([int]([double]$a[0] * 1000)) }
      "wakelines" {
        $secs = [double]$a[1]
        $count = {
          try {
            $fs = [System.IO.File]::Open($errLog, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite -bor [System.IO.FileShare]::Delete)
            $sr = New-Object System.IO.StreamReader($fs)
            $text = $sr.ReadToEnd(); $sr.Dispose(); $fs.Dispose()
            @($text -split "`n" | Where-Object { $_ -like "*aqua-wakes:*" })
          } catch { @() }
        }
        $before = (& $count).Count
        Start-Sleep -Milliseconds ([int]($secs * 1000))
        $lines = & $count
        $new = @($lines | Select-Object -Skip $before)
        "probe-wakelines name=$($a[0]) seconds=$secs lines=$($new.Count)"
        foreach ($l in $new) { "probe-wakeline name=$($a[0]) $($l.Trim())" }
      }
      "move" { Post 0x200 0 (Pack ([double]$a[0]) ([double]$a[1])) }
      "click" { $lastClickMs = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds(); $l = Pack ([double]$a[0]) ([double]$a[1]); Post 0x200 0 $l; Start-Sleep -Milliseconds 80; Post 0x201 1 $l; Start-Sleep -Milliseconds 60; Post 0x202 0 $l }
      "rclick" { $l = Pack ([double]$a[0]) ([double]$a[1]); Post 0x200 0 $l; Start-Sleep -Milliseconds 80; Post 0x204 2 $l; Start-Sleep -Milliseconds 60; Post 0x205 0 $l }
      "wheel" {
        $pt = New-Object W+POINT; $pt.X = [int]([double]$a[0] * $scale); $pt.Y = [int]([double]$a[1] * $scale)
        [void][W]::ClientToScreen($h, [ref]$pt)
        Post 0x200 0 (Pack ([double]$a[0]) ([double]$a[1]))
        $delta = [int]$a[2]
        $w = [IntPtr](([int64]($delta -band 0xFFFF)) -shl 16)
        $l = [IntPtr](([int64]($pt.Y -band 0xFFFF) -shl 16) -bor ($pt.X -band 0xFFFF))
        Post 0x20A $w $l
      }
      "key" { $k = $a[0].ToLower(); $code = if ($vk.ContainsKey($k)) { $vk[$k] } else { [int][char]$a[0].ToUpper()[0] }; Post 0x100 $code 0; Start-Sleep -Milliseconds 30; Post 0x101 $code ([IntPtr]0xC0000001) }
      "type" { foreach ($c in $arg.ToCharArray()) { Post 0x102 ([int]$c) 1; Start-Sleep -Milliseconds 25 } }
      "keypost" {
        $gap = [int]$a[1]
        $text = $a[0] -replace "_", " "
        $postLog = New-Object System.Collections.Generic.List[string]
        foreach ($c in $text.ToCharArray()) {
          $vkc = [int][char]([string]$c).ToUpper()[0]
          $t = [W]::PreciseNow()
          Post 0x100 $vkc 0
          $postLog.Add("$t")
          Post 0x101 $vkc ([IntPtr]0xC0000001)
          Start-Sleep -Milliseconds $gap
        }
        Set-Content -Path (Join-Path $Out "$Prefix-keyposts.txt") -Value $postLog
        "probe-keypost posts=$($postLog.Count) gap_ms=$gap pid=$procId"
      }
      "mark" {
        $proc.Refresh()
        $ms = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
        "perf-mark name=$($a[0]) unix_ms=$ms private_mb=$([int]($proc.PrivateMemorySize64/1MB)) working_set_mb=$([int]($proc.WorkingSet64/1MB))"
      }
      "cpu" {
        $proc.Refresh()
        if ($proc.HasExited) { "probe-exited name=$($a[0]) exit_code=$($proc.ExitCode)"; continue }
        $ms = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
        "probe-cpu name=$($a[0]) unix_ms=$ms cpu_ms=$([int]$proc.TotalProcessorTime.TotalMilliseconds) user_ms=$([int]$proc.UserProcessorTime.TotalMilliseconds) private_mb=$([math]::Round($proc.PrivateMemorySize64/1MB,1)) working_set_mb=$([math]::Round($proc.WorkingSet64/1MB,1)) threads=$($proc.Threads.Count) handles=$($proc.HandleCount)"
      }
      "lat" { Probe-Latency $a[0] "idle" ([int]$a[1]) ([double]$a[2]) $false 0 0 0 }
      "scrolllat" { Probe-Latency $a[0] "scroll" ([int]$a[3]) ([double]$a[4]) $true ([double]$a[1]) ([double]$a[2]) ([int]$a[5]) }
      "sample" {
        & "C:\Users\sander\AppData\Local\aqua-port-tools\sample.ps1" -Pid0 $procId -Count ([int]$a[1]) 2>&1 | ForEach-Object { "probe-sample $_" }
      }
      "setsize" {
        $nw = [int]($a[1]); $nh = [int]($a[2])
        $wr0 = New-Object W+RECT; [void][W]::GetWindowRect($h, [ref]$wr0)
        [void][W]::SetWindowPos($h, $(if ($env:AQUA_PLACE -eq "top") { [IntPtr](-1) } else { [IntPtr]1 }), 40, 40, $nw, $nh, 0x0010)
      }
      "threads" {
        $secs = [int]$a[1]
        try {
          if (-not ("TD" -as [type])) { Add-Type -TypeDefinition @"
using System; using System.Runtime.InteropServices; using System.Text;
public static class TD {
  [DllImport("kernel32.dll")] public static extern IntPtr OpenThread(uint a, bool i, uint id);
  [DllImport("kernel32.dll")] public static extern bool CloseHandle(IntPtr h);
  [DllImport("kernel32.dll")] public static extern int GetThreadDescription(IntPtr h, out IntPtr d);
  [DllImport("ntdll.dll")] public static extern int NtQueryInformationThread(IntPtr h, int cls, out IntPtr info, int len, IntPtr ret);
  public static long Start(uint id) { IntPtr h = OpenThread(0x0040, false, id); if (h == IntPtr.Zero) return 0; IntPtr a; int r = NtQueryInformationThread(h, 9, out a, IntPtr.Size, IntPtr.Zero); CloseHandle(h); return r == 0 ? a.ToInt64() : 0; }
  public static string Name(uint id) { IntPtr h = OpenThread(0x0040, false, id); if (h == IntPtr.Zero) return ""; IntPtr d; string r = ""; if (GetThreadDescription(h, out d) >= 0 && d != IntPtr.Zero) r = Marshal.PtrToStringUni(d); CloseHandle(h); return r; }
}
"@ }
          $snap = {
            $m = @{}
            $cs = @{}
            Get-CimInstance Win32_PerfRawData_PerfProc_Thread -Filter "Name like '$($proc.ProcessName)/%'" | ForEach-Object { $cs[[int]$_.IDThread] = [double]$_.ContextSwitchesPersec }
            $proc.Refresh()
            foreach ($t in $proc.Threads) { $m[$t.Id] = @{ cpu = $t.TotalProcessorTime.TotalMilliseconds; cs = $cs[$t.Id]; name = [TD]::Name([uint32]$t.Id); start = ([TD]::Start([uint32]$t.Id) - $proc.MainModule.BaseAddress.ToInt64()); module = (($proc.Modules | Where-Object { $a0 = [TD]::Start([uint32]$t.Id); $a0 -ge $_.BaseAddress.ToInt64() -and $a0 -lt ($_.BaseAddress.ToInt64() + $_.ModuleMemorySize) } | Select-Object -First 1).ModuleName); wait = "$($t.WaitReason)" } }
            $m
          }
          $s0 = & $snap
          Start-Sleep -Seconds $secs
          $s1 = & $snap
          $active = 0
          foreach ($id in $s1.Keys) {
            $b = $s0[$id]; $e = $s1[$id]
            if ($b) { $dc = $e.cs - $b.cs; $dm = $e.cpu - $b.cpu } else { $dc = $e.cs; $dm = $e.cpu }
            $started = ""; try { $started = [int](($proc.Threads | Where-Object { $_.Id -eq $id } | Select-Object -First 1).StartTime - $proc.StartTime).TotalMilliseconds } catch {}
            if ($dc -gt 0 -or $dm -gt 0) { $active++ }
            if ($dc -gt 0 -or $dm -gt 0) { "probe-thread name=$($a[0]) tid=$id thread='$($e.name)' started_ms=$started module=$($e.module) start_rva=0x$([Convert]::ToString([int64]$e.start,16)) ctx_per_s=$([math]::Round($dc / $secs,1)) cpu_ms_per_s=$([math]::Round($dm / $secs,1))" }
          }
        "probe-threads-done name=$($a[0]) seconds=$secs active=$active total=$($s1.Count)"
        } catch { "probe-threads name=$($a[0]) unavailable=$($_.Exception.Message)" }
      }
      "wakes" {
        $secs = [int]$a[1]
        $pn = (Get-Process -Id $procId).ProcessName
        $snap = {
          $m = @{}
          Get-CimInstance Win32_PerfRawData_PerfProc_Thread -Filter "Name like '$pn/%'" -ErrorAction SilentlyContinue | Where-Object { $_.IDProcess -eq $procId } | ForEach-Object { $m[[int]$_.IDThread] = [double]$_.ContextSwitchesPersec }
          $m
        }
        $prev = & $snap
        $perSec = @()
        $perThread = @{}
        for ($i = 0; $i -lt $secs; $i++) {
          Start-Sleep -Milliseconds 1000
          $cur = & $snap
          $sum = 0.0
          foreach ($id in $cur.Keys) { if ($prev.ContainsKey($id)) { $d = [math]::Max(0.0, $cur[$id] - $prev[$id]); $sum += $d; $perThread[$id] = [double]$perThread[$id] + $d } }
          $perSec += $sum
          $prev = $cur
        }
        $avg = ($perSec | Measure-Object -Average).Average
        $mx = ($perSec | Measure-Object -Maximum).Maximum
        $top = ($perThread.GetEnumerator() | Sort-Object Value -Descending | Select-Object -First 4 | ForEach-Object { "$($_.Key)=$([math]::Round($_.Value / $secs,0))" }) -join ","
        "probe-wakes name=$($a[0]) seconds=$secs context_switches_per_sec_avg=$([math]::Round($avg,1)) max=$([math]::Round($mx,1)) top_threads=$top attempt=1"
      }
      "display" {
        try {
          $vc = @(Get-CimInstance Win32_VideoController | Where-Object { $_.CurrentRefreshRate })
          $plan = ((powercfg /getactivescheme) -join " ") -replace "^.*\((.*)\).*$", '$1'
          $ac = (Get-CimInstance -Namespace root\wmi -ClassName BatteryStatus -ErrorAction SilentlyContinue | Select-Object -First 1)
          $gpus = ($vc | ForEach-Object { "$($_.Name -replace ' ','_'):$($_.CurrentHorizontalResolution)x$($_.CurrentVerticalResolution)@$($_.CurrentRefreshRate)Hz" }) -join ","
          "probe-display name=$($a[0]) gpus=$gpus power_plan=$($plan -replace ' ','_') ac_online=$(if ($ac) { $ac.PowerOnline } else { 'n/a' })"
        } catch { "probe-display name=$($a[0]) unavailable=$($_.Exception.Message)" }
      }
      "load" {
        $secs = [int]$a[1]
        try {
          $set = Get-Counter -Counter "\Processor(_Total)\% Processor Time" -SampleInterval 1 -MaxSamples $secs -ErrorAction Stop
          $avg = ($set | ForEach-Object { $_.CounterSamples[0].CookedValue } | Measure-Object -Average).Average
          "probe-load name=$($a[0]) seconds=$secs system_cpu_pct=$([math]::Round($avg,1))"
        } catch { "probe-load name=$($a[0]) unavailable=$($_.Exception.Message)" }
      }
      "gpumem" {
        try {
          $s = Get-Counter -Counter "\GPU Process Memory(pid_$($procId)*)\Dedicated Usage" -ErrorAction Stop
          $sum = ($s.CounterSamples | Measure-Object -Property CookedValue -Sum).Sum
          "probe-gpumem name=$($a[0]) dedicated_mb=$([math]::Round($sum/1MB,1))"
        } catch { "probe-gpumem name=$($a[0]) unavailable=$($_.Exception.Message)" }
      }
      "snapshot" { $frames[$a[0]] = [W]::Grab($h) }
      "changed" {
        $f1 = [W]::Grab($h)
        $n = [W]::ChangedPixels($frames[$a[0]], $f1, [int]([double]$a[1] * $scale), [int]([double]$a[2] * $scale), [int]([double]$a[3] * $scale), [int]([double]$a[4] * $scale))
        $total = [int]([double]$a[3] * $scale) * [int]([double]$a[4] * $scale)
        "probe-changed name=$($a[0]) pixels=$n total=$total nondominant=$([math]::Round([W]::NonDominant($f1),3))"
      }
      "trigger" { New-Item -ItemType File -Force -Path $arg | Out-Null; "probe-trigger unix_ms=$([DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds())" }
      "waitopen" {
        $thread = $a[0]; $limit = [double]$a[1]; $found = $false
        $wait = [System.Diagnostics.Stopwatch]::StartNew()
        while ($wait.Elapsed.TotalSeconds -lt $limit -and -not $found) {
          if ($Events -and (Test-Path -LiteralPath $Events)) {
            foreach ($line in (Get-Content -LiteralPath $Events -ErrorAction SilentlyContinue)) {
              if ($line -match '"event":"opened"' -and $line -match ('"thread":"' + [regex]::Escape($thread) + '"')) {
                $ms = [double]([regex]::Match($line, '"unix_ms":(\d+)').Groups[1].Value)
                "probe-open thread=$thread unix_ms=$ms since_launch_ms=$([math]::Round($ms - $launchMs,0)) since_click_ms=$(if ($lastClickMs -gt 0) { [math]::Round($ms - $lastClickMs,0) } else { -1 })"
                $found = $true; break
              }
            }
          }
          if (-not $found) { Start-Sleep -Milliseconds 20 }
        }
        if (-not $found) { "probe-open-failed thread=$thread waited_s=$limit" }
      }
      "typelat" {
        $rx = [int]([double]$a[1] * $scale); $ry = [int]([double]$a[2] * $scale); $rw = [int]([double]$a[3] * $scale); $rh = [int]([double]$a[4] * $scale)
        $count = [int]$a[5]; $gap = [int]$a[6]
        $lat = New-Object System.Collections.Generic.List[double]; $timeouts = 0
        $grabs = New-Object System.Collections.Generic.List[double]
        $chars = "The quick brown fox jumps over the lazy dog "
        for ($i = 0; $i -lt $count; $i++) {
          $f0 = [W]::Grab($h)
          $sw = [System.Diagnostics.Stopwatch]::StartNew()
          Post 0x102 ([int]$chars[$i % $chars.Length]) 1
          $n = 0; $t = 0
          do {
            $g0 = $sw.Elapsed.TotalMilliseconds
            $f1 = [W]::Grab($h)
            $t = $sw.Elapsed.TotalMilliseconds
            $grabs.Add($t - $g0)
            $n = [W]::ChangedPixels($f0, $f1, $rx, $ry, $rw, $rh)
          } while ($n -lt 25 -and $t -lt 1500)
          if ($n -lt 25) { $timeouts++ } else { $lat.Add($t) }
          Start-Sleep -Milliseconds $gap
        }
        for ($i = 0; $i -lt $count; $i++) { Post 0x100 8 0; Start-Sleep -Milliseconds 20; Post 0x101 8 ([IntPtr]0xC0000001); Start-Sleep -Milliseconds 20 }
        $gs = @($grabs | Sort-Object)
        Summarize-Latency $a[0] "type" $lat $timeouts "grab_p50_ms=$([math]::Round($gs[[int](($gs.Count - 1) / 2)],1))"
      }
      "resizelat" {
        $count = [int]$a[1]; $gap = [double]$a[2]
        $lat = New-Object System.Collections.Generic.List[double]; $timeouts = 0; $setpos = New-Object System.Collections.Generic.List[double]
        $wr0 = New-Object W+RECT; [void][W]::GetWindowRect($h, [ref]$wr0)
        $baseW = $wr0.R - $wr0.L; $baseH = $wr0.B - $wr0.T
        $total = [System.Diagnostics.Stopwatch]::StartNew()
        for ($i = 0; $i -lt $count; $i++) {
          $tick = $total.Elapsed.TotalMilliseconds
          $phase = [math]::Sin($i * 0.21)
          $nw = [int]($baseW + $phase * 260 * $scale); $nh = [int]($baseH + $phase * 140 * $scale)
          $sw = [System.Diagnostics.Stopwatch]::StartNew()
          [void][W]::SetWindowPos($h, $(if ($env:AQUA_PLACE -eq "top") { [IntPtr](-1) } else { [IntPtr]1 }), 40, 40, $nw, $nh, 0x0010)
          $setpos.Add($sw.Elapsed.TotalMilliseconds)
          $r = [IntPtr]::Zero
          $ok = [W]::SendMessageTimeout($h, 0, [IntPtr]::Zero, [IntPtr]::Zero, 3, 2000, [ref]$r)
          $sw.Stop()
          if ($ok -eq [IntPtr]::Zero) { $timeouts++ } else { $lat.Add($sw.Elapsed.TotalMilliseconds) }
          $rest = $gap - ($total.Elapsed.TotalMilliseconds - $tick)
          if ($rest -gt 1) { Start-Sleep -Milliseconds ([int]$rest) }
        }
        [void][W]::SetWindowPos($h, $(if ($env:AQUA_PLACE -eq "top") { [IntPtr](-1) } else { [IntPtr]1 }), 40, 40, $baseW, $baseH, 0x0010)
        Summarize-Latency ($a[0] + "_setpos") "resize" $setpos 0 ""
        Summarize-Latency $a[0] "resize" $lat $timeouts ""
      }
      "shot" { Start-Sleep -Milliseconds 400; [W]::Shot($h, (Join-Path $Out ("$Prefix-" + $a[0] + ".png"))) }
      default { throw "unknown step $cmd" }
    }
  }
  $proc.Refresh()
  "pid=$procId dpi=$dpi working_set_mb=$([int]($proc.WorkingSet64/1MB)) private_mb=$([int]($proc.PrivateMemorySize64/1MB))"
  $fg1 = [W]::GetForegroundWindow()
  $fgPid0 = [uint32]0; [void][W]::GetWindowThreadProcessId($fg0, [ref]$fgPid0)
  $fgPid1 = [uint32]0; [void][W]::GetWindowThreadProcessId($fg1, [ref]$fgPid1)
  "foreground_changed=$($fg0 -ne $fg1) foreground_stolen_by_app=$($fgPid1 -eq [uint32]$procId) foreground_before_pid=$fgPid0 foreground_after_pid=$fgPid1"
} finally {
  Stop-Process -Id $procId -Force -ErrorAction SilentlyContinue
  Start-Sleep -Milliseconds 300
  Remove-Item -Recurse -Force $run -ErrorAction SilentlyContinue
}
