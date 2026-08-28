param(
    [string]$PackagePath,
    [string]$AppExePath,
    [string]$ExpectedVersion,
    [string]$ExpectedSha256,
    [int]$ParentProcessId,
    [string]$StatusPath,
    [string]$ResultPath
)

$ErrorActionPreference = 'Stop'
$utf8 = New-Object Text.UTF8Encoding($false)
$ready = $false
$outcome = 'install_failed'
$packageGuard = $null

function Write-AtomicText([string]$path, [string]$text) {
    $temporary = $path + '.tmp'
    [IO.File]::WriteAllText($temporary, $text, $utf8)
    # Windows PowerShell coerces $null to an empty string for .NET string parameters.
    if ([IO.File]::Exists($path)) { [IO.File]::Replace($temporary, $path, [System.Management.Automation.Language.NullString]::Value) }
    else { [IO.File]::Move($temporary, $path) }
}
function Write-Result([string]$status) {
    $record = @{ schema = 1; target = $AppExePath; version = $ExpectedVersion; status = $status }
    Write-AtomicText $ResultPath ($record | ConvertTo-Json -Compress)
}
function Same-Version([string]$left, [string]$right) {
    try {
        $a = [version]$left.Trim()
        $b = [version]$right.Trim()
        return $a.Major -eq $b.Major -and $a.Minor -eq $b.Minor -and
            [Math]::Max(0, $a.Build) -eq [Math]::Max(0, $b.Build) -and
            [Math]::Max(0, $a.Revision) -eq [Math]::Max(0, $b.Revision)
    } catch { return $false }
}
function App-IsRunning {
    foreach ($candidate in [Diagnostics.Process]::GetProcessesByName('ElunviCanvas')) {
        try { if ($candidate.MainModule.FileName -ieq $AppExePath) { return $true } }
        catch { }
        finally { $candidate.Dispose() }
    }
    return $false
}

try {
    $appFile = Get-Item -LiteralPath $AppExePath
    $packageFile = Get-Item -LiteralPath $PackagePath
    if ($appFile.PSIsContainer -or $packageFile.PSIsContainer -or $appFile.Name -ine 'ElunviCanvas.exe') { throw 'Invalid executable' }
    $AppExePath = $appFile.FullName
    $targetDirectory = $appFile.Directory.FullName
    $downloadDirectory = $packageFile.Directory.FullName
    if ($targetDirectory -eq [IO.Path]::GetPathRoot($targetDirectory) -or
        $targetDirectory -ieq [Environment]::GetFolderPath('UserProfile')) { throw 'Invalid target directory' }
    if ([IO.Path]::GetDirectoryName([IO.Path]::GetFullPath($StatusPath)) -ine $downloadDirectory -or
        [IO.Path]::GetDirectoryName([IO.Path]::GetFullPath($ResultPath)) -ine (Join-Path $targetDirectory 'data')) { throw 'Invalid status directory' }
    if ($ExpectedVersion -notmatch '^\d+\.\d+\.\d+(\.\d+)?$' -or $ExpectedSha256 -notmatch '^[a-fA-F0-9]{64}$') { throw 'Invalid update metadata' }
    $packageGuard = [IO.File]::Open($PackagePath, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    if ((Get-FileHash -LiteralPath $PackagePath -Algorithm SHA256).Hash -ine $ExpectedSha256) { throw 'Package hash mismatch' }
    $parentProcess = Get-Process -Id $ParentProcessId -ErrorAction SilentlyContinue
    if ($null -ne $parentProcess -and $parentProcess.MainModule.FileName -ine $AppExePath) { throw 'Parent process mismatch' }
    # Do not let the UI exit until PowerShell has started and completed its preflight.
    Write-AtomicText $StatusPath 'ready'
    $ready = $true
    if ($null -ne $parentProcess) {
        if (-not $parentProcess.WaitForExit(120000)) { throw 'Application did not exit' }
        $parentProcess.Dispose()
    }
    $installerInfo = New-Object Diagnostics.ProcessStartInfo
    $installerInfo.FileName = $PackagePath
    $installerInfo.WorkingDirectory = $downloadDirectory
    $installerInfo.UseShellExecute = $false
    $installerInfo.CreateNoWindow = $true
    $installerInfo.Arguments = @('/SP-', '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NOCANCEL',
        '/NORESTART', '/CLOSEAPPLICATIONS', '/NORESTARTAPPLICATIONS', '/RESTARTEXITCODE=3010',
        ('/DIR="' + $targetDirectory + '"'), ('/LOG="' + (Join-Path $downloadDirectory 'install.log') + '"')) -join ' '
    $installerProcess = [Diagnostics.Process]::Start($installerInfo)
    $installerProcess.WaitForExit()
    $exitCode = $installerProcess.ExitCode
    $installerProcess.Dispose()
    if ($exitCode -eq 3010) { $outcome = 'reboot_required' }
    elseif ($exitCode -eq 0) {
        $installedVersion = [Diagnostics.FileVersionInfo]::GetVersionInfo($AppExePath).FileVersion
        if (Same-Version $installedVersion $ExpectedVersion) { $outcome = 'installed' }
        else { $outcome = 'version_mismatch' }
    }
    Write-Result $outcome
} catch {
    if (-not $ready) {
        try { Write-AtomicText $StatusPath 'failed' } catch { }
        exit 1
    }
    try { Write-Result $outcome } catch { }
} finally {
    if ($null -ne $packageGuard) { $packageGuard.Dispose() }
}

# Inno Setup may already have launched the updated app in its [Run] section.
# Otherwise reopen the same path, including on installation failure.
try {
    $alreadyRunning = App-IsRunning
    [IO.File]::AppendAllText((Join-Path $downloadDirectory 'supervisor.log'), "restart-running=$alreadyRunning`n", $utf8)
    if (-not $alreadyRunning) {
        $appInfo = New-Object Diagnostics.ProcessStartInfo
        $appInfo.FileName = $AppExePath
        $appInfo.WorkingDirectory = $targetDirectory
        $appInfo.Arguments = '--updated'
        $appInfo.UseShellExecute = $false
        $restarted = [Diagnostics.Process]::Start($appInfo)
        $restarted.Dispose()
        [IO.File]::AppendAllText((Join-Path $downloadDirectory 'supervisor.log'), "restart-requested`n", $utf8)
    }
} catch {
    [IO.File]::AppendAllText((Join-Path $downloadDirectory 'supervisor.log'), ("restart-error=" + $_.Exception.GetType().Name + "`n"), $utf8)
    try { Write-Result 'restart_failed' } catch { }
    exit 1
}
