# Syntaur installer for Windows — https://syntaur.dev
# Usage: irm https://get.syntaur.dev/install.ps1 | iex
#Requires -Version 5.1
$ErrorActionPreference = "Stop"

$Brand = "Syntaur"
$Version = "0.1.0"
$Binary = "syntaur.exe"
$InstallDir = "$env:LOCALAPPDATA\Syntaur"
$DashboardUrl = "http://localhost:18789"

Write-Host ""
Write-Host "  $([char]0x265E) $Brand v$Version"
Write-Host "  Your personal AI platform"
Write-Host ""

# Detect architecture
$Arch = if ([Environment]::Is64BitOperatingSystem) { "x86_64" } else {
    Write-Host "Error: 32-bit Windows is not supported." -ForegroundColor Red
    exit 1
}

Write-Host "  Platform: windows-$Arch"
Write-Host ""

# Create install directory
if (-not (Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
}

# Download binary
$DownloadUrl = "https://github.com/buddyholly007/syntaur/releases/download/v$Version/syntaur-windows-$Arch.exe"
$BinaryPath = Join-Path $InstallDir $Binary

Write-Host "  Downloading $Brand..."
try {
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    Invoke-WebRequest -Uri $DownloadUrl -OutFile $BinaryPath -UseBasicParsing
} catch {
    Write-Host ""
    Write-Host "  Note: Download server not yet available." -ForegroundColor Yellow
    Write-Host "  For now, copy the binary manually to $BinaryPath"
    Write-Host "  Then run: $Binary"
    Write-Host ""
    exit 0
}

# Download viewer (lightweight dashboard window — no full browser needed)
$ViewerBinary = "syntaur-viewer.exe"
$ViewerPath = Join-Path $InstallDir $ViewerBinary
$ViewerUrl = "https://github.com/buddyholly007/syntaur/releases/download/v$Version/syntaur-viewer-windows-$Arch.exe"

Write-Host "  Downloading dashboard viewer..."
try {
    Invoke-WebRequest -Uri $ViewerUrl -OutFile $ViewerPath -UseBasicParsing
    Write-Host "  Viewer installed"
} catch {
    Write-Host "  Viewer download not available — shortcuts will open in browser" -ForegroundColor Yellow
}

# Determine shortcut target: use viewer if available, otherwise URL
if (Test-Path $ViewerPath) {
    $ShortcutTarget = $ViewerPath
    $ShortcutWorkDir = $InstallDir
} else {
    $ShortcutTarget = $DashboardUrl
    $ShortcutWorkDir = ""
}

# Add to PATH if not already there
$UserPath = [Environment]::GetEnvironmentVariable("PATH", "User")
if ($UserPath -notlike "*$InstallDir*") {
    Write-Host "  Adding $InstallDir to PATH..."
    [Environment]::SetEnvironmentVariable("PATH", "$InstallDir;$UserPath", "User")
    $env:PATH = "$InstallDir;$env:PATH"
}

# --- Create Start Menu shortcut ---
$StartMenuDir = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs"
$StartMenuShortcut = Join-Path $StartMenuDir "Syntaur.lnk"

$WshShell = New-Object -ComObject WScript.Shell

$Shortcut = $WshShell.CreateShortcut($StartMenuShortcut)
$Shortcut.TargetPath = $ShortcutTarget
if ($ShortcutWorkDir) { $Shortcut.WorkingDirectory = $ShortcutWorkDir }
$Shortcut.IconLocation = "$BinaryPath,0"
$Shortcut.Description = "Syntaur - Your personal AI platform"
$Shortcut.Save()

Write-Host "  Start Menu shortcut installed"

# --- Create Desktop shortcut ---
$DesktopShortcut = Join-Path ([Environment]::GetFolderPath("Desktop")) "Syntaur.lnk"

$Shortcut = $WshShell.CreateShortcut($DesktopShortcut)
$Shortcut.TargetPath = $ShortcutTarget
if ($ShortcutWorkDir) { $Shortcut.WorkingDirectory = $ShortcutWorkDir }
$Shortcut.IconLocation = "$BinaryPath,0"
$Shortcut.Description = "Syntaur - Your personal AI platform"
$Shortcut.Save()

Write-Host "  Desktop shortcut installed"

# --- Auto-start via Startup folder ---
$StartupDir = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Startup"
$StartupShortcut = Join-Path $StartupDir "Syntaur Service.lnk"

$Shortcut = $WshShell.CreateShortcut($StartupShortcut)
$Shortcut.TargetPath = $BinaryPath
$Shortcut.WorkingDirectory = $InstallDir
$Shortcut.WindowStyle = 7  # Minimized
$Shortcut.Description = "Syntaur AI Platform - background service"
$Shortcut.Save()

Write-Host "  Auto-start configured (runs at login)"

# Clean up COM object
[System.Runtime.Interopservices.Marshal]::ReleaseComObject($WshShell) | Out-Null

Write-Host ""
Write-Host "  $([char]0x2713) $Brand installed to $BinaryPath" -ForegroundColor Green
Write-Host ""
Write-Host "  To start now:"
Write-Host "    Start-Process '$BinaryPath'"
Write-Host ""
Write-Host "  Then open Syntaur from the Start Menu or Desktop shortcut, or go to:"
Write-Host "    $DashboardUrl"
Write-Host ""
