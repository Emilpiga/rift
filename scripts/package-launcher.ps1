# Build the lightweight Rift launcher for sharing with playtesters.
#
# If -ManifestUrl is provided, it is baked into the executable at compile time
# so you can share just rift-launcher.exe. If omitted, the script writes a
# launcher-manifest-url.txt file next to the executable instead.

[CmdletBinding()]
param(
    [string]$ManifestUrl = $env:RIFT_LAUNCHER_MANIFEST_URL,
    [string]$OutDir = "dist\rift-launcher"
)

$ErrorActionPreference = 'Stop'
Set-Location -Path (Join-Path $PSScriptRoot '..')

if ($ManifestUrl) {
    Write-Host "==> baking RIFT_LAUNCHER_MANIFEST_URL=$ManifestUrl"
    $env:RIFT_LAUNCHER_MANIFEST_URL = $ManifestUrl
} else {
    Write-Host '==> no manifest URL baked in; writing launcher-manifest-url.txt placeholder'
    Remove-Item Env:RIFT_LAUNCHER_MANIFEST_URL -ErrorAction SilentlyContinue
}

Write-Host '==> cargo build --release -p rift-launcher'
cargo build --release -p rift-launcher
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

if (Test-Path $OutDir) { Remove-Item -Recurse -Force $OutDir }
New-Item -ItemType Directory -Path $OutDir | Out-Null

Copy-Item 'target\release\rift-launcher.exe' (Join-Path $OutDir 'rift-launcher.exe')

if (-not $ManifestUrl) {
    'https://example.com/rift/playtest/manifest.txt' |
        Set-Content -Encoding ASCII (Join-Path $OutDir 'launcher-manifest-url.txt')
}

Write-Host "done: $OutDir"