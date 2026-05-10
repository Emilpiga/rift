# Pack a redistributable rift-client bundle on Windows. Produces
# dist\rift-client-<host>.zip containing the release binary, the
# assets folder, and a README so a playtester can run it without
# touching Cargo.
#
# Usage:
#   # Bake the server address into the binary so playtesters can
#   # just double-click rift.exe with no flags:
#   .\scripts\package-client.ps1 -Server 137.66.39.118:34000
#
#   # Or take whatever's already in $env:RIFT_DEFAULT_SERVER:
#   $env:RIFT_DEFAULT_SERVER = "137.66.39.118:34000"
#   .\scripts\package-client.ps1
#
#   # Build a "no default, must pass --connect" client:
#   .\scripts\package-client.ps1 -Server ""

[CmdletBinding()]
param(
    # Address baked into the client at compile time. Players who
    # run rift.exe with no flags connect here. Leave empty to
    # ship a build that requires --connect.
    [string]$Server = $env:RIFT_DEFAULT_SERVER
)

$ErrorActionPreference = 'Stop'
Set-Location -Path (Join-Path $PSScriptRoot '..')

if ($Server) {
    Write-Host "==> baking RIFT_DEFAULT_SERVER=$Server"
    $env:RIFT_DEFAULT_SERVER = $Server
} else {
    Write-Host "==> no server baked in; clients will need --connect"
    Remove-Item Env:RIFT_DEFAULT_SERVER -ErrorAction SilentlyContinue
}

Write-Host '==> cargo build --release -p rift-client'
cargo build --release -p rift-client
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

# Host triple for the archive name (e.g. x86_64-pc-windows-msvc).
$host_line = (& rustc -vV) | Where-Object { $_ -match '^host:' }
$host_triple = ($host_line -split '\s+')[1]

$outDir   = 'dist'
$stage    = Join-Path $outDir 'rift-client'
$archive  = Join-Path $outDir "rift-client-$host_triple.zip"

if (Test-Path $stage)   { Remove-Item -Recurse -Force $stage }
if (Test-Path $archive) { Remove-Item -Force $archive }
New-Item -ItemType Directory -Path $stage | Out-Null

Write-Host '==> staging binary + assets'
Copy-Item 'target\release\rift.exe' (Join-Path $stage 'rift.exe')
Copy-Item -Recurse 'assets' (Join-Path $stage 'assets')

# Generate (or refresh) the third-party license attribution
# file and stage it next to the binary. Required by every
# storefront we'd plausibly publish through, and by the MIT /
# Apache-2.0 / BSD-* licenses themselves. Skipped with a
# warning if cargo-about isn't installed so a developer
# without it can still cut a local test build — but a release
# bundle without THIRD_PARTY.txt is not legally distributable.
if (Get-Command cargo-about -ErrorAction SilentlyContinue) {
    Write-Host '==> regenerating THIRD_PARTY.txt'
    & (Join-Path $PSScriptRoot 'gen-third-party.ps1')
    if ($LASTEXITCODE -ne 0) { throw "gen-third-party failed" }
    Copy-Item 'dist\THIRD_PARTY.txt' (Join-Path $stage 'THIRD_PARTY.txt')
} else {
    Write-Warning 'cargo-about not installed; THIRD_PARTY.txt will be missing'
    Write-Warning 'from this bundle. Run `cargo install cargo-about` and'
    Write-Warning 're-package before distributing.'
}

$serverLine = if ($Server) {
    "Connects automatically to $Server."
} else {
    "Run with --connect HOST:PORT to join a server."
}
@"
Rift Crawler — playtest build ($host_triple)

Run the game:
    rift.exe

$serverLine

Override the baked-in server (if any):
    rift.exe --connect HOST:PORT
or set the env var:
    set RIFT_SERVER=HOST:PORT

Skip multiplayer entirely:
    rift.exe --offline

Notes:
* You need a Vulkan-capable GPU + a modern driver. Every
  mainstream 2018+ GPU qualifies. If the game complains about
  "no Vulkan instance", update your graphics driver.
* All assets must stay next to the binary. Don't move rift.exe
  out of this folder.
"@ | Set-Content -Encoding UTF8 (Join-Path $stage 'README.txt')

Write-Host "==> creating $archive"
Compress-Archive -Path $stage -DestinationPath $archive -Force

Write-Host "done: $archive"
