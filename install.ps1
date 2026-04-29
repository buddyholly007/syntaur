# Syntaur installer for Windows — https://syntaur.dev
# Usage:
#   irm https://get.syntaur.dev/install.ps1 | iex               # interactive
#   irm https://get.syntaur.dev/install.ps1 | iex -Args --server # server mode
#   irm https://get.syntaur.dev/install.ps1 | iex -Args --connect # viewer only
#Requires -Version 5.1
$ErrorActionPreference = "Stop"

$Brand = "Syntaur"
# MUST match the VERSION file at repo root. Run scripts/sync-version.sh
# before tagging a release so this and install.sh stay in sync with the
# workspace version in Cargo.toml. install.ps1 ships standalone.
$Version = "0.5.9"
$Binary = "syntaur.exe"
$InstallDir = "$env:LOCALAPPDATA\Syntaur"
$DashboardUrl = "http://localhost:18789"

Write-Host ""
Write-Host "  $([char]0x265E) $Brand v$Version"
Write-Host "  Your personal AI platform"
Write-Host ""

# Parse mode
$Mode = ""
if ($args -contains "--server") { $Mode = "server" }
if ($args -contains "--connect") { $Mode = "connect" }

if (-not $Mode) {
    Write-Host "  How would you like to use Syntaur?"
    Write-Host ""
    Write-Host "  1) Run the server on this computer"
    Write-Host "     Your AI runs here. Access from phone, laptop, any device."
    Write-Host "     (This computer needs to stay on.)"
    Write-Host ""
    Write-Host "  2) Connect to my Syntaur server"
    Write-Host "     Syntaur is already running elsewhere. Just install the viewer."
    Write-Host ""
    $Choice = Read-Host "  Choose [1/2]"
    $Mode = if ($Choice -eq "2") { "connect" } else { "server" }
    Write-Host ""
}

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

$BinaryPath = Join-Path $InstallDir $Binary

# Download gateway binary (server mode only)
if ($Mode -eq "server") {
    $DownloadUrl = "https://github.com/buddyholly007/syntaur/releases/download/v$Version/syntaur-windows-$Arch.exe"
    Write-Host "  Downloading $Brand server..."
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

# --- URL shortcut on Desktop (opens in default browser) ---
# Parallel to the .lnk above but triggers the system browser instead of the
# viewer app. Users who prefer their real browser (saved logins, extensions)
# double-click this one. Only created when SYNTAUR_URL is set (remote
# gateway case); local installs don't need it.
if ($env:SYNTAUR_URL) {
    $UrlShortcutPath = Join-Path ([Environment]::GetFolderPath("Desktop")) "Syntaur (Browser).url"
    @"
[InternetShortcut]
URL=$env:SYNTAUR_URL
IconIndex=0
"@ | Out-File -FilePath $UrlShortcutPath -Encoding ASCII
    Write-Host "  Browser shortcut on Desktop: Syntaur (Browser).url"
}

# --- Tailscale auto-setup (Tier 2 onboarding) ---
# If the caller passed a Tailscale pre-auth key via SYNTAUR_TS_AUTHKEY
# (the personalized-invite path mints one and bakes it into the command
# you send to a family member), bring Tailscale up on this machine so the
# viewer can reach the household gateway the moment it launches.
if ($env:SYNTAUR_TS_AUTHKEY) {
    Write-Host ""
    Write-Host "  Setting up Tailscale..."

    $TailscaleInstalled = $false
    $TsPath = "${env:ProgramFiles}\Tailscale\tailscale.exe"
    if (Test-Path $TsPath) {
        $TailscaleInstalled = $true
    } else {
        # Use Tailscale's official MSI installer. Silent install requires
        # admin — if the user isn't admin, fall back to opening the
        # download page and leave the join step for the viewer's
        # onboarding screen to detect.
        $TsMsiUrl = "https://pkgs.tailscale.com/stable/tailscale-setup-latest.msi"
        $TsMsiPath = Join-Path $env:TEMP "tailscale-setup.msi"
        Write-Host "  Downloading Tailscale installer..."
        try {
            Invoke-WebRequest -Uri $TsMsiUrl -OutFile $TsMsiPath -UseBasicParsing
            # /qb = basic UI (progress bar only). Users without admin rights
            # will see UAC prompt here; that's unavoidable for any Windows
            # system install.
            $msi = Start-Process msiexec.exe -ArgumentList "/i `"$TsMsiPath`" /qb" -Wait -PassThru
            if ($msi.ExitCode -eq 0 -and (Test-Path $TsPath)) {
                $TailscaleInstalled = $true
            }
        } catch {
            Write-Host "  ! Tailscale install failed. Open https://tailscale.com/download/windows to install manually." -ForegroundColor Yellow
        }
    }

    if ($TailscaleInstalled) {
        try {
            & $TsPath up --authkey="$env:SYNTAUR_TS_AUTHKEY" --accept-routes
            Write-Host "  $([char]0x2713) Tailscale connected to your household tailnet" -ForegroundColor Green
        } catch {
            Write-Host "  ! tailscale up failed. You can retry:" -ForegroundColor Yellow
            Write-Host "    `"$TsPath`" up --authkey=`$env:SYNTAUR_TS_AUTHKEY --accept-routes"
        }
    }
}

# --- Auto-start via Startup folder (server mode only) ---
if ($Mode -eq "server") {
$StartupDir = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Startup"
$StartupShortcut = Join-Path $StartupDir "Syntaur Service.lnk"

$Shortcut = $WshShell.CreateShortcut($StartupShortcut)
$Shortcut.TargetPath = $BinaryPath
$Shortcut.WorkingDirectory = $InstallDir
$Shortcut.WindowStyle = 7  # Minimized
$Shortcut.Description = "Syntaur AI Platform - background service"
$Shortcut.Save()

Write-Host "  Auto-start configured (runs at login)"
} # end server-only auto-start

# Clean up COM object
[System.Runtime.Interopservices.Marshal]::ReleaseComObject($WshShell) | Out-Null

Write-Host ""
if ($Mode -eq "server") {
    Write-Host "  $([char]0x2713) $Brand server installed" -ForegroundColor Green
    Write-Host ""
    Write-Host "  To start now:"
    Write-Host "    Start-Process '$BinaryPath'"
    Write-Host ""
    Write-Host "  Open Syntaur from the Start Menu or Desktop shortcut, or go to:"
    Write-Host "    $DashboardUrl"
    Write-Host ""
    Write-Host "  To access from your phone or other computers:"
    Write-Host "    1. Install Tailscale on this computer and your other devices"
    Write-Host "    2. Open the Tailscale URL shown in the Syntaur dashboard"
} else {
    Write-Host "  $([char]0x2713) $Brand viewer installed" -ForegroundColor Green
    Write-Host ""
    Write-Host "  Open Syntaur from the Start Menu to connect to your server."
    Write-Host "  The setup wizard will ask for your server address."
}
Write-Host ""
