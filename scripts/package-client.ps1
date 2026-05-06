# Pack a redistributable rift-client bundle on Windows. Produces
# dist\rift-client-<host>.zip containing the release binary, the
# assets folder, and a README so a playtester can run it without
# touching Cargo.
#
# Usage:
#   .\scripts\package-client.ps1

$ErrorActionPreference = 'Stop'
Set-Location -Path (Join-Path $PSScriptRoot '..')

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
@"
Rift Crawler — playtest build ($host_triple)

Run the game:
    rift.exe

Connect to a server:
    rift.exe --connect HOST:PORT

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
