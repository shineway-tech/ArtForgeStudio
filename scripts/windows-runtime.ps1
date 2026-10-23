function Get-WindowsRuntimeFiles {
    param([string]$RuntimeDirectory)

    $requiredFiles = @('vcruntime140.dll', 'vcruntime140_1.dll')
    $candidates = [System.Collections.Generic.List[string]]::new()
    if ($RuntimeDirectory) {
        $candidates.Add($RuntimeDirectory)
    } else {
        $redistRoots = [System.Collections.Generic.List[string]]::new()
        if ($env:VCToolsRedistDir) {
            $redistRoots.Add($env:VCToolsRedistDir)
        }
        if ($env:VCINSTALLDIR) {
            $versionRoot = Join-Path $env:VCINSTALLDIR 'Redist\MSVC'
            if (Test-Path -LiteralPath $versionRoot -PathType Container) {
                Get-ChildItem -LiteralPath $versionRoot -Directory |
                    Sort-Object Name -Descending |
                    ForEach-Object { $redistRoots.Add($_.FullName) }
            }
        }
        $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
        if (Test-Path -LiteralPath $vswhere -PathType Leaf) {
            $installation = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
            if ($LASTEXITCODE -ne 0) {
                throw 'Unable to locate the Visual Studio C++ redistributable.'
            }
            if ($installation) {
                $versionRoot = Join-Path $installation 'VC\Redist\MSVC'
                if (Test-Path -LiteralPath $versionRoot -PathType Container) {
                    Get-ChildItem -LiteralPath $versionRoot -Directory |
                        Sort-Object Name -Descending |
                        ForEach-Object { $redistRoots.Add($_.FullName) }
                }
            }
        }
        foreach ($root in $redistRoots) {
            $architectureRoot = Join-Path $root 'x64'
            if (Test-Path -LiteralPath $architectureRoot -PathType Container) {
                Get-ChildItem -LiteralPath $architectureRoot -Directory -Filter 'Microsoft.VC*.CRT' |
                    Sort-Object Name -Descending |
                    ForEach-Object { $candidates.Add($_.FullName) }
            }
        }
    }

    foreach ($directory in $candidates) {
        $files = @($requiredFiles | ForEach-Object { Join-Path $directory $_ })
        $missing = @($files | Where-Object { -not (Test-Path -LiteralPath $_ -PathType Leaf) })
        if ($missing.Count -eq 0) {
            return $files | ForEach-Object { (Resolve-Path -LiteralPath $_).Path }
        }
    }
    throw 'Missing x64 VC++ runtime DLLs (vcruntime140.dll and vcruntime140_1.dll). Install the Visual Studio C++ redistributable build tools or set VCToolsRedistDir to their Redist\MSVC\<version> directory. Do not copy DLLs from System32.'
}
