# Windows installer for syntaur-media-bridge.
# Registers a Scheduled Task that starts the bridge at logon.
#
# Usage:
#   Copy syntaur-media-bridge.exe to C:\Program Files\Syntaur\
#   Open PowerShell as the current user (not admin) and run:
#     .\install.ps1

$ErrorActionPreference = "Stop"

$BinName = "syntaur-media-bridge.exe"
$InstallDir = "$env:LOCALAPPDATA\Syntaur"
$BinPath = Join-Path $InstallDir $BinName

if (-not (Test-Path $BinPath)) {
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    if (Test-Path ".\$BinName") {
        Copy-Item ".\$BinName" $BinPath
    } else {
        Write-Error "$BinName not found. Place it next to this script or at $BinPath."
        exit 1
    }
}

$TaskName = "SyntaurMediaBridge"
$Action = New-ScheduledTaskAction -Execute $BinPath
$Trigger = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME
$Settings = New-ScheduledTaskSettingsSet `
    -StartWhenAvailable `
    -DontStopOnIdleEnd `
    -AllowStartIfOnBatteries `
    -DontStopIfGoingOnBatteries `
    -ExecutionTimeLimit ([TimeSpan]::Zero)
$Principal = New-ScheduledTaskPrincipal -UserId $env:USERNAME -LogonType Interactive

try {
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
} catch { }

Register-ScheduledTask `
    -TaskName $TaskName `
    -Action $Action `
    -Trigger $Trigger `
    -Settings $Settings `
    -Principal $Principal `
    -Description "Syntaur Media Bridge — local audio companion" | Out-Null

Start-ScheduledTask -TaskName $TaskName

Write-Host "[OK] syntaur-media-bridge installed at $BinPath"
Write-Host "     Scheduled task '$TaskName' registered (starts at logon)"
Write-Host ""
Write-Host "Next: log in to each music service once:"
Write-Host "  & '$BinPath' --auth-setup --auth-provider apple_music"
Write-Host "  & '$BinPath' --auth-setup --auth-provider spotify"
