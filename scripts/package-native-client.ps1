param(
    [ValidateSet("all", "windows", "macos-x64", "macos-arm64")]
    [string[]]$Target = @("all"),
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$Root = Resolve-Path (Join-Path $ScriptDir "..")
$DistRoot = Join-Path $Root "dist"
$ClientDir = Join-Path $Root "native-client"
$AppName = "ArtForgeStudio"
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
    & cargo @args
}

function Package-Windows {
    $outDir = Join-Path $DistRoot "$AppName-windows-x64"
    $zipPath = Join-Path $DistRoot "${AppName}_${ClientVersion}_windows_x64_portable.zip"

    Build-Client

    Remove-DirectorySafe -Path $outDir -AllowedRoot $DistRoot
    New-Item -ItemType Directory -Force -Path $outDir | Out-Null

    Copy-Item -LiteralPath (Join-Path $Root "target/release/$AppName.exe") -Destination (Join-Path $outDir "$AppName.exe") -Force
    Copy-ClientAssets -Destination (Join-Path $outDir "assets")
    New-DataDirs -PackageRoot $outDir
    New-ZipFromDirectory -Source $outDir -ZipPath $zipPath

    Write-Host "Windows package: $zipPath"
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
  <string>$AppName</string>
  <key>CFBundleDisplayName</key>
  <string>ArtForgeStudio</string>
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
