param([string]$HelperPath = (Join-Path $PSScriptRoot '..\..\native-client\src\runtime\storage\windows-update.ps1'))
$ErrorActionPreference = 'Stop'
$testBase = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\target\update-helper-tests'))
$testRoot = Join-Path $testBase ([guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $testRoot -Force | Out-Null
$compiler = Join-Path ([Runtime.InteropServices.RuntimeEnvironment]::GetRuntimeDirectory()) 'csc.exe'
if (-not (Test-Path -LiteralPath $compiler)) { $compiler = 'C:\Windows\Microsoft.NET\Framework64\v4.0.30319\csc.exe' }
function Assert-True($condition, $message) { if (-not $condition) { throw $message } }
function Wait-For($predicate, $message) {
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    while (-not (& $predicate)) {
        if ([DateTime]::UtcNow -gt $deadline) { throw $message }
        Start-Sleep -Milliseconds 50
    }
}
$utf8 = New-Object Text.UTF8Encoding($false)
foreach ($version in @('1.0.21.0', '1.0.22.0')) {
    $source = [IO.File]::ReadAllText((Join-Path $PSScriptRoot 'fixtures\update-app.cs')).Replace('__VERSION__', $version)
    $sourcePath = Join-Path $testRoot ($version + '.cs')
    [IO.File]::WriteAllText($sourcePath, $source, $utf8)
    & $compiler /nologo /target:winexe (('/out:') + (Join-Path $testRoot ($version + '.exe'))) $sourcePath
    if ($LASTEXITCODE -ne 0) { throw 'Unable to compile application fixture' }
}
& $compiler /nologo /target:winexe (('/out:') + (Join-Path $testRoot 'installer.exe')) (Join-Path $PSScriptRoot 'fixtures\update-installer.cs')
if ($LASTEXITCODE -ne 0) { throw 'Unable to compile installer fixture' }
$processes = @()
try {
    foreach ($scenario in @('success', 'failure', 'unchanged', 'auto-launch', 'bad-hash')) {
        $caseRoot = Join-Path $testRoot $scenario
        $target = Join-Path $caseRoot ("Elunvi Canvas's & [" + [char]0x7d20 + [char]0x6750 + '] %')
        $download = Join-Path $caseRoot 'download'
        New-Item -ItemType Directory -Path $target,$download,(Join-Path $target 'data') -Force | Out-Null
        $appExe = Join-Path $target 'ElunviCanvas.exe'
        Copy-Item -LiteralPath (Join-Path $testRoot '1.0.21.0.exe') -Destination $appExe
        Copy-Item -LiteralPath (Join-Path $testRoot '1.0.22.0.exe') -Destination (Join-Path $download 'payload.exe')
        $installer = Join-Path $download 'installer.exe'
        Copy-Item -LiteralPath (Join-Path $testRoot 'installer.exe') -Destination $installer
        [IO.File]::WriteAllText((Join-Path $download 'scenario.txt'), $scenario, $utf8)
        [IO.File]::WriteAllText((Join-Path $target 'fixture-only.txt'), 'test', $utf8)
        [IO.File]::WriteAllText((Join-Path $target 'data\keep.txt'), 'unchanged user data', $utf8)
        if ($scenario -eq 'auto-launch') { [IO.File]::WriteAllText((Join-Path $target 'hold-open'), 'hold', $utf8) }
        $parent = Start-Process -FilePath $appExe -ArgumentList '--parent' -PassThru -WindowStyle Hidden
        $processes += $parent
        Wait-For { Test-Path -LiteralPath (Join-Path $target 'parent-started') } 'Parent fixture did not start'
        $status = Join-Path $download 'status.txt'
        $result = Join-Path $target 'data\update-result.json'
        [IO.File]::WriteAllText($result, '{"status":"previous-attempt"}', $utf8)
        $sha = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash
        if ($scenario -eq 'bad-hash') { $sha = '0' * 64 }
        $arguments = @('-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-File', ('"' + [IO.Path]::GetFullPath($HelperPath) + '"'),
            '-PackagePath', ('"' + $installer + '"'), '-AppExePath', ('"' + $appExe + '"'),
            '-ExpectedVersion', '1.0.22', '-ExpectedSha256', $sha, '-ParentProcessId', $parent.Id,
            '-StatusPath', ('"' + $status + '"'), '-ResultPath', ('"' + $result + '"'))
        $helper = Start-Process -FilePath 'powershell.exe' -ArgumentList $arguments -PassThru -WindowStyle Hidden
        $processes += $helper
        Wait-For { Test-Path -LiteralPath $status } 'Supervisor did not report readiness or failure'
        if ($scenario -eq 'bad-hash') {
            Assert-True (([IO.File]::ReadAllText($status)).Trim() -eq 'failed') 'Bad hash must keep the original app running'
        } else {
            Assert-True (([IO.File]::ReadAllText($status)).Trim() -eq 'ready') 'Supervisor must be ready before the app exits'
            Assert-True (-not (Test-Path -LiteralPath (Join-Path $download 'received-args.txt'))) 'Installer started before parent exit'
            $packageLocked = $false
            try { $writer = [IO.File]::Open($installer, [IO.FileMode]::Open, [IO.FileAccess]::Write); $writer.Dispose() }
            catch [IO.IOException] { $packageLocked = $true }
            Assert-True $packageLocked 'Verified package can be modified before installation'
        }
        [IO.File]::WriteAllText((Join-Path $target 'parent-exit'), 'exit', $utf8)
        Assert-True ($parent.WaitForExit(5000)) 'Parent did not exit'
        Assert-True ($helper.WaitForExit(15000)) 'Supervisor did not finish'
        if ($scenario -eq 'bad-hash') {
            Assert-True (-not (Test-Path -LiteralPath (Join-Path $download 'received-args.txt'))) 'Unverified installer was launched'
            continue
        }
        $receipt = [IO.File]::ReadAllText($result) | ConvertFrom-Json
        $wanted = if ($scenario -eq 'failure') { 'install_failed' } elseif ($scenario -eq 'unchanged') { 'version_mismatch' } else { 'installed' }
        Assert-True ($receipt.status -eq $wanted) "Wrong result for $scenario"
        $received = [IO.File]::ReadAllLines((Join-Path $download 'received-args.txt'))
        Assert-True ($received -contains ('/DIR=' + $target)) 'Installer was not targeted at the running directory'
        Assert-True (@($received | Where-Object { $_.StartsWith('/LOG=') }).Count -eq 1) 'Installer diagnostics are missing'
        Wait-For { Test-Path -LiteralPath (Join-Path $target 'launches.txt') } 'Application was not restarted'
        $launches = [IO.File]::ReadAllLines((Join-Path $target 'launches.txt'))
        $wantedVersion = if ($wanted -eq 'installed') { '1.0.22.0' } else { '1.0.21.0' }
        Assert-True ($launches.Count -eq 1 -and $launches[0] -eq $wantedVersion) 'Wrong version or duplicate relaunch'
        Assert-True ([IO.File]::ReadAllText((Join-Path $target 'data\keep.txt')) -eq 'unchanged user data') 'User data was modified'
        if (Test-Path -LiteralPath (Join-Path $target 'hold-open')) { Remove-Item -LiteralPath (Join-Path $target 'hold-open') }
        Write-Output "PASS: $scenario"
    }
    Write-Output 'PASS: bad-hash'
} finally {
    foreach ($process in $processes) { if (-not $process.HasExited) { $process.Kill(); $process.WaitForExit() } }
    # Retain fixture files for inspection; never touch the real client or registry.
}
