#!/usr/bin/env pwsh
# Generate `dist/THIRD_PARTY.txt` listing every Rust crate the
# rift-client binary links against, along with their license
# text. PowerShell sibling of `gen-third-party.sh` for Windows
# build hosts.
#
# One-time setup on the build machine:
#   cargo install cargo-about
#
# Usage:
#   .\scripts\gen-third-party.ps1           # writes dist\THIRD_PARTY.txt
#   .\scripts\gen-third-party.ps1 --fail    # also fails on unknown licenses
#
# Re-run any time Cargo.lock changes.

[CmdletBinding()]
param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$ExtraArgs
)

$ErrorActionPreference = 'Stop'

Set-Location (Join-Path $PSScriptRoot '..')

if (-not (Get-Command cargo-about -ErrorAction SilentlyContinue)) {
    Write-Error "cargo-about not installed. Run ``cargo install cargo-about`` and try again."
    exit 1
}

New-Item -ItemType Directory -Force -Path dist | Out-Null

# Scope the report to the rift-client binary's dependency graph
# (--manifest-path) so we don't list server-only deps in the
# client-facing notices file.
Write-Host '==> generating dist\THIRD_PARTY.txt'
$argList = @('about', 'generate', '--config', 'about.toml', '--manifest-path', 'crates\rift-client\Cargo.toml') + $ExtraArgs + @('about.hbs')
cargo @argList | Set-Content -Encoding UTF8 -Path 'dist\THIRD_PARTY.txt'

Write-Host 'done: dist\THIRD_PARTY.txt'
