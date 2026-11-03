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

$RunId = [guid]::NewGuid().ToString("N")
$Root = Join-Path $env:TEMP ("crosshair-windows-smoke-" + $RunId)
$LogDirectory = if ([string]::IsNullOrWhiteSpace($env:RUNNER_TEMP)) { $env:TEMP } else { $env:RUNNER_TEMP }
$DiagnosticLog = Join-Path $LogDirectory ("crosshair-windows-smoke-" + $RunId + ".log")
$env:APPDATA = Join-Path $Root "roaming"
$env:LOCALAPPDATA = Join-Path $Root "local"
$Package = Join-Path $Root "package"
New-Item -ItemType Directory -Force -Path $Package, $env:APPDATA, $env:LOCALAPPDATA | Out-Null
Copy-Item -LiteralPath $SourceCli, $SourceSettings, $InstallScript, $UninstallScript, $Readme -Destination $Package

# Simulate a prior release with an explicit login-startup preference.
$RunKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
New-Item -Path $RunKey -Force | Out-Null
Set-ItemProperty -LiteralPath $RunKey -Name "Crosshair" -Value ('"' + $SourceCli + '" --daemon --no-ui')

$InstallDir = Join-Path $env:LOCALAPPDATA "Programs\Crosshair"
$Bin = Join-Path $InstallDir "crosshair.exe"
$Settings = Join-Path $InstallDir "crosshair-settings.exe"
$StartMenuShortcut = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Crosshair.lnk"
$RunKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
$ConfigPath = Join-Path $env:APPDATA "Crosshair\config.json"

function Write-SmokeLog {
    param([Parameter(Mandatory = $true)][string] $Message)
    $line = "[{0:O}] {1}" -f [DateTime]::UtcNow, $Message
    Add-Content -LiteralPath $DiagnosticLog -Value $line
    Write-Host $line
}

Write-SmokeLog "Starting Windows installer/overlay/Settings/uninstall smoke test."

function Invoke-Crosshair {
    param(
        [Parameter(ValueFromRemainingArguments = $true)][string[]] $Arguments,
        [int] $TimeoutMilliseconds = 10000
    )
    $label = "crosshair " + ($Arguments -join " ")
    Write-SmokeLog ("CLI START: " + $label)
    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $Bin
    $startInfo.WorkingDirectory = $InstallDir
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    foreach ($argument in $Arguments) {
        $startInfo.ArgumentList.Add($argument)
    }
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    if (-not $process.Start()) {
        throw ("Could not start " + $label)
    }
    $stdoutTask = $process.StandardOutput.ReadToEndAsync()
    $stderrTask = $process.StandardError.ReadToEndAsync()
    if (-not $process.WaitForExit($TimeoutMilliseconds)) {
        try { $process.Kill($true) } catch {}
        try { $process.WaitForExit(2000) | Out-Null } catch {}
        $timeoutStdout = ""
        $timeoutStderr = ""
        try { $timeoutStdout = $stdoutTask.GetAwaiter().GetResult() } catch {}
        try { $timeoutStderr = $stderrTask.GetAwaiter().GetResult() } catch {}
        Write-SmokeLog ("CLI TIMEOUT: " + $label + " exceeded " + $TimeoutMilliseconds + " ms; process tree was terminated.")
        if (-not [string]::IsNullOrWhiteSpace($timeoutStdout)) {
            Write-SmokeLog ("CLI TIMEOUT STDOUT: " + $timeoutStdout.Trim())
        }
        if (-not [string]::IsNullOrWhiteSpace($timeoutStderr)) {
            Write-SmokeLog ("CLI TIMEOUT STDERR: " + $timeoutStderr.Trim())
        }
        $process.Dispose()
        throw ($label + " timed out after " + $TimeoutMilliseconds + " ms.")
    }
    $process.WaitForExit()
    $stdout = $stdoutTask.GetAwaiter().GetResult()
    $stderr = $stderrTask.GetAwaiter().GetResult()
    $exitCode = $process.ExitCode
    $process.Dispose()
    $text = ($stdout + $stderr).Trim()
    Write-SmokeLog ("CLI EXIT: " + $label + " (exit " + $exitCode + ")")
    if ($exitCode -ne 0) {
        throw ($label + " failed (" + $exitCode + "): " + $text)
    }
    return $stdout
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
        try {
            $quit = Start-Process -FilePath $Bin -ArgumentList "--quit" -WindowStyle Hidden -PassThru
            if (-not $quit.WaitForExit(5000)) {
                Write-SmokeLog "Cleanup: --quit did not return within five seconds; stopping its CLI process."
                Stop-Process -Id $quit.Id -Force -ErrorAction SilentlyContinue
            }
        } catch {
            Write-SmokeLog ("Cleanup: --quit request could not complete: " + $_.Exception.Message)
        }
        Get-Process -Name "crosshair" -ErrorAction SilentlyContinue |
            Where-Object { $_.Path -eq $Bin } |
            Stop-Process -Force -ErrorAction SilentlyContinue
    }
    Get-Process -Name "crosshair-settings" -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -eq $Settings } |
        Stop-Process -Force -ErrorAction SilentlyContinue
}

try {
    Write-SmokeLog "[1/9] install the same per-user package intended for the laptop"
    & (Join-Path $Package "Install-Crosshair.ps1")
    if ($LASTEXITCODE -ne 0) { throw "Windows installer script failed." }
    if (-not (Test-Path -LiteralPath $Bin -PathType Leaf) -or
        -not (Test-Path -LiteralPath $Settings -PathType Leaf)) {
        throw "Installer did not deploy both native executables."
    }
    $version = Invoke-Crosshair -Arguments @("--version")
    if ($version -notmatch "Crosshair 4\.0\.0") { throw "Unexpected installed version: $version" }
    $migratedRun = (Get-ItemProperty -LiteralPath $RunKey -Name "Crosshair").Crosshair
    $normalizedRun = $migratedRun.Replace("/", "\")
    $normalizedBin = $Bin.Replace("/", "\")
    $pathMatches = $normalizedRun.IndexOf($normalizedBin, [StringComparison]::OrdinalIgnoreCase) -ge 0
    if (-not $pathMatches -or
        $normalizedRun -notmatch "--daemon" -or
        $normalizedRun -notmatch "-WindowStyle Hidden") {
        throw ("Installer did not preserve and modernize the previous login-startup preference. Actual Run value: " + $migratedRun)
    }
    if (-not (Test-Path -LiteralPath $StartMenuShortcut -PathType Leaf)) {
        throw "Installer did not create the Start Menu shortcut."
    }
    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut($StartMenuShortcut)
    $actualShortcutTarget = [IO.Path]::GetFullPath(([string]$shortcut.TargetPath).Replace("/", "\"))
    $expectedShortcutTarget = [IO.Path]::GetFullPath(([string]$Settings).Replace("/", "\"))
    $shortcutTargetMatches = [string]::Equals(
        $actualShortcutTarget,
        $expectedShortcutTarget,
        [StringComparison]::OrdinalIgnoreCase
    )
    $shortcutArguments = [string]$shortcut.Arguments
    if (-not $shortcutTargetMatches -or -not [string]::IsNullOrEmpty($shortcutArguments)) {
        throw ("Start Menu shortcut target mismatch. Actual: '$actualShortcutTarget'; expected: '$expectedShortcutTarget'; arguments: '$shortcutArguments'.")
    }

    Write-SmokeLog "[2/9] real monitor discovery and opt-in autostart registry control"
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
    # Rust PathBuf and PowerShell Join-Path can render the same Windows path with different separators.
    $normalizedRun = $runValue.Replace("/", "\")
    $normalizedBin = $Bin.Replace("/", "\")
    $autostartPathMatches = $normalizedRun.IndexOf($normalizedBin, [StringComparison]::OrdinalIgnoreCase) -ge 0
    if (-not $autostartPathMatches -or
        $normalizedRun -notmatch "--daemon" -or
        $normalizedRun -notmatch "-WindowStyle Hidden") {
        throw ("Opt-in login startup registry value is not configured for the hidden Rust daemon. Actual Run value: " + $runValue)
    }
    $null = Invoke-Crosshair -Arguments @("--remove-autostart")
    $runProperties = Get-ItemProperty -LiteralPath $RunKey -ErrorAction SilentlyContinue
    if ($null -ne $runProperties -and $runProperties.PSObject.Properties.Name -contains "Crosshair") {
        throw "Remove autostart left a Crosshair Run registry value."
    }

    Write-SmokeLog "[3/9] create isolated enabled config and cold-start overlay"
    $null = Invoke-Crosshair -Arguments @("--set", '{"enabled":true,"shape":"dot","size":9}')
    $null = Invoke-Crosshair -Arguments @("--show")
    $status = Wait-ForStatus "Overlay: enabled"
    if (-not $status.Contains("Crosshair daemon: running")) {
        throw "Overlay status did not report a running daemon: $status"
    }

    Write-SmokeLog "[4/9] persisted settings update through named pipe"
    $null = Invoke-Crosshair -Arguments @("--set", '{"enabled":true,"shape":"cross","size":31,"opacity":0.72}')
    $saved = Get-Content -LiteralPath $ConfigPath -Raw | ConvertFrom-Json
    if ($saved.shape -ne "cross" -or $saved.size -ne 31) {
        throw "Named-pipe settings update was not persisted correctly."
    }
    $status = Wait-ForStatus "Overlay: enabled"

    Write-SmokeLog "[5/9] open Settings through the same command used by the Start Menu"
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

    Write-SmokeLog "[6/9] close Settings and verify overlay lifetime is independent"
    $null = $settingsProcess.CloseMainWindow()
    if (-not $settingsProcess.WaitForExit(8000)) {
        throw "Settings did not exit after its window-close request."
    }
    $status = Wait-ForStatus "Overlay: enabled"
    if (-not $status.Contains("Crosshair daemon: running")) {
        throw "Closing Settings unexpectedly stopped the overlay."
    }

    Write-SmokeLog "[7/9] hide removes the overlay and exits daemon"
    $null = Invoke-Crosshair -Arguments @("--hide")
    $status = Wait-ForStatus "Crosshair daemon: not running"
    $saved = Get-Content -LiteralPath $ConfigPath -Raw | ConvertFrom-Json
    if ($saved.enabled) { throw "Hide did not persist enabled=false." }

    Write-SmokeLog "[8/9] cold toggle starts, then toggle-off shuts down again"
    $null = Invoke-Crosshair -Arguments @("--toggle")
    $null = Wait-ForStatus "Overlay: enabled"
    $null = Invoke-Crosshair -Arguments @("--toggle")
    $status = Wait-ForStatus "Crosshair daemon: not running"
    if ($status -notmatch "Overlay setting: disabled") {
        throw "Toggle-off did not leave the overlay disabled: $status"
    }

    Write-SmokeLog "[9/9] uninstall through the installed CLI from its own working directory"
    Write-SmokeLog "Launching --uninstall; the deferred helper must remove the install folder after the CLI exits."
    $uninstallProcess = Start-Process -FilePath $Bin -ArgumentList "--uninstall" -WorkingDirectory $InstallDir -WindowStyle Hidden -PassThru
    if (-not $uninstallProcess.WaitForExit(10000)) {
        throw "The installed CLI did not return from --uninstall within ten seconds."
    }
    if ($uninstallProcess.ExitCode -ne 0) {
        throw "The in-app uninstall command failed with exit code $($uninstallProcess.ExitCode)."
    }
    Write-SmokeLog "The CLI exited; waiting up to 30 seconds for the deferred uninstaller."
    $uninstallDeadline = [DateTime]::UtcNow.AddSeconds(30)
    while ([DateTime]::UtcNow -lt $uninstallDeadline -and (Test-Path -LiteralPath $InstallDir)) {
        Start-Sleep -Milliseconds 100
    }
    if (Test-Path -LiteralPath $InstallDir) {
        throw "The deferred installer did not remove the installation directory after the CLI exited."
    }
    if ((Test-Path -LiteralPath $Bin -PathType Leaf) -or
        (Test-Path -LiteralPath $Settings -PathType Leaf) -or
        (Test-Path -LiteralPath $StartMenuShortcut -PathType Leaf)) {
        throw "Uninstaller left an installed binary or Start Menu shortcut."
    }
    if (-not (Test-Path -LiteralPath $ConfigPath -PathType Leaf)) {
        throw "Uninstaller removed user configuration."
    }
    Write-SmokeLog "PASS: Windows install, monitor discovery, named-pipe preview/set, layered overlay, Settings close, autostart toggle, hide/toggle-off, and uninstall."
}
finally {
    Write-SmokeLog "Entering bounded test cleanup."
    Stop-Crosshair
    try { Remove-ItemProperty -LiteralPath $RunKey -Name "Crosshair" -ErrorAction SilentlyContinue } catch {}
    if (Test-Path -LiteralPath $Bin -PathType Leaf) {
        Write-SmokeLog "The isolated installation remains; removing the temporary test root rather than invoking a second uninstaller."
    }
    Remove-Item -LiteralPath $Root -Recurse -Force -ErrorAction SilentlyContinue
    Write-SmokeLog ("Cleanup finished. Diagnostic log: " + $DiagnosticLog)
}
