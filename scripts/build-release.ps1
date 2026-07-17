param(
    [switch]$SkipTests
)

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$Root = Resolve-Path (Join-Path $ScriptDir "..")

Push-Location $Root
try {
    if (-not $SkipTests) {
        cargo test -p artforge-studio-native
        cargo check -p artforge-studio-native
    }

    & (Join-Path $ScriptDir "package-native-client.ps1") -Target windows
}
finally {
    Pop-Location
}
