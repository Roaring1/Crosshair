[CmdletBinding()]
param(
    [int] $WaitForProcessId = 0
)

$ErrorActionPreference = "Stop"
if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA) -or
    [string]::IsNullOrWhiteSpace($env:APPDATA)) {
    throw "This user session does not expose LOCALAPPDATA and APPDATA."
}
$InstallDir = Join-Path $env:LOCALAPPDATA "Programs\Crosshair"
$InstalledCli = Join-Path $InstallDir "crosshair.exe"
$InstalledSettings = Join-Path $InstallDir "crosshair-settings.exe"
$StartMenuShortcut = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Crosshair.lnk"
$SettingsShortcut = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Crosshair Settings.lnk"
$RunKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"

if (Test-Path -LiteralPath $InstalledCli -PathType Leaf) {
    try { & $InstalledCli --quit *> $null } catch {}
}

# When launched by the in-app Uninstall action, the CLI cannot remove its
# own executable. Wait for that CLI and for the Settings window to close.
if ($WaitForProcessId -gt 0) {
    try { Wait-Process -Id $WaitForProcessId -Timeout 45 -ErrorAction SilentlyContinue } catch {}
}
# The tray controller shares the CLI executable but is independent from the
# overlay daemon; stop only matching installed processes before file removal.
Get-Process -Name "crosshair" -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -eq $InstalledCli -and $_.Id -ne $WaitForProcessId } |
    Stop-Process -Force -ErrorAction SilentlyContinue
$deadline = [DateTime]::UtcNow.AddMinutes(2)
while ([DateTime]::UtcNow -lt $deadline) {
    $cliRunning = Get-Process -Name "crosshair" -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -eq $InstalledCli }
    $settingsRunning = Get-Process -Name "crosshair-settings" -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -eq $InstalledSettings }
    if (-not $cliRunning -and -not $settingsRunning) { break }
    Start-Sleep -Milliseconds 200
}
$cliRunning = Get-Process -Name "crosshair" -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -eq $InstalledCli }
$settingsRunning = Get-Process -Name "crosshair-settings" -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -eq $InstalledSettings }
if ($cliRunning -or $settingsRunning) {
    throw "Close Crosshair and Settings before uninstalling; the running app files were left in place."
}

Remove-Item -LiteralPath $StartMenuShortcut -Force -ErrorAction SilentlyContinue
Remove-Item -LiteralPath $SettingsShortcut -Force -ErrorAction SilentlyContinue
Remove-ItemProperty -LiteralPath $RunKey -Name "Crosshair" -Force -ErrorAction SilentlyContinue

$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
$entries = @($userPath -split ";" | Where-Object {
    -not [string]::IsNullOrWhiteSpace($_) -and
    $_.TrimEnd("\") -ine $InstallDir.TrimEnd("\")
})
[Environment]::SetEnvironmentVariable("Path", ($entries -join ";"), "User")

# Config/logs remain in %APPDATA%\Crosshair and %LOCALAPPDATA%\Crosshair.
Remove-Item -LiteralPath $InstallDir -Recurse -Force -ErrorAction SilentlyContinue
Write-Host "Crosshair binaries, Start Menu shortcuts, autostart and PATH entry removed. User configuration was retained."
