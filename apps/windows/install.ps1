# Hoppy PoC installer for Windows (per-user, no admin needed).
# Usage: put this script next to hoppy.exe (and hoppyw.exe) and run:
#   powershell -ExecutionPolicy Bypass -File install.ps1
$ErrorActionPreference = 'Stop'
$src = Join-Path $PSScriptRoot 'hoppy.exe'
if (-not (Test-Path $src)) { throw "hoppy.exe not found next to install.ps1" }
$srcGui = Join-Path $PSScriptRoot 'hoppyw.exe'

$dir = Join-Path $env:LOCALAPPDATA 'Programs\Hoppy'
New-Item -ItemType Directory -Force -Path $dir | Out-Null
# Running copies (0.1.0 console receiver, tray app) lock the files: stop them first.
Get-Process -Name hoppy, hoppyw -ErrorAction SilentlyContinue | Stop-Process -Force
Copy-Item $src (Join-Path $dir 'hoppy.exe') -Force
if (Test-Path $srcGui) { Copy-Item $srcGui (Join-Path $dir 'hoppyw.exe') -Force }
$exe = Join-Path $dir 'hoppy.exe'
$gui = Join-Path $dir 'hoppyw.exe'

# Adds "Send to -> Hoppy", the right-click verb "Send with Hoppy" (with hoppyw.exe) and the
# receiver at login (tray icon with hoppyw.exe, minimized console without it).
& $exe install
if ($LASTEXITCODE -ne 0) { throw "hoppy install failed" }

# Start receiving now.
if (Test-Path $gui) {
    Start-Process -FilePath $gui
    Write-Host "Hoppy installed in $dir and running: look for its icon near the clock."
} else {
    Start-Process -FilePath $exe -ArgumentList 'receive' -WindowStyle Minimized
    Write-Host "Hoppy installed in $dir (console mode: hoppyw.exe was not next to this script)."
}
Write-Host "Send: right-click files -> Send with Hoppy (Windows 11: under Show more options, or run 'hoppy classic-menu on --restart-explorer')."
Write-Host "If Windows asks about the firewall, allow Private networks."
