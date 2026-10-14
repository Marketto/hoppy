# Hoppy PoC installer for Windows (per-user, no admin needed).
# Usage: put this script next to hoppy.exe and run:
#   powershell -ExecutionPolicy Bypass -File install.ps1
$ErrorActionPreference = 'Stop'
$src = Join-Path $PSScriptRoot 'hoppy.exe'
if (-not (Test-Path $src)) { throw "hoppy.exe not found next to install.ps1" }

$dir = Join-Path $env:LOCALAPPDATA 'Programs\Hoppy'
New-Item -ItemType Directory -Force -Path $dir | Out-Null
Copy-Item $src (Join-Path $dir 'hoppy.exe') -Force
$exe = Join-Path $dir 'hoppy.exe'

# Adds "Send to -> Hoppy" and the minimized receiver at login.
& $exe install
if ($LASTEXITCODE -ne 0) { throw "hoppy install failed" }

# Start receiving now, minimized.
Start-Process -FilePath $exe -ArgumentList 'receive' -WindowStyle Minimized
Write-Host "Hoppy installed in $dir. Right-click files -> Send to -> Hoppy."
Write-Host "If Windows asks about the firewall, allow Private networks."
