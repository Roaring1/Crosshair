$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path -Parent $PSScriptRoot
$SourceCli = Join-Path $RepoRoot "target\release\crosshair.exe"
$SourceSettings = Join-Path $RepoRoot "target\release\crosshair-settings.exe"
$InstallScript = Join-Path $RepoRoot "packaging\windows\Install-Crosshair.ps1"
$UninstallScript = Join-Path $RepoRoot "packaging\windows\Uninstall-Crosshair.ps1"
$Readme = Join-Path $RepoRoot "packaging\windows\README-WINDOWS.txt"
foreach ($path in @($SourceCli, $SourceSettings, $InstallScript, $UninstallScript, $Readme)) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Windows smoke package input is missing: $path"
    }
}

$Root = Join-Path $env:TEMP ("crosshair-windows-smoke-" + [guid]::NewGuid().ToString("N"))
$env:APPDATA = Join-Path $Root "roaming"
$env:LOCALAPPDATA = Join-Path $Root "local"
$Package = Join-Path $Root "package"
New-Item -ItemType Directory -Force -Path $Package, $env:APPDATA, $env:LOCALAPPDATA | Out-Null
Copy-Item -LiteralPath $SourceCli, $SourceSettings, $InstallScript, $UninstallScript, $Readme -Destination $Package

$InstallDir = Join-Path $env:LOCALAPPDATA "Programs\Crosshair"
$Bin = Join-Path $InstallDir "crosshair.exe"
$Settings = Join-Path $InstallDir "crosshair-settings.exe"
$StartMenuShortcut = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Crosshair.lnk"
$RunKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
$ConfigPath = Join-Path $env:APPDATA "Crosshair\config.json"

function Invoke-Crosshair {
    param([Parameter(ValueFromRemainingArguments = $true)][string[]] $Arguments)
    $output = & $Bin @Arguments 2>&1
    $exitCode = $LASTEXITCODE
    $text = ($output | ForEach-Object { "$_" }) -join [Environment]::NewLine
    if ($exitCode -ne 0) {
        throw ("crosshair " + ($Arguments -join ' ') + " failed (" + $exitCode + "): " + $text)
    }
    return $text
}

function Wait-ForStatus {
    param(
        [Parameter(Mandatory = $true)][string] $Expected,
        [int] $TimeoutSeconds = 12
    )
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $last = ""
    while ([DateTime]::UtcNow -lt $deadline) {
        try {
            $last = Invoke-Crosshair -Arguments @("--status")
            if ($last.Contains($Expected)) {
                return $last
            }
        } catch {
            $last = $_.Exception.Message
        }
        Start-Sleep -Milliseconds 100
    }
    throw "Timed out waiting for status '$Expected'. Last response: $last"
}

function Stop-Crosshair {
    if (Test-Path -LiteralPath $Bin -PathType Leaf) {
        try { & $Bin --quit *> $null } catch {}
        Get-Process -Name "crosshair" -ErrorAction SilentlyContinue |
            Where-Object { $_.Path -eq $Bin } |
            Stop-Process -Force -ErrorAction SilentlyContinue
    }
    Get-Process -Name "crosshair-settings" -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -eq $Settings } |
        Stop-Process -Force -ErrorAction SilentlyContinue
}

try {
    Write-Host "[1/9] install the same per-user package intended for the laptop"
    & (Join-Path $Package "Install-Crosshair.ps1")
    if ($LASTEXITCODE -ne 0) { throw "Windows installer script failed." }
    if (-not (Test-Path -LiteralPath $Bin -PathType Leaf) -or
        -not (Test-Path -LiteralPath $Settings -PathType Leaf)) {
        throw "Installer did not deploy both native executables."
    }
    $version = Invoke-Crosshair -Arguments @("--version")
    if ($version -notmatch "Crosshair 4\.0\.0") { throw "Unexpected installed version: $version" }
    if (-not (Test-Path -LiteralPath $StartMenuShortcut -PathType Leaf)) {
        throw "Installer did not create the Start Menu shortcut."
    }
    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut($StartMenuShortcut)
    if ($shortcut.TargetPath -ne $Bin -or $shortcut.Arguments -ne "--config") {
        throw "Start Menu shortcut does not target the installed Rust Settings launcher."
    }

    Write-Host "[2/9] real monitor discovery and opt-in autostart registry control"
    $monitorJson = Invoke-Crosshair -Arguments @("--list-monitors")
    $monitors = $monitorJson | ConvertFrom-Json
    if (@($monitors).Count -lt 1) { throw "Windows reported no active monitors." }
    foreach ($monitor in $monitors) {
        if ([string]::IsNullOrWhiteSpace($monitor.name)) {
            throw "Monitor enumeration returned an empty display name."
        }
    }
    $null = Invoke-Crosshair -Arguments @("--autostart")
    $runValue = (Get-ItemProperty -LiteralPath $RunKey -Name "Crosshair").Crosshair
    if ($runValue -notmatch [regex]::Escape($Bin) -or $runValue -notmatch "--daemon") {
        throw "Opt-in login startup did not point to the installed Rust daemon."
    }
    $null = Invoke-Crosshair -Arguments @("--remove-autostart")
    $runProperties = Get-ItemProperty -LiteralPath $RunKey -ErrorAction SilentlyContinue
    if ($null -ne $runProperties -and $runProperties.PSObject.Properties.Name -contains "Crosshair") {
        throw "Remove autostart left a Crosshair Run registry value."
    }

    Write-Host "[3/9] create isolated enabled config and cold-start overlay"
    $null = Invoke-Crosshair -Arguments @("--set", '{"enabled":true,"shape":"dot","size":9}')
    $null = Invoke-Crosshair -Arguments @("--show")
    $status = Wait-ForStatus "Overlay: enabled"
    if (-not $status.Contains("Crosshair daemon: running")) {
        throw "Overlay status did not report a running daemon: $status"
    }

    Write-Host "[4/9] persisted settings update through named pipe"
    $null = Invoke-Crosshair -Arguments @("--set", '{"enabled":true,"shape":"cross","size":31,"opacity":0.72}')
    $saved = Get-Content -LiteralPath $ConfigPath -Raw | ConvertFrom-Json
    if ($saved.shape -ne "cross" -or $saved.size -ne 31) {
        throw "Named-pipe settings update was not persisted correctly."
    }
    $status = Wait-ForStatus "Overlay: enabled"

    Write-Host "[5/9] open Settings through the same command used by the Start Menu"
    $before = @(Get-Process -Name "crosshair-settings" -ErrorAction SilentlyContinue |
        ForEach-Object { $_.Id })
    $null = Invoke-Crosshair -Arguments @("--config")
    $settingsProcess = $null
    $deadline = [DateTime]::UtcNow.AddSeconds(12)
    while ([DateTime]::UtcNow -lt $deadline -and $null -eq $settingsProcess) {
        $settingsProcess = Get-Process -Name "crosshair-settings" -ErrorAction SilentlyContinue |
            Where-Object { $_.Id -notin $before } |
            Select-Object -First 1
        if ($null -eq $settingsProcess) { Start-Sleep -Milliseconds 100 }
        else {
            $settingsProcess.Refresh()
            if ($settingsProcess.MainWindowHandle -eq 0) {
                $settingsProcess = $null
                Start-Sleep -Milliseconds 100
            }
        }
    }
    if ($null -eq $settingsProcess) {
        throw "The installed CLI did not open a visible sibling Settings window."
    }

    Write-Host "[6/9] close Settings and verify overlay lifetime is independent"
    $null = $settingsProcess.CloseMainWindow()
    if (-not $settingsProcess.WaitForExit(8000)) {
        throw "Settings did not exit after its window-close request."
    }
    $status = Wait-ForStatus "Overlay: enabled"
    if (-not $status.Contains("Crosshair daemon: running")) {
        throw "Closing Settings unexpectedly stopped the overlay."
    }

    Write-Host "[7/9] hide removes the overlay and exits daemon"
    $null = Invoke-Crosshair -Arguments @("--hide")
    $status = Wait-ForStatus "Crosshair daemon: not running"
    $saved = Get-Content -LiteralPath $ConfigPath -Raw | ConvertFrom-Json
    if ($saved.enabled) { throw "Hide did not persist enabled=false." }

    Write-Host "[8/9] cold toggle starts, then toggle-off shuts down again"
    $null = Invoke-Crosshair -Arguments @("--toggle")
    $null = Wait-ForStatus "Overlay: enabled"
    $null = Invoke-Crosshair -Arguments @("--toggle")
    $status = Wait-ForStatus "Crosshair daemon: not running"
    if ($status -notmatch "Overlay setting: disabled") {
        throw "Toggle-off did not leave the overlay disabled: $status"
    }

    Write-Host "[9/9] uninstall Rust binaries and preserve per-user configuration"
    & (Join-Path $Package "Uninstall-Crosshair.ps1")
    if ($LASTEXITCODE -ne 0) { throw "Windows uninstall script failed." }
    if ((Test-Path -LiteralPath $Bin -PathType Leaf) -or
        (Test-Path -LiteralPath $Settings -PathType Leaf) -or
        (Test-Path -LiteralPath $StartMenuShortcut -PathType Leaf)) {
        throw "Uninstaller left an installed binary or Start Menu shortcut."
    }
    if (-not (Test-Path -LiteralPath $ConfigPath -PathType Leaf)) {
        throw "Uninstaller removed user configuration."
    }
    Write-Host "PASS: Windows install, monitor discovery, named-pipe preview/set, layered overlay, Settings close, autostart toggle, hide/toggle-off, and uninstall."
}
finally {
    Stop-Crosshair
    try { Remove-ItemProperty -LiteralPath $RunKey -Name "Crosshair" -ErrorAction SilentlyContinue } catch {}
    if (Test-Path -LiteralPath $Bin -PathType Leaf) {
        & (Join-Path $Package "Uninstall-Crosshair.ps1") *> $null
    }
    Remove-Item -LiteralPath $Root -Recurse -Force -ErrorAction SilentlyContinue
}
