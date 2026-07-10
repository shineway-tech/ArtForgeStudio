param(
    [switch]$SkipTests
)

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$Root = Resolve-Path (Join-Path $ScriptDir "..")
$DistRoot = Join-Path $Root "dist"
$DistDir = Join-Path $DistRoot "ArtAITRust"
$ExeSource = Join-Path $Root "target\release\artait-gui.exe"
$ExeTarget = Join-Path $DistDir "ArtAITRust.exe"

Push-Location $Root
try {
    if (-not $SkipTests) {
        cargo fmt --all -- --check
        cargo test --workspace
        cargo check --workspace
    }

    cargo build --release -p artait-gui

    $resolvedRoot = [System.IO.Path]::GetFullPath($Root)
    $resolvedDist = [System.IO.Path]::GetFullPath($DistDir)
    if (-not $resolvedDist.StartsWith($resolvedRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to clean dist outside workspace: $resolvedDist"
    }

    if (Test-Path $DistDir) {
        Remove-Item -LiteralPath $DistDir -Recurse -Force
    }
    New-Item -ItemType Directory -Force -Path $DistDir | Out-Null

    Copy-Item -LiteralPath $ExeSource -Destination $ExeTarget
    Copy-Item -LiteralPath (Join-Path $Root "config.example.json") -Destination (Join-Path $DistDir "config.example.json")
    Copy-Item -LiteralPath (Join-Path $Root "README.md") -Destination (Join-Path $DistDir "README.md")
    Copy-Item -LiteralPath (Join-Path $Root "PROJECT_STRUCTURE.md") -Destination (Join-Path $DistDir "PROJECT_STRUCTURE.md")

    @"
@echo off
setlocal
cd /d "%~dp0"
ArtAITRust.exe
"@ | Set-Content -Path (Join-Path $DistDir "run-artait-rust.bat") -Encoding ASCII

    Write-Host "Release package written to: $DistDir"
}
finally {
    Pop-Location
}
