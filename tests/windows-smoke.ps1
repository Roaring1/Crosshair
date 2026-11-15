$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path -Parent $PSScriptRoot
$SourceCli = Join-Path $RepoRoot "target\release\crosshair.exe"
$SourceSettings = Join-Path $RepoRoot "target\release\crosshair-settings.exe"
$SourceIcon = Join-Path $RepoRoot "crosshair.ico"
$InstallScript = Join-Path $RepoRoot "packaging\windows\Install-Crosshair.ps1"
$UninstallScript = Join-Path $RepoRoot "packaging\windows\Uninstall-Crosshair.ps1"
$Readme = Join-Path $RepoRoot "packaging\windows\README-WINDOWS.txt"
foreach ($path in @($SourceCli, $SourceSettings, $SourceIcon, $InstallScript, $UninstallScript, $Readme)) {
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
Copy-Item -LiteralPath $SourceCli, $SourceSettings, $SourceIcon, $InstallScript, $UninstallScript, $Readme -Destination $Package

# Simulate a prior release with an explicit login-startup preference.
$RunKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
New-Item -Path $RunKey -Force | Out-Null
Set-ItemProperty -LiteralPath $RunKey -Name "Crosshair" -Value ('"' + $SourceCli + '" --daemon --no-ui')

$InstallDir = Join-Path $env:LOCALAPPDATA "Programs\Crosshair"
$Bin = Join-Path $InstallDir "crosshair.exe"
$Settings = Join-Path $InstallDir "crosshair-settings.exe"
$InstalledIcon = Join-Path $InstallDir "crosshair.ico"
$StartMenuShortcut = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Crosshair.lnk"
$SettingsShortcut = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Crosshair Settings.lnk"
$RunKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
$ConfigPath = Join-Path $env:APPDATA "Crosshair\config.json"

if (-not ("CrosshairSmokeNative" -as [type])) {
    Add-Type -TypeDefinition @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class CrosshairSmokeNative {
    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll", EntryPoint = "PostMessageW", SetLastError = true)]
    public static extern bool PostMessage(IntPtr hWnd, uint message, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll", EntryPoint = "FindWindowW", CharSet = CharSet.Unicode, SetLastError = true)]
    public static extern IntPtr FindWindow(string className, string windowName);
    [DllImport("user32.dll", EntryPoint = "EnumWindows", SetLastError = true)]
    public static extern bool EnumWindows(EnumWindowsProc callback, IntPtr lParam);
    [DllImport("user32.dll", EntryPoint = "GetWindowThreadProcessId", SetLastError = true)]
    public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);
    [DllImport("user32.dll", EntryPoint = "IsWindowVisible")]
    public static extern bool IsWindowVisible(IntPtr hWnd);
    [DllImport("user32.dll", EntryPoint = "GetClassNameW", CharSet = CharSet.Unicode)]
    public static extern int GetClassName(IntPtr hWnd, StringBuilder className, int maxCount);
    [DllImport("user32.dll", EntryPoint = "GetWindowTextW", CharSet = CharSet.Unicode)]
    public static extern int GetWindowText(IntPtr hWnd, StringBuilder title, int maxCount);

    public static string DescribeWindow(IntPtr hWnd) {
        StringBuilder className = new StringBuilder(256);
        StringBuilder title = new StringBuilder(256);
        GetClassName(hWnd, className, className.Capacity);
        GetWindowText(hWnd, title, title.Capacity);
        return "class='" + className.ToString() + "' title='" + title.ToString() +
               "' visible=" + IsWindowVisible(hWnd).ToString();
    }

    public static IntPtr[] FindVisibleWindowsForProcess(int processId, string expectedTitle) {
        List<IntPtr> handles = new List<IntPtr>();
        EnumWindowsProc callback = delegate(IntPtr hWnd, IntPtr lParam) {
            uint ownerProcessId;
            GetWindowThreadProcessId(hWnd, out ownerProcessId);
            if (ownerProcessId == (uint)processId && IsWindowVisible(hWnd)) {
                StringBuilder title = new StringBuilder(256);
                GetWindowText(hWnd, title, title.Capacity);
                if (String.Equals(title.ToString(), expectedTitle, StringComparison.OrdinalIgnoreCase)) {
                    handles.Add(hWnd);
                }
            }
            return true;
        };
        EnumWindows(callback, IntPtr.Zero);
        GC.KeepAlive(callback);
        return handles.ToArray();
    }
}
"@
}

function Write-SmokeLog {
    param([Parameter(Mandatory = $true)][string] $Message)
    $line = "[{0:O}] {1}" -f [DateTime]::UtcNow, $Message
    Add-Content -LiteralPath $DiagnosticLog -Value $line
    Write-Host $line
}

function Wait-ForProcessExit {
    param(
        [Parameter(Mandatory = $true)][Diagnostics.Process] $Process,
        [Parameter(Mandatory = $true)][int] $TimeoutMilliseconds
    )
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ($watch.ElapsedMilliseconds -lt $TimeoutMilliseconds) {
        try {
            if ($Process.HasExited) { return $true }
        } catch {
            return $true
        }
        Start-Sleep -Milliseconds 50
    }
    try { return [bool]$Process.HasExited } catch { return $true }
}

Write-SmokeLog "Starting Windows installer/overlay/Settings/uninstall smoke test."

function Invoke-Crosshair {
    param(
        [Parameter(ValueFromRemainingArguments = $true)][string[]] $Arguments,
        [int] $TimeoutMilliseconds = 15000
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
    if (-not (Wait-ForProcessExit -Process $process -TimeoutMilliseconds $TimeoutMilliseconds)) {
        # Avoid Process.Kill(true) and WaitForExit after a timeout: on Windows
        # they can block while child processes still own inherited handles.
        # Stop only this CLI PID; the test's finally block separately stops
        # the isolated daemon and Settings process by their exact install path.
        Write-SmokeLog ("CLI TIMEOUT: " + $label + " exceeded " + $TimeoutMilliseconds + " ms; terminating CLI PID " + $process.Id + ".")
        try {
            Stop-Process -Id $process.Id -Force -ErrorAction Stop
            Write-SmokeLog ("CLI process " + $process.Id + " termination requested.")
        } catch {
            Write-SmokeLog ("CLI process termination request failed: " + $_.Exception.Message)
        }
        if ($stdoutTask.IsCompleted) {
            try {
                $timeoutStdout = $stdoutTask.GetAwaiter().GetResult()
                if (-not [string]::IsNullOrWhiteSpace($timeoutStdout)) {
                    Write-SmokeLog ("CLI TIMEOUT STDOUT: " + $timeoutStdout.Trim())
                }
            } catch {}
        } else {
            Write-SmokeLog "CLI stdout capture remained open after timeout; it will not be awaited."
        }
        if ($stderrTask.IsCompleted) {
            try {
                $timeoutStderr = $stderrTask.GetAwaiter().GetResult()
                if (-not [string]::IsNullOrWhiteSpace($timeoutStderr)) {
                    Write-SmokeLog ("CLI TIMEOUT STDERR: " + $timeoutStderr.Trim())
                }
            } catch {}
        } else {
            Write-SmokeLog "CLI stderr capture remained open after timeout; it will not be awaited."
        }
        $daemonLog = Join-Path $env:LOCALAPPDATA "Crosshair\crosshair.log"
        if (Test-Path -LiteralPath $daemonLog -PathType Leaf) {
            Write-SmokeLog "Captured Windows daemon diagnostic log:"
            Get-Content -LiteralPath $daemonLog | ForEach-Object { Write-SmokeLog ("DAEMON: " + $_) }
        } else {
            Write-SmokeLog "No Windows daemon diagnostic log was created."
        }
        $process.Dispose()
        throw ($label + " timed out after " + $TimeoutMilliseconds + " ms.")
    }
    # HasExited polling has confirmed that the CLI process exited.
    # Do not call WaitForExit(): it can also wait for redirected stream readers,
    # which may remain open while a detached daemon inherits an OS pipe handle.
    $exitCode = $process.ExitCode
    Write-SmokeLog ("CLI PROCESS EXITED: " + $label + " (exit " + $exitCode + ")")
    $stdout = ""
    $stderr = ""
    if ($stdoutTask.IsCompleted) {
        try { $stdout = $stdoutTask.GetAwaiter().GetResult() } catch {}
    } else {
        Write-SmokeLog ("CLI stdout pipe remained open after " + $label + " exited; not waiting for detached descendants.")
    }
    if ($stderrTask.IsCompleted) {
        try { $stderr = $stderrTask.GetAwaiter().GetResult() } catch {}
    } else {
        Write-SmokeLog ("CLI stderr pipe remained open after " + $label + " exited; not waiting for detached descendants.")
    }
    $process.Dispose()
    $text = ($stdout + $stderr).Trim()
    Write-SmokeLog ("CLI EXIT: " + $label + " (exit " + $exitCode + ")")
    if ($exitCode -ne 0) {
        $daemonLog = Join-Path $env:LOCALAPPDATA "Crosshair\crosshair.log"
        if (Test-Path -LiteralPath $daemonLog -PathType Leaf) {
            Write-SmokeLog "Captured Windows daemon diagnostic log after CLI failure:"
            Get-Content -LiteralPath $daemonLog | ForEach-Object { Write-SmokeLog ("DAEMON: " + $_) }
        }
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
    Write-SmokeLog "Status wait timed out for '$Expected'. Last response: $last"
    $appLog = Join-Path $env:LOCALAPPDATA "Crosshair\crosshair.log"
    if (Test-Path -LiteralPath $appLog -PathType Leaf) {
        Write-SmokeLog "Crosshair app trace at status timeout:"
        Get-Content -LiteralPath $appLog -Tail 160 | ForEach-Object { Write-SmokeLog ("APP: " + $_) }
    }
    throw "Timed out waiting for status '$Expected'. Last response: $last"
}

function Wait-ForNoOverlayDaemon {
    $deadline = [DateTime]::UtcNow.AddSeconds(8)
    do {
        $remaining = @(
            Get-Process -Name "crosshair" -ErrorAction SilentlyContinue |
                Where-Object { $_.Path -eq $Bin }
        )
        if ($remaining.Count -eq 0) { return }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)

    $pids = ($remaining | ForEach-Object { $_.Id }) -join ", "
    throw "The overlay is disabled but its daemon process is still running (PIDs: $pids)."
}

function Start-TrayController {
    $launcher = Start-Process -FilePath $Settings -ArgumentList "--launch" -WorkingDirectory $InstallDir -WindowStyle Hidden -PassThru
    if (-not (Wait-ForProcessExit -Process $launcher -TimeoutMilliseconds 5000)) {
        throw "The System tray Start Menu launcher did not hand off promptly."
    }
    if ($launcher.ExitCode -ne 0) {
        throw "The System tray launcher failed with exit code $($launcher.ExitCode)."
    }

    $hwnd = [IntPtr]::Zero
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    while ([DateTime]::UtcNow -lt $deadline -and $hwnd -eq [IntPtr]::Zero) {
        $hwnd = [CrosshairSmokeNative]::FindWindow("CrosshairRustTrayWindow", "Crosshair Tray Controller")
        if ($hwnd -eq [IntPtr]::Zero) { Start-Sleep -Milliseconds 100 }
    }
    if ($hwnd -eq [IntPtr]::Zero) {
        throw "The System tray launcher did not create the Crosshair tray controller."
    }

    [uint32]$ownerPid = 0
    $null = [CrosshairSmokeNative]::GetWindowThreadProcessId($hwnd, [ref]$ownerPid)
    $process = Get-Process -Id ([int]$ownerPid) -ErrorAction Stop
    if ($process.ProcessName -ne "crosshair") {
        throw "Crosshair tray controller window belongs to an unexpected process."
    }
    $status = Wait-ForStatus "Overlay: enabled"
    if (-not $status.Contains("Crosshair daemon: running")) {
        throw "Tray startup did not start the saved enabled overlay from a cold state."
    }
    return [PSCustomObject]@{ Hwnd = $hwnd; Process = $process }
}

function Assert-NoTrayController {
    $hwnd = [CrosshairSmokeNative]::FindWindow("CrosshairRustTrayWindow", "Crosshair Tray Controller")
    if ($hwnd -ne [IntPtr]::Zero) {
        throw "A tray controller remained alive while the saved overlay state was OFF."
    }
    Wait-ForNoOverlayDaemon
}

function Stop-Crosshair {
    if (Test-Path -LiteralPath $Bin -PathType Leaf) {
        try {
            $quit = Start-Process -FilePath $Bin -ArgumentList "--quit" -WindowStyle Hidden -PassThru
            if (-not (Wait-ForProcessExit -Process $quit -TimeoutMilliseconds 5000)) {
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

$trayProcess = $null
try {
    Write-SmokeLog "[1/11] install the same per-user package intended for the laptop"
    & (Join-Path $Package "Install-Crosshair.ps1")
    if ($LASTEXITCODE -ne 0) { throw "Windows installer script failed." }
    if (-not (Test-Path -LiteralPath $Bin -PathType Leaf) -or
        -not (Test-Path -LiteralPath $Settings -PathType Leaf) -or
        -not (Test-Path -LiteralPath $InstalledIcon -PathType Leaf)) {
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
    if (-not (Test-Path -LiteralPath $StartMenuShortcut -PathType Leaf) -or
        -not (Test-Path -LiteralPath $SettingsShortcut -PathType Leaf)) {
        throw "Installer did not create both the mode-aware app and unconditional Settings shortcuts."
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
    $shortcutIcon = [string]$shortcut.IconLocation
    if (-not $shortcutTargetMatches -or $shortcutArguments -ne "--launch" -or
        $shortcutIcon -notlike ($InstalledIcon + ",*")) {
        throw ("Start Menu shortcut mismatch. Target: '$actualShortcutTarget'; expected: '$expectedShortcutTarget'; arguments: '$shortcutArguments'; icon: '$shortcutIcon'.")
    }
    $settingsShortcut = $shell.CreateShortcut($SettingsShortcut)
    $settingsTarget = [IO.Path]::GetFullPath(([string]$settingsShortcut.TargetPath).Replace("/", "\"))
    if (-not [string]::Equals($settingsTarget, $expectedShortcutTarget, [StringComparison]::OrdinalIgnoreCase) -or
        -not [string]::IsNullOrEmpty([string]$settingsShortcut.Arguments)) {
        throw "The unconditional Settings shortcut does not point directly to crosshair-settings.exe."
    }

    Write-SmokeLog "[2/11] real monitor discovery and opt-in autostart registry control"
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

    Write-SmokeLog "[3/11] command-only CLI launch is quiet, while explicit UI remains available"
    $null = Invoke-Crosshair -Arguments @("--set", '{"enabled":true,"shape":"dot","size":9,"activity_mode":"command"}')
    $settingsBeforeQuietLaunch = @(Get-Process -Name "crosshair-settings" -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
    $quietLaunch = Invoke-Crosshair -Arguments @()
    if (-not [string]::IsNullOrEmpty([string]$quietLaunch)) {
        throw "Command-only plain launch produced unexpected output: $quietLaunch"
    }
    $settingsAfterQuietLaunch = @(Get-Process -Name "crosshair-settings" -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
    if (@($settingsAfterQuietLaunch | Where-Object { $_ -notin $settingsBeforeQuietLaunch }).Count -gt 0) {
        throw "Command-only plain launch unexpectedly opened Settings."
    }
    $null = Invoke-Crosshair -Arguments @("--show")
    $status = Wait-ForStatus "Overlay: enabled"
    if (-not $status.Contains("Crosshair daemon: running")) {
        throw "Overlay status did not report a running daemon: $status"
    }

    Write-SmokeLog "[4/11] Start Menu launcher honors Command only and App UI without a console"
    $null = Invoke-Crosshair -Arguments @("--set", '{"activity_mode":"command"}')
    $settingsBeforeCommandLaunch = @(Get-Process -Name "crosshair-settings" -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
    $commandLauncher = Start-Process -FilePath $Settings -ArgumentList "--launch" -WorkingDirectory $InstallDir -WindowStyle Hidden -PassThru
    if (-not (Wait-ForProcessExit -Process $commandLauncher -TimeoutMilliseconds 5000)) {
        throw "The command-only Start Menu launcher did not exit promptly."
    }
    if ($commandLauncher.ExitCode -ne 0) { throw "The command-only launcher failed with exit code $($commandLauncher.ExitCode)." }
    $settingsAfterCommandLaunch = @(Get-Process -Name "crosshair-settings" -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
    if (@($settingsAfterCommandLaunch | Where-Object { $_ -notin $settingsBeforeCommandLaunch }).Count -gt 0) {
        throw "The command-only Start Menu launcher unexpectedly opened Settings."
    }
    $null = Invoke-Crosshair -Arguments @("--set", '{"activity_mode":"ui"}')
    $uiLauncher = Start-Process -FilePath $Settings -ArgumentList "--launch" -WorkingDirectory $InstallDir -WindowStyle Hidden -PassThru
    $uiHwnd = [IntPtr]::Zero
    $uiDeadline = [DateTime]::UtcNow.AddSeconds(10)
    while ([DateTime]::UtcNow -lt $uiDeadline -and $uiHwnd -eq [IntPtr]::Zero) {
        $uiLauncher.Refresh()
        if ($uiLauncher.HasExited) { throw "The App UI Start Menu launcher exited before showing Settings (exit $($uiLauncher.ExitCode))." }
        $uiWindows = @([CrosshairSmokeNative]::FindVisibleWindowsForProcess($uiLauncher.Id, "Crosshair Settings"))
        if ($uiWindows.Count -gt 0) { $uiHwnd = [IntPtr]$uiWindows[0] }
        if ($uiHwnd -eq [IntPtr]::Zero) { Start-Sleep -Milliseconds 100 }
    }
    if ($uiHwnd -eq [IntPtr]::Zero) { throw "The App UI Start Menu launcher did not open Settings." }
    if (-not [CrosshairSmokeNative]::PostMessage($uiHwnd, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)) {
        throw "Could not close Settings after validating App UI launch."
    }
    if (-not (Wait-ForProcessExit -Process $uiLauncher -TimeoutMilliseconds 8000)) {
        throw "The App UI Start Menu Settings window did not close cleanly."
    }
    $null = Invoke-Crosshair -Arguments @("--set", '{"activity_mode":"command"}')

    Write-SmokeLog "[5/11] persist the tray default through named-pipe config updates"
    $null = Invoke-Crosshair -Arguments @("--set", '{"enabled":true,"shape":"cross","size":31,"opacity":0.72,"activity_mode":"tray"}')
    $saved = Get-Content -LiteralPath $ConfigPath -Raw | ConvertFrom-Json
    if ($saved.shape -ne "cross" -or $saved.size -ne 31 -or $saved.activity_mode -ne "tray") {
        throw "Named-pipe visual or activity-mode settings update was not persisted correctly."
    }
    $status = Wait-ForStatus "Overlay: enabled"

    Write-SmokeLog "[6/11] OFF state has no tray process, even when tray mode is the saved launch preference"
    $null = Invoke-Crosshair -Arguments @("--hide")
    $null = Wait-ForStatus "Crosshair daemon: not running"
    $disabledTray = Start-Process -FilePath $Bin -ArgumentList "--tray" -WorkingDirectory $InstallDir -WindowStyle Hidden -PassThru
    if (-not (Wait-ForProcessExit -Process $disabledTray -TimeoutMilliseconds 5000)) {
        throw "The direct --tray command remained resident while the overlay was OFF."
    }
    if ($disabledTray.ExitCode -ne 0) {
        throw "The disabled direct --tray command failed with exit code $($disabledTray.ExitCode)."
    }
    Assert-NoTrayController

    $disabledLauncher = Start-Process -FilePath $Settings -ArgumentList "--launch" -WorkingDirectory $InstallDir -WindowStyle Hidden -PassThru
    $recoverySettingsHwnd = [IntPtr]::Zero
    $recoveryDeadline = [DateTime]::UtcNow.AddSeconds(10)
    while ([DateTime]::UtcNow -lt $recoveryDeadline -and $recoverySettingsHwnd -eq [IntPtr]::Zero) {
        $windows = @([CrosshairSmokeNative]::FindVisibleWindowsForProcess($disabledLauncher.Id, "Crosshair Settings"))
        if ($windows.Count -gt 0) { $recoverySettingsHwnd = [IntPtr]$windows[0] }
        if ($recoverySettingsHwnd -eq [IntPtr]::Zero) { Start-Sleep -Milliseconds 100 }
    }
    if ($recoverySettingsHwnd -eq [IntPtr]::Zero) {
        throw "Tray mode while OFF did not open foreground Settings as a re-enable path."
    }
    if (-not [CrosshairSmokeNative]::PostMessage($recoverySettingsHwnd, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)) {
        throw "Could not close foreground Settings after validating the OFF-state recovery path."
    }
    if (-not (Wait-ForProcessExit -Process $disabledLauncher -TimeoutMilliseconds 8000)) {
        throw "Foreground Settings did not close cleanly after the OFF-state recovery-path check."
    }
    Start-Sleep -Milliseconds 300
    Assert-NoTrayController

    # Restore enabled=true only in the isolated test config to verify that a
    # cold System tray launch starts the overlay before creating its controller.
    $saved = Get-Content -LiteralPath $ConfigPath -Raw | ConvertFrom-Json
    $saved.enabled = $true
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [IO.File]::WriteAllText($ConfigPath, ($saved | ConvertTo-Json -Depth 16), $utf8NoBom)
    $tray = Start-TrayController
    $trayHwnd = $tray.Hwnd
    $trayProcess = $tray.Process
    $trayCallback = 0x8000 + 41
    $wmLeftButtonUp = 0x0202
    $trayLog = Join-Path $env:LOCALAPPDATA "Crosshair\crosshair.log"

    Write-SmokeLog "[6a/11] external --hide closes both the daemon and an already-open tray"
    $null = Invoke-Crosshair -Arguments @("--hide")
    $null = Wait-ForStatus "Crosshair daemon: not running"
    if (-not (Wait-ForProcessExit -Process $trayProcess -TimeoutMilliseconds 8000)) {
        throw "The tray controller remained resident after an external --hide."
    }
    Assert-NoTrayController

    Write-SmokeLog "[6b/11] tray left-click toggles OFF and closes its own controller"
    $null = Invoke-Crosshair -Arguments @("--show")
    $tray = Start-TrayController
    $trayHwnd = $tray.Hwnd
    $trayProcess = $tray.Process
    $callbackTraceBefore = @(Select-String -LiteralPath $trayLog -SimpleMatch "tray: notification callback received: event=0x0202" -ErrorAction SilentlyContinue).Count
    $toggleOffTraceBefore = @(Select-String -LiteralPath $trayLog -SimpleMatch "tray: toggle turned the overlay OFF; closing tray controller" -ErrorAction SilentlyContinue).Count
    if (-not [CrosshairSmokeNative]::PostMessage($trayHwnd, $trayCallback, [IntPtr]::Zero, [IntPtr]$wmLeftButtonUp)) {
        throw "Could not post the tray icon left-click callback."
    }
    $callbackDeadline = [DateTime]::UtcNow.AddSeconds(8)
    $callbackTrace = $false
    $toggleOffTrace = $false
    while ([DateTime]::UtcNow -lt $callbackDeadline -and (-not $callbackTrace -or -not $toggleOffTrace)) {
        $callbackTrace = @(Select-String -LiteralPath $trayLog -SimpleMatch "tray: notification callback received: event=0x0202" -ErrorAction SilentlyContinue).Count -gt $callbackTraceBefore
        $toggleOffTrace = @(Select-String -LiteralPath $trayLog -SimpleMatch "tray: toggle turned the overlay OFF; closing tray controller" -ErrorAction SilentlyContinue).Count -gt $toggleOffTraceBefore
        if (-not $callbackTrace -or -not $toggleOffTrace) { Start-Sleep -Milliseconds 50 }
    }
    if (-not $callbackTrace -or -not $toggleOffTrace) {
        $lastTrayTrace = Get-Content -LiteralPath $trayLog | Where-Object { $_ -match "tray:" } | Select-Object -Last 30
        throw ("Tray toggle did not complete its acknowledged OFF path. callback=$callbackTrace toggleOff=$toggleOffTrace; trace: " + ($lastTrayTrace -join " | "))
    }
    $null = Wait-ForStatus "Crosshair daemon: not running"
    if (-not (Wait-ForProcessExit -Process $trayProcess -TimeoutMilliseconds 8000)) {
        throw "The tray controller remained resident after its own toggle turned the overlay OFF."
    }
    Assert-NoTrayController

    # Re-enable and relaunch tray for the independent Settings-close and
    # explicit Exit Crosshair menu acceptance steps below.
    $null = Invoke-Crosshair -Arguments @("--show")
    $tray = Start-TrayController
    $trayHwnd = $tray.Hwnd
    $trayProcess = $tray.Process

    Write-SmokeLog "[7/11] open Settings explicitly while the tray is the saved default"
    $before = @(Get-Process -Name "crosshair-settings" -ErrorAction SilentlyContinue |
        ForEach-Object { $_.Id })
    $null = Invoke-Crosshair -Arguments @("--config")
    $settingsProcess = $null
    $deadline = [DateTime]::UtcNow.AddSeconds(12)
    while ([DateTime]::UtcNow -lt $deadline -and $null -eq $settingsProcess) {
        $candidates = @(Get-Process -Name "crosshair-settings" -ErrorAction SilentlyContinue |
            Where-Object { $_.Id -notin $before })
        foreach ($candidate in $candidates) {
            $candidate.Refresh()
            $visibleSettingsHwnds = @(
                [CrosshairSmokeNative]::FindVisibleWindowsForProcess($candidate.Id, "Crosshair Settings")
            )
            if ($visibleSettingsHwnds.Count -gt 0) {
                $settingsProcess = $candidate
                break
            }
        }
        if ($null -eq $settingsProcess) { Start-Sleep -Milliseconds 100 }
    }
    if ($null -eq $settingsProcess) {
        throw "The installed CLI did not create a visible window titled 'Crosshair Settings'."
    }

    Write-SmokeLog "[8/11] close Settings and verify overlay and tray lifetimes are independent"
    # Do not use Process.MainWindowHandle as a fallback. Winit creates its own
    # thread event-target HWND, and IME windows can also belong to this PID.
    # Posting WM_CLOSE to either internal HWND destroys the event loop plumbing
    # instead of closing the actual Settings UI.
    $settingsHwnds = @(
        [CrosshairSmokeNative]::FindVisibleWindowsForProcess($settingsProcess.Id, "Crosshair Settings")
    )
    Write-SmokeLog ("Settings top-level HWNDs for pid " + $settingsProcess.Id + ": " + ($settingsHwnds -join ", "))
    foreach ($settingsHwnd in $settingsHwnds) {
        Write-SmokeLog ("Settings HWND " + $settingsHwnd + " " + [CrosshairSmokeNative]::DescribeWindow([IntPtr]$settingsHwnd))
    }
    $postedHwnds = @{}
    foreach ($settingsHwnd in $settingsHwnds) {
        $posted = [CrosshairSmokeNative]::PostMessage([IntPtr]$settingsHwnd, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
        $postedHwnds[$settingsHwnd.ToInt64().ToString()] = $true
        Write-SmokeLog ("Settings WM_CLOSE queued: hwnd=" + $settingsHwnd + "; success=" + $posted)
    }
    if ($postedHwnds.Count -eq 0) {
        throw ("No top-level Settings window was found for pid " + $settingsProcess.Id + ".")
    }

    $closeWatch = [Diagnostics.Stopwatch]::StartNew()
    $lastCloseRetry = 0
    $closeCallbackSeen = $false
    $daemonLog = Join-Path $env:LOCALAPPDATA "Crosshair\crosshair.log"
    while (-not $settingsProcess.HasExited -and $closeWatch.ElapsedMilliseconds -lt 8000) {
        if (-not $closeCallbackSeen -and (Test-Path -LiteralPath $daemonLog -PathType Leaf)) {
            $closeCallbackSeen = Select-String -LiteralPath $daemonLog -SimpleMatch "window close: on_close_requested callback entered" -Quiet
        }
        if (-not $closeCallbackSeen -and $closeWatch.ElapsedMilliseconds -ge ($lastCloseRetry + 1500)) {
            $settingsProcess.Refresh()
            $currentHwnds = @(
                [CrosshairSmokeNative]::FindVisibleWindowsForProcess($settingsProcess.Id, "Crosshair Settings")
            )
            foreach ($currentHwnd in $currentHwnds) {
                $currentKey = $currentHwnd.ToInt64().ToString()
                $posted = [CrosshairSmokeNative]::PostMessage([IntPtr]$currentHwnd, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
                $postedHwnds[$currentKey] = $true
                Write-SmokeLog ("Settings WM_CLOSE retry: hwnd=" + $currentHwnd + "; success=" + $posted + "; " + [CrosshairSmokeNative]::DescribeWindow([IntPtr]$currentHwnd) + "; callbackSeen=" + $closeCallbackSeen)
            }
            $lastCloseRetry = $closeWatch.ElapsedMilliseconds
        }
        Start-Sleep -Milliseconds 100
    }
    if (-not (Wait-ForProcessExit -Process $settingsProcess -TimeoutMilliseconds 250)) {
        $settingsProcess.Refresh()
        Write-SmokeLog ("Settings close timeout: pid=" + $settingsProcess.Id + "; hasExited=" + $settingsProcess.HasExited + "; hwnd=" + $settingsProcess.MainWindowHandle + "; callbackSeen=" + $closeCallbackSeen)
        $daemonLog = Join-Path $env:LOCALAPPDATA "Crosshair\crosshair.log"
        if (Test-Path -LiteralPath $daemonLog -PathType Leaf) {
            Write-SmokeLog "Captured Crosshair and Settings lifecycle diagnostics:"
            Get-Content -LiteralPath $daemonLog | ForEach-Object { Write-SmokeLog ("APP: " + $_) }
        } else {
            Write-SmokeLog "No Crosshair diagnostic log was created while Settings was open."
        }
        throw "Settings did not exit after its window-close request."
    }
    Write-SmokeLog "Settings process exited after WM_CLOSE."
    $status = Wait-ForStatus "Overlay: enabled"
    if (-not $status.Contains("Crosshair daemon: running")) {
        throw "Closing Settings unexpectedly stopped the overlay."
    }
    Write-SmokeLog "[8b/11] select Exit Crosshair from the tray menu and verify full shutdown"
    $exitTraceBefore = @(Select-String -LiteralPath $trayLog -SimpleMatch "tray: Exit Crosshair: overlay hide completed; closing tray controller" -ErrorAction SilentlyContinue).Count
    if (-not [CrosshairSmokeNative]::PostMessage($trayHwnd, $trayCallback, [IntPtr]::Zero, [IntPtr]0x0205)) {
        throw "Could not post the tray right-click callback."
    }
    $menuHwnd = [IntPtr]::Zero
    $menuDeadline = [DateTime]::UtcNow.AddSeconds(5)
    while ([DateTime]::UtcNow -lt $menuDeadline -and $menuHwnd -eq [IntPtr]::Zero) {
        $menuHwnd = [CrosshairSmokeNative]::FindWindow("#32768", $null)
        if ($menuHwnd -eq [IntPtr]::Zero) { Start-Sleep -Milliseconds 50 }
    }
    if ($menuHwnd -eq [IntPtr]::Zero) {
        throw "The Crosshair tray context menu did not open."
    }
    # VK_END selects the final menu entry ("Exit Crosshair"); Enter activates
    # the same TrackPopupMenu return path used by a real user selection.
    $null = [CrosshairSmokeNative]::PostMessage($menuHwnd, 0x0100, [IntPtr]0x23, [IntPtr]::Zero)
    $null = [CrosshairSmokeNative]::PostMessage($menuHwnd, 0x0101, [IntPtr]0x23, [IntPtr]::Zero)
    $null = [CrosshairSmokeNative]::PostMessage($menuHwnd, 0x0100, [IntPtr]0x0D, [IntPtr]::Zero)
    $null = [CrosshairSmokeNative]::PostMessage($menuHwnd, 0x0101, [IntPtr]0x0D, [IntPtr]::Zero)
    if (-not (Wait-ForProcessExit -Process $trayProcess -TimeoutMilliseconds 8000)) {
        throw "Exit Crosshair did not close the tray controller."
    }
    $exitTraceDeadline = [DateTime]::UtcNow.AddSeconds(5)
    $exitTraceSeen = $false
    while ([DateTime]::UtcNow -lt $exitTraceDeadline -and -not $exitTraceSeen) {
        $exitTraceSeen = @(Select-String -LiteralPath $trayLog -SimpleMatch "tray: Exit Crosshair: overlay hide completed; closing tray controller" -ErrorAction SilentlyContinue).Count -gt $exitTraceBefore
        if (-not $exitTraceSeen) { Start-Sleep -Milliseconds 50 }
    }
    if (-not $exitTraceSeen) {
        throw "Exit Crosshair did not request --hide before closing the tray."
    }
    $status = Wait-ForStatus "Crosshair daemon: not running"
    if (-not $status.Contains("Overlay setting: disabled")) {
        throw "Exit Crosshair left the overlay enabled: $status"
    }
    Wait-ForNoOverlayDaemon

    Write-SmokeLog "[9/11] hide remains idempotent when the overlay daemon is already stopped"
    $null = Invoke-Crosshair -Arguments @("--hide")
    $status = Wait-ForStatus "Crosshair daemon: not running"
    Wait-ForNoOverlayDaemon
    $saved = Get-Content -LiteralPath $ConfigPath -Raw | ConvertFrom-Json
    if ($saved.enabled) { throw "Hide did not persist enabled=false." }

    Write-SmokeLog "[10/11] cold toggle starts, then toggle-off shuts down again"
    $null = Invoke-Crosshair -Arguments @("--toggle")
    $null = Wait-ForStatus "Overlay: enabled"
    $null = Invoke-Crosshair -Arguments @("--toggle")
    $status = Wait-ForStatus "Crosshair daemon: not running"
    if ($status -notmatch "Overlay setting: disabled") {
        throw "Toggle-off did not leave the overlay disabled: $status"
    }
    Wait-ForNoOverlayDaemon

    Write-SmokeLog "[11/11] uninstall through the installed CLI from its own working directory"
    Write-SmokeLog "Launching --uninstall; the deferred helper must remove the install folder after the CLI exits."
    $uninstallProcess = Start-Process -FilePath $Bin -ArgumentList "--uninstall" -WorkingDirectory $InstallDir -WindowStyle Hidden -PassThru
    if (-not (Wait-ForProcessExit -Process $uninstallProcess -TimeoutMilliseconds 10000)) {
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
        (Test-Path -LiteralPath $InstalledIcon -PathType Leaf) -or
        (Test-Path -LiteralPath $StartMenuShortcut -PathType Leaf) -or
        (Test-Path -LiteralPath $SettingsShortcut -PathType Leaf)) {
        throw "Uninstaller left an installed binary or Start Menu shortcut."
    }
    if (-not (Test-Path -LiteralPath $ConfigPath -PathType Leaf)) {
        throw "Uninstaller removed user configuration."
    }
    Write-SmokeLog "PASS: Windows install, all three App Activity launch modes, no-console Start Menu UI/tray handoff, native tray cold-start/toggle-off daemon exit/full Exit Crosshair shutdown, explicit Settings access, monitor discovery, named-pipe updates, layered overlay, autostart, hide/toggle-off and uninstall."
}
finally {
    Write-SmokeLog "Entering bounded test cleanup."
    $appLog = Join-Path $env:LOCALAPPDATA "Crosshair\crosshair.log"
    if (Test-Path -LiteralPath $appLog -PathType Leaf) {
        Write-SmokeLog "Final Crosshair app trace:"
        Get-Content -LiteralPath $appLog -Tail 160 | ForEach-Object { Write-SmokeLog ("APP: " + $_) }
    }
    if ($null -ne $trayProcess -and -not $trayProcess.HasExited) {
        try { Stop-Process -Id $trayProcess.Id -Force -ErrorAction SilentlyContinue } catch {}
    }
    Stop-Crosshair
    try { Remove-ItemProperty -LiteralPath $RunKey -Name "Crosshair" -ErrorAction SilentlyContinue } catch {}
    if (Test-Path -LiteralPath $Bin -PathType Leaf) {
        Write-SmokeLog "The isolated installation remains; removing the temporary test root rather than invoking a second uninstaller."
    }
    Remove-Item -LiteralPath $Root -Recurse -Force -ErrorAction SilentlyContinue
    Write-SmokeLog ("Cleanup finished. Diagnostic log: " + $DiagnosticLog)
}
