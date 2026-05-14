<#
.SYNOPSIS
    Dev launcher for rift.

.EXAMPLES
    .\scripts\rift.ps1 server
    .\scripts\rift.ps1 client
    .\scripts\rift.ps1 client -- --connect 127.0.0.1:34000
    .\scripts\rift.ps1 both
    .\scripts\rift.ps1 build
#>
[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [ValidateSet("server", "client", "both", "build", "help")]
    [string]$Command = "help",

    [Parameter(Position = 1)]
    [ValidateSet("start", "build", "run")]
    [string]$Sub = "start",

    [switch]$Release,

    # Build and publish a playtest launcher feed, then produce
    # a shareable rift-launcher.exe. Configure once with:
    #   RIFT_LAUNCHER_BASE_URL=https://cdn.example.com/rift/playtest
    #   RIFT_LAUNCHER_UPLOAD=rclone-remote:path/or/local/static/root
    [switch]$Launcher,

    # Public URL where the generated launcher feed will be hosted.
    [string]$LauncherBaseUrl,

    # Upload destination for the generated feed. Local folders are
    # copied directly; non-local values use `rclone copy`.
    [string]$LauncherUpload,

    # Version written into manifest.txt.
    [string]$LauncherVersion = (Get-Date).ToUniversalTime().ToString('yyyy.MM.dd.HHmm'),

    # Server address baked into the playtest client. Defaults to
    # RIFT_DEFAULT_SERVER, matching package-client.ps1.
    [string]$PlaytestServer,

    # Build with the real Steamworks-backed auth verifier on
    # both client and server (`--features steam-auth`). When
    # set, the dev-auth key is left untouched but ignored at
    # runtime; the server refuses to enable both verifiers at
    # once. Requires Steam running locally; defaults to the
    # Spacewar sandbox appid (480) unless `RIFT_STEAM_APPID`
    # is set.
    [switch]$Steam,

    # Override the dev account identity. When provided, sets
    # `RIFT_DEV_USER` for the launched client(s) so the
    # dev-auth verifier uses this exact name instead of
    # minting a randomized `dev-XXXXXX`. No effect on the
    # server side or when `-Steam` is set.
    [Parameter()]
    [string]$User,

    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$Rest
)

$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

function Import-DotEnvFile {
    param([Parameter(Mandatory = $true)][string]$Path)
    if (-not (Test-Path $Path)) { return }

    foreach ($rawLine in Get-Content $Path) {
        $line = $rawLine.Trim()
        if (-not $line -or $line.StartsWith('#')) { continue }
        if ($line.StartsWith('export ')) { $line = $line.Substring(7).TrimStart() }

        $parts = $line.Split('=', 2)
        if ($parts.Count -ne 2) { continue }

        $name = $parts[0].Trim()
        $value = $parts[1].Trim()
        if ($name -notmatch '^[A-Za-z_][A-Za-z0-9_]*$') { continue }
        if ($value.Length -ge 2) {
            $first = $value[0]
            $last = $value[$value.Length - 1]
            if (($first -eq '"' -and $last -eq '"') -or ($first -eq "'" -and $last -eq "'")) {
                $value = $value.Substring(1, $value.Length - 2)
            }
        }

        if (-not [Environment]::GetEnvironmentVariable($name, 'Process')) {
            [Environment]::SetEnvironmentVariable($name, $value, 'Process')
        }
    }
}

Import-DotEnvFile ".env"
Import-DotEnvFile ".env.local"

if (-not $PSBoundParameters.ContainsKey('LauncherBaseUrl')) { $LauncherBaseUrl = $env:RIFT_LAUNCHER_BASE_URL }
if (-not $PSBoundParameters.ContainsKey('LauncherUpload')) { $LauncherUpload = $env:RIFT_LAUNCHER_UPLOAD }
if (-not $PSBoundParameters.ContainsKey('PlaytestServer')) { $PlaytestServer = $env:RIFT_DEFAULT_SERVER }

$ServerBind   = if ($env:RIFT_SERVER_BIND) { $env:RIFT_SERVER_BIND } else { "127.0.0.1:34000" }
$ClientCnx    = if ($env:RIFT_CONNECT)     { $env:RIFT_CONNECT }     else { "127.0.0.1:34000" }
$ServerLog    = if ($env:RIFT_SERVER_LOG)  { $env:RIFT_SERVER_LOG }  else { "info" }
$ClientLog    = if ($env:RIFT_CLIENT_LOG)  { $env:RIFT_CLIENT_LOG }  else { "info" }

# Auto-provision a stable RIFT_DEV_AUTH_KEY for the dev
# environment if the operator hasn't set one. The key is a
# 32-byte random hex string written to `.env.dev-auth` at the
# repo root and reused on every subsequent launch so client
# and server agree across runs. The file is gitignored. NEVER
# set RIFT_DEV_AUTH_KEY (or commit this file) on a production
# server — dev auth must stay disabled there.
if (-not $env:RIFT_DEV_AUTH_KEY) {
    $devAuthFile = ".env.dev-auth"
    if (-not (Test-Path $devAuthFile)) {
        # `[RandomNumberGenerator]::Fill` is .NET Core / PS7+;
        # use the older `Create()` API so this works on stock
        # Windows PowerShell 5.1 too.
        $rng = [System.Security.Cryptography.RandomNumberGenerator]::Create()
        try {
            $bytes = New-Object byte[] 32
            $rng.GetBytes($bytes)
        } finally {
            $rng.Dispose()
        }
        $hex = -join ($bytes | ForEach-Object { $_.ToString("x2") })
        Set-Content -Path $devAuthFile -Value $hex -NoNewline
        Write-Host "rift: generated dev auth key at $devAuthFile (gitignored)"
    }
    $env:RIFT_DEV_AUTH_KEY = (Get-Content -Raw $devAuthFile).Trim()
}

$BuildProfile = if ($Release) { "release" } else { "debug" }

if ($User) {
    $env:RIFT_DEV_USER = $User
    Write-Host "rift: RIFT_DEV_USER=$User"
}

if ($Steam) {
    if (-not $env:RIFT_STEAM_APPID) {
        # Spacewar — Valve's public sandbox appid. Anyone with
        # a Steam account can sign / validate tickets against
        # it without owning a real product. Swap to your real
        # appid once you've onboarded with Valve.
        $env:RIFT_STEAM_APPID = "480"
        Write-Host "rift: RIFT_STEAM_APPID not set, defaulting to 480 (Spacewar sandbox)"
    }
    Write-Host "rift: building with --features steam-auth (RIFT_STEAM_APPID=$($env:RIFT_STEAM_APPID))"
}

function Invoke-Cargo {
    param([Parameter(ValueFromRemainingArguments = $true)][string[]]$Args)
    $featureArgs = @()
    $features = @()
    if ($Steam) { $features += "steam-auth" }
    if ($features.Count -gt 0) {
        $featureArgs = @("--features", ($features -join ","))
    }
    if ($Release) {
        & cargo build --release @featureArgs @Args
    } else {
        & cargo build @featureArgs @Args
    }
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed (exit $LASTEXITCODE)" }
}

function Build-Server { Invoke-Cargo -p rift-server; Copy-SteamDll }
function Build-Client { Invoke-Cargo -p rift-client; Copy-SteamDll }
function Run-Server   { $env:RUST_LOG = $ServerLog; & ".\target\$BuildProfile\rift-server.exe" --bind $ServerBind @Rest }
function Run-Client   { $env:RUST_LOG = $ClientLog; & ".\target\$BuildProfile\rift.exe" --connect $ClientCnx @Rest }

function Test-LocalUploadPath {
    param([Parameter(Mandatory = $true)][string]$Path)
    return ($Path -match '^[A-Za-z]:[\\/]') -or
        $Path.StartsWith('\\') -or
        $Path.StartsWith('/') -or
        $Path.StartsWith('.') -or
        $Path.StartsWith('~') -or
        ($Path -notmatch ':')
}

function Publish-LauncherFeed {
    param(
        [Parameter(Mandatory = $true)][string]$Source,
        [Parameter(Mandatory = $true)][string]$Destination
    )

    if (Test-LocalUploadPath $Destination) {
        Write-Host "==> mirroring launcher feed to $Destination"
        New-Item -ItemType Directory -Path $Destination -Force | Out-Null
        robocopy $Source $Destination /MIR /NFL /NDL /NJH /NJS /NP | Out-Host
        $robocopyExit = $LASTEXITCODE
        $global:LASTEXITCODE = 0
        if ($robocopyExit -gt 7) { throw "robocopy mirror failed (exit $robocopyExit)" }
        return
    }

    $rcloneExe = $null
    $rclone = Get-Command rclone -ErrorAction SilentlyContinue
    if ($rclone) {
        $rcloneExe = $rclone.Source
    } else {
        $localRclone = Join-Path $PWD "tools\rclone.exe"
        if (Test-Path -LiteralPath $localRclone) {
            $rcloneExe = (Get-Item -LiteralPath $localRclone).FullName
        }
    }
    if (-not $rcloneExe) {
        throw "RIFT_LAUNCHER_UPLOAD is not a local path and rclone was not found. Install rclone or set RIFT_LAUNCHER_UPLOAD to a local static-host folder."
    }

    Write-Host "==> syncing launcher feed with rclone to $Destination"
    & $rcloneExe sync $Source $Destination --checksum --progress
    if ($LASTEXITCODE -ne 0) { throw "rclone sync failed" }
}

function Publish-Launcher {
    if (-not $Release) {
        throw "launcher publishing must be run with -Release"
    }
    if (-not $LauncherBaseUrl) {
        throw "missing launcher feed URL. Pass -LauncherBaseUrl or set RIFT_LAUNCHER_BASE_URL."
    }
    if (-not $LauncherUpload) {
        throw "missing launcher upload destination. Pass -LauncherUpload or set RIFT_LAUNCHER_UPLOAD."
    }

    $baseUrl = $LauncherBaseUrl.TrimEnd('/')
    $manifestUrl = "$baseUrl/manifest.txt"
    $feedDir = "dist\launcher-feed"

    Write-Host "rift: publishing launcher build $LauncherVersion"
    Write-Host "rift: feed URL $baseUrl"
    Write-Host "rift: upload destination $LauncherUpload"

    if ($PlaytestServer) {
        Write-Host "rift: baking RIFT_DEFAULT_SERVER=$PlaytestServer"
        $env:RIFT_DEFAULT_SERVER = $PlaytestServer
    } else {
        Write-Host "rift: no default server baked in; clients will need --connect"
        Remove-Item Env:RIFT_DEFAULT_SERVER -ErrorAction SilentlyContinue
    }

    Build-Client

    if (Test-Path -LiteralPath "dist\THIRD_PARTY.txt" -PathType Leaf) {
        Write-Host "rift: reusing existing dist\THIRD_PARTY.txt"
    } else {
        Write-Host "rift: no dist\THIRD_PARTY.txt found; launcher feed will omit it"
    }

    & (Join-Path $PSScriptRoot 'build-launcher-feed.ps1') `
        -BaseUrl $baseUrl `
        -Version $LauncherVersion `
        -ClientExe "target\release\rift.exe" `
        -AssetsDir "assets" `
        -OutDir $feedDir

    Publish-LauncherFeed -Source $feedDir -Destination $LauncherUpload

    & (Join-Path $PSScriptRoot 'package-launcher.ps1') -ManifestUrl $manifestUrl

    Write-Host ""
    Write-Host "rift: launcher ready: dist\rift-launcher\rift-launcher.exe"
    Write-Host "rift: testers will update from $manifestUrl"
}

# `steamworks-sys` builds `steam_api64.dll` into its OUT_DIR
# but does NOT copy it next to the produced binaries, so
# launching either exe fails with "steam_api64.dll was not
# found". Fish it out of the build directory and drop it next
# to the binaries. No-op when `-Steam` is not set.
function Copy-SteamDll {
    if (-not $Steam) { return }
    $buildDir = ".\target\$BuildProfile\build"
    if (-not (Test-Path $buildDir)) { return }
    $dll = Get-ChildItem -Path $buildDir -Recurse -Filter "steam_api64.dll" -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if (-not $dll) {
        Write-Warning "rift: steam_api64.dll not found under $buildDir; Steam build will fail at launch"
        return
    }
    $dest = ".\target\$BuildProfile\steam_api64.dll"
    # The DLL is loaded by any running rift-server.exe /
    # rift.exe, which holds an exclusive lock on it. If the
    # destination already exists and is identical to the
    # source, skip; otherwise warn and continue rather than
    # erroring out so a `client build` while the server is
    # running still succeeds.
    if (Test-Path $dest) {
        $srcLen = (Get-Item $dll.FullName).Length
        $dstLen = (Get-Item $dest).Length
        if ($srcLen -eq $dstLen) { return }
    }
    try {
        Copy-Item -Path $dll.FullName -Destination $dest -Force -ErrorAction Stop
    } catch [System.IO.IOException] {
        Write-Warning "rift: could not refresh $dest (in use by a running process). Stop the running server/client and rebuild if the SDK version changed."
    }
}

if ($Launcher) {
    Publish-Launcher
    return
}

switch ($Command) {
    "server" {
        if ($Sub -eq "build") { Build-Server; return }
        if ($Sub -eq "start") { Build-Server }
        Run-Server
    }
    "client" {
        if ($Sub -eq "build") { Build-Client; return }
        if ($Sub -eq "start") { Build-Client }
        Run-Client
    }
    "both" {
        if ($Steam) {
            # `--features steam-auth` is per-crate, so we can't
            # build the workspace in one shot when it's set.
            Build-Server
            Build-Client
        } else {
            Invoke-Cargo --workspace
        }
        $prevLog = $env:RUST_LOG
        $env:RUST_LOG = $ServerLog
        $srv = Start-Process -PassThru -NoNewWindow `
            -FilePath ".\target\$BuildProfile\rift-server.exe" `
            -ArgumentList "--bind", $ServerBind
        $env:RUST_LOG = $prevLog
        Start-Sleep -Milliseconds 500
        try { Run-Client } finally {
            if (-not $srv.HasExited) { Stop-Process -Id $srv.Id -Force }
        }
    }
    "build" {
        if ($Steam) {
            Build-Server
            Build-Client
        } else {
            Invoke-Cargo --workspace
        }
    }
    default {
        Write-Host @"
rift dev launcher

.\scripts\rift.ps1 server [start|build|run]
.\scripts\rift.ps1 client [start|build|run]
.\scripts\rift.ps1 both
.\scripts\rift.ps1 build
.\scripts\rift.ps1 -Release -Launcher

Env: RIFT_SERVER_BIND, RIFT_CONNECT, RIFT_SERVER_LOG, RIFT_CLIENT_LOG, RIFT_STEAM_APPID
RIFT_LAUNCHER_BASE_URL, RIFT_LAUNCHER_UPLOAD, RIFT_DEFAULT_SERVER

Flags:
-Release     Build the release profile.
-Launcher    Package, upload, and build the playtest launcher.
-Steam       Build both crates with --features steam-auth.
             Requires Steam running; defaults to Spacewar
             sandbox (appid 480) unless RIFT_STEAM_APPID set.
"@
    }
}
