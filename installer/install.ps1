$ErrorActionPreference = 'Stop'

$AppName = 'Oubliette'
$InstallDir = Join-Path $env:LOCALAPPDATA $AppName
$StartMenuDir = Join-Path ([System.Environment]::GetFolderPath('StartMenu')) 'Programs'
$DesktopDir = [System.Environment]::GetFolderPath('Desktop')
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path

function Section($text) {
    Write-Host ''
    Write-Host '================================================================' -ForegroundColor DarkGray
    Write-Host "  $text" -ForegroundColor Cyan
    Write-Host '================================================================' -ForegroundColor DarkGray
}

function OK($text)   { Write-Host "  + $text" -ForegroundColor Green }
function Warn($text) { Write-Host "  ! $text" -ForegroundColor Yellow }
function Info($text) { Write-Host "    $text" -ForegroundColor Gray }

Section "$AppName Installer"

# 1. WinFSP check ----------------------------------------------------------
$winfspPaths = @(
    'C:\Program Files (x86)\WinFsp\bin\winfsp-x64.dll',
    'C:\Program Files\WinFsp\bin\winfsp-x64.dll'
)
$winfspFound = $winfspPaths | Where-Object { Test-Path $_ } | Select-Object -First 1

if ($winfspFound) {
    OK "WinFSP detected at $winfspFound"
} else {
    Warn 'WinFSP is not installed.'
    Info 'WinFSP is a free MIT-licensed kernel driver required to mount the oubliette.'
    Info 'Opening https://winfsp.dev for you...'
    Start-Process 'https://winfsp.dev'
    Write-Host ''
    $ans = Read-Host '  Press Enter once WinFSP is installed (or type "skip" to continue anyway)'
    if ($ans -ne 'skip') {
        $winfspFound = $winfspPaths | Where-Object { Test-Path $_ } | Select-Object -First 1
        if ($winfspFound) {
            OK "WinFSP detected at $winfspFound"
        } else {
            Warn "Still didn't find WinFSP. Continuing anyway -- you'll need to install it before mounting."
        }
    }
}

# 2. Copy binaries to install dir ------------------------------------------
Section 'Copying files'

$required = @('oubliette.exe', 'oubliette-gui.exe')

# Search paths, in order of preference:
#   1. Next to install.ps1 (a distributed bundle)
#   2. ..\target\release\ (built from source with cargo build --release)
#   3. ..\target\debug\   (built from source with cargo build)
$searchDirs = @(
    $ScriptDir,
    (Join-Path $ScriptDir '..\target\release'),
    (Join-Path $ScriptDir '..\target\debug')
)

$sourceDir = $null
foreach ($dir in $searchDirs) {
    $allHere = $true
    foreach ($f in $required) {
        if (-not (Test-Path (Join-Path $dir $f))) { $allHere = $false; break }
    }
    if ($allHere) {
        $sourceDir = (Resolve-Path $dir).Path
        break
    }
}

if (-not $sourceDir) {
    Write-Host ''
    Write-Host '  ERROR: oubliette.exe and oubliette-gui.exe not found.' -ForegroundColor Red
    Write-Host '  Searched:' -ForegroundColor Red
    foreach ($dir in $searchDirs) { Write-Host "      $dir" -ForegroundColor Red }
    Write-Host ''
    Write-Host '  If you cloned the repo, build them first:' -ForegroundColor Yellow
    Write-Host '      cargo build --release' -ForegroundColor White
    Write-Host '  ...then re-run this installer.' -ForegroundColor Yellow
    exit 1
}

Info "Using binaries from: $sourceDir"

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null

foreach ($f in $required) {
    $src = Join-Path $sourceDir $f
    $dst = Join-Path $InstallDir $f
    Copy-Item -Force $src $dst
    OK "$f"
}

# Bundle WinFSP DLL next to the binaries if we found one
if ($winfspFound) {
    $dllDst = Join-Path $InstallDir 'winfsp-x64.dll'
    if (-not (Test-Path $dllDst)) {
        Copy-Item -Force $winfspFound $dllDst
        OK 'winfsp-x64.dll (from WinFSP install)'
    }
}

# Or copy a sibling DLL if one was shipped alongside install.ps1
$siblingDll = Join-Path $ScriptDir 'winfsp-x64.dll'
if ((Test-Path $siblingDll) -and -not (Test-Path (Join-Path $InstallDir 'winfsp-x64.dll'))) {
    Copy-Item -Force $siblingDll (Join-Path $InstallDir 'winfsp-x64.dll')
    OK 'winfsp-x64.dll (from installer bundle)'
}

# 3. Start Menu shortcut ---------------------------------------------------
Section 'Start Menu shortcut'

$wshell = New-Object -ComObject WScript.Shell
$shortcutPath = Join-Path $StartMenuDir "$AppName.lnk"
$sc = $wshell.CreateShortcut($shortcutPath)
$sc.TargetPath = Join-Path $InstallDir 'oubliette-gui.exe'
$sc.WorkingDirectory = $InstallDir
$sc.Description = "$AppName -- Discord-backed encrypted drive"
$sc.IconLocation = (Join-Path $InstallDir 'oubliette-gui.exe') + ',0'
$sc.Save()
OK "Created: $shortcutPath"

# 4. Desktop shortcut (optional) -------------------------------------------
$ans = Read-Host "  Also add a desktop shortcut? [y/N]"
if ($ans -match '^[yY]') {
    $deskShort = Join-Path $DesktopDir "$AppName.lnk"
    $sc2 = $wshell.CreateShortcut($deskShort)
    $sc2.TargetPath = Join-Path $InstallDir 'oubliette-gui.exe'
    $sc2.WorkingDirectory = $InstallDir
    $sc2.Description = "$AppName -- Discord-backed encrypted drive"
    $sc2.IconLocation = (Join-Path $InstallDir 'oubliette-gui.exe') + ',0'
    $sc2.Save()
    OK "Created: $deskShort"
}

# 5. Write uninstaller -----------------------------------------------------
Section 'Writing uninstaller'

$uninstallScript = @"
`$ErrorActionPreference = 'SilentlyContinue'

Write-Host '$AppName uninstaller' -ForegroundColor Cyan
Write-Host ''
Write-Host 'This will remove:' -ForegroundColor Gray
Write-Host '  - $InstallDir (the binaries)' -ForegroundColor Gray
Write-Host '  - Start Menu and Desktop shortcuts' -ForegroundColor Gray
Write-Host ''
Write-Host 'It WILL NOT remove:' -ForegroundColor Yellow
Write-Host '  - Your Discord channels (delete them manually if desired)' -ForegroundColor Yellow
Write-Host '  - Your config at `$env:APPDATA\oubliette' -ForegroundColor Yellow
Write-Host '  - Your local cache' -ForegroundColor Yellow
Write-Host ''
`$ans = Read-Host 'Continue? [y/N]'
if (`$ans -notmatch '^[yY]') { exit }

# Try to kill any running processes
Get-Process oubliette,oubliette-gui -ErrorAction SilentlyContinue | Stop-Process -Force

Remove-Item -Force '$shortcutPath' -ErrorAction SilentlyContinue
Remove-Item -Force '$(Join-Path $DesktopDir "$AppName.lnk")' -ErrorAction SilentlyContinue
Remove-Item -Recurse -Force '$InstallDir' -ErrorAction SilentlyContinue

Write-Host ''
Write-Host 'Uninstalled.' -ForegroundColor Green
Read-Host 'Press Enter to close'
"@

Set-Content -Path (Join-Path $InstallDir 'uninstall.ps1') -Value $uninstallScript -Encoding UTF8
OK "uninstall.ps1 -- run it to remove the app later"

# 6. Done ------------------------------------------------------------------
Section 'All done'
Write-Host ''
Write-Host '  Installed to:' -ForegroundColor Gray
Write-Host "      $InstallDir" -ForegroundColor White
Write-Host ''
Write-Host "  Launch via Start Menu -> $AppName" -ForegroundColor Gray
Write-Host '  Or run:' -ForegroundColor Gray
Write-Host "      $(Join-Path $InstallDir 'oubliette-gui.exe')" -ForegroundColor White
Write-Host ''

$ans = Read-Host "  Launch $AppName now? [Y/n]"
if ($ans -notmatch '^[nN]') {
    Start-Process (Join-Path $InstallDir 'oubliette-gui.exe')
}
