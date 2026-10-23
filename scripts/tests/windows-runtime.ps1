param([string]$HelperPath = (Join-Path $PSScriptRoot '..\windows-runtime.ps1'))
$ErrorActionPreference = 'Stop'
. $HelperPath

$testBase = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\target\runtime-package-tests'))
$testRoot = Join-Path $testBase ([guid]::NewGuid().ToString('N'))
$source = Join-Path $testRoot 'Microsoft.VC143.CRT'
$destination = Join-Path $testRoot 'package'
New-Item -ItemType Directory -Path $source,$destination -Force | Out-Null

try {
    $expectedFiles = @('vcruntime140.dll', 'vcruntime140_1.dll')
    foreach ($name in $expectedFiles) {
        [IO.File]::WriteAllText((Join-Path $source $name), "fixture-$name")
    }
    [IO.File]::WriteAllText((Join-Path $source 'unrelated.dll'), 'not a runtime dependency')
    $files = @(Get-WindowsRuntimeFiles -RuntimeDirectory $source)
    if ($files.Count -ne 2) { throw 'Both required VC++ runtime DLLs must be selected.' }
    Copy-Item -LiteralPath $files -Destination $destination -Force
    foreach ($name in $expectedFiles) {
        $originalHash = (Get-FileHash -LiteralPath (Join-Path $source $name)).Hash
        $copiedHash = (Get-FileHash -LiteralPath (Join-Path $destination $name)).Hash
        if ($originalHash -ne $copiedHash) { throw "Runtime DLL was not copied intact: $name" }
    }
    if (Test-Path -LiteralPath (Join-Path $destination 'unrelated.dll')) {
        throw 'Unrelated DLLs must not be bundled.'
    }
    foreach ($name in $expectedFiles) {
        $file = Join-Path $source $name
        $backup = "$file.saved"
        Move-Item -LiteralPath $file -Destination $backup
        try {
            $rejected = $false
            try { Get-WindowsRuntimeFiles -RuntimeDirectory $source | Out-Null }
            catch { $rejected = $true }
            if (-not $rejected) { throw "Missing $name must fail before replacing an existing package." }
        } finally {
            Move-Item -LiteralPath $backup -Destination $file
        }
    }
    Write-Output 'PASS: x64 runtime selection, intact copies, unrelated DLL exclusion and missing DLL rejection'
} finally {
    $resolvedRoot = [IO.Path]::GetFullPath($testRoot)
    if (-not $resolvedRoot.StartsWith($testBase + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Refusing to remove a test directory outside the test root.'
    }
    Remove-Item -LiteralPath $resolvedRoot -Recurse -Force
}
