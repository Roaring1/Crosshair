[CmdletBinding()]
param(
    [switch] $NoPathUpdate
)

$ErrorActionPreference = "Stop"
$PackageRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$SourceCli = Join-Path $PackageRoot "crosshair.exe"
$SourceSettings = Join-Path $PackageRoot "crosshair-settings.exe"
foreach ($source in @($SourceCli, $SourceSettings)) {
    if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
        throw "Package is missing $source."
    }
}
if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA) -or
    [string]::IsNullOrWhiteSpace($env:APPDATA)) {
    throw "This user session does not expose LOCALAPPDATA and APPDATA."
}

$InstallDir = Join-Path $env:LOCALAPPDATA "Programs\Crosshair"
$InstalledCli = Join-Path $InstallDir "crosshair.exe"
$StartMenu = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs"
$Shortcut = Join-Path $StartMenu "Crosshair.lnk"
$RunKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
$HadAutostart = $false
try {
    $oldStartup = Get-ItemProperty -LiteralPath $RunKey -Name "Crosshair" -ErrorAction Stop
    $HadAutostart = -not [string]::IsNullOrWhiteSpace($oldStartup.Crosshair)
} catch {}

# Close the previous daemon before replacing the native executables.
if (Test-Path -LiteralPath $InstalledCli -PathType Leaf) {
    try { & $InstalledCli --quit *> $null } catch {}
    $deadline = [DateTime]::UtcNow.AddSeconds(8)
    while ([DateTime]::UtcNow -lt $deadline) {
        $running = Get-Process -Name "crosshair" -ErrorAction SilentlyContinue |
            Where-Object { $_.Path -eq $InstalledCli }
        if (-not $running) { break }
        Start-Sleep -Milliseconds 100
    }
}

New-Item -ItemType Directory -Force -Path $InstallDir, $StartMenu | Out-Null

# The Rust installer copies the CLI and its sibling Settings executable.
& $SourceCli --install
if ($LASTEXITCODE -ne 0) {
    throw "Crosshair's native installer returned exit code $LASTEXITCODE."
}
$InstalledSettings = Join-Path $InstallDir "crosshair-settings.exe"
foreach ($installed in @($InstalledCli, $InstalledSettings)) {
    if (-not (Test-Path -LiteralPath $installed -PathType Leaf)) {
        throw "The native installer did not create $installed."
    }
}
$version = & $InstalledCli --version
if ($LASTEXITCODE -ne 0 -or $version -notmatch "Crosshair 4\.0\.0") {
    throw "Installed CLI version verification failed: $version"
}

# Keep the scripts alongside the app so Settings' Uninstall action can
# defer deletion until the running CLI and GUI processes have exited.
foreach ($file in @("Install-Crosshair.ps1", "Uninstall-Crosshair.ps1", "README-WINDOWS.txt")) {
    $packaged = Join-Path $PackageRoot $file
    if (Test-Path -LiteralPath $packaged -PathType Leaf) {
        Copy-Item -LiteralPath $packaged -Destination (Join-Path $InstallDir $file) -Force
    }
}

$shell = New-Object -ComObject WScript.Shell
$link = $shell.CreateShortcut($Shortcut)
$link.TargetPath = $InstalledSettings
$link.Arguments = ""
$link.WorkingDirectory = $InstallDir
$link.Description = "Configure the Crosshair overlay"
$link.IconLocation = "$InstalledCli,0"
$link.Save()

if (-not $NoPathUpdate) {
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $entries = @($userPath -split ";" | Where-Object {
        -not [string]::IsNullOrWhiteSpace($_) -and
        $_.TrimEnd("\") -ine $InstallDir.TrimEnd("\")
    })
    # Put the native Rust install first so a prior Python command cannot shadow it.
    $entries = @($InstallDir) + $entries
    [Environment]::SetEnvironmentVariable("Path", ($entries -join ";"), "User")
}

# Preserve a previous explicit login-startup choice, but never enable it
# on a fresh install.
if ($HadAutostart) {
    # Recreate the previous opt-in preference through the native CLI so the
    # registry receives the current hidden PowerShell launcher, not the old
    # direct console-subsystem command.
    & $InstalledCli --autostart
    if ($LASTEXITCODE -ne 0) {
        throw "Could not migrate the previous Crosshair login-startup preference."
    }
}

Write-Host "Crosshair 4.0.0 installed to $InstallDir"
Write-Host "Start Menu shortcut: $Shortcut"
if ($HadAutostart) {
    Write-Host "Previous login startup preference was migrated to the Rust executable."
} else {
    Write-Host "Login autostart remains disabled."
}
Write-Host "Existing user configuration and Python files were not removed."
