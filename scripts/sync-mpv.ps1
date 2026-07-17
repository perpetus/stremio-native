[CmdletBinding()]
param(
    [switch]$CheckOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = [IO.Path]::GetFullPath((Join-Path $scriptDir ".."))
$lockPath = Join-Path $repoRoot "mpv.lock.json"
$pinnedRustPath = Join-Path $repoRoot "playback-mpv\src\pinned.rs"
$workDir = Join-Path $repoRoot "target\mpv-prebuilt-sync"
$artifactKey = "windows-x86_64-v3-dynamic"
$githubHeaders = @{ "User-Agent" = "stremio-native-mpv-sync" }

function Get-Sha256([string]$Path) {
    (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Assert-Hash([string]$Path, [string]$Expected) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "Required MPV file is missing: $Path"
    }
    $actual = Get-Sha256 $Path
    if ($actual -ne $Expected.ToLowerInvariant()) {
        throw "SHA-256 mismatch for $Path`nExpected: $Expected`nActual:   $actual"
    }
}

function Assert-WorkspacePath([string]$Path) {
    $root = $repoRoot.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    $resolved = [IO.Path]::GetFullPath($Path)
    if (-not $resolved.StartsWith($root, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to modify a path outside the repository: $resolved"
    }
    $resolved
}

function Remove-WorkspaceTree([string]$Path) {
    $resolved = Assert-WorkspacePath $Path
    if (Test-Path -LiteralPath $resolved) {
        Remove-Item -LiteralPath $resolved -Recurse -Force
    }
}

function Write-Utf8Lf([string]$Path, [string]$Contents) {
    $normalized = $Contents.Replace("`r`n", "`n").Replace("`r", "`n")
    $normalized = [regex]::Replace($normalized, "`n+$", "") + "`n"
    [IO.File]::WriteAllText($Path, $normalized, [Text.UTF8Encoding]::new($false))
}

function Get-ClientApi([string]$ClientHeader) {
    $match = Select-String `
        -LiteralPath $ClientHeader `
        -Pattern '^#define MPV_CLIENT_API_VERSION MPV_MAKE_VERSION\((\d+),\s*(\d+)\)' |
        Select-Object -First 1
    if (-not $match) {
        throw "MPV_CLIENT_API_VERSION was not found in $ClientHeader"
    }
    [ordered]@{
        major = [int]$match.Matches[0].Groups[1].Value
        minor = [int]$match.Matches[0].Groups[2].Value
    }
}

function Get-PeMachine([string]$Path) {
    $stream = [IO.File]::OpenRead($Path)
    $reader = [IO.BinaryReader]::new($stream)
    try {
        if ($reader.ReadUInt16() -ne 0x5A4D) {
            throw "$Path is not a PE image"
        }
        $stream.Position = 0x3C
        $peOffset = $reader.ReadUInt32()
        $stream.Position = $peOffset
        if ($reader.ReadUInt32() -ne 0x00004550) {
            throw "$Path has an invalid PE signature"
        }
        $reader.ReadUInt16()
    }
    finally {
        $reader.Dispose()
        $stream.Dispose()
    }
}

function Download-Verified([string]$Url, [string]$Destination, [string]$Sha256) {
    if ((Test-Path -LiteralPath $Destination) -and (Get-Sha256 $Destination) -ne $Sha256.ToLowerInvariant()) {
        Remove-Item -LiteralPath $Destination -Force
    }
    if (-not (Test-Path -LiteralPath $Destination)) {
        $parent = Split-Path -Parent $Destination
        New-Item -ItemType Directory -Force -Path $parent | Out-Null
        Invoke-WebRequest -Headers $githubHeaders -Uri $Url -OutFile $Destination
    }
    Assert-Hash $Destination $Sha256
}

function Get-LatestOptimizedAsset {
    $release = Invoke-RestMethod `
        -Headers $githubHeaders `
        -Uri "https://api.github.com/repos/shinchiro/mpv-winbuild-cmake/releases/latest"
    $assets = @($release.assets | Where-Object {
        $_.name -match '^mpv-dev-x86_64-v3-\d{8}-git-[0-9a-f]+\.7z$'
    })
    if ($assets.Count -ne 1) {
        throw "Expected one optimized x86-64 libmpv asset in release $($release.tag_name), found $($assets.Count)"
    }
    $digest = [string]$assets[0].digest
    if (-not $digest.StartsWith("sha256:", [StringComparison]::OrdinalIgnoreCase)) {
        throw "GitHub did not publish a SHA-256 digest for $($assets[0].name)"
    }
    [pscustomobject]@{
        release = [string]$release.tag_name
        name = [string]$assets[0].name
        sha256 = $digest.Substring(7).ToLowerInvariant()
    }
}

if (-not (Test-Path -LiteralPath $lockPath)) {
    throw "MPV lock file is missing: $lockPath"
}
$locked = Get-Content -LiteralPath $lockPath -Raw | ConvertFrom-Json
if ($locked.linkage -ne "dynamic") {
    throw "The MPV lock describes $($locked.linkage) linkage; dynamic linkage is required"
}
$artifactProperty = $locked.artifacts.PSObject.Properties[$artifactKey]
if (-not $artifactProperty) {
    throw "The MPV lock does not contain artifact $artifactKey"
}
$artifact = $artifactProperty.Value

if ($CheckOnly) {
    $latest = Get-LatestOptimizedAsset
    if ($locked.distribution.release -ne $latest.release -or
        $locked.distribution.asset -ne $latest.name -or
        $locked.distribution.sha256 -ne $latest.sha256) {
        throw "MPV is stale: repository pins $($locked.distribution.asset), latest optimized build is $($latest.name) from release $($latest.release)"
    }
    Write-Host "MPV $($locked.mpv.version) optimized runtime is current ($($latest.name))."
    exit 0
}

if ($artifact.target -ne "x86_64-pc-windows-msvc" -or
    $artifact.architecture -ne "x86_64" -or
    $artifact.cpuBaseline -ne "x86-64-v3") {
    throw "The pinned runtime must target x86_64-pc-windows-msvc with the x86-64-v3 CPU baseline"
}
if ([IO.Path]::GetFileName([string]$locked.distribution.asset) -ne $locked.distribution.asset) {
    throw "The locked MPV asset name is not a plain file name"
}

New-Item -ItemType Directory -Force -Path $workDir | Out-Null
$archivePath = Join-Path $workDir $locked.distribution.asset
Download-Verified $locked.distribution.url $archivePath $locked.distribution.sha256

$sevenZip = Get-Command "7z.exe" -ErrorAction SilentlyContinue
if (-not $sevenZip) {
    throw "7-Zip is required to extract the pinned libmpv archive"
}
$extractDir = Join-Path $workDir "extracted"
Remove-WorkspaceTree $extractDir
New-Item -ItemType Directory -Force -Path $extractDir | Out-Null
& $sevenZip.Source x -y "-o$extractDir" $archivePath | Out-Host
if ($LASTEXITCODE -ne 0) {
    throw "7-Zip failed with exit code $LASTEXITCODE"
}

$sourceDll = Join-Path $extractDir "libmpv-2.dll"
$sourceImportLibrary = Join-Path $extractDir "libmpv.dll.a"
Assert-Hash $sourceDll $artifact.runtimeLibrary.sha256
Assert-Hash $sourceImportLibrary $artifact.importLibrary.sha256
if ((Get-PeMachine $sourceDll) -ne 0x8664) {
    throw "The pinned libmpv DLL is not a 64-bit x86 PE image"
}

$clientHeader = Join-Path $extractDir $locked.headers.'client.h'.file
$api = Get-ClientApi $clientHeader
if ($api.major -ne $locked.clientApi.major -or $api.minor -ne $locked.clientApi.minor) {
    throw "Client API mismatch: archive contains $($api.major).$($api.minor), lock contains $($locked.clientApi.major).$($locked.clientApi.minor)"
}

$stagingDir = Join-Path $workDir "package"
Remove-WorkspaceTree $stagingDir
New-Item -ItemType Directory -Force -Path `
    (Join-Path $stagingDir "bin"), `
    (Join-Path $stagingDir "lib"), `
    (Join-Path $stagingDir "include\mpv") | Out-Null
Copy-Item -LiteralPath $sourceDll -Destination (Join-Path $stagingDir $artifact.runtimeLibrary.file)
Copy-Item -LiteralPath $sourceImportLibrary -Destination (Join-Path $stagingDir $artifact.importLibrary.file)

foreach ($headerProperty in $locked.headers.PSObject.Properties) {
    $header = $headerProperty.Value
    $source = Join-Path $extractDir $header.file
    Assert-Hash $source $header.sha256
    Copy-Item -LiteralPath $source -Destination (Join-Path $stagingDir $header.file)
}

foreach ($licenseProperty in $locked.licenses.PSObject.Properties) {
    $license = $licenseProperty.Value
    $destination = Join-Path $stagingDir $licenseProperty.Name
    Download-Verified $license.url $destination $license.sha256
}

$runtimeManifest = @"
{
  "schemaVersion": 1,
  "target": "$($artifact.target)",
  "architecture": "$($artifact.architecture)",
  "cpuBaseline": "$($artifact.cpuBaseline)",
  "linkName": "$($artifact.linkName)",
  "clientApi": {
    "major": $($locked.clientApi.major),
    "minor": $($locked.clientApi.minor)
  },
  "importLibrary": {
    "file": "$($artifact.importLibrary.file)",
    "sha256": "$($artifact.importLibrary.sha256)"
  },
  "runtimeLibrary": {
    "file": "$($artifact.runtimeLibrary.file)",
    "sha256": "$($artifact.runtimeLibrary.sha256)"
  }
}
"@
$runtimeManifestPath = Join-Path $stagingDir $artifact.sdkManifest.file
Write-Utf8Lf $runtimeManifestPath $runtimeManifest
Assert-Hash $runtimeManifestPath $artifact.sdkManifest.sha256

$sourceNotice = @"
libmpv Windows binary provenance
================================

Distribution: $($locked.distribution.provider) release $($locked.distribution.release)
MPV version: $($locked.mpv.version)
MPV revision: $($locked.mpv.revision)
CPU baseline: $($artifact.cpuBaseline)
Archive: $($locked.distribution.asset)
Archive SHA-256: $($locked.distribution.sha256)
Archive URL: $($locked.distribution.url)
Build scripts and dependency revisions: $($locked.distribution.repository)/tree/$($locked.distribution.release)
MPV source: $($locked.mpv.repository)/commit/$($locked.mpv.revision)

The mpv project lists shinchiro's builds on its Windows installation page:
https://mpv.io/installation/

The upstream GNU-style COFF import archive is stored as lib/mpv.lib so Cargo's
MSVC target can discover it by link name. Its bytes are otherwise unchanged.
"@
$sourceNoticePath = Join-Path $stagingDir $artifact.sourceNotice.file
Write-Utf8Lf $sourceNoticePath $sourceNotice
Assert-Hash $sourceNoticePath $artifact.sourceNotice.sha256

$distDir = Assert-WorkspacePath (Join-Path $repoRoot $artifact.directory)
Remove-WorkspaceTree $distDir
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $distDir) | Out-Null
Move-Item -LiteralPath (Assert-WorkspacePath $stagingDir) -Destination $distDir

$pinnedRust = @"
// This file is updated by scripts/sync-mpv.ps1 from the pinned MPV headers.
pub const HEADER_CLIENT_API_VERSION: ApiVersion = ApiVersion::new($($api.major), $($api.minor));
"@
Write-Utf8Lf $pinnedRustPath $pinnedRust

Write-Host "Packaged $($locked.mpv.version) optimized libmpv runtime into $distDir"
