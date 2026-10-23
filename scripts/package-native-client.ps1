param(
    [ValidateSet("all", "windows", "macos-x64", "macos-arm64")]
    [string[]]$Target = @("all"),
    [ValidateSet("local", "release")]
    [string]$BuildChannel = "release",
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
. (Join-Path $ScriptDir "windows-runtime.ps1")
$Root = Resolve-Path (Join-Path $ScriptDir "..")
$DistRoot = Join-Path $Root "dist"
$ClientDir = Join-Path $Root "native-client"
$AppName = "ElunviCanvas"
$metadataJson = & cargo metadata --manifest-path (Join-Path $Root "Cargo.toml") --format-version 1 --no-deps
if ($LASTEXITCODE -ne 0) {
    throw "Unable to read Cargo package metadata"
}
$metadata = $metadataJson | ConvertFrom-Json
$clientPackage = $metadata.packages | Where-Object { $_.name -eq "artforge-studio-native" } | Select-Object -First 1
if (-not $clientPackage) {
    throw "Unable to find artforge-studio-native in Cargo metadata"
}
$ClientVersion = $clientPackage.version
$IsMacHost = [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
    [System.Runtime.InteropServices.OSPlatform]::OSX
)

$targetsToBuild = [System.Collections.Generic.List[string]]::new()
if ($Target -contains "all") {
    @("windows", "macos-x64", "macos-arm64") | ForEach-Object { $targetsToBuild.Add($_) }
} else {
    $Target | ForEach-Object { $targetsToBuild.Add($_) }
}

function Remove-DirectorySafe {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$AllowedRoot
    )

    if (-not (Test-Path -LiteralPath $Path)) {
        return
    }

    $resolvedPath = [System.IO.Path]::GetFullPath($Path)
    $resolvedRoot = [System.IO.Path]::GetFullPath($AllowedRoot)
    if (-not $resolvedPath.StartsWith($resolvedRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to remove outside dist: $resolvedPath"
    }
    Remove-Item -LiteralPath $Path -Recurse -Force
}

function Copy-DirectoryContents {
    param(
        [Parameter(Mandatory = $true)][string]$Source,
        [Parameter(Mandatory = $true)][string]$Destination
    )

    if (-not (Test-Path -LiteralPath $Source)) {
        return
    }

    New-Item -ItemType Directory -Force -Path $Destination | Out-Null
    Get-ChildItem -LiteralPath $Source -Force | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination $Destination -Recurse -Force
    }
}

function Copy-ClientAssets {
    param([Parameter(Mandatory = $true)][string]$Destination)

    New-Item -ItemType Directory -Force -Path $Destination | Out-Null
    Copy-DirectoryContents -Source (Join-Path $ClientDir "assets") -Destination $Destination
    Copy-DirectoryContents -Source (Join-Path $Root "assets") -Destination $Destination
}

function New-DataDirs {
    param([Parameter(Mandatory = $true)][string]$PackageRoot)

    @("data", "data/input", "data/out", "data/prompt") | ForEach-Object {
        New-Item -ItemType Directory -Force -Path (Join-Path $PackageRoot $_) | Out-Null
    }
}

function New-ZipFromDirectory {
    param(
        [Parameter(Mandatory = $true)][string]$Source,
        [Parameter(Mandatory = $true)][string]$ZipPath
    )

    if (Test-Path -LiteralPath $ZipPath) {
        Remove-Item -LiteralPath $ZipPath -Force
    }

    Add-Type -AssemblyName System.IO.Compression
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [System.IO.Compression.ZipFile]::CreateFromDirectory(
        $Source,
        $ZipPath,
        [System.IO.Compression.CompressionLevel]::Optimal,
        $true
    )
}

function Assert-WindowsPortableArchive {
    param(
        [Parameter(Mandatory = $true)][string]$PackageRoot,
        [Parameter(Mandatory = $true)][string]$ZipPath
    )

    $archive = [System.IO.Compression.ZipFile]::OpenRead($ZipPath)
    try {
        $entries = @($archive.Entries)
        $rootPrefix = (Split-Path -Leaf $PackageRoot) + [System.IO.Path]::DirectorySeparatorChar
        foreach ($relativePath in @(
            "$AppName.exe",
            "vcruntime140.dll",
            "vcruntime140_1.dll",
            "assets$([System.IO.Path]::DirectorySeparatorChar)app.ico",
            "data$([System.IO.Path]::DirectorySeparatorChar)input$([System.IO.Path]::DirectorySeparatorChar)",
            "data$([System.IO.Path]::DirectorySeparatorChar)out$([System.IO.Path]::DirectorySeparatorChar)",
            "data$([System.IO.Path]::DirectorySeparatorChar)prompt$([System.IO.Path]::DirectorySeparatorChar)"
        )) {
            $entryName = $rootPrefix + $relativePath
            $matches = @($entries | Where-Object { $_.FullName -eq $entryName })
            if ($matches.Count -ne 1) {
                throw "Windows portable ZIP is missing or duplicates $relativePath"
            }
            if ($relativePath -in @("vcruntime140.dll", "vcruntime140_1.dll")) {
                $sourcePath = Join-Path $PackageRoot $relativePath
                $entryStream = $matches[0].Open()
                $sha256 = [System.Security.Cryptography.SHA256]::Create()
                try {
                    $archiveHash = [BitConverter]::ToString($sha256.ComputeHash($entryStream)).Replace("-", "")
                }
                finally {
                    $entryStream.Dispose()
                    $sha256.Dispose()
                }
                $sourceHash = (Get-FileHash -LiteralPath $sourcePath -Algorithm SHA256).Hash
                if ($archiveHash -ne $sourceHash) {
                    throw "Windows portable ZIP contains a damaged runtime DLL: $relativePath"
                }
            }
        }
    }
    finally {
        $archive.Dispose()
    }
}

function Build-Client {
    param(
        [string]$RustTarget
    )

    if ($SkipBuild) {
        return
    }

    $args = @("build", "--release", "--locked", "--manifest-path", (Join-Path $ClientDir "Cargo.toml"))
    if ($RustTarget -ne "") {
        $args += @("--target", $RustTarget)
    }
    $previousBuildChannel = $env:ELUNVI_BUILD_CHANNEL
    try {
        $env:ELUNVI_BUILD_CHANNEL = $BuildChannel
        & cargo @args
        if ($LASTEXITCODE -ne 0) {
            throw "Cargo build failed with exit code $LASTEXITCODE. Packaging stopped."
        }
    }
    finally {
        if ($null -eq $previousBuildChannel) {
            Remove-Item Env:\ELUNVI_BUILD_CHANNEL -ErrorAction SilentlyContinue
        } else {
            $env:ELUNVI_BUILD_CHANNEL = $previousBuildChannel
        }
    }
}

function Package-Windows {
    $runtimeFiles = @(Get-WindowsRuntimeFiles)
    $outDir = Join-Path $DistRoot "$AppName-windows-x64"
    $zipPath = Join-Path $DistRoot "${AppName}_${ClientVersion}_windows_x64_portable.zip"
    $existingData = Join-Path $outDir "data"
    $dataBackupRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("${AppName}-package-data-" + [guid]::NewGuid().ToString("N"))
    $dataBackup = Join-Path $dataBackupRoot "data"
    $hasExistingData = Test-Path -LiteralPath $existingData

    if ($hasExistingData) {
        Copy-DirectoryContents -Source $existingData -Destination $dataBackup
    }

    try {
        Build-Client

        Remove-DirectorySafe -Path $outDir -AllowedRoot $DistRoot
        New-Item -ItemType Directory -Force -Path $outDir | Out-Null

        Copy-Item -LiteralPath (Join-Path $Root "target/release/$AppName.exe") -Destination (Join-Path $outDir "$AppName.exe") -Force
        Copy-Item -LiteralPath $runtimeFiles -Destination $outDir -Force
        Copy-ClientAssets -Destination (Join-Path $outDir "assets")
        New-DataDirs -PackageRoot $outDir
        New-ZipFromDirectory -Source $outDir -ZipPath $zipPath
        Assert-WindowsPortableArchive -PackageRoot $outDir -ZipPath $zipPath

        Write-Host "Windows package: $zipPath"
    }
    finally {
        if ($hasExistingData) {
            Copy-DirectoryContents -Source $dataBackup -Destination (Join-Path $outDir "data")
            Remove-DirectorySafe -Path $dataBackupRoot -AllowedRoot ([System.IO.Path]::GetTempPath())
        }
    }
}

function Package-Macos {
    param(
        [Parameter(Mandatory = $true)][string]$ArchName,
        [Parameter(Mandatory = $true)][string]$RustTarget
    )

    $outDir = Join-Path $DistRoot "$AppName-macos-$ArchName"
    $appDir = Join-Path $outDir "$AppName.app"
    $contentsDir = Join-Path $appDir "Contents"
    $macosDir = Join-Path $contentsDir "MacOS"
    $resourcesDir = Join-Path $contentsDir "Resources"
    $zipPath = Join-Path $DistRoot "$AppName-macos-$ArchName.zip"

    if (-not $IsMacHost -and -not $SkipBuild) {
        throw "macOS $ArchName packages must be built on macOS or a CI runner configured with the Apple SDK/linker. Re-run this target on macOS, or use -SkipBuild only to assemble from an existing $RustTarget release binary."
    }

    Build-Client -RustTarget $RustTarget

    Remove-DirectorySafe -Path $outDir -AllowedRoot $DistRoot
    New-Item -ItemType Directory -Force -Path $macosDir | Out-Null
    New-Item -ItemType Directory -Force -Path $resourcesDir | Out-Null

    $binary = Join-Path $Root "target/$RustTarget/release/$AppName"
    $targetBinary = Join-Path $macosDir $AppName
    Copy-Item -LiteralPath $binary -Destination $targetBinary -Force
    if ($IsMacHost) {
        & chmod +x $targetBinary
    }
    Copy-ClientAssets -Destination (Join-Path $resourcesDir "assets")
    New-DataDirs -PackageRoot $resourcesDir

    @"
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>zh_CN</string>
  <key>CFBundleExecutable</key>
  <string>$AppName</string>
  <key>CFBundleIdentifier</key>
  <string>com.artforgestudio.client</string>
  <key>CFBundleName</key>
  <string>Elunvi Canvas</string>
  <key>CFBundleDisplayName</key>
  <string>Elunvi Canvas</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>$ClientVersion</string>
  <key>CFBundleVersion</key>
  <string>$ClientVersion</string>
  <key>LSMinimumSystemVersion</key>
  <string>11.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
</dict>
</plist>
"@ | Set-Content -Path (Join-Path $contentsDir "Info.plist") -Encoding UTF8

    New-ZipFromDirectory -Source $outDir -ZipPath $zipPath
    Write-Host "macOS $ArchName package: $zipPath"
}

Push-Location $Root
try {
    New-Item -ItemType Directory -Force -Path $DistRoot | Out-Null
    foreach ($item in $targetsToBuild) {
        switch ($item) {
            "windows" { Package-Windows }
            "macos-x64" { Package-Macos -ArchName "x64" -RustTarget "x86_64-apple-darwin" }
            "macos-arm64" { Package-Macos -ArchName "arm64" -RustTarget "aarch64-apple-darwin" }
        }
    }
}
finally {
    Pop-Location
}
