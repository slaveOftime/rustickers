<#
  Windows screenshot helper for rustickers visual checks.

  Captures the largest visible top-level window owned by a chosen process
  to a PNG via PrintWindow + PW_RENDERFULLCONTENT, so WebView2 / Edge
  composited surfaces -- the thing the wry WebView paints into -- actually
  appear in the bitmap. This is the runtime equivalent of the
  blank-preview regression check used to validate
  GPUI_DISABLE_DIRECT_COMPOSITION (see .agents/skills/gpui-visual-check).

  Two ways to pick a window:

    -ProcessName rustickers             # picks the largest window of that exe
    -WindowTitle "Clipboard history"    # first window whose title contains this
    -ProcessId  12345                   # first window of that PID
                                         (any of the three can be combined; the
                                          most specific one wins)

  Required:
    - PowerShell 5.1+ (built into Windows 10+)
    - System.Drawing.Common (built-in on Windows desktop SKUs)

  Out:
    - $OutPath: PNG file
    - Exit:     0 on success, 2 if no process, 3 if no window, 4 on capture fail
    - Stderr:   a one-line failure description on each non-zero exit
#>

[CmdletBinding()]
param(
    [string]$ProcessName,
    [string]$WindowTitle,
    [int]$ProcessId,
    [Parameter(Mandatory)]
    [string]$OutPath,
    [int]$WaitMs = 1500
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

if ($WaitMs -gt 0) {
    Start-Sleep -Milliseconds $WaitMs
}

Add-Type -AssemblyName System.Drawing

# P/Invoke surface. We avoid generic Win32 enumeration (which would need
# EnumWindows + a per-callback state object) by funneling through
# Get-Process + Process.MainWindowHandle; for rustickers the chooser and
# each sticker open their own process-handle-sized surface, so the largest
# one is usually the right one to capture for a single-window scene.
if (-not ([System.Management.Automation.PSTypeName]'Win32.WindowSnap').Type)
{
    # PowerShell 7 (Core) stopped auto-resolving System.Drawing.Common;
    # the Add-Type below needs the assembly name explicitly or the C#
    # compiler will say "type 'Image' could not be found". PowerShell 5.1
    # is happy without it, so the explicit reference is harmless there.
    $referenced = @(
        'System.Drawing.Common',
        'System.Drawing',
        'System.Runtime.InteropServices.RuntimeInformation'
    )
    Add-Type -ReferencedAssemblies $referenced @"
using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;

namespace Win32 {
    public static class WindowSnap {
        // Form-feed flip so alpha is treated as fully opaque by viewers
        // that respect it. PNGs don't have a transparency-by-default
        // contract anyway, so just pass the raw ARGB through.
        [DllImport("user32.dll")] static extern bool PrintWindow(IntPtr hwnd, IntPtr hdc, uint flags);
        [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr hwnd, out RECT r);
        [DllImport("user32.dll")] static extern IntPtr GetWindowDC(IntPtr hwnd);
        [DllImport("user32.dll")] static extern int   ReleaseDC(IntPtr hwnd, IntPtr hdc);
        [DllImport("gdi32.dll")]  static extern IntPtr CreateCompatibleDC(IntPtr hdc);
        [DllImport("gdi32.dll")]  static extern IntPtr CreateCompatibleBitmap(IntPtr hdc, int w, int h);
        [DllImport("gdi32.dll")]  static extern bool DeleteDC(IntPtr h);
        [DllImport("gdi32.dll")]  static extern bool DeleteObject(IntPtr h);
        [DllImport("gdi32.dll")]  static extern IntPtr SelectObject(IntPtr dc, IntPtr o);

        [StructLayout(LayoutKind.Sequential)]
        public struct RECT { public int Left, Top, Right, Bottom; }

        // PW_RENDERFULLCONTENT = 2 -- the only flag combo that actually
        // pulls WebView2's own surface into our bitmap.
        const uint PW_RENDERFULLCONTENT = 0x00000002;

        public static bool CaptureToPng(IntPtr hwnd, string path, out int width, out int height) {
            width = 0; height = 0;
            if (hwnd == IntPtr.Zero) return false;

            RECT r;
            if (!GetWindowRect(hwnd, out r)) return false;
            int w = Math.Max(1, r.Right - r.Left);
            int h = Math.Max(1, r.Bottom - r.Top);

            IntPtr screenDc = GetWindowDC(IntPtr.Zero);
            IntPtr memDc    = CreateCompatibleDC(screenDc);
            IntPtr bitmap   = CreateCompatibleBitmap(screenDc, w, h);
            IntPtr prev     = SelectObject(memDc, bitmap);

            bool ok = PrintWindow(hwnd, memDc, PW_RENDERFULLCONTENT);
            Image img = null;
            try {
                if (ok) {
                    img = Image.FromHbitmap(bitmap);
                }
            }
            finally {
                SelectObject(memDc, prev);
                if (bitmap != IntPtr.Zero) DeleteObject(bitmap);
                if (memDc   != IntPtr.Zero) DeleteDC(memDc);
                ReleaseDC(IntPtr.Zero, screenDc);
            }

            if (img == null) return false;
            try {
                img.Save(path, ImageFormat.Png);
                width  = img.Width;
                height = img.Height;
                return true;
            }
            finally {
                img.Dispose();
            }
        }

        public static int Area(IntPtr hwnd) {
            RECT r;
            if (hwnd == IntPtr.Zero) return -1;
            if (!GetWindowRect(hwnd, out r)) return -1;
            return (r.Right - r.Left) * (r.Bottom - r.Top);
        }
    }
}
"@
}

function Resolve-TargetWindow {
    [CmdletBinding()]
    param(
        [string]$Name,
        [string]$Title,
        [int]$Id
    )
    $candidates = @()
    if ($Id -gt 0) {
        $proc = Get-Process -Id $Id -ErrorAction SilentlyContinue
        if ($proc) { $candidates += $proc }
    }
    if ($Name) {
        $procs = Get-Process -Name $Name -ErrorAction SilentlyContinue
        foreach ($p in $procs) {
            $already = $false
            foreach ($c in $candidates) { if ($c.Id -eq $p.Id) { $already = $true; break } }
            if (-not $already) { $candidates += $p }
        }
    }

    if (-not $candidates -or $candidates.Count -eq 0) {
        [Console]::Error.WriteLine("no process matched (Name='$Name' Id=$Id)")
        return $null
    }

    $snapshots = New-Object System.Collections.Generic.List[object]
    foreach ($p in $candidates) {
        $hwnd = $p.MainWindowHandle
        if ($hwnd -eq [IntPtr]::Zero) { continue }
        $title = ""
        try { $title = $p.MainWindowTitle } catch { }
        $area = 0
        try { $area = [Win32.WindowSnap]::Area($hwnd) } catch { }
        $snapshots.Add([pscustomobject]@{
            Process = $p
            Hwnd    = $hwnd
            Title   = $title
            Area    = if ($area -gt 0) { $area } else { 0 }
        })
    }

    if ($snapshots.Count -eq 0) {
        [Console]::Error.WriteLine("matched processes have no main window")
        return $null
    }

    # Title filter (case-insensitive substring).
    if ($Title) {
        $titled = $snapshots | Where-Object { $_.Title -and $_.Title -like "*$Title*" }
        if ($titled) { $snapshots = $titled }
    }

    $best = $snapshots | Sort-Object -Property Area -Descending | Select-Object -First 1
    return $best
}

$target = Resolve-TargetWindow -Name $ProcessName -Title $WindowTitle -Id $ProcessId
if (-not $target) { exit 3 }

[int]$w = 0
[int]$h = 0
$ok = [Win32.WindowSnap]::CaptureToPng($target.Hwnd, $OutPath, [ref]$w, [ref]$h)
if (-not $ok) {
    [Console]::Error.WriteLine("PrintWindow failed for hwnd=$($target.Hwnd)")
    exit 4
}

Write-Output ("{0} bytes  {1}x{2}  process={3}  hwnd=0x{4:X}  title={5}" -f `
    (Get-Item $OutPath).Length, $w, $h, $target.Process.ProcessName, $target.Hwnd.ToInt64(), $target.Title)
exit 0
