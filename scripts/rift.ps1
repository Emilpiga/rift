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

    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$Rest
)

$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

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

function Invoke-Cargo {
    param([Parameter(ValueFromRemainingArguments = $true)][string[]]$Args)
    if ($Release) { & cargo build --release @Args } else { & cargo build @Args }
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed (exit $LASTEXITCODE)" }
}

function Build-Server { Invoke-Cargo -p rift-server }
function Build-Client { Invoke-Cargo -p rift-client }
function Run-Server   { $env:RUST_LOG = $ServerLog; & ".\target\$BuildProfile\rift-server.exe" --bind $ServerBind @Rest }
function Run-Client   { $env:RUST_LOG = $ClientLog; & ".\target\$BuildProfile\rift.exe" --connect $ClientCnx @Rest }

switch ($Command) {
    "server" {
        if ($Sub -eq "build") { Build-Server; return }
        if ($Sub -eq "start") { Build-Server }
        Run-Server
        if ($Sub -eq "run") { Run-Server }
    }
    "client" {
        if ($Sub -eq "build") { Build-Client; return }
        if ($Sub -eq "start") { Build-Client }
        Run-Client
        if ($Sub -eq "run") { Run-Client }
    }
    "both" {
        Invoke-Cargo --workspace
        $prevLog = $env:RUST_LOG
        $env:RUST_LOG = $ServerLog
        $srv = Start-Process -PassThru -NoNewWindow `
            -FilePath ".\target\$BuildProfile\rift-server.exe" `
            -ArgumentList "--bind", $ServerBind
        $env:RUST_LOG = $prevLog
        Start-Sleep -Milliseconds 500
        try { Run-Client } finally { if (-not $srv.HasExited) { Stop-Process -Id $srv.Id -Force } }
    }
    "build" { Invoke-Cargo --workspace }
    default {
        Write-Host @"
rift dev launcher

  .\scripts\rift.ps1 server [start|build|run]
  .\scripts\rift.ps1 client [start|build|run]
  .\scripts\rift.ps1 both
  .\scripts\rift.ps1 build

Env: RIFT_SERVER_BIND, RIFT_CONNECT, RIFT_SERVER_LOG, RIFT_CLIENT_LOG
"@
    }
}
