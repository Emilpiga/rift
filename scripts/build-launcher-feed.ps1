# Build a static update feed for rift-launcher.
#
# Default mode streams the release executable and assets directly into the
# launcher feed layout, without creating a zipped client bundle first. The
# resulting feed can be uploaded with `rclone sync` so only changed files move
# to the CDN and stale remote files are removed.
#
# Example:
#   cargo build --release -p rift-client
#   .\scripts\build-launcher-feed.ps1 `
#       -BaseUrl "https://cdn.example.com/rift/playtest" `
#       -Version "2026.05.14.1"

[CmdletBinding()]
param(
    # Optional legacy folder produced by package-client.ps1. When omitted,
    # the feed is built directly from ClientExe and AssetsDir.
    [string]$Stage,

    # Release executable to publish when Stage is omitted.
    [string]$ClientExe = "target\release\rift.exe",

    # Asset directory to publish when Stage is omitted.
    [string]$AssetsDir = "assets",

    # Optional README to include as README.txt. Generated when omitted.
    [string]$Readme,

    # Optional third-party license attribution file to include.
    [string]$ThirdParty = "dist\THIRD_PARTY.txt",

    # Output folder to upload to static hosting.
    [string]$OutDir = "dist\launcher-feed",

    # Public HTTPS URL where OutDir will be hosted.
    [Parameter(Mandatory = $true)]
    [string]$BaseUrl,

    # Human-readable version written to the manifest.
    [string]$Version = (Get-Date).ToUniversalTime().ToString('yyyy.MM.dd.HHmm'),

    # Game executable path relative to the install root.
    [string]$Entrypoint = "rift.exe"
)

$ErrorActionPreference = 'Stop'
Set-Location -Path (Join-Path $PSScriptRoot '..')

$BaseUrl = $BaseUrl.TrimEnd('/')
$filesRoot = Join-Path $OutDir 'files'

function Convert-ToUrlPath([string]$relativePath) {
    $parts = $relativePath -split '[\/]+'
    ($parts | ForEach-Object { [uri]::EscapeDataString($_) }) -join '/'
}

function Get-RelativePathCompat([string]$BasePath, [string]$FullPath) {
    $base = ([System.IO.Path]::GetFullPath($BasePath)) -replace '[\/]+$', ''
    $full = [System.IO.Path]::GetFullPath($FullPath)
    $prefix = "$base\"
    if (-not $full.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "file is outside base folder: $FullPath"
    }
    $full.Substring($prefix.Length)
}

function Get-Sha256Hex([string]$Path) {
    $sha = [System.Security.Cryptography.SHA256]::Create()
    $stream = [System.IO.File]::OpenRead($Path)
    try {
        $bytes = $sha.ComputeHash($stream)
        ([System.BitConverter]::ToString($bytes) -replace '-', '').ToLowerInvariant()
    } finally {
        $stream.Dispose()
        $sha.Dispose()
    }
}

function Copy-FileIfChanged {
    param(
        [Parameter(Mandatory = $true)][string]$Source,
        [Parameter(Mandatory = $true)][string]$Destination,
        [Parameter(Mandatory = $true)][string]$Hash,
        [Parameter(Mandatory = $true)][long]$Size
    )

    $needsCopy = $true
    if (Test-Path -LiteralPath $Destination) {
        $destItem = Get-Item -LiteralPath $Destination
        if ($destItem.Length -eq $Size) {
            $needsCopy = (Get-Sha256Hex $Destination) -ne $Hash
        }
    }

    if ($needsCopy) {
        $destDir = Split-Path -Parent $Destination
        if ($destDir) { New-Item -ItemType Directory -Path $destDir -Force | Out-Null }
        [System.IO.File]::Copy($Source, $Destination, $true)
        return $true
    }

    return $false
}

function New-TempFeedFile {
    param(
        [Parameter(Mandatory = $true)][string]$RelativePath,
        [Parameter(Mandatory = $true)][string]$Content
    )
    $path = Join-Path $OutDir "generated\$RelativePath"
    $dir = Split-Path -Parent $path
    if ($dir) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($path, $Content, $utf8NoBom)
    $path
}

function Add-FeedFile {
    param(
        [Parameter(Mandatory = $true)][System.Collections.Generic.List[string]]$Manifest,
        [System.Collections.Generic.HashSet[string]]$Expected,
        [Parameter(Mandatory = $true)][string]$Source,
        [Parameter(Mandatory = $true)][string]$Relative
    )

    $relative = $Relative -replace '\\', '/'
    $sourceFull = [System.IO.Path]::GetFullPath($Source)
    if (-not (Test-Path -LiteralPath $sourceFull -PathType Leaf)) {
        throw "feed source file not found: $Source"
    }

    $item = Get-Item -LiteralPath $sourceFull
    $hash = Get-Sha256Hex $sourceFull
    $dest = Join-Path $filesRoot $relative
    $copied = Copy-FileIfChanged -Source $sourceFull -Destination $dest -Hash $hash -Size $item.Length
    if ($copied) { $script:CopiedFiles += 1 }

    $url = "$BaseUrl/files/$(Convert-ToUrlPath $relative)"
    $Manifest.Add("file`t$relative`t$($item.Length)`t$hash`t$url")
    [void]$Expected.Add($relative.ToLowerInvariant())
}

function Add-DirectoryToFeed {
    param(
        [Parameter(Mandatory = $true)][System.Collections.Generic.List[string]]$Manifest,
        [System.Collections.Generic.HashSet[string]]$Expected,
        [Parameter(Mandatory = $true)][string]$SourceDir,
        [Parameter(Mandatory = $true)][string]$TargetPrefix
    )

    $sourceFull = (Resolve-Path -LiteralPath $SourceDir).Path
    $allFiles = Get-ChildItem -LiteralPath $sourceFull -Recurse -File | Sort-Object FullName
    foreach ($file in $allFiles) {
        $relative = Get-RelativePathCompat $sourceFull $file.FullName
        $targetRelative = if ($TargetPrefix) { Join-Path $TargetPrefix $relative } else { $relative }
        Add-FeedFile -Manifest $Manifest -Expected $Expected -Source $file.FullName -Relative $targetRelative
    }
}

function Remove-StaleFeedFiles {
    param([Parameter(Mandatory = $true)][System.Collections.Generic.HashSet[string]]$Expected)

    if (-not (Test-Path -LiteralPath $filesRoot)) { return }
    $filesFull = (Resolve-Path -LiteralPath $filesRoot).Path
    foreach ($file in Get-ChildItem -LiteralPath $filesRoot -Recurse -File) {
        $relative = (Get-RelativePathCompat $filesFull $file.FullName) -replace '\\', '/'
        if (-not $Expected.Contains($relative.ToLowerInvariant())) {
            Remove-Item -LiteralPath $file.FullName -Force
            $script:RemovedFiles += 1
        }
    }
}

New-Item -ItemType Directory -Path $filesRoot -Force | Out-Null
if (Test-Path -LiteralPath (Join-Path $OutDir 'generated')) {
    Remove-Item -LiteralPath (Join-Path $OutDir 'generated') -Recurse -Force
}

$script:CopiedFiles = 0
$script:RemovedFiles = 0
$manifest = New-Object System.Collections.Generic.List[string]
$expected = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::OrdinalIgnoreCase)
$manifest.Add('rift-launcher-manifest-v1')
$manifest.Add("version`t$Version")
$manifest.Add("entrypoint`t$Entrypoint")

if ($Stage) {
    if (-not (Test-Path -LiteralPath $Stage)) {
        throw "stage folder not found: $Stage. Run scripts\package-client.ps1 first, or omit -Stage to publish directly from target\release and assets."
    }
    Add-DirectoryToFeed -Manifest $manifest -Expected $expected -SourceDir $Stage -TargetPrefix ''
} else {
    Add-FeedFile -Manifest $manifest -Expected $expected -Source $ClientExe -Relative 'rift.exe'
    Add-DirectoryToFeed -Manifest $manifest -Expected $expected -SourceDir $AssetsDir -TargetPrefix 'assets'

    if ($Readme) {
        Add-FeedFile -Manifest $manifest -Expected $expected -Source $Readme -Relative 'README.txt'
    } else {
        $readmePath = New-TempFeedFile -RelativePath 'README.txt' -Content @"
Rift Crawler playtest build

Run the game through rift-launcher.exe. The launcher keeps this folder updated.

If you need to override the baked-in server:
    rift.exe --connect HOST:PORT
"@
        Add-FeedFile -Manifest $manifest -Expected $expected -Source $readmePath -Relative 'README.txt'
    }

    if ($ThirdParty -and (Test-Path -LiteralPath $ThirdParty -PathType Leaf)) {
        Add-FeedFile -Manifest $manifest -Expected $expected -Source $ThirdParty -Relative 'THIRD_PARTY.txt'
    }
}

Remove-StaleFeedFiles -Expected $expected
if (Test-Path -LiteralPath (Join-Path $OutDir 'generated')) {
    Remove-Item -LiteralPath (Join-Path $OutDir 'generated') -Recurse -Force
}

$manifestPath = Join-Path $OutDir 'manifest.txt'
$manifest | Set-Content -Encoding ASCII $manifestPath

Write-Host "feed:     $OutDir"
Write-Host "manifest: $manifestPath"
Write-Host "url:      $BaseUrl/manifest.txt"
Write-Host "files:    $($expected.Count)"
Write-Host "copied:   $script:CopiedFiles changed file(s) locally"
Write-Host "removed:  $script:RemovedFiles stale file(s) locally"